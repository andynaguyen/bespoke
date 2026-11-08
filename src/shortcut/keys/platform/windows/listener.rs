use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN,
    VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, HHOOK,
    KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG, MSLLHOOKSTRUCT, MsgWaitForMultipleObjects, PM_REMOVE,
    PeekMessageW, QS_ALLINPUT, RegisterClassW, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WINDOW_EX_STYLE, WINDOW_STYLE, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_POWERBROADCAST, WM_QUIT,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSW,
};
use windows::core::PCWSTR;

use crate::shortcut::keys::error::Result;
use crate::shortcut::keys::platform::state::{
    BlockingHotkeys, RECONCILABLE, release_events, stale_modifiers,
};
use crate::shortcut::keys::types::{Key, KeyEvent, Modifiers};

use super::keycode::{is_altgr_phantom_ctrl, map_key, vk_to_modifier};

const HOOK_LOOP_TIMEOUT_MS: u32 = 10;

/// Modifiers whose lone press-and-release makes the shell activate a menu:
/// Win opens the Start menu, Alt focuses the active window's menu bar.
const MENU_MODIFIERS: Modifiers = Modifiers::CMD.union(Modifiers::OPT);

/// Unassigned virtual key injected as the "menu mask" (AutoHotkey's default
/// mask key). The shell arms its menu when a Win/Alt down→up pair passes
/// with no other key in between; when we swallow the real key of a Win/Alt
/// hotkey, this inert keystroke is substituted so the modifier release no
/// longer reads as a lone tap (cjpais/Handy#917). The VK maps to nothing in
/// this crate (`vk_to_modifier` and `map_key` both return None), so even
/// without the dwExtraInfo marker it could never be blocked on its way to
/// the shell.
const MENU_MASK_VK: u16 = 0xE8;

const MENU_MASK_EXTRA_INFO: usize = 0x484B_4D4D;

// WTS session notification plumbing not exposed by the `windows` crate bindings.
const NOTIFY_FOR_THIS_SESSION: u32 = 0;
const WM_WTSSESSION_CHANGE: u32 = 0x02B1;
const WTS_CONSOLE_CONNECT: usize = 0x1;
const WTS_REMOTE_CONNECT: usize = 0x3;
const WTS_SESSION_LOCK: usize = 0x7;
const WTS_SESSION_UNLOCK: usize = 0x8;
// WM_POWERBROADCAST wParam values (winuser.h) -- the PBT_* constants live
// behind the Win32_System_Power feature, not worth enabling for two values.
// RESUMEAUTOMATIC fires on every wake; RESUMESUSPEND follows it only when the
// wake was user-initiated.
const PBT_APMRESUMESUSPEND: usize = 0x7;
const PBT_APMRESUMEAUTOMATIC: usize = 0x12;

const MODIFIER_KEYS: [(VIRTUAL_KEY, Modifiers); 8] = [
    (VK_LWIN, Modifiers::CMD_LEFT),
    (VK_RWIN, Modifiers::CMD_RIGHT),
    (VK_LSHIFT, Modifiers::SHIFT_LEFT),
    (VK_RSHIFT, Modifiers::SHIFT_RIGHT),
    (VK_LCONTROL, Modifiers::CTRL_LEFT),
    (VK_RCONTROL, Modifiers::CTRL_RIGHT),
    (VK_LMENU, Modifiers::OPT_LEFT),
    (VK_RMENU, Modifiers::OPT_RIGHT),
];

struct HookContext {
    event_sender: Sender<KeyEvent>,
    current_modifiers: Modifiers,
    blocking_hotkeys: Option<BlockingHotkeys>,
    altgr_phantom_ctrl: bool,
    menu_mask_sent: bool,
}

