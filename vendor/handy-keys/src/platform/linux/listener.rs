//! A single thread polls every keyboard-capable `/dev/input/event*` device
//! plus an inotify watch on `/dev/input` for hotplug. Reading evdev
//! directly needs no X11 or Wayland connection, so the listener works
//! identically on both and on a bare console; it requires read access to
//! the device nodes (typically membership in the `input` group).
//!

use std::collections::HashSet;
use std::ffi::CString;
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use evdev::raw_stream::RawDevice;
use evdev::uinput::VirtualDevice;
use evdev::{
    AbsoluteAxisCode, EventSummary, EventType, InputEvent, KeyCode, RelativeAxisCode,
    SynchronizationCode,
};

use crate::error::{Error, Result};
use crate::platform::state::{release_events, stale_modifiers, RECONCILABLE};
use crate::platform::state::{BlockingHotkeys, ListenerState};
use crate::types::{Key, KeyEvent, Modifiers};

use super::keycode::{key_code_to_key, key_code_to_modifier};

const DEV_INPUT: &str = "/dev/input";

const POLL_TIMEOUT_MS: i32 = 100;

const MODIFIER_KEYS: [(KeyCode, Modifiers); 8] = [
    (KeyCode::KEY_LEFTMETA, Modifiers::CMD_LEFT),
    (KeyCode::KEY_RIGHTMETA, Modifiers::CMD_RIGHT),
    (KeyCode::KEY_LEFTSHIFT, Modifiers::SHIFT_LEFT),
    (KeyCode::KEY_RIGHTSHIFT, Modifiers::SHIFT_RIGHT),
    (KeyCode::KEY_LEFTCTRL, Modifiers::CTRL_LEFT),
    (KeyCode::KEY_RIGHTCTRL, Modifiers::CTRL_RIGHT),
    (KeyCode::KEY_LEFTALT, Modifiers::OPT_LEFT),
    (KeyCode::KEY_RIGHTALT, Modifiers::OPT_RIGHT),
];

const CLONE_NAME_PREFIX: &str = "handy-keys passthrough: ";

/// Kernel cap on a uinput device name (`UINPUT_MAX_NAME_SIZE`). The evdev
/// crate enforces it with a hard `assert!(name.len() + 1 < 80)` inside
/// `VirtualDevice::builder().name(..)`, so an over-long name is a *panic*,
/// not an `Err` — which kills the whole listener thread and silently
/// disables every hotkey. We truncate before building so the name always
/// fits: the usable budget is 78 bytes (80 - 1 for the NUL - the `<`, not
/// `<=`). Source device names longer than 78 - prefix bytes are common in
/// practice (e.g. input-remapper's `input-remapper <make> <model> forwarded`
/// virtual keyboards), so this is a real crash, not a theoretical one.
const MAX_CLONE_NAME_BYTES: usize = 78;

const KEY_RELEASED: i32 = 0;
const KEY_PRESSED: i32 = 1;
const KEY_AUTOREPEAT: i32 = 2;

struct GrabConfig {
    own_outputs: HashSet<PathBuf>,
}

struct LinuxState {
    listener: ListenerState,
    blocked_keys: HashSet<KeyCode>,
    blocked_modifiers: Modifiers,
}

struct OpenDevice {
    path: PathBuf,
    dev: RawDevice,
    /// The kernel reported SYN_DROPPED: events were lost. Per the evdev
    /// protocol, everything up to and including the next SYN_REPORT is
    /// unreliable for state tracking; modifier state is then reconciled
    /// against EVIOCGKEY. (The unreliable events are still forwarded to a
    /// grabbed device's clone — the user really typed them.)
    resyncing: bool,
    output: Option<VirtualDevice>,
    output_nodes: Vec<PathBuf>,
    /// This keyboard should be grabbed, but keys were physically held when
    /// it was opened. Grabbing mid-press would split the press/release pair
    /// across two devices and wedge the key at the compositor (libinput
    /// discards unbalanced key-ups), so the device stays read-only until
    /// EVIOCGKEY reports it quiet — the pending release itself wakes the
    /// poll loop, and the grab completes then.
    pending_grab: bool,
    pending: Vec<InputEvent>,
}

