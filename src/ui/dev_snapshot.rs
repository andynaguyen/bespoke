//! Debug-build helper that renders pages and saves PNGs of the settings
//! window, for checking the UI against the React app without clicking
//! through it by hand.
//!
//! `BESPOKE_DEV_SNAPSHOT=/tmp/shots BESPOKE_DEV_PAGES=dictate,settings:system`
//! opens each page in turn, writes `<dir>/<page>.png`, then quits. Pages are
//! sidebar sections (`dictate`, `stats`, `models`, `annotate`, `mcp`,
//! `debug`, `settings`), `settings:<tab>`, `dictate:<tab>`, `stats:<tab>`, or
//! `onboarding:accessibility|model`. A `@<px>` suffix, like `debug@900`,
//! scrolls the page down before capture, and a `#<n>` suffix presses Tab n
//! times with focus rings shown. After those, `!<key>` presses a key, like
//! `settings:appearance#2!down` to open the second control's menu.
//! `toast:<error|info|warning>` shows a sample toast over the current page,
//! and `dialog:entry` or `dialog:whatsnew` opens that dialog.
//! `overlay:<kind>` shows the recording overlay with sample data and
//! captures its window instead (see `ui::overlay::dev_preview`), and
//! `capsule:<kind>` does the same for Annotate's capsule (see
//! `annotate::capsule::dev_preview`). `scrawl:choose` opens Annotate's
//! picker and `scrawl:latest` its latest capture, and both capture the main
//! display. `annotate:latest` opens the newest capture on the Annotate page.
//! `BESPOKE_DEV_DESKTOP=1` keeps the desktop and windows behind the target
//! window in the capture, so glass shows what it refracts. The capture
//! includes the window's popups and dialogs (`ui::popup`).
//! `BESPOKE_DEV_THEME=dark|light` forces the palette and `BESPOKE_DEV_LANGUAGE=ar`
//! the UI language, without saving either.
//!
//! macOS only: it captures the window with `CGWindowListCreateImageFromArray`, which
//! needs no Screen Recording permission for the app's own windows.

use std::cell::Cell;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{AnyWindowHandle, App, AsyncApp};

thread_local! {
    /// Whether the current page is an overlay preview.
    static CAPTURE_OVERLAY: Cell<bool> = const { Cell::new(false) };
    /// Whether the current page is an Annotate capsule preview.
    static CAPTURE_CAPSULE: Cell<bool> = const { Cell::new(false) };
    /// Whether the current page captures the whole screen.
    static CAPTURE_SCREEN: Cell<bool> = const { Cell::new(false) };
}

/// The window the current page draws in.
fn target_window(cx: &App) -> Option<AnyWindowHandle> {
    #[cfg(target_os = "macos")]
    if CAPTURE_CAPSULE.get() {
        return crate::annotate::capsule::window_handle(cx);
    }
    if CAPTURE_OVERLAY.get() {
        crate::ui::overlay::window_handle(cx)
    } else {
        crate::ui::main_window_handle(cx).map(Into::into)
    }
}

/// Whether a snapshot run was requested.
pub fn requested() -> bool {
    std::env::var_os("BESPOKE_DEV_SNAPSHOT").is_some()
}

