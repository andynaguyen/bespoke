//! Native calls GPUI does not expose: hiding a window without closing it,
//! the macOS activation policy and appearance, permission checks, and the
//! screen geometry the overlay is placed against.

use gpui::{App, Window};

/// Hides `window` without destroying it, so its views keep their state.
/// Returns false when the platform cannot hide windows (the caller closes
/// the window instead).
///
/// The native call runs on the next main-thread turn: AppKit notifies GPUI
/// synchronously when key status changes, and GPUI cannot handle that while
/// the caller holds the app.
pub fn hide_window(window: &Window, cx: &App) -> bool {
    imp::hide_window(window, cx)
}

/// Shows a window hidden with [`hide_window`] and gives it focus.
pub fn show_window(window: &Window, cx: &App) {
    imp::show_window(window, cx)
}

pub fn is_window_visible(window: &Window) -> bool {
    imp::is_window_visible(window)
}

/// Windows: a dark or light native title bar (`DWMWA_USE_IMMERSIVE_DARK_MODE`),
/// what tao's `set_theme` did for the Tauri window. Other platforms theme
/// their chrome elsewhere (macOS) or not at all (Linux, like the original).
pub fn set_title_bar_dark(window: &Window, dark: bool) {
    #[cfg(target_os = "windows")]
    imp::set_title_bar_dark(window, dark);
    #[cfg(not(target_os = "windows"))]
    let _ = (window, dark);
}

