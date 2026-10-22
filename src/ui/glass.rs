//! Liquid Glass behind GPUI elements.
//!
//! GPUI draws every window into one Metal layer and has no backdrop blur. On
//! macOS 26 and later, [`glass`] keeps an `NSGlassEffectView` under the
//! element's bounds instead: the view sits in the window's content view just
//! below GPUI's own view, so the system draws the glass and its refraction
//! and GPUI draws the element's content on top. Older macOS gets an
//! `NSVisualEffectView`, and other platforms a translucent fill.
//!
//! The window must not paint an opaque background over the glass, so windows
//! that use it open with `WindowBackgroundAppearance::Transparent` or
//! `Blurred`.
//!
//! Each painted [`glass`] element creates or moves its native view. After the
//! frame, views that no element painted are removed.

use gpui::{
    canvas, div, fill, App, Bounds, Hsla, IntoElement, ParentElement, Pixels, SharedString, Styled,
    Window,
};

use super::theme::palette;

/// The materials. `Window` is the heavy blur behind a whole window, like
/// AppKit's window backgrounds. `Regular` glass suits floating panels that
/// hold text; `Clear` glass keeps what is behind it visible and suits small
/// floating controls like the recording capsule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlassStyle {
    Window,
    Regular,
    Clear,
}

/// The parameters of one native glass view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassSpec {
    pub style: GlassStyle,
    pub radius: Pixels,
    /// Tints the glass, for example blue for a primary button.
    pub tint: Option<Hsla>,
}

/// An element that fills its parent with glass. Put it first in a
/// `.relative()` parent so the parent's other children draw over it.
/// `id` must be unique within the window.
pub fn glass(id: impl Into<SharedString>, style: GlassStyle, radius: Pixels) -> impl IntoElement {
    glass_with(
        id,
        GlassSpec {
            style,
            radius,
            tint: None,
        },
    )
}

/// [`glass`] with a tint.
pub fn glass_with(id: impl Into<SharedString>, spec: GlassSpec) -> impl IntoElement {
    let id = id.into();
    div().absolute().inset_0().child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| paint_glass(&id, bounds, spec, window, cx),
        )
        .size_full(),
    )
}

/// Whether native glass draws on this machine. When false, [`glass`] paints
/// a translucent fill instead.
pub fn native_available() -> bool {
    imp::available()
}

fn paint_glass(
    id: &SharedString,
    bounds: Bounds<Pixels>,
    spec: GlassSpec,
    window: &mut Window,
    cx: &mut App,
) {
    if imp::place(id, bounds, spec, window, cx) {
        return;
    }
    // No native glass: a translucent fill in the palette's panel color.
    let p = palette(cx);
    let color = spec.tint.unwrap_or(match spec.style {
        GlassStyle::Window | GlassStyle::Regular => p.glass,
        GlassStyle::Clear => p.glass_clear,
    });
    window.paint_quad(fill(bounds, color).corner_radii(spec.radius));
}

#[cfg(target_os = "macos")]
mod imp {
    use std::collections::{HashMap, HashSet};

    use gpui::{App, Bounds, Global, Hsla, Pixels, Rgba, SharedString, Window};
    use objc2::rc::Retained;
    use objc2::runtime::AnyClass;
    use objc2::{msg_send, ClassType, Message};
    use objc2_app_kit::{NSColor, NSView, NSWindowOrderingMode};
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    use super::{GlassSpec, GlassStyle};

    struct Entry {
        view: Retained<NSView>,
        generation: u64,
        /// Paint order within the frame; later entries stack higher.
        order: usize,
        frame: Option<(NSRect, GlassSpec)>,
    }

    /// One window's state: its frame count, GPUI's view (the glass sits
    /// right below it), and the stacking order of its last frame.
    struct WindowGlass {
        frame: u64,
        painted: usize,
        gpui_view: Retained<NSView>,
        stack: Vec<SharedString>,
    }

    /// The native views, keyed by GPUI's view pointer and the element id.
    /// Each window counts its own frames, so a frame in one window never
    /// removes another window's glass.
    #[derive(Default)]
    struct Registry {
        entries: HashMap<(usize, SharedString), Entry>,
        windows: HashMap<usize, WindowGlass>,
        /// Windows that painted glass since the last sweep.
        painted: HashSet<usize>,
    }

    impl Global for Registry {}