thread_local! {
    static HOOK_CONTEXT: std::cell::RefCell<Option<HookContext>> = const { std::cell::RefCell::new(None) };

    static REINSTALL_REQUESTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn take_reinstall_request() -> bool {
    REINSTALL_REQUESTED.with(|flag| flag.replace(false))
}

#[derive(Default)]
struct DrainOutcome {
    quit: bool,
    reconcile: bool,
    reinstall_hooks: bool,
}

fn drain_thread_messages(msg: &mut MSG) -> DrainOutcome {
    let mut outcome = DrainOutcome::default();
    unsafe {
        while PeekMessageW(msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                outcome.quit = true;
                return outcome;
            }
            if msg.message == WM_WTSSESSION_CHANGE {
                match msg.wParam.0 {
                    WTS_SESSION_LOCK => outcome.reconcile = true,
                    WTS_SESSION_UNLOCK | WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT => {
                        outcome.reconcile = true;
                        outcome.reinstall_hooks = true;
                    }
                    _ => {}
                }
            }
            // WM_POWERBROADCAST is documented as sent, not posted, so the
            // wndproc is the expected delivery path; this arm is a cheap
            // safety net in case some Windows build posts it instead.
            if msg.message == WM_POWERBROADCAST
                && matches!(msg.wParam.0, PBT_APMRESUMESUSPEND | PBT_APMRESUMEAUTOMATIC)
            {
                outcome.reconcile = true;
                outcome.reinstall_hooks = true;
            }
            let _ = TranslateMessage(msg);
            DispatchMessageW(msg);
        }
    }
    outcome
}

fn wait_for_message_or_timeout(timeout_ms: u32) {
    unsafe {
        let _ = MsgWaitForMultipleObjects(None, false, timeout_ms, QS_ALLINPUT);
    }
}

fn physical_modifiers() -> Modifiers {
    let mut held = Modifiers::empty();
    for (vk, modifier) in MODIFIER_KEYS {
        if unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000 != 0 {
            held |= modifier;
        }
    }
    held
}

/// Physically-held modifiers we missed the press of and should adopt.
///
/// While AltGr's phantom Left Ctrl is held, CTRL_LEFT is excluded: it is
/// physically down per GetAsyncKeyState, but the user only pressed Right
/// Alt. A real Left Ctrl press during that window still sets CTRL_LEFT
/// through its own hook event (which carries a non-phantom scancode).
fn adoptable_modifiers(
    tracked: Modifiers,
    physical: Modifiers,
    altgr_phantom_ctrl: bool,
) -> Modifiers {
    let mut adopt = physical & RECONCILABLE & !tracked;
    if altgr_phantom_ctrl {
        adopt &= !Modifiers::CTRL_LEFT;
    }
    adopt
}

/// Corrects drift from missed events: secure desktop transitions swallow
/// key-ups (Win+L delivers the Win key DOWN but its UP happens on the secure
/// desktop), leaving modifiers stuck on until the user happens to press them
/// Known micro-race: GetAsyncKeyState reads the state *now*, not at the time
/// the event being processed was generated, so a modifier pressed in the
/// sub-millisecond window between a keystroke entering the queue and its hook
/// callback running gets adopted onto that earlier keystroke's event. It is
/// self-correcting (the modifier's own event arrives right after) and
/// unavoidable with this API — macOS avoids it because CGEvent flags are
/// stamped per event.
fn reconcile_modifiers(ctx: &mut HookContext) {
    let physical = physical_modifiers();
    if ctx.altgr_phantom_ctrl && !physical.contains(Modifiers::CTRL_LEFT) {
        ctx.altgr_phantom_ctrl = false;
    }
    for event in release_events(
        ctx.current_modifiers,
        stale_modifiers(ctx.current_modifiers, physical),
    ) {
        ctx.current_modifiers = event.modifiers;
        let _ = ctx.event_sender.send(event);
    }
    ctx.current_modifiers |=
        adoptable_modifiers(ctx.current_modifiers, physical, ctx.altgr_phantom_ctrl);
    if !ctx.current_modifiers.intersects(MENU_MODIFIERS) {
        ctx.menu_mask_sent = false;
    }
}

fn reconcile_modifiers_in_context() {
    HOOK_CONTEXT.with(|ctx_cell| {
        if let Some(ctx) = ctx_cell.borrow_mut().as_mut() {
            reconcile_modifiers(ctx);
        }
    });
}

