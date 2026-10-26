//! Menus, toasts, tooltips and dialogs on Liquid Glass.
//!
//! Native glass sits under GPUI's drawing in its window (`ui::glass`), so a
//! menu drawn inside the main window would hide its glass under the page
//! painted there. Each of these surfaces gets a borderless child window of
//! the main window instead, which moves with it and draws the surface over
//! glass.
//!
//! The main window drives menus, toasts and tooltips by painting an
//! [`anchor`] each frame: an element that draws nothing and shows its popup
//! next to its own bounds. A main window frame that paints no anchor for a
//! popup hides it, which [`frame_marker`] tracks. The popup measures its
//! content and sizes its window to it, like `ui::hover_card`.
//!
//! These popups never become key, so the main window keeps keyboard focus.
//! A click in one runs its handler with the popup's window, and [`in_host`]
//! hands the handler the main window instead.
//!
//! A dialog needs the keyboard for its fields, so [`DialogWindow`] is a key
//! window over the whole main window that draws the dialog itself.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    canvas, div, prelude::*, px, size, AnyElement, AnyView, AnyWindowHandle, App, Bounds, Context,
    Div, Entity, Global, Pixels, SharedString, Size, Window, WindowBackgroundAppearance,
    WindowBounds, WindowDecorations, WindowKind, WindowOptions,
};
use gpui_component::Root;

use super::glass::{glass, GlassStyle};
use super::theme::palette;

type Builder = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// The window's size before its content is measured. Content wider than
/// this still lays out at full size.
const START_SIZE: (f32, f32) = (480., 640.);
/// The closest a popup comes to the main window's edges.
const EDGE_MARGIN: f32 = 8.;

/// Where a popup goes relative to its anchor.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Placement {
    /// `gap` below the anchor, lined up with its left edge, or with its
    /// right edge when `end` is set.
    Below { gap: Pixels, end: bool },
    /// With its top-left corner on the anchor's.
    At,
    /// In the anchor's bottom-right corner, `margin` in from both edges.
    BottomRight { margin: Pixels },
}

/// How a popup's window looks and behaves.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Spec {
    pub placement: Placement,
    /// The content's width. `None` sizes the window to the content.
    pub width: Option<Pixels>,
    /// Rounds the window's frame at this radius and gives it the system
    /// shadow and hairline. `None` for content that draws several shapes,
    /// like a stack of toasts.
    pub frame_radius: Option<Pixels>,
    /// Whether the popup takes clicks. A tooltip lets them through.
    pub interactive: bool,
}

/// Whether `window` can host popups: the main window on macOS. Elsewhere
/// callers draw the surface inside the window, on an opaque background.
pub fn hosts(window: &Window, cx: &App) -> bool {
    cfg!(target_os = "macos")
        && super::main_window_handle(cx)
            .is_some_and(|handle| handle.window_id() == window.window_handle().window_id())
}

/// Runs `f` with the main window. A click handler in a popup gets the
/// popup's window, and most handlers expect the window the control lives in.
pub fn in_host(window: &mut Window, cx: &mut App, f: impl FnOnce(&mut Window, &mut App)) {
    match super::main_window_handle(cx) {
        Some(handle) if handle.window_id() != window.window_handle().window_id() => {
            let _ = handle.update(cx, |_, window, cx| f(window, cx));
        }
        _ => f(window, cx),
    }
}

/// A glass panel `radius` round for popup content. Children draw over the
/// glass. `id` must be unique within the popup's window.
pub fn surface(id: impl Into<SharedString>, radius: Pixels) -> Div {
    div()
        .relative()
        .rounded(radius)
        .child(glass(id, GlassStyle::Regular, radius))
}

/// Shows popup `id` next to this element's bounds while the main window
/// paints it. Size the element to the anchor: a dropdown's trigger, or the
/// window for a toast. `content` builds the popup's content in the popup's
/// window on each of its frames.
pub fn anchor(
    id: impl Into<SharedString>,
    spec: Spec,
    content: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
) -> impl IntoElement {
    let id = id.into();
    let content: Builder = Rc::new(content);
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let host = window.bounds();
            let anchor = Bounds::new(host.origin + bounds.origin, bounds.size);
            let popups = cx.default_global::<Popups>();
            let frame = popups.frame;
            if let Some(popup) = popups.entries.get_mut(&id) {
                popup.seen = frame;
            }
            cx.defer(move |cx| show(id, spec, anchor, host, content, cx));
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Counts the main window's frames, so a frame that paints no anchor for a
/// shown popup hides it. Make it the main window's first child, so it paints
/// before every anchor.
pub fn frame_marker() -> impl IntoElement {
    canvas(
        |_, _, _| {},
        |_, _, _, cx| {
            cx.default_global::<Popups>().frame += 1;
            cx.defer(sweep);
        },
    )
    .absolute()
    .size_0()
}

/// Draws a popup's content and reports its size.
struct PopupView {
    id: SharedString,
    content: Option<Builder>,
    width: Option<Pixels>,
}

impl Render for PopupView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let Some(content) = self.content.clone() else {
            return div().into_any_element();
        };
        let id = self.id.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_start()
            .text_color(p.text)
            .text_size(px(13.))
            .line_height(px(18.))
            .when(crate::i18n::is_rtl(), |el| el.text_right())
            // The window takes the content's size (`fit`).
            .on_children_prepainted(move |bounds, _, cx| {
                if let Some(content) = bounds.first() {
                    let (id, measured) = (id.clone(), content.size);
                    cx.defer(move |cx| fit(&id, measured, cx));
                }
            })
            .child(
                div()
                    .flex_none()
                    .when_some(self.width, |el, width| el.w(width))
                    .child(content(window, cx)),
            )
            .into_any_element()
    }
}

