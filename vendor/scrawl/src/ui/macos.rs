//! The AppKit side of the picker and review windows: window levels and styles, display
//! placement, and handing the keys back to the app that had them. Bespoke's tray menu replaces
//! Scrawl's menu bar item.

use std::sync::Arc;

use gpui::{AnyElement, App, Div, Window};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSApplication, NSColor, NSStatusWindowLevel, NSWindow, NSWindowButton,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::MainThreadMarker;

use super::{LaserOverlay, MenuAction};
use crate::capture::SharedCaptureTelemetry;

/// Where the file browser is called in buttons ("Show in Finder").
pub(super) const FILE_MANAGER: &str = "Finder";
/// The review header's leading inset, clear of the traffic lights.
pub(super) const HEADER_LEAD: f32 = 78.0;
pub(super) const HEADER_TRAIL: f32 = 16.0;
/// Global coordinates are points, as GPUI lays out.
pub(super) fn points_per_pixel(_window: &Window) -> f32 {
    1.0
}

/// The GPUI display that is the screen at `frame` (global points), else the primary one.
pub(super) fn display_at(
    frame: [f32; 4],
    cx: &App,
) -> Option<std::rc::Rc<dyn gpui::PlatformDisplay>> {
    let screens = crate::screens::displays();
    let id = crate::screens::index_for_rect(&screens, frame.map(f64::from)).map(|i| screens[i].id);
    id.and_then(|id| cx.displays().into_iter().find(|d| u32::from(d.id()) == id))
        .or_else(|| cx.primary_display())
}

/// A window covering the screen at `frame` lays out at its size: global coordinates are points.
pub(super) fn layout_size(
    frame: [f32; 4],
    _display: Option<&dyn gpui::PlatformDisplay>,
) -> gpui::Size<gpui::Pixels> {
    gpui::size(gpui::px(frame[2]), gpui::px(frame[3]))
}

/// Moves a window to cover the screen at `frame` (global points).
fn move_window_to_screen(ns_win: &NSWindow, frame: [f64; 4]) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(primary_h) = objc2_app_kit::NSScreen::screens(mtm)
        .firstObject()
        .map(|s| s.frame().size.height)
    else {
        return;
    };
    let [x, y, w, h] = crate::screens::flip(frame, primary_h);
    ns_win.setFrame_display(
        objc2_foundation::NSRect::new(
            objc2_foundation::NSPoint::new(x, y),
            objc2_foundation::NSSize::new(w, h),
        ),
        true,
    );
}

/// Moves the open picker to cover the screen at `frame` (global points).
pub(super) fn move_picker_window(frame: [f64; 4], _cx: &mut App) {
    if let Some(ns_win) = find_picker_ns_window() {
        move_window_to_screen(&ns_win, frame);
    }
}

/// GPUI places windows relative to their display on macOS.
pub(super) fn window_origin(
    _display: Option<&dyn gpui::PlatformDisplay>,
) -> gpui::Point<gpui::Pixels> {
    gpui::Point::default()
}

/// Dragging a region moves its window.
pub(super) fn window_drag_region(region: Div) -> Div {
    use gpui::{InteractiveElement, MouseButton};
    region.on_mouse_down(MouseButton::Left, |_, window, _| {
        window.start_window_move();
    })
}

/// The traffic lights are native; nothing to draw.
pub(super) fn caption_buttons() -> Option<AnyElement> {
    None
}

/// AppKit gives every window the app's icon.
pub(super) fn decorate_app_window(_window: &mut Window) {}

/// The app that was in front when Scrawl activated itself for the picker or the review window
/// (0: none).
static PREVIOUS_FRONT_APP: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// Called just before Scrawl activates itself to take the keys. Scrawl in front already (one
/// key window opening over another) keeps the app remembered first.
fn remember_front_app() {
    use objc2_app_kit::NSWorkspace;
    let front = NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map_or(0, |app| app.processIdentifier());
    if front > 0 && front != std::process::id() as i32 {
        PREVIOUS_FRONT_APP.store(front, std::sync::atomic::Ordering::Release);
    }
}

/// A menu bar (accessory) app isn't active when its menu or capsule is used, and keys only
/// reach its windows while it is: remember the app in front, then activate ignoring other apps
/// (the cooperative `NSApp.activate()` may leave the window behind the app in front).
pub(super) fn take_keys(cx: &mut App) {
    remember_front_app();
    cx.activate(true);
}

/// Makes the picker key once the app is active (`take_keys`), on a later main-thread turn. A
/// nonactivating panel made key in an inactive app sends the app's previous key window
/// (Bespoke's main window) a spurious `windowDidBecomeKey`, and GPUI's workaround for it
/// deadlocks the main thread. The held switcher never activates the app: it reads its keys from
/// the key tap, and the picker stays in front without them.
pub(super) fn focus_picker_soon(cx: &mut App) {
    cx.spawn(async move |cx| {
        for _ in 0..20 {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(25))
                .await;
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            if NSApplication::sharedApplication(mtm).isActive() {
                if let Some(picker) = find_picker_ns_window() {
                    picker.makeKeyAndOrderFront(None);
                }
                return;
            }
        }
    })
    .detach();
}