/// Strips the recording overlay panel down to its content: no title-bar
/// buttons and no window shadow (the card draws its own flat border).
pub fn prepare_overlay_window(window: &Window) {
    #[cfg(target_os = "macos")]
    imp::prepare_overlay_window(window);
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

/// Rounds the main window's frame at `radius::WINDOW`, the radius its glass
/// is drawn with. macOS 26 rounds a window without a toolbar at 16pt and
/// draws its hairline and shadow there, which left a sliver of desktop
/// between the frame and the glass at each corner.
pub fn prepare_main_window(window: &Window) {
    #[cfg(target_os = "macos")]
    imp::prepare_main_window(window);
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

#[cfg(target_os = "macos")]
pub use imp::{
    accessibility_granted, launch_as_accessory, microphone_granted,
    prevent_panels_from_taking_focus, request_accessibility, request_microphone,
    set_activation_policy_regular, set_app_appearance, CursorScreen,
};

#[cfg(target_os = "macos")]
pub use imp::capsule;

#[cfg(target_os = "macos")]
mod imp {
    use gpui::Window;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
    use objc2::{msg_send, sel, MainThreadMarker};
    use objc2_app_kit::{
        NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
        NSApplicationActivationPolicy, NSEvent, NSScreen, NSView, NSWindow,
    };
    use objc2_foundation::{NSDictionary, NSNumber, NSPoint, NSRect, NSString};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return None;
        };
        // SAFETY: GPUI hands out its live content view; it outlives this call.
        let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        view.window()
    }

    pub fn hide_window(window: &Window, cx: &gpui::App) -> bool {
        if let Some(ns_window) = ns_window(window) {
            cx.foreground_executor()
                .spawn(async move { ns_window.orderOut(None) })
                .detach();
        }
        true
    }

    pub fn show_window(window: &Window, cx: &gpui::App) {
        let ns_window = ns_window(window);
        cx.foreground_executor()
            .spawn(async move {
                if let Some(ns_window) = ns_window {
                    if ns_window.isMiniaturized() {
                        ns_window.deminiaturize(None);
                    }
                    ns_window.makeKeyAndOrderFront(None);
                }
                if let Some(mtm) = MainThreadMarker::new() {
                    #[allow(deprecated)]
                    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
                }
            })
            .detach();
    }

    pub fn is_window_visible(window: &Window) -> bool {
        ns_window(window).is_some_and(|w| w.isVisible())
    }

    pub fn prepare_main_window(window: &Window) {
        if !crate::ui::glass::native_available() {
            return;
        }
        let Some(ns_window) = ns_window(window) else {
            return;
        };
        let radius = f64::from(f32::from(crate::ui::theme::radius::WINDOW));
        // AppKit has no public setter for a window's corner radius. Check
        // the private one exists, so a later macOS falls back to its own.
        // SAFETY: `_setCornerRadius:` takes a CGFloat and returns nothing.
        unsafe {
            let responds: bool =
                msg_send![&*ns_window, respondsToSelector: sel!(_setCornerRadius:)];
            if responds {
                let _: () = msg_send![&*ns_window, _setCornerRadius: radius];
                ns_window.invalidateShadow();
            }
        }
    }

    pub fn prepare_overlay_window(window: &Window) {
        let Some(ns_window) = ns_window(window) else {
            return;
        };
        ns_window.setHasShadow(false);
        // Keeps it from becoming key (`prevent_panels_from_taking_focus`).
        // SAFETY: NSWindow adopts NSUserInterfaceItemIdentification.
        unsafe {
            let identifier = NSString::from_str(OVERLAY_IDENTIFIER);
            let _: () = msg_send![&*ns_window, setIdentifier: &*identifier];
        }
        // NSWindowCloseButton, NSWindowMiniaturizeButton, NSWindowZoomButton.
        for kind in 0isize..3 {
            // SAFETY: `standardWindowButton:` returns an autoreleased NSButton
            // or nil; `setHidden:` takes a BOOL.
            unsafe {
                let button: *mut AnyObject = msg_send![&*ns_window, standardWindowButton: kind];
                if !button.is_null() {
                    let _: () = msg_send![button, setHidden: true];
                }
            }
        }
    }

    /// Calls for a capsule window that floats over every app, like Annotate's.
    /// Points are global, with the origin at the top left of the main
    /// display, as Scrawl counts them.
    pub mod capsule {
        use gpui::Window;
        use objc2::rc::Retained;
        use objc2::MainThreadMarker;
        use objc2_app_kit::{
            NSEvent, NSScreen, NSStatusWindowLevel, NSWindow, NSWindowCollectionBehavior,
        };
        use objc2_foundation::NSPoint;

        /// The main display's height, to flip AppKit's bottom-left y.
        fn flip_height() -> Option<f64> {
            let mtm = MainThreadMarker::new()?;
            let screens = NSScreen::screens(mtm);
            Some(screens.iter().next()?.frame().size.height)
        }

        /// A capsule's `NSWindow`, held apart from GPUI's window so the
        /// capsule's view can move and hide it while GPUI updates that view.
        #[derive(Clone)]
        pub struct CapsuleWindow(Retained<NSWindow>);

        impl CapsuleWindow {
            pub fn of(window: &Window) -> Option<Self> {
                super::ns_window(window).map(Self)
            }

            /// Raises the window above other apps' windows on every Space
            /// and over full-screen apps, and returns its window number.
            pub fn float(&self) -> u32 {
                self.0.setLevel(NSStatusWindowLevel + 5);
                self.0.setCollectionBehavior(
                    NSWindowCollectionBehavior::CanJoinAllSpaces
                        | NSWindowCollectionBehavior::FullScreenAuxiliary
                        | NSWindowCollectionBehavior::Stationary
                        | NSWindowCollectionBehavior::IgnoresCycle,
                );
                self.0.setHidesOnDeactivate(false);
                self.0.windowNumber().max(0) as u32
            }

            /// Moves the window so its top-left corner is at `origin`. Runs on
            /// the next main-thread turn: AppKit reports the move to GPUI
            /// synchronously, and GPUI cannot take that while the caller holds
            /// the app.
            pub fn move_to(&self, origin: [f64; 2], cx: &gpui::App) {
                let window = self.0.clone();
                cx.foreground_executor()
                    .spawn(async move {
                        if let Some(height) = flip_height() {
                            window
                                .setFrameTopLeftPoint(NSPoint::new(origin[0], height - origin[1]));
                        }
                    })
                    .detach();
            }

            /// The window's frame as `[x, y, width, height]`.
            pub fn frame(&self) -> Option<[f64; 4]> {
                let height = flip_height()?;
                let f = self.0.frame();
                Some([
                    f.origin.x,
                    height - f.origin.y - f.size.height,
                    f.size.width,
                    f.size.height,
                ])
            }

            /// Lets clicks through to the apps below.
            pub fn set_ignores_mouse(&self, ignore: bool) {
                if self.0.ignoresMouseEvents() != ignore {
                    self.0.setIgnoresMouseEvents(ignore);
                }
            }

            /// Orders the window in or out, on the next main-thread turn like
            /// `move_to`.
            pub fn set_visible(&self, visible: bool, cx: &gpui::App) {
                let window = self.0.clone();
                cx.foreground_executor()
                    .spawn(async move {
                        if visible {
                            window.orderFrontRegardless();
                        } else {
                            window.orderOut(None);
                        }
                    })
                    .detach();
            }
        }

        /// The pointer, and whether the primary button is down.
        pub fn pointer() -> Option<([f64; 2], bool)> {
            let height = flip_height()?;
            let p = NSEvent::mouseLocation();
            let down = NSEvent::pressedMouseButtons() & 1 != 0;
            Some(([p.x, height - p.y], down))
        }
    }

    /// Regular shows the Dock icon; Accessory hides it while the app lives
    /// in the menu bar.
    pub fn set_activation_policy_regular(regular: bool) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let policy = if regular {
            NSApplicationActivationPolicy::Regular
        } else {
            NSApplicationActivationPolicy::Accessory
        };
        NSApplication::sharedApplication(mtm).setActivationPolicy(policy);
    }

    /// Forces light (`Some(false)`), dark (`Some(true)`) or the system
    /// appearance (`None`) for every window, including native chrome.
    pub fn set_app_appearance(dark: Option<bool>) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        let appearance = dark.and_then(|dark| {
            // SAFETY: the appearance name constants are static NSStrings.
            let name = unsafe {
                if dark {
                    NSAppearanceNameDarkAqua
                } else {
                    NSAppearanceNameAqua
                }
            };
            NSAppearance::appearanceNamed(name)
        });
        app.setAppearance(appearance.as_deref());
    }

    /// The recording overlay's `NSWindow.identifier`.
    const OVERLAY_IDENTIFIER: &str = "bespoke-overlay";
    static PANEL_CAN_BECOME_KEY: std::sync::OnceLock<objc2::runtime::Imp> =
        std::sync::OnceLock::new();

    fn is_overlay(window: &AnyObject) -> bool {
        // SAFETY: `identifier` returns an autoreleased NSString or nil.
        let identifier: Option<Retained<NSString>> = unsafe { msg_send![window, identifier] };
        identifier.is_some_and(|id| id.to_string() == OVERLAY_IDENTIFIER)
    }

    /// GPUI's popup panel class answers YES to `canBecomeKeyWindow`, so a
    /// click on the overlay would take keyboard focus from the app the user
    /// is dictating into. The class method is swapped for one that answers NO
    /// for the overlay. Other popups keep GPUI's answer: Annotate's target
    /// picker needs the keys.
    pub fn prevent_panels_from_taking_focus() {
        extern "C" fn can_become_key(this: &AnyObject, sel: Sel) -> Bool {
            if is_overlay(this) {
                return Bool::NO;
            }
            let Some(original) = PANEL_CAN_BECOME_KEY.get() else {
                return Bool::YES;
            };
            // SAFETY: `original` is GPUIPanel's `BOOL (id, SEL)`
            // implementation of the same selector.
            let original: extern "C" fn(&AnyObject, Sel) -> Bool =
                unsafe { std::mem::transmute(*original) };
            original(this, sel)
        }
        let Some(class) = AnyClass::get(c"GPUIPanel") else {
            log::warn!("GPUIPanel class not found; overlay may take focus when clicked");
            return;
        };
        let sel = sel!(canBecomeKeyWindow);
        // SAFETY: looks up and replaces an existing `BOOL (id, SEL)` method
        // with one of the same signature.
        unsafe {
            let method = objc2::ffi::class_getInstanceMethod(class, sel);
            if let Some(original) = objc2::ffi::method_getImplementation(method) {
                let _ = PANEL_CAN_BECOME_KEY.set(original);
            }
            objc2::ffi::class_replaceMethod(
                class as *const AnyClass as *mut AnyClass,
                sel,
                std::mem::transmute::<
                    extern "C" fn(&AnyObject, Sel) -> Bool,
                    unsafe extern "C-unwind" fn(),
                >(can_become_key),
                c"c@:".as_ptr(),
            );
        }
    }

    static LAUNCH_AS_ACCESSORY: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    static SET_POLICY: std::sync::OnceLock<objc2::runtime::Imp> = std::sync::OnceLock::new();

    /// Launches the app without a Dock icon. GPUI sets the Regular policy
    /// when launching finishes, and demoting it to Accessory afterwards is
    /// the transition that left a Dock icon behind on macOS 26 (#1787). So
    /// the launch-time call is answered with Accessory instead; showing the
    /// window later promotes the app, which is the supported direction.
    /// Call after `Application::new` and before `run`.
    pub fn launch_as_accessory() {
        extern "C" fn set_activation_policy(this: &AnyObject, sel: Sel, policy: isize) -> Bool {
            // NSApplicationActivationPolicyRegular = 0, Accessory = 1.
            let policy = if policy == 0
                && LAUNCH_AS_ACCESSORY.swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                1
            } else {
                policy
            };
            let Some(original) = SET_POLICY.get() else {
                return Bool::NO;
            };
            // SAFETY: `original` is NSApplication's `BOOL (id, SEL, NSInteger)`
            // implementation of the same selector.
            let original: extern "C" fn(&AnyObject, Sel, isize) -> Bool =
                unsafe { std::mem::transmute(*original) };
            original(this, sel, policy)
        }

        let Some(class) = AnyClass::get(c"GPUIApplication") else {
            log::warn!("GPUIApplication class not found; launching with a Dock icon");
            return;
        };
        let sel = sel!(setActivationPolicy:);
        // SAFETY: looks up and replaces an instance method with one of the
        // same `BOOL (id, SEL, NSInteger)` signature, before the app runs.
        unsafe {
            let method = objc2::ffi::class_getInstanceMethod(class, sel);
            let Some(original) = objc2::ffi::method_getImplementation(method) else {
                return;
            };
            if SET_POLICY.set(original).is_err() {
                return;
            }
            LAUNCH_AS_ACCESSORY.store(true, std::sync::atomic::Ordering::SeqCst);
            objc2::ffi::class_replaceMethod(
                class as *const AnyClass as *mut AnyClass,
                sel,
                std::mem::transmute::<
                    extern "C" fn(&AnyObject, Sel, isize) -> Bool,
                    unsafe extern "C-unwind" fn(),
                >(set_activation_policy),
                c"c@:q".as_ptr(),
            );
        }
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXIsProcessTrustedWithOptions(options: *const AnyObject) -> bool;
    }

    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {}

    pub fn accessibility_granted() -> bool {
        // SAFETY: no arguments; reads the process's TCC state.
        unsafe { AXIsProcessTrusted() }
    }

    /// Shows the system prompt that sends the user to the Accessibility
    /// pane, adding Bespoke to the list.
    pub fn request_accessibility() {
        let key = NSString::from_str("AXTrustedCheckOptionPrompt");
        let value = NSNumber::new_bool(true);
        let options = NSDictionary::from_slices(&[&*key], &[&*value]);
        // SAFETY: NSDictionary is toll-free bridged to CFDictionaryRef.
        unsafe {
            AXIsProcessTrustedWithOptions(Retained::as_ptr(&options).cast());
        }
    }

    fn capture_device_class() -> Option<&'static AnyClass> {
        AnyClass::get(c"AVCaptureDevice")
    }

    /// `AVMediaTypeAudio`.
    fn audio_media_type() -> Retained<NSString> {
        NSString::from_str("soun")
    }

    /// True when the microphone permission is `AVAuthorizationStatusAuthorized`.
    pub fn microphone_granted() -> bool {
        let Some(class) = capture_device_class() else {
            return false;
        };
        let media_type = audio_media_type();
        // SAFETY: class method returning an NSInteger status.
        let status: isize =
            unsafe { msg_send![class, authorizationStatusForMediaType: &*media_type] };
        status == 3
    }

    /// Shows the microphone permission prompt (only the first time; later
    /// calls return immediately with the stored answer).
    pub fn request_microphone() {
        let Some(class) = capture_device_class() else {
            return;
        };
        let media_type = audio_media_type();
        let handler = block2::RcBlock::new(|granted: Bool| {
            log::info!(
                "Microphone permission request finished: {}",
                granted.as_bool()
            );
        });
        // SAFETY: the completion handler is a `void (^)(BOOL)` block.
        unsafe {
            let _: () = msg_send![
                class,
                requestAccessForMediaType: &*media_type,
                completionHandler: &*handler
            ];
        }
    }

    /// The screen under the mouse pointer, in coordinates relative to that
    /// screen's top-left corner.
    pub struct CursorScreen {
        /// `CGDirectDisplayID`, matching `u32::from(gpui::DisplayId)`.
        pub display_id: u32,
        pub width: f64,
        /// Distance from the top of the screen to the bottom of its visible
        /// frame (above the Dock).
        pub visible_bottom: f64,
    }

    impl CursorScreen {
        pub fn current() -> Option<Self> {
            let mtm = MainThreadMarker::new()?;
            let mouse: NSPoint = NSEvent::mouseLocation();
            let screens = NSScreen::screens(mtm);
            let contains = |rect: NSRect| {
                mouse.x >= rect.origin.x
                    && mouse.x < rect.origin.x + rect.size.width
                    && mouse.y >= rect.origin.y
                    && mouse.y < rect.origin.y + rect.size.height
            };
            let screen = screens
                .iter()
                .find(|screen| contains(screen.frame()))
                .or_else(|| NSScreen::mainScreen(mtm))?;
            let frame = screen.frame();
            let visible = screen.visibleFrame();
            let key = NSString::from_str("NSScreenNumber");
            let number = screen.deviceDescription().objectForKey(&key)?;
            let number = number.downcast::<NSNumber>().ok()?;
            Some(Self {
                display_id: number.unsignedIntValue(),
                width: frame.size.width,
                visible_bottom: frame.origin.y + frame.size.height - visible.origin.y,
            })
        }
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use gpui::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        IsWindowVisible, SetForegroundWindow, ShowWindow, SW_HIDE, SW_RESTORE, SW_SHOW,
    };

    fn hwnd(window: &Window) -> Option<HWND> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return None;
        };
        Some(HWND(handle.hwnd.get() as *mut _))
    }

    pub fn set_title_bar_dark(window: &Window, dark: bool) {
        use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
        let Some(hwnd) = hwnd(window) else {
            return;
        };
        let value: i32 = dark.into();
        // SAFETY: `value` is a BOOL-sized integer that outlives the call, and
        // the attribute takes exactly that.
        unsafe {
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &value as *const i32 as *const core::ffi::c_void,
                std::mem::size_of::<i32>() as u32,
            );
        }
    }

    pub fn hide_window(window: &Window, cx: &gpui::App) -> bool {
        let Some(hwnd) = hwnd(window) else {
            return false;
        };
        let raw = hwnd.0 as isize;
        cx.foreground_executor()
            .spawn(async move {
                // SAFETY: GPUI owns this live window handle.
                unsafe {
                    let _ = ShowWindow(HWND(raw as *mut _), SW_HIDE);
                }
            })
            .detach();
        true
    }

    pub fn show_window(window: &Window, cx: &gpui::App) {
        let Some(hwnd) = hwnd(window) else {
            return;
        };
        let raw = hwnd.0 as isize;
        cx.foreground_executor()
            .spawn(async move {
                let hwnd = HWND(raw as *mut _);
                // SAFETY: GPUI owns this live window handle.
                unsafe {
                    let _ = ShowWindow(hwnd, SW_SHOW);
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                    let _ = SetForegroundWindow(hwnd);
                }
            })
            .detach();
    }

    pub fn is_window_visible(window: &Window) -> bool {
        // SAFETY: GPUI owns this live window handle.
        hwnd(window).is_some_and(|hwnd| unsafe { IsWindowVisible(hwnd).as_bool() })
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod imp {
    use gpui::Window;

    /// GPUI on Linux cannot unmap a window, and it ends the app when the
    /// last window closes. Minimizing keeps the window, and with it the app.
    pub fn hide_window(window: &Window, _cx: &gpui::App) -> bool {
        window.minimize_window();
        true
    }

    pub fn show_window(window: &Window, _cx: &gpui::App) {
        window.activate_window();
    }

    pub fn is_window_visible(_window: &Window) -> bool {
        true
    }
}