struct Popup {
    view: Entity<PopupView>,
    #[cfg(target_os = "macos")]
    native: super::platform::capsule::CapsuleWindow,
    spec: Spec,
    /// The anchor and the main window, in global screen points with the
    /// origin at the top left of the main display, as `Window::bounds`
    /// counts them.
    anchor: Bounds<Pixels>,
    host: Bounds<Pixels>,
    /// The content's measured size.
    size: Option<Size<Pixels>>,
    /// The main window frame that last painted the anchor.
    seen: u64,
    shown: bool,
    /// Whether the window is opaque again after appearing. It orders in
    /// transparent and stays so until its content is measured and placed,
    /// so it never shows the last popup's content or size.
    revealed: bool,
}

#[derive(Default)]
struct Popups {
    entries: HashMap<SharedString, Popup>,
    /// Counts the main window's frames (`frame_marker`).
    frame: u64,
}

impl Global for Popups {}

fn show(
    id: SharedString,
    spec: Spec,
    anchor: Bounds<Pixels>,
    host: Bounds<Pixels>,
    content: Builder,
    cx: &mut App,
) {
    if !cx.default_global::<Popups>().entries.contains_key(&id) {
        let Some(popup) = open(&id, spec, cx) else {
            return;
        };
        cx.global_mut::<Popups>().entries.insert(id.clone(), popup);
    }
    let popups = cx.global_mut::<Popups>();
    let frame = popups.frame;
    let Some(popup) = popups.entries.get_mut(&id) else {
        return;
    };
    popup.seen = frame;
    let moved = popup.anchor != anchor || popup.host != host || popup.spec != spec;
    popup.spec = spec;
    popup.anchor = anchor;
    popup.host = host;
    let appearing = !popup.shown;
    if appearing {
        popup.shown = true;
        popup.revealed = false;
        popup.size = None;
    }
    let view = popup.view.clone();
    view.update(cx, |view, cx| {
        view.content = Some(content);
        view.width = spec.width;
        cx.notify();
    });
    if appearing {
        #[cfg(target_os = "macos")]
        {
            let parent = super::main_window_handle(cx).and_then(|handle| {
                handle
                    .update(cx, |_, window, _| {
                        super::platform::capsule::CapsuleWindow::of(window)
                    })
                    .ok()
                    .flatten()
            });
            if let (Some(parent), Some(popup)) = (parent, cx.global::<Popups>().entries.get(&id)) {
                popup.native.show_attached(&parent, 0., false, cx);
            }
        }
        // GPUI skips drawing a window that is ordered out, and the window
        // comes in on the next main-thread turn, so draw again once it is on
        // screen.
        let view = view.downgrade();
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(16))
                .await;
            let _ = view.update(cx, |_, cx| cx.notify());
        })
        .detach();
    } else if moved {
        place(&id, cx);
    }
}

/// Records the content's measured size and moves the window over it.
fn fit(id: &SharedString, measured: Size<Pixels>, cx: &mut App) {
    let Some(popup) = cx.default_global::<Popups>().entries.get_mut(id) else {
        return;
    };
    if !popup.shown || (popup.revealed && popup.size == Some(measured)) {
        return;
    }
    popup.size = Some(measured);
    place(id, cx);
    let Some(popup) = cx.global_mut::<Popups>().entries.get_mut(id) else {
        return;
    };
    if !popup.revealed {
        popup.revealed = true;
        #[cfg(target_os = "macos")]
        popup.native.clone().set_alpha(1., cx);
    }
}