/// Run (deferred) after any window closes: once neither a picker nor a review window is left,
/// the app that was in front before gets the keys back. `NSApp.deactivate()` isn't enough: a
/// menu bar (accessory) app stays frontmost with no windows, and keys go nowhere. Deferred, a
/// picker replacing another one is open again by the time this looks.
pub(super) fn hand_back_keys(cx: &mut App) {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    if cx.windows().iter().any(|handle| {
        handle.downcast::<super::TargetPickerWindow>().is_some()
            || handle.downcast::<super::BundleReviewWindow>().is_some()
    }) {
        return;
    }
    let previous = PREVIOUS_FRONT_APP.swap(0, std::sync::atomic::Ordering::AcqRel);
    // Scrawl shares the app with Bespoke's windows: only hand the keys back when Scrawl took
    // them from another app, never when one of Bespoke's own windows closes.
    if previous == 0 {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    if !app.isActive() {
        return;
    }
    let reactivated = previous > 0
        && NSRunningApplication::runningApplicationWithProcessIdentifier(previous)
            .is_some_and(|prev| prev.activateWithOptions(NSApplicationActivationOptions::empty()));
    if !reactivated {
        app.deactivate();
    }
}

pub(super) fn open_overlay(
    telemetry: Arc<SharedCaptureTelemetry>,
    on: Option<[f64; 4]>,
    _cx: &mut gpui::Context<super::ScrawlCoordinator>,
) -> Result<LaserOverlay, String> {
    LaserOverlay::open(telemetry, on)
}

/// The picker: borderless at status level + 1 over the whole screen at `frame` (global points,
/// menu bar included), taking clicks. Returns its window number, to leave out of the recording.
pub(super) fn configure_picker_window(
    _window: &mut Window,
    frame: [f64; 4],
    _cx: &mut App,
) -> Option<u32> {
    let ns_win = find_picker_ns_window()?;
    ns_win.setStyleMask(NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel);
    configure_floating_window(&ns_win, 1);
    move_window_to_screen(&ns_win, frame);
    // Keys come later (`focus_picker_soon`): AppKit calls back into GPUI as a window becomes
    // key, and GPUI is still opening this one.
    ns_win.orderFrontRegardless();
    let num = ns_win.windowNumber().max(0) as u32;
    (num > 0).then_some(num)
}

/// Whether macOS asks for reduced motion (Accessibility ▸ Display ▸ Reduce motion).
pub(super) fn reduce_motion() -> bool {
    use objc2::runtime::AnyClass;
    let Some(cls) = AnyClass::get(c"NSWorkspace") else {
        return false;
    };
    unsafe {
        let workspace: *mut AnyObject = msg_send![cls, sharedWorkspace];
        !workspace.is_null() && msg_send![workspace, accessibilityDisplayShouldReduceMotion]
    }
}

pub(super) fn install_status_bar() -> Option<flume::Receiver<MenuAction>> {
    Some(crate::host::menu_receiver())
}

fn find_picker_ns_window() -> Option<Retained<NSWindow>> {
    let mtm = MainThreadMarker::new()?;
    let app = NSApplication::sharedApplication(mtm);
    for win in app.windows() {
        if win.ignoresMouseEvents() {
            continue;
        }
        let size = win.frame().size;
        if size.width >= 600.0 && size.height >= 200.0 && (size.width - 1120.0).abs() > 8.0 {
            return Some(win);
        }
    }
    None
}

fn sync_full_size_content_view(ns_window: &NSWindow) {
    for kind in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        if let Some(button) = ns_window.standardWindowButton(kind) {
            button.setHidden(true);
        }
    }
    if let Some(cv) = ns_window.contentView() {
        let bounds = cv.bounds();
        for sub in cv.subviews() {
            sub.setFrame(bounds);
        }
        if let Some(frame_view) = unsafe { cv.superview() } {
            for sub in frame_view.subviews() {
                if sub
                    .class()
                    .name()
                    .to_bytes()
                    .windows(8)
                    .any(|w| w == b"Titlebar")
                {
                    sub.setHidden(true);
                }
            }
        }
    }
}

fn configure_floating_window(ns_window: &NSWindow, level_offset: isize) {
    ns_window.setHidesOnDeactivate(false);
    ns_window.setIgnoresMouseEvents(false);
    ns_window.setOpaque(false);
    ns_window.setBackgroundColor(Some(&NSColor::clearColor()));
    ns_window.setHasShadow(false);
    ns_window.setLevel(NSStatusWindowLevel + level_offset);
    ns_window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    sync_full_size_content_view(ns_window);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reveal_appkit_queries_pass_objc2_verification() {
        let _ = reduce_motion();
        let cls = objc2::runtime::AnyClass::get(c"NSEvent").expect("NSEvent");
        let _: objc2_foundation::NSPoint = unsafe { msg_send![cls, mouseLocation] };
    }
}