pub fn start(cx: &mut App) {
    let Some(dir) = std::env::var_os("BESPOKE_DEV_SNAPSHOT").map(PathBuf::from) else {
        return;
    };
    let pages: Vec<String> = std::env::var("BESPOKE_DEV_PAGES")
        .unwrap_or_else(|_| "dictate".into())
        .split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    if let Ok(language) = std::env::var("BESPOKE_DEV_LANGUAGE") {
        crate::i18n::set_language(&language);
        cx.refresh_windows();
    }
    if let Ok(theme) = std::env::var("BESPOKE_DEV_THEME") {
        let setting = match theme.as_str() {
            "dark" => crate::settings::Theme::Dark,
            "light" => crate::settings::Theme::Light,
            _ => crate::settings::Theme::System,
        };
        crate::ui::theme::apply_theme_setting(setting, cx);
    }
    let _ = std::fs::create_dir_all(&dir);
    cx.spawn(async move |cx| {
        wait(cx, 1500).await;
        for page in pages {
            let (rest, key) = page.split_once('!').unwrap_or((page.as_str(), ""));
            let (target, tabs) = rest.split_once('#').unwrap_or((rest, ""));
            let _ = cx.update(|cx| navigate(target, cx));
            // A locked or covered screen stops GPUI's display link, so draw
            // by hand, twice, to pick up images that finish loading.
            for _ in 0..3 {
                wait(cx, 400).await;
                force_draw(cx);
            }
            if let Ok(count) = tabs.parse::<usize>() {
                // Tab presses walk the tab stops of the frame just drawn.
                let _ = cx.update(|cx| press_tab(count, cx));
                for _ in 0..2 {
                    wait(cx, 200).await;
                    force_draw(cx);
                }
            }
            if !key.is_empty() {
                let _ = cx.update(|cx| press_key(key, cx));
                for _ in 0..2 {
                    wait(cx, 200).await;
                    force_draw(cx);
                }
            }
            let path = dir.join(format!("{}.png", page.replace([':', '@', '#', '!'], "-")));
            let saved = cx.update(|cx| capture(&path, cx)).unwrap_or(false);
            log::info!("snapshot {} -> {} ({})", page, path.display(), saved);
        }
        let _ = cx.update(|cx| cx.quit());
    })
    .detach();
}