/// Wndproc for the watcher window. (The `windows` crate's DefWindowProcW is
/// a generic Rust wrapper, so it cannot be used as lpfnWndProc directly.)
///
/// Handles both delivery modes together with the drain loop: the drain loop
/// covers posted delivery of WM_WTSSESSION_CHANGE (what Windows 11 26200
/// does, verified live), while this path covers builds that deliver it via
/// SendMessage, which bypasses the message queue — and WM_POWERBROADCAST,
/// which is documented as always sent. Double handling on the posted path is
/// harmless: the second reconcile finds nothing stale, and re-install
/// requests coalesce through REINSTALL_REQUESTED. The re-install itself
/// stays on the message loop, which owns the hook handles; this wndproc only
/// requests it.
unsafe extern "system" fn watcher_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_WTSSESSION_CHANGE => match wparam.0 {
                WTS_SESSION_LOCK => reconcile_modifiers_in_context(),
                WTS_SESSION_UNLOCK | WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT => {
                    reconcile_modifiers_in_context();
                    REINSTALL_REQUESTED.with(|flag| flag.set(true));
                }
                _ => {}
            },
            WM_POWERBROADCAST => {
                if matches!(wparam.0, PBT_APMRESUMESUSPEND | PBT_APMRESUMEAUTOMATIC) {
                    reconcile_modifiers_in_context();
                    REINSTALL_REQUESTED.with(|flag| flag.set(true));
                }
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

/// Create a hidden top-level window that watches WTS session changes and
/// power (suspend/resume) broadcasts. Top-level, not message-only, is load-
/// bearing: broadcast messages such as WM_POWERBROADCAST are never delivered
/// to message-only windows. Without WS_VISIBLE it has no taskbar or Alt-Tab
/// presence. WM_WTSSESSION_CHANGE is posted to it and picked out of the
/// queue by `drain_thread_messages` (verified live: lock 0x7 and unlock 0x8
/// both arrive); WM_POWERBROADCAST is sent and lands in `watcher_wndproc`.
///
/// Returns None on failure. Non-fatal: hooks still work, and stale modifiers
/// are still corrected lazily by `reconcile_modifiers` on the next key event.
unsafe fn create_watcher_window() -> Option<HWND> {
    unsafe {
        let class_name: Vec<u16> = "HandyKeysSystemWatcher\0".encode_utf16().collect();
        let wnd_class = WNDCLASSW {
            lpfnWndProc: Some(watcher_wndproc),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&wnd_class);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR::null(),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
        )
        .ok()?;

        if WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION).is_err() {
            let _ = DestroyWindow(hwnd);
            return None;
        }

        Some(hwnd)
    }
}

unsafe fn destroy_watcher_window(hwnd: HWND) {
    unsafe {
        let _ = WTSUnRegisterSessionNotification(hwnd);
        let _ = DestroyWindow(hwnd);
    }
}

/// Re-install the low-level hooks, defensively: Windows silently removes an LL
/// hook whose callback exceeds its timeout budget, and a session away from the
/// interactive desktop or a suspend/resume cycle is a common moment for that
/// to surface (Handy #1620). (Note lock/unlock does NOT inherently invalidate
/// hooks -- they survived it in live testing.)
///
/// The replacements are installed before the old hooks are removed, so a
/// failure never leaves us hook-less. No messages are pumped between install
/// and unhook, so no event is delivered twice.
unsafe fn reinstall_hooks(kb_hook: &mut HHOOK, mouse_hook: &mut HHOOK) -> bool {
    unsafe {
        let new_kb = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), None, 0) {
            Ok(h) => h,
            Err(_) => return false,
        };
        let new_mouse = match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), None, 0) {
            Ok(h) => h,
            Err(_) => {
                let _ = UnhookWindowsHookEx(new_kb);
                return false;
            }
        };
        let _ = UnhookWindowsHookEx(*kb_hook);
        let _ = UnhookWindowsHookEx(*mouse_hook);
        *kb_hook = new_kb;
        *mouse_hook = new_mouse;
        true
    }
}

pub(crate) struct WindowsListenerState {
    pub event_receiver: mpsc::Receiver<KeyEvent>,
    pub thread_handle: Option<JoinHandle<()>>,
    pub running: Arc<AtomicBool>,
}