    fn glass_class() -> Option<&'static AnyClass> {
        AnyClass::get(c"NSGlassEffectView")
    }

    pub fn available() -> bool {
        glass_class().is_some()
    }

    fn gpui_view(window: &Window) -> Option<&'static NSView> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return None;
        };
        // SAFETY: GPUI's view lives as long as its window, and entries for a
        // closed window are dropped on the next sweep.
        Some(unsafe { handle.ns_view.cast::<NSView>().as_ref() })
    }

    fn ns_color(color: Hsla) -> Retained<NSColor> {
        let c: Rgba = color.into();
        NSColor::colorWithSRGBRed_green_blue_alpha(c.r.into(), c.g.into(), c.b.into(), c.a.into())
    }

    /// Whether `style` draws with `NSGlassEffectView` (else with
    /// `NSVisualEffectView`).
    fn uses_glass(style: GlassStyle) -> bool {
        style != GlassStyle::Window && glass_class().is_some()
    }

    fn new_view(frame: NSRect, style: GlassStyle) -> Retained<NSView> {
        let class = if uses_glass(style) {
            glass_class().unwrap_or(objc2_app_kit::NSVisualEffectView::class())
        } else {
            objc2_app_kit::NSVisualEffectView::class()
        };
        // SAFETY: both classes are NSView subclasses with `initWithFrame:`,
        // and the effect view setters take these enum values.
        unsafe {
            let view: Retained<NSView> = msg_send![msg_send![class, alloc], initWithFrame: frame];
            if !uses_glass(style) {
                // NSVisualEffectMaterialUnderWindowBackground for the window,
                // NSVisualEffectMaterialHUDWindow for older macOS's panels.
                let material: isize = if style == GlassStyle::Window { 21 } else { 13 };
                // Behind the window, unless a debug backdrop stands in for it
                // inside the window.
                let blending: isize = if cfg!(debug_assertions)
                    && std::env::var_os("BESPOKE_DEV_BACKDROP").is_some()
                {
                    1
                } else {
                    0
                };
                let _: () = msg_send![&view, setMaterial: material];
                let _: () = msg_send![&view, setBlendingMode: blending];
                let _: () = msg_send![&view, setState: 1isize];
                view.setWantsLayer(true);
            }
            view
        }
    }

    fn configure(view: &NSView, spec: GlassSpec) {
        let radius = f64::from(f32::from(spec.radius));
        // SAFETY: the selectors exist on the class `new_view` picked.
        unsafe {
            if uses_glass(spec.style) {
                let style: isize = match spec.style {
                    GlassStyle::Clear => 1,
                    _ => 0,
                };
                let _: () = msg_send![view, setStyle: style];
                let _: () = msg_send![view, setCornerRadius: radius];
                let tint = spec.tint.map(ns_color);
                let _: () = msg_send![view, setTintColor: tint.as_deref()];
            } else if let Some(layer) = view.layer() {
                let _: () = msg_send![&layer, setCornerRadius: radius];
                let _: () = msg_send![&layer, setMasksToBounds: true];
            }
        }
    }

    /// Creates or moves the glass view for `id`. Returns false when there is
    /// no native view to draw into.
    pub fn place(
        id: &SharedString,
        bounds: Bounds<Pixels>,
        spec: GlassSpec,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let Some(gpui_view) = gpui_view(window) else {
            return false;
        };
        let Some(content) = (unsafe { gpui_view.superview() }) else {
            return false;
        };
        // AppKit's content view counts y up from the bottom.
        let height = content.bounds().size.height;
        let frame = NSRect::new(
            NSPoint::new(
                f32::from(bounds.origin.x).into(),
                height - f64::from(f32::from(bounds.origin.y + bounds.size.height)),
            ),
            NSSize::new(
                f32::from(bounds.size.width).into(),
                f32::from(bounds.size.height).into(),
            ),
        );

        let registry = cx.default_global::<Registry>();
        let window_key = gpui_view as *const NSView as usize;
        if registry.painted.is_empty() {
            cx.defer(sweep);
        }
        let registry = cx.default_global::<Registry>();
        let window = registry.windows.entry(window_key).or_insert_with(|| {
            #[cfg(debug_assertions)]
            dev_backdrop(&content);
            WindowGlass {
                frame: 0,
                painted: 0,
                gpui_view: gpui_view.retain(),
                stack: Vec::new(),
            }
        });
        if registry.painted.insert(window_key) {
            window.frame += 1;
            window.painted = 0;
        }
        let generation = window.frame;
        let order = window.painted;
        window.painted += 1;
        let key = (window_key, id.clone());
        if registry
            .entries
            .get(&key)
            .is_some_and(|entry| entry.frame.is_some_and(|(_, old)| old.style != spec.style))
        {
            if let Some(entry) = registry.entries.remove(&key) {
                entry.view.removeFromSuperview();
            }
        }
        let entry = registry.entries.entry(key).or_insert_with(|| {
            let view = new_view(frame, spec.style);
            content.addSubview_positioned_relativeTo(
                &view,
                NSWindowOrderingMode::Below,
                Some(gpui_view),
            );
            Entry {
                view,
                generation,
                order,
                frame: None,
            }
        });
        entry.generation = generation;
        entry.order = order;
        if entry.frame != Some((frame, spec)) {
            if entry.frame.map(|(_, s)| s) != Some(spec) {
                configure(&entry.view, spec);
            }
            entry.view.setFrame(frame);
            entry.frame = Some((frame, spec));
        }
        true
    }

    /// Debug builds: `BESPOKE_DEV_BACKDROP=<png>` fills the window behind its
    /// glass with an image, so a snapshot shows what the glass refracts
    /// without capturing other apps.
    #[cfg(debug_assertions)]
    fn dev_backdrop(content: &NSView) {
        use objc2::AnyThread;
        use objc2_app_kit::{NSImage, NSImageScaling, NSImageView};
        use objc2_foundation::NSString;

        let Some(path) = std::env::var_os("BESPOKE_DEV_BACKDROP") else {
            return;
        };
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return;
        };
        let path = NSString::from_str(&path.to_string_lossy());
        let Some(image) = NSImage::initWithContentsOfFile(NSImage::alloc(), &path) else {
            return;
        };
        let view = NSImageView::imageViewWithImage(&image, mtm);
        view.setImageScaling(NSImageScaling::ScaleAxesIndependently);
        view.setFrame(content.bounds());
        view.setAutoresizingMask(
            objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
                | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Below, None);
    }

    /// Runs after a frame: removes the views no element painted in it, in
    /// the windows that painted, and the views of closed windows.
    fn sweep(cx: &mut App) {
        let registry = cx.default_global::<Registry>();
        let painted = std::mem::take(&mut registry.painted);
        let frames: HashMap<usize, u64> = registry
            .windows
            .iter()
            .map(|(key, window)| (*key, window.frame))
            .collect();
        registry.entries.retain(|(window_key, _), entry| {
            let stale = painted.contains(window_key) && entry.generation != frames[window_key];
            let keep = !stale && entry.view.window().is_some();
            if !keep {
                entry.view.removeFromSuperview();
            }
            keep
        });
        registry
            .windows
            .retain(|_, window| window.gpui_view.window().is_some());

        // Restack a window's glass when its paint order changed, so a view
        // created late (or recreated) does not cover the ones painted after it.
        for window_key in painted {
            let mut stack: Vec<(usize, SharedString)> = registry
                .entries
                .iter()
                .filter(|((key, _), _)| *key == window_key)
                .map(|((_, id), entry)| (entry.order, id.clone()))
                .collect();
            stack.sort();
            let stack: Vec<SharedString> = stack.into_iter().map(|(_, id)| id).collect();
            let Some(window) = registry.windows.get_mut(&window_key) else {
                continue;
            };
            if window.stack == stack {
                continue;
            }
            let Some(content) = (unsafe { window.gpui_view.superview() }) else {
                continue;
            };
            for id in &stack {
                if let Some(entry) = registry.entries.get(&(window_key, id.clone())) {
                    entry.view.removeFromSuperview();
                    content.addSubview_positioned_relativeTo(
                        &entry.view,
                        NSWindowOrderingMode::Below,
                        Some(&window.gpui_view),
                    );
                }
            }
            window.stack = stack;
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use gpui::{App, Bounds, Pixels, SharedString, Window};

    use super::GlassSpec;

    pub fn available() -> bool {
        false
    }

    pub fn place(
        _id: &SharedString,
        _bounds: Bounds<Pixels>,
        _spec: GlassSpec,
        _window: &mut Window,
        _cx: &mut App,
    ) -> bool {
        false
    }
}