pub(crate) struct LinuxListenerState {
    pub event_receiver: Receiver<KeyEvent>,
    pub thread_handle: Option<JoinHandle<()>>,
    pub running: Arc<AtomicBool>,
    pub blocking_hotkeys: Option<BlockingHotkeys>,
}

pub(crate) fn spawn(blocking_hotkeys: Option<BlockingHotkeys>) -> Result<LinuxListenerState> {
    let mut grab = blocking_hotkeys.as_ref().map(|_| GrabConfig {
        own_outputs: HashSet::new(),
    });
    if grab.is_some() {
        check_uinput_access()?;
    }

    let inotify = Inotify::watch(DEV_INPUT).map_err(|e| {
        Error::Platform(format!(
            "failed to watch {DEV_INPUT} for device hotplug: {e}"
        ))
    })?;
    let devices = open_capturable_devices(&mut grab)?;

    let (tx, rx) = mpsc::channel();
    let mut state = LinuxState {
        listener: ListenerState::new(tx, blocking_hotkeys.clone()),
        blocked_keys: HashSet::new(),
        blocked_modifiers: Modifiers::empty(),
    };
    state.listener.current_modifiers = physical_modifiers(&devices);

    let running = Arc::new(AtomicBool::new(true));
    let thread_running = Arc::clone(&running);

    let handle = thread::spawn(move || {
        run_loop(devices, inotify, grab, state, thread_running);
    });

    Ok(LinuxListenerState {
        event_receiver: rx,
        thread_handle: Some(handle),
        running,
        blocking_hotkeys,
    })
}

fn check_uinput_access() -> Result<()> {
    // VirtualDevice::builder() opens /dev/uinput; nothing is created yet.
    VirtualDevice::builder().map(|_| ()).map_err(|e| {
        Error::Platform(format!(
            "hotkey blocking requires write access to /dev/uinput (non-blocked keystrokes are \
             re-injected through it): {e}. Grant it with a udev rule (e.g. KERNEL==\"uinput\", \
             GROUP=\"input\", MODE=\"0660\" plus input-group membership), or use the non-blocking \
             listener"
        ))
    })
}

fn open_capturable_devices(grab: &mut Option<GrabConfig>) -> Result<Vec<OpenDevice>> {
    let entries = fs::read_dir(DEV_INPUT)
        .map_err(|e| Error::Platform(format!("cannot read {DEV_INPUT}: {e}")))?;

    let mut devices = Vec::new();
    let mut permission_denied = 0usize;

    for entry in entries.flatten() {
        let path = entry.path();
        if !is_event_node(&path) {
            continue;
        }
        match open_device(&path, grab) {
            Ok(Some(device)) => devices.push(device),
            Ok(None) => {}
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => permission_denied += 1,
            Err(_) => {}
        }
    }

    if devices.is_empty() {
        return Err(Error::Platform(if permission_denied > 0 {
            format!(
                "permission denied opening {permission_denied} device node(s) under {DEV_INPUT}. \
                 Reading keyboard events requires read access to these nodes: add your user to \
                 the 'input' group (sudo usermod -aG input $USER) and log out and back in"
            )
        } else {
            format!("no keyboard-capable input devices found under {DEV_INPUT}")
        }));
    }

    Ok(devices)
}

fn is_event_node(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("event"))
}

fn open_device(path: &Path, grab: &mut Option<GrabConfig>) -> io::Result<Option<OpenDevice>> {
    if let Some(cfg) = grab {
        if cfg.own_outputs.contains(path) {
            return Ok(None);
        }
    }
    let dev = RawDevice::open(path)?;
    if !is_capturable(&dev) {
        return Ok(None);
    }
    set_nonblocking(dev.as_raw_fd())?;

    let mut device = OpenDevice {
        path: path.to_path_buf(),
        dev,
        resyncing: false,
        output: None,
        output_nodes: Vec::new(),
        pending_grab: false,
        pending: Vec::new(),
    };

    if let Some(cfg) = grab {
        if is_passthrough_clone(&device.dev) {
            eprintln!(
                "handy-keys: {} is another handy-keys blocking listener's output; its hotkeys \
                 will be detected but not blocked by this listener",
                path.display()
            );
        } else if is_keyboard(&device.dev) {
            if keys_held(&device.dev) {
                device.pending_grab = true;
            } else {
                complete_grab(&mut device, cfg);
            }
        } else if has_keyboard_keys(&device.dev) {
            eprintln!(
                "handy-keys: {} exposes keyboard keys on a pointer device; its hotkeys will be \
                 detected but not blocked (pointer devices are never grabbed)",
                path.display()
            );
        }
    }

    Ok(Some(device))
}