/// Moves the window to its place by the anchor, inside the main window.
fn place(id: &SharedString, cx: &App) {
    let Some(popup) = cx.global::<Popups>().entries.get(id) else {
        return;
    };
    let Some(size) = popup.size else {
        return;
    };
    let frame = frame_for(popup.spec.placement, popup.anchor, popup.host, size);
    #[cfg(target_os = "macos")]
    popup.native.set_frame(
        [points(frame.origin.x), points(frame.origin.y)],
        [points(size.width), points(size.height)],
        cx,
    );
    #[cfg(not(target_os = "macos"))]
    let _ = frame;
}

/// The popup's frame for content of `size`, kept `EDGE_MARGIN` inside the
/// main window where it fits.
fn frame_for(
    placement: Placement,
    anchor: Bounds<Pixels>,
    host: Bounds<Pixels>,
    size: Size<Pixels>,
) -> Bounds<Pixels> {
    let origin = match placement {
        Placement::Below { gap, end } => gpui::point(
            if end {
                anchor.right() - size.width
            } else {
                anchor.left()
            },
            anchor.bottom() + gap,
        ),
        Placement::At => anchor.origin,
        Placement::BottomRight { margin } => gpui::point(
            anchor.right() - margin - size.width,
            anchor.bottom() - margin - size.height,
        ),
    };
    let clamp = |value: Pixels, start: Pixels, end: Pixels, length: Pixels| {
        let margin = px(EDGE_MARGIN);
        let value = if value + length > end - margin {
            end - margin - length
        } else {
            value
        };
        if value < start + margin {
            start + margin
        } else {
            value
        }
    };
    Bounds::new(
        gpui::point(
            clamp(origin.x, host.left(), host.right(), size.width),
            clamp(origin.y, host.top(), host.bottom(), size.height),
        ),
        size,
    )
}

/// AppKit's points for `pixels`.
#[cfg(target_os = "macos")]
fn points(pixels: Pixels) -> f64 {
    f64::from(f32::from(pixels))
}

/// Runs after a main window frame: hides the popups whose anchor it did not
/// paint.
fn sweep(cx: &mut App) {
    let popups = cx.default_global::<Popups>();
    let frame = popups.frame;
    let stale: Vec<SharedString> = popups
        .entries
        .iter()
        .filter(|(_, popup)| popup.shown && popup.seen != frame)
        .map(|(id, _)| id.clone())
        .collect();
    for id in stale {
        hide(&id, cx);
    }
}

fn hide(id: &SharedString, cx: &mut App) {
    let Some(popup) = cx.global_mut::<Popups>().entries.get_mut(id) else {
        return;
    };
    popup.shown = false;
    #[cfg(target_os = "macos")]
    {
        let native = popup.native.clone();
        native.hide_detached(cx);
    }
}

fn open(id: &SharedString, spec: Spec, cx: &mut App) -> Option<Popup> {
    let view = cx.new(|_| PopupView {
        id: id.clone(),
        content: None,
        width: None,
    });
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            gpui::point(px(0.), px(0.)),
            size(px(START_SIZE.0), px(START_SIZE.1)),
        ))),
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some(crate::app_handle::APP_IDENTIFIER.into()),
        window_decorations: Some(WindowDecorations::Client),
        ..Default::default()
    };
    let root = view.clone();
    let window = match cx.open_window(options, move |window, cx| {
        // Never key, so the main window keeps the keyboard.
        super::platform::prepare_overlay_window(window);
        cx.new(|cx| Root::new(root, window, cx))
    }) {
        Ok(window) => window,
        Err(e) => {
            log::error!("Failed to open popup {id}: {e}");
            return None;
        }
    };
    #[cfg(target_os = "macos")]
    let native = window
        .update(cx, |_, window, _| {
            super::platform::capsule::CapsuleWindow::of(window)
        })
        .ok()
        .flatten()?;
    #[cfg(target_os = "macos")]
    {
        native.set_ignores_mouse(!spec.interactive);
        if let Some(radius) = spec.frame_radius {
            native.set_shadow(true);
            native.set_corner_radius(points(radius));
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = window;
    Some(Popup {
        view,
        #[cfg(target_os = "macos")]
        native,
        spec,
        anchor: Bounds::default(),
        host: Bounds::default(),
        size: None,
        seen: 0,
        shown: false,
        revealed: false,
    })
}

/// A modal dialog's window: a transparent key window over the whole main
/// window that draws the dialog over its own glass. The main window dims
/// itself underneath and calls [`DialogWindow::sync`] as it paints, so the
/// dialog's window follows it when it resizes.
#[derive(Clone)]
pub struct DialogWindow {
    handle: AnyWindowHandle,
    #[cfg(target_os = "macos")]
    native: super::platform::capsule::CapsuleWindow,
    #[cfg(target_os = "macos")]
    parent: super::platform::capsule::CapsuleWindow,
    frame: Rc<Cell<Bounds<Pixels>>>,
}

/// Holds the dialog's view and reports keyboard use for focus rings, like
/// the main window's root.
struct DialogHost {
    view: AnyView,
}

impl Render for DialogHost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        div()
            .size_full()
            .relative()
            .text_color(p.text)
            .text_size(px(13.))
            .line_height(px(18.))
            .when(crate::i18n::is_rtl(), |el| el.text_right())
            .capture_key_down(|_, window, cx| {
                crate::ui::components::set_keyboard_modality(true, window, cx)
            })
            .capture_any_mouse_down(|_, window, cx| {
                crate::ui::components::set_keyboard_modality(false, window, cx)
            })
            .child(self.view.clone())
    }
}