pub(crate) fn spawn(blocking_hotkeys: Option<BlockingHotkeys>) -> Result<WindowsListenerState> {
    let (tx, rx) = mpsc::channel();
    let running = Arc::new(AtomicBool::new(true));
    let thread_running = Arc::clone(&running);
    let thread_blocking = blocking_hotkeys;

    let handle = thread::spawn(move || {
        HOOK_CONTEXT.with(|ctx| {
            *ctx.borrow_mut() = Some(HookContext {
                event_sender: tx,
                current_modifiers: Modifiers::empty(),
                blocking_hotkeys: thread_blocking,
                altgr_phantom_ctrl: false,
                menu_mask_sent: false,
            });
        });

        let kb_hook =
            unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), None, 0) };

        let mut kb_hook = match kb_hook {
            Ok(h) => h,
            Err(e) => {
                log::error!("Failed to install keyboard hook: {:?}", e);
                return;
            }
        };

        let mouse_hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), None, 0) };

        let mut mouse_hook = match mouse_hook {
            Ok(h) => h,
            Err(e) => {
                log::error!("Failed to install mouse hook: {:?}", e);
                unsafe {
                    let _ = UnhookWindowsHookEx(kb_hook);
                }
                return;
            }
        };

        let watcher_hwnd = unsafe { create_watcher_window() };

        let mut msg = MSG::default();
        loop {
            if !thread_running.load(Ordering::SeqCst) {
                break;
            }

            let outcome = drain_thread_messages(&mut msg);
            if outcome.quit {
                break;
            }
            if outcome.reconcile {
                reconcile_modifiers_in_context();
            }
            if outcome.reinstall_hooks || take_reinstall_request() {
                unsafe {
                    if reinstall_hooks(&mut kb_hook, &mut mouse_hook) {
                        log::info!("re-armed hooks after session/power change");
                    } else {
                        log::warn!("failed to re-install hooks after session/power change");
                    }
                }
            }

            wait_for_message_or_timeout(HOOK_LOOP_TIMEOUT_MS);
        }

        if let Some(hwnd) = watcher_hwnd {
            unsafe {
                destroy_watcher_window(hwnd);
            }
        }
        unsafe {
            let _ = UnhookWindowsHookEx(kb_hook);
            let _ = UnhookWindowsHookEx(mouse_hook);
        }

        HOOK_CONTEXT.with(|ctx| {
            *ctx.borrow_mut() = None;
        });
    });

    Ok(WindowsListenerState {
        event_receiver: rx,
        thread_handle: Some(handle),
        running,
    })
}

/// This function is called by Windows for every keyboard event system-wide.
/// It must return quickly to avoid input lag.
unsafe extern "system" fn keyboard_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code < 0 {
            return CallNextHookEx(None, code, wparam, lparam);
        }

        let mut should_block = false;
        let mut needs_mask = false;

        HOOK_CONTEXT.with(|ctx_cell| {
            let mut ctx_ref = ctx_cell.borrow_mut();
            if let Some(ctx) = ctx_ref.as_mut() {
                let kb_struct = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
                let vk_code = kb_struct.vkCode as u16;
                let scan_code = kb_struct.scanCode;
                let is_extended = (kb_struct.flags.0 & LLKHF_EXTENDED.0) != 0;

                let is_key_down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);

                if vk_code == MENU_MASK_VK && kb_struct.dwExtraInfo == MENU_MASK_EXTRA_INFO {
                    return;
                }

                if is_altgr_phantom_ctrl(vk_code, scan_code) {
                    ctx.altgr_phantom_ctrl = is_key_down;
                    return;
                }

                if let Some(modifier) = vk_to_modifier(vk_code) {
                    let prev_modifiers = ctx.current_modifiers;

                    if is_key_down {
                        ctx.current_modifiers |= modifier;
                    } else {
                        ctx.current_modifiers &= !modifier;
                    }

                    if ctx.current_modifiers != prev_modifiers {
                        should_block =
                            should_block_hotkey(&ctx.blocking_hotkeys, ctx.current_modifiers, None);

                        if needs_menu_mask(should_block, ctx.current_modifiers, ctx.menu_mask_sent)
                        {
                            ctx.menu_mask_sent = true;
                            needs_mask = true;
                        }

                        let _ = ctx.event_sender.send(KeyEvent {
                            modifiers: ctx.current_modifiers,
                            key: None,
                            is_key_down,
                        });
                    }

                    if !ctx.current_modifiers.intersects(MENU_MODIFIERS) {
                        ctx.menu_mask_sent = false;
                    }
                } else {
                    reconcile_modifiers(ctx);

                    if let Some(key) = map_key(vk_code, scan_code, is_extended) {
                        should_block = should_block_hotkey(
                            &ctx.blocking_hotkeys,
                            ctx.current_modifiers,
                            Some(key),
                        );

                        if needs_menu_mask(should_block, ctx.current_modifiers, ctx.menu_mask_sent)
                        {
                            ctx.menu_mask_sent = true;
                            needs_mask = true;
                        }

                        let _ = ctx.event_sender.send(KeyEvent {
                            modifiers: ctx.current_modifiers,
                            key: Some(key),
                            is_key_down,
                        });
                    }
                }
            }
        });

        if needs_mask {
            send_menu_mask();
        }

        if should_block {
            LRESULT(1)
        } else {
            CallNextHookEx(None, code, wparam, lparam)
        }
    }
}

unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code < 0 {
            return CallNextHookEx(None, code, wparam, lparam);
        }

        if !matches!(
            wparam.0 as u32,
            WM_LBUTTONDOWN
                | WM_LBUTTONUP
                | WM_RBUTTONDOWN
                | WM_RBUTTONUP
                | WM_MBUTTONDOWN
                | WM_MBUTTONUP
                | WM_XBUTTONDOWN
                | WM_XBUTTONUP
        ) {
            return CallNextHookEx(None, code, wparam, lparam);
        }

        let mut should_block = false;
        let mut needs_mask = false;

        HOOK_CONTEXT.with(|ctx_cell| {
            let mut ctx_ref = ctx_cell.borrow_mut();
            if let Some(ctx) = ctx_ref.as_mut() {
                let mouse_struct = &*(lparam.0 as *const MSLLHOOKSTRUCT);

                reconcile_modifiers(ctx);

                let has_modifiers = !ctx.current_modifiers.is_empty();

                let (key, is_down) = match wparam.0 as u32 {
                    WM_LBUTTONDOWN if has_modifiers => (Some(Key::MouseLeft), true),
                    WM_LBUTTONUP if has_modifiers => (Some(Key::MouseLeft), false),
                    WM_RBUTTONDOWN if has_modifiers => (Some(Key::MouseRight), true),
                    WM_RBUTTONUP if has_modifiers => (Some(Key::MouseRight), false),
                    WM_MBUTTONDOWN => (Some(Key::MouseMiddle), true),
                    WM_MBUTTONUP => (Some(Key::MouseMiddle), false),
                    WM_XBUTTONDOWN => {
                        let xbutton = (mouse_struct.mouseData >> 16) & 0xFFFF;
                        let key = if xbutton == 1 {
                            Some(Key::MouseX1)
                        } else if xbutton == 2 {
                            Some(Key::MouseX2)
                        } else {
                            None
                        };
                        (key, true)
                    }
                    WM_XBUTTONUP => {
                        let xbutton = (mouse_struct.mouseData >> 16) & 0xFFFF;
                        let key = if xbutton == 1 {
                            Some(Key::MouseX1)
                        } else if xbutton == 2 {
                            Some(Key::MouseX2)
                        } else {
                            None
                        };
                        (key, false)
                    }
                    _ => (None, false),
                };

                if let Some(key) = key {
                    should_block = should_block_hotkey(
                        &ctx.blocking_hotkeys,
                        ctx.current_modifiers,
                        Some(key),
                    );

                    if needs_menu_mask(should_block, ctx.current_modifiers, ctx.menu_mask_sent) {
                        ctx.menu_mask_sent = true;
                        needs_mask = true;
                    }

                    let _ = ctx.event_sender.send(KeyEvent {
                        modifiers: ctx.current_modifiers,
                        key: Some(key),
                        is_key_down: is_down,
                    });
                }
            }
        });

        if needs_mask {
            send_menu_mask();
        }

        if should_block {
            LRESULT(1)
        } else {
            CallNextHookEx(None, code, wparam, lparam)
        }
    }
}

fn needs_menu_mask(should_block: bool, modifiers: Modifiers, already_sent: bool) -> bool {
    should_block && !already_sent && modifiers.intersects(MENU_MODIFIERS)
}

fn send_menu_mask() {
    let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(MENU_MASK_VK),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: MENU_MASK_EXTRA_INFO,
            },
        },
    };
    let inputs = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        log::warn!("menu mask injection failed");
    }
}