fn complete_grab(device: &mut OpenDevice, cfg: &mut GrabConfig) {
    device.pending_grab = false;
    match grab_with_output(&mut device.dev) {
        Ok((output, nodes)) => {
            cfg.own_outputs.extend(nodes.iter().cloned());
            device.output = Some(output);
            device.output_nodes = nodes;
        }
        Err(e) => {
            eprintln!(
                "handy-keys: cannot grab {} for hotkey blocking ({e}); its hotkeys will be \
                 detected but not blocked",
                device.path.display()
            );
        }
    }
}

fn keys_held(dev: &RawDevice) -> bool {
    dev.get_key_state()
        .map(|keys| keys.iter().next().is_some())
        .unwrap_or(false)
}

fn is_passthrough_clone(dev: &RawDevice) -> bool {
    dev.name()
        .is_some_and(|name| name.starts_with(CLONE_NAME_PREFIX))
}

fn is_capturable(dev: &RawDevice) -> bool {
    if !dev.supported_events().contains(EventType::KEY) {
        return false;
    }
    match dev.supported_keys() {
        Some(keys) => keys
            .iter()
            .any(|code| key_code_to_key(code).is_some() || key_code_to_modifier(code).is_some()),
        None => false,
    }
}

fn is_keyboard(dev: &RawDevice) -> bool {
    let is_pointer = dev
        .supported_relative_axes()
        .is_some_and(|axes| axes.contains(RelativeAxisCode::REL_X))
        || dev
            .supported_absolute_axes()
            .is_some_and(|axes| axes.contains(AbsoluteAxisCode::ABS_X));
    !is_pointer && has_keyboard_keys(dev)
}

fn has_keyboard_keys(dev: &RawDevice) -> bool {
    dev.supported_keys().is_some_and(|keys| {
        keys.iter().any(|code| {
            key_code_to_modifier(code).is_some()
                || key_code_to_key(code).is_some_and(|key| {
                    !matches!(
                        key,
                        Key::MouseLeft
                            | Key::MouseRight
                            | Key::MouseMiddle
                            | Key::MouseX1
                            | Key::MouseX2
                            | Key::Mouse6
                            | Key::Mouse7
                            | Key::Mouse8
                    )
                })
        })
    })
}

fn truncate_on_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn grab_with_output(dev: &mut RawDevice) -> io::Result<(VirtualDevice, Vec<PathBuf>)> {
    let full_name = format!("{CLONE_NAME_PREFIX}{}", dev.name().unwrap_or("keyboard"));
    let name = truncate_on_char_boundary(&full_name, MAX_CLONE_NAME_BYTES);

    let mut builder = VirtualDevice::builder()?
        .name(name)
        .input_id(dev.input_id());
    if let Some(keys) = dev.supported_keys() {
        builder = builder.with_keys(keys)?;
    }
    if let Some(misc) = dev.misc_properties() {
        builder = builder.with_msc(misc)?;
    }
    let mut output = builder.build()?;
    // Blocks briefly until sysfs lists the clone's device node, so the
    // node is registered for self-filtering before inotify reports it.
    let nodes: Vec<PathBuf> = output
        .enumerate_dev_nodes_blocking()?
        .filter_map(|node| node.ok())
        .collect();

    // The kernel has queued events on our fd since open(), but until the
    // grab lands the compositor received them too — forwarding them would
    // deliver every pre-grab keystroke twice. Discard them (modifier state
    // is seeded from EVIOCGKEY, not from these events); only what arrives
    // after the grab is exclusively ours to re-inject.
    discard_pending_events(dev)?;

    dev.grab()?;
    Ok((output, nodes))
}