async fn wait(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

fn navigate(page: &str, cx: &mut App) {
    #[cfg(target_os = "macos")]
    if let Some(action) = page.strip_prefix("scrawl:") {
        CAPTURE_SCREEN.set(true);
        let action = match action {
            "latest" => scrawl::MenuAction::OpenLatest,
            _ => scrawl::MenuAction::ChooseTarget,
        };
        scrawl::host::menu(action);
        return;
    }
    CAPTURE_SCREEN.set(false);
    #[cfg(target_os = "macos")]
    if let Some(kind) = page.strip_prefix("capsule:") {
        CAPTURE_CAPSULE.set(true);
        crate::annotate::capsule::dev_preview(kind, cx);
        return;
    }
    CAPTURE_CAPSULE.set(false);
    if let Some(kind) = page.strip_prefix("overlay:") {
        CAPTURE_OVERLAY.set(true);
        crate::ui::overlay::dev_preview(kind, cx);
        return;
    }
    if CAPTURE_OVERLAY.replace(false) {
        crate::ui::overlay::hide(cx);
    }
    if let Some(kind) = page.strip_prefix("toast:") {
        sample_toast(kind, cx);
        return;
    }
    if let Some(kind) = page.strip_prefix("dialog:") {
        open_dialog(kind, cx);
        return;
    }
    if page == "annotate:latest" {
        if let Some(capture) = scrawl::host::recent_captures(1).first() {
            crate::ui::pages::captures::inspect(capture.dir().to_path_buf(), cx);
        }
        return;
    }
    let Some(main) = crate::ui::main_view(cx) else {
        log::warn!("snapshot: no main view");
        return;
    };
    let Some(handle) = crate::ui::main_window_handle(cx) else {
        log::warn!("snapshot: no main window");
        return;
    };
    let page = page.to_string();
    let result = handle.update(cx, move |_, window, cx| {
        main.update(cx, |main, cx| main.dev_navigate(&page, window, cx));
    });
    if let Err(e) = result {
        log::warn!("snapshot: navigation failed: {e}");
    }
}

fn sample_toast(kind: &str, cx: &mut App) {
    use crate::ui::toast::{show, ToastAction, ToastKind};
    let kind = match kind {
        "info" => ToastKind::Info,
        "warning" => ToastKind::Warning,
        _ => ToastKind::Error,
    };
    let action = ToastAction {
        label: "How to fix".into(),
        on_click: std::rc::Rc::new(|_, _| {}),
    };
    show(
        kind,
        "Microphone unavailable",
        Some("Bespoke could not open the selected microphone.".into()),
        Some(action),
        cx,
    );
}

fn open_dialog(kind: &str, cx: &mut App) {
    let Some(handle) = crate::ui::main_window_handle(cx) else {
        return;
    };
    let kind = kind.to_string();
    let _ = handle.update(cx, move |_, window, cx| match kind.as_str() {
        "whatsnew" => {
            if let Some(note) = crate::ui::whats_new::latest_release_note() {
                crate::ui::whats_new::open_modal(note, false, window, cx);
            }
        }
        _ => crate::ui::pages::open_entry_dialog(window, cx),
    });
}

/// Presses `key`, like `down` or `space`, in the target window.
fn press_key(key: &str, cx: &mut App) {
    let Some(handle) = target_window(cx) else {
        return;
    };
    let Ok(keystroke) = gpui::Keystroke::parse(key) else {
        log::warn!("snapshot: unknown key {key}");
        return;
    };
    let _ = handle.update(cx, |_, window, cx| {
        crate::ui::components::set_keyboard_modality(true, window, cx);
        window.dispatch_keystroke(keystroke, cx);
    });
}

/// Moves keyboard focus `count` tab stops forward with focus rings shown.
fn press_tab(count: usize, cx: &mut App) {
    let Some(handle) = target_window(cx) else {
        return;
    };
    let _ = handle.update(cx, |_, window, cx| {
        crate::ui::components::set_keyboard_modality(true, window, cx);
        for _ in 0..count {
            window.focus_next();
        }
    });
}

/// Asks GPUI's view to draw now, the way AppKit's `displayLayer:` does.
/// Must run with no `App` borrow held.
#[cfg(target_os = "macos")]
fn force_draw(cx: &mut AsyncApp) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let view = cx
        .update(|cx| {
            let handle = target_window(cx)?;
            handle
                .update(cx, |_, window, _| {
                    let raw = HasWindowHandle::window_handle(window).ok()?;
                    match raw.as_raw() {
                        RawWindowHandle::AppKit(raw) => Some(raw.ns_view.as_ptr() as usize),
                        _ => None,
                    }
                })
                .ok()
                .flatten()
        })
        .ok()
        .flatten();
    if let Some(view) = view {
        let view = view as *mut objc2::runtime::AnyObject;
        // SAFETY: GPUI's content view implements displayLayer:, and nothing
        // borrows the app while it draws.
        unsafe {
            let _: () = objc2::msg_send![view, displayLayer: std::ptr::null_mut::<objc2::runtime::AnyObject>()];
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn force_draw(_cx: &mut AsyncApp) {}

#[cfg(target_os = "macos")]
fn capture(path: &std::path::Path, cx: &mut App) -> bool {
    if CAPTURE_SCREEN.get() {
        // Annotate's picker and capsule span several windows, so these pages
        // capture the main display. Needs the Screen Recording permission.
        return std::process::Command::new("screencapture")
            .args(["-x", "-m"])
            .arg(path)
            .status()
            .is_ok_and(|status| status.success());
    }
    capture_window(path, cx)
}

#[cfg(target_os = "macos")]
fn capture_window(path: &std::path::Path, cx: &mut App) -> bool {
    use std::ffi::{c_void, CString};

    use objc2_app_kit::NSView;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGImageRelease(image: *mut c_void);
        fn CGWindowListCreate(list: u32, window: u32) -> *const c_void;
        fn CGWindowListCreateImageFromArray(
            bounds: CGRect,
            windows: *const c_void,
            image: u32,
        ) -> *mut c_void;
    }
    #[link(name = "ImageIO", kind = "framework")]
    unsafe extern "C" {
        fn CGImageDestinationCreateWithURL(
            url: *const c_void,
            kind: *const c_void,
            count: usize,
            options: *const c_void,
        ) -> *mut c_void;
        fn CGImageDestinationAddImage(dest: *mut c_void, image: *mut c_void, props: *const c_void);
        fn CGImageDestinationFinalize(dest: *mut c_void) -> bool;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFURLCreateFromFileSystemRepresentation(
            alloc: *const c_void,
            path: *const u8,
            len: isize,
            is_dir: bool,
        ) -> *const c_void;
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            s: *const i8,
            encoding: u32,
        ) -> *const c_void;
        fn CFRelease(value: *const c_void);
        fn CFArrayGetCount(array: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
        fn CFArrayCreate(
            alloc: *const c_void,
            values: *const *const c_void,
            count: isize,
            callbacks: *const c_void,
        ) -> *const c_void;
    }

    const ON_SCREEN_BELOW_WINDOW: u32 = 1 << 2;
    const BOUNDS_IGNORE_FRAMING: u32 = 1 << 0;
    const UTF8: u32 = 0x0800_0100;

    let Some(handle) = target_window(cx) else {
        return false;
    };
    let found = handle
        .update(cx, |_, window, _| {
            let raw = HasWindowHandle::window_handle(window).ok()?;
            let RawWindowHandle::AppKit(raw) = raw.as_raw() else {
                return None;
            };
            // SAFETY: GPUI's live content view.
            let view: &NSView = unsafe { raw.ns_view.cast::<NSView>().as_ref() };
            let ns_window = view.window()?;
            let screen_height = ns_window.screen()?.frame().size.height;
            let frame = ns_window.frame();
            // CoreGraphics counts y down from the top of the main screen.
            let rect = CGRect {
                x: frame.origin.x,
                y: screen_height - frame.origin.y - frame.size.height,
                w: frame.size.width,
                h: frame.size.height,
            };
            // Popups and dialogs are child windows (`ui::popup`).
            let children: Vec<u32> = ns_window
                .childWindows()
                .map(|children| {
                    children
                        .iter()
                        .filter(|child| child.isVisible())
                        .map(|child| child.windowNumber() as u32)
                        .collect()
                })
                .unwrap_or_default();
            Some((ns_window.windowNumber() as u32, rect, children))
        })
        .ok()
        .flatten();
    let Some((number, frame, children)) = found else {
        return false;
    };

    // `BESPOKE_DEV_DESKTOP=1` keeps what is on screen below the window, so
    // glass shows what it refracts. It needs the Screen Recording permission.
    let desktop = std::env::var_os("BESPOKE_DEV_DESKTOP").is_some();
    let null_rect = CGRect {
        x: f64::INFINITY,
        y: f64::INFINITY,
        w: 0.,
        h: 0.,
    };
    let path_bytes = path.to_string_lossy().into_owned();
    let kind = CString::new("public.png").unwrap();
    // SAFETY: plain CoreGraphics/ImageIO calls; every created object is
    // released before returning.
    unsafe {
        // The window's popups above it, the window, then (with
        // `BESPOKE_DEV_DESKTOP`) everything below it, front to back.
        let below = if desktop {
            CGWindowListCreate(ON_SCREEN_BELOW_WINDOW, number)
        } else {
            std::ptr::null()
        };
        let mut ids: Vec<*const c_void> = children
            .iter()
            .chain(std::iter::once(&number))
            .map(|&id| id as usize as *const c_void)
            .collect();
        if !below.is_null() {
            for index in 0..CFArrayGetCount(below) {
                ids.push(CFArrayGetValueAtIndex(below, index));
            }
            CFRelease(below);
        }
        let windows = CFArrayCreate(
            std::ptr::null(),
            ids.as_ptr(),
            ids.len() as isize,
            std::ptr::null(),
        );
        let bounds = if desktop { frame } else { null_rect };
        let image = CGWindowListCreateImageFromArray(bounds, windows, BOUNDS_IGNORE_FRAMING);
        CFRelease(windows);
        if image.is_null() {
            return false;
        }
        let url = CFURLCreateFromFileSystemRepresentation(
            std::ptr::null(),
            path_bytes.as_ptr(),
            path_bytes.len() as isize,
            false,
        );
        let uti = CFStringCreateWithCString(std::ptr::null(), kind.as_ptr(), UTF8);
        let dest = CGImageDestinationCreateWithURL(url, uti, 1, std::ptr::null());
        let ok = if dest.is_null() {
            false
        } else {
            CGImageDestinationAddImage(dest, image, std::ptr::null());
            let ok = CGImageDestinationFinalize(dest);
            CFRelease(dest);
            ok
        };
        CFRelease(uti);
        CFRelease(url);
        CGImageRelease(image);
        ok
    }
}

#[cfg(not(target_os = "macos"))]
fn capture(_path: &std::path::Path, _cx: &mut App) -> bool {
    false
}