fn should_block_hotkey(
    blocking_hotkeys: &Option<BlockingHotkeys>,
    modifiers: Modifiers,
    key: Option<Key>,
) -> bool {
    if let Some(hotkeys) = blocking_hotkeys {
        if let Ok(set) = hotkeys.lock() {
            return set
                .iter()
                .any(|h| h.modifiers.matches(modifiers) && h.key == key);
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use windows::Win32::UI::WindowsAndMessaging::PostQuitMessage;

    fn clear_message_queue() {
        let mut msg = MSG::default();
        unsafe { while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {} }
    }

    #[test]
    fn wait_times_out_when_no_messages() {
        clear_message_queue();
        let start = Instant::now();
        wait_for_message_or_timeout(20);
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(8),
            "expected wait to block close to timeout, elapsed={elapsed:?}"
        );
        clear_message_queue();
    }

    #[test]
    fn wait_returns_immediately_when_message_is_pending() {
        clear_message_queue();
        unsafe {
            PostQuitMessage(0);
        }
        let start = Instant::now();
        wait_for_message_or_timeout(200);
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(100),
            "expected pending message to wake wait early, elapsed={elapsed:?}"
        );
        clear_message_queue();
    }

    #[test]
    fn drain_messages_stops_on_wm_quit() {
        clear_message_queue();
        unsafe {
            PostQuitMessage(0);
        }
        let mut msg = MSG::default();
        assert!(drain_thread_messages(&mut msg).quit);
        clear_message_queue();
    }

    #[test]
    fn should_block_registered_mouse_hotkey() {
        use crate::shortcut::keys::types::Hotkey;
        let mut hotkeys = std::collections::HashSet::new();
        hotkeys.insert(Hotkey::new(Modifiers::empty(), Key::MouseX1).unwrap());
        let blocking_hotkeys = Some(std::sync::Arc::new(std::sync::Mutex::new(hotkeys)));

        assert!(should_block_hotkey(
            &blocking_hotkeys,
            Modifiers::empty(),
            Some(Key::MouseX1)
        ));
        assert!(!should_block_hotkey(
            &blocking_hotkeys,
            Modifiers::empty(),
            Some(Key::MouseX2)
        ));
    }

    fn post_thread_message(message: u32, wparam: usize) {
        use windows::Win32::System::Threading::GetCurrentThreadId;
        use windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
        unsafe {
            PostThreadMessageW(GetCurrentThreadId(), message, WPARAM(wparam), LPARAM(0)).unwrap();
        }
    }

    #[test]
    fn drain_reports_session_unlock_with_reinstall() {
        clear_message_queue();
        post_thread_message(WM_WTSSESSION_CHANGE, WTS_SESSION_UNLOCK);
        let mut msg = MSG::default();
        let outcome = drain_thread_messages(&mut msg);
        assert!(outcome.reconcile);
        assert!(outcome.reinstall_hooks);
        assert!(!outcome.quit);
        clear_message_queue();
    }

    #[test]
    fn drain_reports_session_lock_without_reinstall() {
        clear_message_queue();
        post_thread_message(WM_WTSSESSION_CHANGE, WTS_SESSION_LOCK);
        let mut msg = MSG::default();
        let outcome = drain_thread_messages(&mut msg);
        assert!(outcome.reconcile);
        assert!(!outcome.reinstall_hooks);
        clear_message_queue();
    }

    #[test]
    fn drain_ignores_unrelated_session_events() {
        clear_message_queue();
        post_thread_message(WM_WTSSESSION_CHANGE, 0x6);
        let mut msg = MSG::default();
        let outcome = drain_thread_messages(&mut msg);
        assert!(!outcome.reconcile);
        assert!(!outcome.reinstall_hooks);
        clear_message_queue();
    }

    #[test]
    fn drain_reports_resume_with_reinstall() {
        clear_message_queue();
        post_thread_message(WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC);
        let mut msg = MSG::default();
        let outcome = drain_thread_messages(&mut msg);
        assert!(outcome.reconcile);
        assert!(outcome.reinstall_hooks);
        assert!(!outcome.quit);
        clear_message_queue();
    }

    #[test]
    fn drain_ignores_non_resume_power_events() {
        clear_message_queue();
        post_thread_message(WM_POWERBROADCAST, 0x4);
        let mut msg = MSG::default();
        let outcome = drain_thread_messages(&mut msg);
        assert!(!outcome.reconcile);
        assert!(!outcome.reinstall_hooks);
        clear_message_queue();
    }

    fn send_to_wndproc(msg: u32, wparam: usize) {
        unsafe {
            watcher_wndproc(HWND::default(), msg, WPARAM(wparam), LPARAM(0));
        }
    }

    #[test]
    fn wndproc_requests_reinstall_on_resume() {
        let _ = take_reinstall_request();
        send_to_wndproc(WM_POWERBROADCAST, PBT_APMRESUMESUSPEND);
        assert!(take_reinstall_request());
        assert!(!take_reinstall_request());
    }

    #[test]
    fn wndproc_requests_reinstall_on_session_unlock() {
        let _ = take_reinstall_request();
        send_to_wndproc(WM_WTSSESSION_CHANGE, WTS_SESSION_UNLOCK);
        assert!(take_reinstall_request());
    }

    #[test]
    fn wndproc_does_not_request_reinstall_on_lock_or_suspend() {
        let _ = take_reinstall_request();
        send_to_wndproc(WM_WTSSESSION_CHANGE, WTS_SESSION_LOCK);
        send_to_wndproc(WM_POWERBROADCAST, 0x4);
        assert!(!take_reinstall_request());
    }

    #[test]
    fn adoption_picks_up_missed_presses() {
        let adopt = adoptable_modifiers(
            Modifiers::CTRL_LEFT,
            Modifiers::CTRL_LEFT | Modifiers::SHIFT_LEFT,
            false,
        );
        assert_eq!(adopt, Modifiers::SHIFT_LEFT);
    }

    #[test]
    fn adoption_skips_ctrl_left_while_altgr_phantom_held() {
        let adopt = adoptable_modifiers(
            Modifiers::OPT_RIGHT,
            Modifiers::CTRL_LEFT | Modifiers::OPT_RIGHT,
            true,
        );
        assert_eq!(adopt, Modifiers::empty());
    }

    #[test]
    fn adoption_keeps_other_modifiers_while_altgr_phantom_held() {
        let adopt = adoptable_modifiers(
            Modifiers::OPT_RIGHT,
            Modifiers::CTRL_LEFT | Modifiers::OPT_RIGHT | Modifiers::SHIFT_LEFT,
            true,
        );
        assert_eq!(adopt, Modifiers::SHIFT_LEFT);
    }

    #[test]
    fn menu_mask_fires_for_blocked_key_under_win() {
        assert!(needs_menu_mask(true, Modifiers::CMD_LEFT, false));
    }

    #[test]
    fn menu_mask_fires_under_alt() {
        // Alt has the sibling heuristic: a lone Alt tap focuses the menu bar.
        assert!(needs_menu_mask(
            true,
            Modifiers::OPT_LEFT | Modifiers::SHIFT_LEFT,
            false
        ));
    }

    #[test]
    fn menu_mask_only_when_blocking() {
        assert!(!needs_menu_mask(false, Modifiers::CMD_LEFT, false));
    }

    #[test]
    fn menu_mask_once_per_hold() {
        assert!(!needs_menu_mask(true, Modifiers::CMD_LEFT, true));
    }

    #[test]
    fn menu_mask_not_for_ctrl_shift_combos() {
        assert!(!needs_menu_mask(
            true,
            Modifiers::CTRL_LEFT | Modifiers::SHIFT_LEFT,
            false
        ));
    }

    #[test]
    fn menu_mask_vk_is_inert_to_our_own_mapping() {
        assert!(vk_to_modifier(MENU_MASK_VK).is_none());
        assert!(map_key(MENU_MASK_VK, 0, false).is_none());
    }

    #[test]
    fn reconcile_clears_mask_flag_when_hold_ends_off_desktop() {
        let (tx, rx) = mpsc::channel();
        let mut ctx = HookContext {
            event_sender: tx,
            current_modifiers: Modifiers::CMD_LEFT,
            blocking_hotkeys: None,
            altgr_phantom_ctrl: false,
            menu_mask_sent: true,
        };
        reconcile_modifiers(&mut ctx);
        assert_eq!(ctx.current_modifiers, Modifiers::empty());
        assert!(!ctx.menu_mask_sent);
        let release = rx.try_recv().expect("expected a synthetic release");
        assert!(!release.is_key_down);
    }

    #[test]
    fn phantom_flag_never_releases_a_tracked_ctrl() {
        let tracked = Modifiers::CTRL_LEFT | Modifiers::OPT_RIGHT;
        let physical = Modifiers::CTRL_LEFT | Modifiers::OPT_RIGHT;
        assert_eq!(stale_modifiers(tracked, physical), Modifiers::empty());
        assert_eq!(
            adoptable_modifiers(tracked, physical, true),
            Modifiers::empty()
        );
    }
}