fn discard_pending_events(dev: &mut RawDevice) -> io::Result<()> {
    loop {
        match dev.fetch_events() {
            Ok(events) => {
                if events.count() == 0 {
                    return Ok(());
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn physical_modifiers(devices: &[OpenDevice]) -> Modifiers {
    let mut held = Modifiers::empty();
    for device in devices {
        if let Ok(keys) = device.dev.get_key_state() {
            for (code, modifier) in MODIFIER_KEYS {
                if keys.contains(code) {
                    held |= modifier;
                }
            }
        }
    }
    held
}

fn reconcile_modifiers(state: &mut LinuxState, physical: Modifiers) {
    for event in release_events(
        state.listener.current_modifiers,
        stale_modifiers(state.listener.current_modifiers, physical),
    ) {
        state.listener.current_modifiers = event.modifiers;
        let _ = state.listener.event_sender.send(event);
    }
    state.listener.current_modifiers |= physical & RECONCILABLE & !state.listener.current_modifiers;
    state.blocked_modifiers &= physical;
}

fn run_loop(
    mut devices: Vec<OpenDevice>,
    inotify: Inotify,
    mut grab: Option<GrabConfig>,
    mut state: LinuxState,
    running: Arc<AtomicBool>,
) {
    while running.load(Ordering::SeqCst) {
        let mut pollfds = Vec::with_capacity(devices.len() + 1);
        pollfds.push(libc::pollfd {
            fd: inotify.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        });
        for device in &devices {
            pollfds.push(libc::pollfd {
                fd: device.dev.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            });
        }

        let rc = unsafe { libc::poll(pollfds.as_mut_ptr(), pollfds.len() as _, POLL_TIMEOUT_MS) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::EAGAIN) | Some(libc::ENOMEM) => {
                    thread::sleep(std::time::Duration::from_millis(POLL_TIMEOUT_MS as u64));
                    continue;
                }
                _ => {
                    eprintln!("handy-keys: poll on input devices failed: {err}");
                    break;
                }
            }
        }
        if rc == 0 {
            continue;
        }

        let mut needs_reconcile = false;
        let mut dead = Vec::new();
        for (i, pollfd) in pollfds[1..].iter().enumerate() {
            if pollfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                dead.push(i);
                continue;
            }
            if pollfd.revents & libc::POLLIN == 0 {
                continue;
            }
            match drain_device(&mut devices[i], &mut grab, &mut state) {
                Ok(reconcile) => needs_reconcile |= reconcile,
                Err(_) => dead.push(i),
            }
        }

        if !dead.is_empty() {
            for i in dead.into_iter().rev() {
                let device = devices.remove(i);
                if let Some(cfg) = &mut grab {
                    for node in &device.output_nodes {
                        cfg.own_outputs.remove(node);
                    }
                }
            }
            needs_reconcile = true;
        }

        // Add new devices only after dead entries are gone: a replugged
        // keyboard typically reuses the same /dev/input/eventN path, and
        // when its removal and reappearance land in the same wakeup the
        // dedup check must not match the stale entry.
        if pollfds[0].revents & libc::POLLIN != 0 {
            for path in inotify.drain_event_nodes() {
                try_add_device(&mut devices, &path, &mut grab, &mut state);
            }
        }

        if needs_reconcile {
            reconcile_modifiers(&mut state, physical_modifiers(&devices));
        }
    }
}

fn drain_device(
    device: &mut OpenDevice,
    grab: &mut Option<GrabConfig>,
    state: &mut LinuxState,
) -> io::Result<bool> {
    let mut needs_reconcile = false;
    let mut saw_release = false;
    loop {
        match device.dev.fetch_events() {
            Ok(events) => {
                for event in events {
                    match event.destructure() {
                        EventSummary::Synchronization(_, SynchronizationCode::SYN_DROPPED, _) => {
                            device.resyncing = true;
                        }
                        EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _) => {
                            if device.resyncing {
                                device.resyncing = false;
                                needs_reconcile = true;
                            }
                            if let Some(output) = &mut device.output {
                                if !device.pending.is_empty() {
                                    if let Err(e) = output.emit(&device.pending) {
                                        eprintln!(
                                            "handy-keys: failed to re-inject events for {}: {e}",
                                            device.path.display()
                                        );
                                    }
                                    device.pending.clear();
                                }
                            }
                        }
                        EventSummary::Key(_, code, value) => {
                            saw_release |= value == KEY_RELEASED;
                            let block = if device.resyncing {
                                false
                            } else {
                                process_key(state, code, value)
                            };
                            if !block && device.output.is_some() {
                                device.pending.push(event);
                            }
                        }
                        _ => {
                            if device.output.is_some() {
                                device.pending.push(event);
                            }
                        }
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }

    if device.pending_grab && saw_release {
        if let Some(cfg) = grab {
            if !keys_held(&device.dev) {
                complete_grab(device, cfg);
            }
        }
    }
    Ok(needs_reconcile)
}

fn process_key(state: &mut LinuxState, code: KeyCode, value: i32) -> bool {
    let is_key_down = match value {
        KEY_RELEASED => false,
        KEY_PRESSED | KEY_AUTOREPEAT => true,
        _ => return false,
    };

    if let Some(changed_modifier) = key_code_to_modifier(code) {
        if value == KEY_AUTOREPEAT {
            return state.blocked_modifiers.contains(changed_modifier);
        }
        let prev_modifiers = state.listener.current_modifiers;
        if is_key_down {
            state.listener.current_modifiers |= changed_modifier;
        } else {
            state.listener.current_modifiers &= !changed_modifier;
        }

        let changed = state.listener.current_modifiers != prev_modifiers;
        if changed {
            let _ = state.listener.event_sender.send(KeyEvent {
                modifiers: state.listener.current_modifiers,
                key: None,
                is_key_down,
                changed_modifier: Some(changed_modifier),
            });
        }

        return if is_key_down {
            let block = changed
                && state
                    .listener
                    .should_block(state.listener.current_modifiers, None);
            if block {
                state.blocked_modifiers |= changed_modifier;
            } else {
                state.blocked_modifiers &= !changed_modifier;
            }
            block
        } else {
            let block = state.blocked_modifiers.contains(changed_modifier);
            state.blocked_modifiers &= !changed_modifier;
            block
        };
    }

    let Some(key) = key_code_to_key(code) else {
        return false;
    };

    let block = match value {
        KEY_PRESSED => {
            let block = state
                .listener
                .should_block(state.listener.current_modifiers, Some(key));
            if block {
                state.blocked_keys.insert(code);
            } else {
                state.blocked_keys.remove(&code);
            }
            block
        }
        KEY_AUTOREPEAT => state.blocked_keys.contains(&code),
        _ => state.blocked_keys.remove(&code),
    };

    if matches!(key, Key::MouseLeft | Key::MouseRight)
        && state.listener.current_modifiers.is_empty()
    {
        return block;
    }

    let _ = state.listener.event_sender.send(KeyEvent {
        modifiers: state.listener.current_modifiers,
        key: Some(key),
        is_key_down,
        changed_modifier: None,
    });

    block
}

/// Try to open a device node that just appeared (or became accessible) in
/// /dev/input. Failures are expected and silent: inotify fires IN_CREATE
/// before udev has granted the input group read access, and the follow-up
/// IN_ATTRIB after the chmod retries the open. Hotplugged keyboards are
/// grabbed like scanned ones when blocking is enabled.
fn try_add_device(
    devices: &mut Vec<OpenDevice>,
    path: &Path,
    grab: &mut Option<GrabConfig>,
    state: &mut LinuxState,
) {
    if devices.iter().any(|d| d.path == path) {
        return;
    }
    if let Ok(Some(device)) = open_device(path, grab) {
        state.listener.current_modifiers |= physical_modifiers(std::slice::from_ref(&device));
        devices.push(device);
    }
}

struct Inotify {
    fd: OwnedFd,
}

impl Inotify {
    fn watch(dir: &str) -> io::Result<Self> {
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };

        let c_dir = CString::new(dir).expect("watch dir contains no NUL");
        // IN_CREATE: a node appeared. IN_ATTRIB: udev adjusted its
        // ownership/mode, which is the moment opening typically first
        // succeeds for non-root users.
        let wd = unsafe {
            libc::inotify_add_watch(
                fd.as_raw_fd(),
                c_dir.as_ptr(),
                libc::IN_CREATE | libc::IN_ATTRIB,
            )
        };
        if wd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { fd })
    }

    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    fn drain_event_nodes(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = unsafe {
                libc::read(
                    self.fd.as_raw_fd(),
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                )
            };
            if n <= 0 {
                break;
            }

            let mut offset = 0usize;
            let header_len = std::mem::size_of::<libc::inotify_event>();
            while offset + header_len <= n as usize {
                let event = unsafe {
                    std::ptr::read_unaligned(buf.as_ptr().add(offset) as *const libc::inotify_event)
                };
                let name_start = offset + header_len;
                let name_end = name_start + event.len as usize;
                if name_end > n as usize {
                    break;
                }
                let name_bytes = &buf[name_start..name_end];
                let name_len = name_bytes
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(name_bytes.len());
                if let Ok(name) = std::str::from_utf8(&name_bytes[..name_len]) {
                    if name.starts_with("event") {
                        paths.push(Path::new(DEV_INPUT).join(name));
                    }
                }
                offset = name_end;
            }
        }
        paths.sort();
        paths.dedup();
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Hotkey;
    use std::sync::mpsc::Receiver;
    use std::sync::Mutex;

    fn state_and_receiver_with(
        hotkeys: &[Hotkey],
    ) -> (LinuxState, Receiver<KeyEvent>, BlockingHotkeys) {
        let (tx, rx) = mpsc::channel();
        let blocking: BlockingHotkeys =
            Arc::new(Mutex::new(hotkeys.iter().cloned().collect::<HashSet<_>>()));
        let state = LinuxState {
            listener: ListenerState::new(tx, Some(Arc::clone(&blocking))),
            blocked_keys: HashSet::new(),
            blocked_modifiers: Modifiers::empty(),
        };
        (state, rx, blocking)
    }

    fn state_and_receiver() -> (LinuxState, Receiver<KeyEvent>) {
        let (state, rx, _) = state_and_receiver_with(&[]);
        (state, rx)
    }

    fn recv_event(rx: &Receiver<KeyEvent>) -> KeyEvent {
        rx.try_recv().expect("expected a key event")
    }

    #[test]
    fn modifier_events_emit_only_when_state_changes() {
        let (mut state, rx) = state_and_receiver();

        process_key(&mut state, KeyCode::KEY_LEFTSHIFT, KEY_PRESSED);
        let event = recv_event(&rx);
        assert_eq!(event.modifiers, Modifiers::SHIFT_LEFT);
        assert_eq!(event.key, None);
        assert!(event.is_key_down);
        assert_eq!(event.changed_modifier, Some(Modifiers::SHIFT_LEFT));

        process_key(&mut state, KeyCode::KEY_LEFTSHIFT, KEY_PRESSED);
        assert!(rx.try_recv().is_err());

        process_key(&mut state, KeyCode::KEY_LEFTSHIFT, KEY_RELEASED);
        let event = recv_event(&rx);
        assert_eq!(event.modifiers, Modifiers::empty());
        assert!(!event.is_key_down);
        assert_eq!(event.changed_modifier, Some(Modifiers::SHIFT_LEFT));
    }

    #[test]
    fn modifier_autorepeat_is_silent() {
        let (mut state, rx) = state_and_receiver();

        process_key(&mut state, KeyCode::KEY_LEFTCTRL, KEY_PRESSED);
        let _ = recv_event(&rx);

        process_key(&mut state, KeyCode::KEY_LEFTCTRL, KEY_AUTOREPEAT);
        assert!(rx.try_recv().is_err());
        assert_eq!(state.listener.current_modifiers, Modifiers::CTRL_LEFT);
    }

    #[test]
    fn autorepeat_is_a_repeated_key_down() {
        let (mut state, rx) = state_and_receiver();

        process_key(&mut state, KeyCode::KEY_A, KEY_AUTOREPEAT);
        let event = recv_event(&rx);
        assert_eq!(event.key, Some(Key::A));
        assert!(event.is_key_down);
        assert_eq!(event.changed_modifier, None);
    }

    #[test]
    fn keys_carry_current_modifiers() {
        let (mut state, rx) = state_and_receiver();

        process_key(&mut state, KeyCode::KEY_LEFTMETA, KEY_PRESSED);
        let _ = recv_event(&rx);

        process_key(&mut state, KeyCode::KEY_F20, KEY_PRESSED);
        let event = recv_event(&rx);
        assert_eq!(event.key, Some(Key::F20));
        assert_eq!(event.modifiers, Modifiers::CMD_LEFT);
        assert!(event.is_key_down);
    }

    #[test]
    fn unmapped_keys_and_values_are_ignored() {
        let (mut state, rx) = state_and_receiver();

        process_key(&mut state, KeyCode::KEY_POWER, KEY_PRESSED);
        assert!(rx.try_recv().is_err());

        process_key(&mut state, KeyCode::KEY_A, 3);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn common_mouse_buttons_require_modifiers() {
        let (mut state, rx) = state_and_receiver();

        process_key(&mut state, KeyCode::BTN_LEFT, KEY_PRESSED);
        assert!(rx.try_recv().is_err());

        process_key(&mut state, KeyCode::KEY_LEFTSHIFT, KEY_PRESSED);
        let _ = recv_event(&rx);

        process_key(&mut state, KeyCode::BTN_LEFT, KEY_PRESSED);
        let event = recv_event(&rx);
        assert_eq!(event.key, Some(Key::MouseLeft));
        assert_eq!(event.modifiers, Modifiers::SHIFT_LEFT);
    }

    #[test]
    fn side_mouse_buttons_always_report() {
        let (mut state, rx) = state_and_receiver();

        process_key(&mut state, KeyCode::BTN_SIDE, KEY_PRESSED);
        let event = recv_event(&rx);
        assert_eq!(event.key, Some(Key::MouseX1));
        assert_eq!(event.modifiers, Modifiers::empty());
    }

    #[test]
    fn reconcile_clears_stale_and_adopts_missed() {
        let (mut state, rx) = state_and_receiver();
        state.listener.current_modifiers = Modifiers::CMD_LEFT | Modifiers::SHIFT_LEFT;

        reconcile_modifiers(&mut state, Modifiers::SHIFT_LEFT | Modifiers::CTRL_RIGHT);

        let event = recv_event(&rx);
        assert_eq!(event.changed_modifier, Some(Modifiers::CMD_LEFT));
        assert!(!event.is_key_down);
        assert_eq!(event.modifiers, Modifiers::SHIFT_LEFT);
        assert!(rx.try_recv().is_err());
        assert_eq!(
            state.listener.current_modifiers,
            Modifiers::SHIFT_LEFT | Modifiers::CTRL_RIGHT
        );
    }

    #[test]
    fn blocked_hotkey_press_repeat_and_release() {
        let hotkey = Hotkey::new(Modifiers::empty(), Key::F20).unwrap();
        let (mut state, rx, _blocking) = state_and_receiver_with(&[hotkey]);

        assert!(process_key(&mut state, KeyCode::KEY_F20, KEY_PRESSED));
        assert!(process_key(&mut state, KeyCode::KEY_F20, KEY_AUTOREPEAT));
        assert!(process_key(&mut state, KeyCode::KEY_F20, KEY_RELEASED));

        assert_eq!(rx.try_iter().count(), 3);

        assert!(!process_key(&mut state, KeyCode::KEY_A, KEY_PRESSED));
        assert!(!process_key(&mut state, KeyCode::KEY_A, KEY_RELEASED));
    }

    #[test]
    fn modifiers_gate_blocking() {
        let hotkey = Hotkey::new(Modifiers::CTRL, Key::Space).unwrap();
        let (mut state, _rx, _blocking) = state_and_receiver_with(&[hotkey]);

        assert!(!process_key(&mut state, KeyCode::KEY_SPACE, KEY_PRESSED));
        assert!(!process_key(&mut state, KeyCode::KEY_SPACE, KEY_RELEASED));

        assert!(!process_key(&mut state, KeyCode::KEY_LEFTCTRL, KEY_PRESSED));
        assert!(process_key(&mut state, KeyCode::KEY_SPACE, KEY_PRESSED));
        assert!(process_key(&mut state, KeyCode::KEY_SPACE, KEY_RELEASED));
        assert!(!process_key(
            &mut state,
            KeyCode::KEY_LEFTCTRL,
            KEY_RELEASED
        ));
    }

    #[test]
    fn release_follows_press_decision_when_hotkeys_change() {
        let hotkey = Hotkey::new(Modifiers::empty(), Key::F20).unwrap();
        let (mut state, _rx, blocking) = state_and_receiver_with(&[hotkey]);

        assert!(process_key(&mut state, KeyCode::KEY_F20, KEY_PRESSED));
        blocking.lock().unwrap().clear();
        assert!(process_key(&mut state, KeyCode::KEY_F20, KEY_RELEASED));

        assert!(!process_key(&mut state, KeyCode::KEY_F20, KEY_PRESSED));
        blocking
            .lock()
            .unwrap()
            .insert(Hotkey::new(Modifiers::empty(), Key::F20).unwrap());
        assert!(!process_key(&mut state, KeyCode::KEY_F20, KEY_RELEASED));
    }

    #[test]
    fn modifier_only_hotkey_blocks_press_and_release_symmetrically() {
        let hotkey = Hotkey::new(Modifiers::CMD, None).unwrap();
        let (mut state, _rx, blocking) = state_and_receiver_with(&[hotkey]);

        assert!(process_key(&mut state, KeyCode::KEY_LEFTMETA, KEY_PRESSED));
        assert!(process_key(
            &mut state,
            KeyCode::KEY_LEFTMETA,
            KEY_AUTOREPEAT
        ));
        assert!(process_key(&mut state, KeyCode::KEY_LEFTMETA, KEY_RELEASED));

        blocking.lock().unwrap().clear();
        assert!(!process_key(&mut state, KeyCode::KEY_LEFTMETA, KEY_PRESSED));
        blocking
            .lock()
            .unwrap()
            .insert(Hotkey::new(Modifiers::CMD, None).unwrap());
        assert!(!process_key(
            &mut state,
            KeyCode::KEY_LEFTMETA,
            KEY_RELEASED
        ));
    }

    #[test]
    fn forwarded_modifier_press_clears_stale_blocked_bit() {
        let (mut state, _rx) = state_and_receiver();
        state.blocked_modifiers = Modifiers::CMD_LEFT;

        assert!(!process_key(&mut state, KeyCode::KEY_LEFTMETA, KEY_PRESSED));
        assert!(!process_key(
            &mut state,
            KeyCode::KEY_LEFTMETA,
            KEY_RELEASED
        ));
    }

    #[test]
    fn reconcile_prunes_stale_blocked_modifiers() {
        let hotkey = Hotkey::new(Modifiers::CMD, None).unwrap();
        let (mut state, _rx, _blocking) = state_and_receiver_with(&[hotkey]);

        assert!(process_key(&mut state, KeyCode::KEY_LEFTMETA, KEY_PRESSED));
        assert_eq!(state.blocked_modifiers, Modifiers::CMD_LEFT);

        reconcile_modifiers(&mut state, Modifiers::empty());
        assert_eq!(state.blocked_modifiers, Modifiers::empty());
    }

    #[test]
    fn short_clone_name_is_left_intact() {
        let name = format!("{CLONE_NAME_PREFIX}Some Keyboard");
        assert_eq!(truncate_on_char_boundary(&name, MAX_CLONE_NAME_BYTES), name);
    }

    #[test]
    fn long_clone_name_is_truncated_within_kernel_limit() {
        let name = format!(
            "{CLONE_NAME_PREFIX}input-remapper SteelSeries SteelSeries Apex 3 TKL forwarded"
        );
        assert!(name.len() > MAX_CLONE_NAME_BYTES);

        let truncated = truncate_on_char_boundary(&name, MAX_CLONE_NAME_BYTES);
        assert!(truncated.len() <= MAX_CLONE_NAME_BYTES);
        assert!(truncated.len() + 1 < 80);
        assert!(truncated.starts_with(CLONE_NAME_PREFIX));
    }

    #[test]
    fn truncation_never_splits_a_utf8_code_point() {
        let name = "é".repeat(50);
        let truncated = truncate_on_char_boundary(&name, 5);
        assert!(truncated.len() <= 5);
        assert_eq!(truncated, "éé");
    }
}