impl DialogWindow {
    /// Opens a window over `host`, the main window, with `build`'s view and
    /// gives it keyboard focus. `build` gets the dialog's window.
    pub fn open(
        host: &Window,
        build: impl FnOnce(&mut Window, &mut App) -> AnyView + 'static,
        cx: &mut App,
    ) -> Option<Self> {
        let frame = host.bounds();
        #[cfg(target_os = "macos")]
        let parent = super::platform::capsule::CapsuleWindow::of(host)?;
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(frame)),
            titlebar: None,
            focus: false,
            show: false,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            app_id: Some(crate::app_handle::APP_IDENTIFIER.into()),
            window_decorations: Some(WindowDecorations::Client),
            ..Default::default()
        };
        let handle = match cx.open_window(options, move |window, cx| {
            let view = build(window, cx);
            let host = cx.new(|_| DialogHost { view });
            cx.new(|cx| Root::new(host, window, cx))
        }) {
            Ok(handle) => handle,
            Err(e) => {
                log::error!("Failed to open the dialog window: {e}");
                return None;
            }
        };
        #[cfg(target_os = "macos")]
        let native = handle
            .update(cx, |_, window, _| {
                super::platform::capsule::CapsuleWindow::of(window)
            })
            .ok()
            .flatten()?;
        #[cfg(target_os = "macos")]
        {
            native.set_corner_radius(points(super::theme::radius::WINDOW));
            native.show_attached(&parent, 1., true, cx);
        }
        Some(Self {
            handle: handle.into(),
            #[cfg(target_os = "macos")]
            native,
            #[cfg(target_os = "macos")]
            parent,
            frame: Rc::new(Cell::new(frame)),
        })
    }

    /// Keeps the window over the main window, whose frame is `host`. A
    /// dialog opened while the main window was hidden (What's New at a
    /// hidden launch) comes in here, once the main window draws.
    pub fn sync(&self, host: Bounds<Pixels>, cx: &App) {
        #[cfg(target_os = "macos")]
        if !self.native.is_visible() {
            self.native.show_attached(&self.parent, 1., true, cx);
        }
        if self.frame.replace(host) == host {
            return;
        }
        #[cfg(target_os = "macos")]
        self.native.set_frame(
            [points(host.origin.x), points(host.origin.y)],
            [points(host.size.width), points(host.size.height)],
            cx,
        );
        #[cfg(not(target_os = "macos"))]
        let _ = cx;
    }

    /// Closes the window. Deferred, since the dialog may be closing itself
    /// from inside its own window.
    pub fn close(self, cx: &mut App) {
        cx.defer(move |cx| {
            #[cfg(target_os = "macos")]
            self.native.hide_detached(cx);
            let _ = self
                .handle
                .update(cx, |_, window, _| window.remove_window());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(gpui::point(px(x), px(y)), size(px(w), px(h)))
    }

    #[test]
    fn places_below_and_inside_the_window() {
        let host = bounds(100., 100., 800., 600.);
        let trigger = bounds(300., 200., 180., 34.);
        let menu = size(px(180.), px(120.));
        let below = Placement::Below {
            gap: px(4.),
            end: false,
        };
        assert_eq!(
            frame_for(below, trigger, host, menu),
            bounds(300., 238., 180., 120.)
        );
        // Near the bottom it moves up to stay inside the window.
        let low = bounds(300., 640., 180., 34.);
        assert_eq!(
            frame_for(below, low, host, menu).origin.y,
            px(100. + 600. - 8. - 120.)
        );
        // End-aligned menus line up with the trigger's right edge.
        let end = Placement::Below {
            gap: px(4.),
            end: true,
        };
        assert_eq!(
            frame_for(end, trigger, host, size(px(240.), px(80.)))
                .origin
                .x,
            px(480. - 240.)
        );
    }

    #[test]
    fn places_toasts_in_the_corner() {
        let host = bounds(0., 0., 1000., 800.);
        let placement = Placement::BottomRight { margin: px(24.) };
        assert_eq!(
            frame_for(placement, host, host, size(px(356.), px(60.))),
            bounds(620., 716., 356., 60.)
        );
    }
}
