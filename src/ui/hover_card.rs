//! A hover card on Liquid Glass, like the one over a heatmap day.
//!
//! Native glass sits under GPUI's drawing in its window (`ui::glass`), so a
//! card drawn inside the main window would hide its glass under whatever the
//! page paints there. The card gets a window of its own instead: one shared,
//! transparent window that ignores the mouse, moved over the anchor and
//! ordered in and out. The window takes the card's measured size, so the
//! system shadow and hairline it draws around its frame hug the card.

use std::time::Duration;

use gpui::{
    div, ease_out_quint, prelude::*, px, size, Animation, AnimationExt, App, Bounds, Context,
    Entity, FontWeight, Global, Pixels, Point, SharedString, Size, Window,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowKind, WindowOptions,
};

use super::glass::{glass, GlassStyle};
use super::theme::{palette, Alpha};

/// The widest a card gets, and the window's size before a card is measured.
const WINDOW_W: f32 = 300.;
const WINDOW_H: f32 = 220.;
/// The gap between the anchor and the card.
const GAP: f32 = 6.;
const RADIUS: f32 = 16.;
/// How long the card's text takes to fade and rise into place.
const FADE_IN: Duration = Duration::from_millis(120);

/// What a card shows: a title, then label and value rows.
#[derive(Clone, PartialEq)]
pub struct Card {
    pub title: SharedString,
    pub rows: Vec<(SharedString, SharedString)>,
}

struct HoverCardView {
    card: Option<Card>,
    /// Counts cards shown, so each new one fades in.
    shown: usize,
}

impl Render for HoverCardView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let Some(card) = self.card.clone() else {
            return div().into_any_element();
        };
        let row = |(label, value): &(SharedString, SharedString)| {
            div()
                .flex()
                .justify_between()
                .gap_4()
                .child(
                    div()
                        .text_color(p.text.a(0.62))
                        .whitespace_nowrap()
                        .child(label.clone()),
                )
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .whitespace_nowrap()
                        .child(value.clone()),
                )
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_start()
            // The window fits itself to the card (`fit`).
            .on_children_prepainted(|bounds, _, cx| {
                if let Some(card) = bounds.first() {
                    let measured = card.size;
                    cx.defer(move |cx| fit(measured, cx));
                }
            })
            .child(
                div()
                    .relative()
                    .flex_none()
                    .max_w(px(WINDOW_W))
                    .min_w(px(200.))
                    .rounded(px(RADIUS))
                    .child(glass("hover-card", GlassStyle::Regular, px(RADIUS)))
                    .child(
                        div()
                            .px(px(16.))
                            .pt(px(13.))
                            .pb(px(12.))
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .text_color(p.text)
                            .text_size(px(12.5))
                            .line_height(px(20.))
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .line_height(px(20.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(card.title.clone()),
                            )
                            .child(div().flex().flex_col().children(card.rows.iter().map(row)))
                            .with_animation(
                                SharedString::from(format!("hover-card-{}", self.shown)),
                                Animation::new(FADE_IN).with_easing(ease_out_quint()),
                                |content, t| content.opacity(t).mt(px(3. * (1. - t))),
                            ),
                    ),
            )
            .into_any_element()
    }
}

/// The shared card window, opened on first use.
struct HoverCard {
    view: Entity<HoverCardView>,
    #[cfg(target_os = "macos")]
    native: Option<super::platform::capsule::CapsuleWindow>,
    /// The card and the top-left of the day it shows over, to skip repeat
    /// calls.
    showing: Option<(Card, Point<Pixels>)>,
    /// The card's measured size, which the window takes.
    size: Size<Pixels>,
}

impl HoverCard {
    /// Puts the window's frame above `anchor`, at the card's size.
    fn place(&self, anchor: Point<Pixels>, cx: &App) {
        #[cfg(target_os = "macos")]
        if let Some(native) = &self.native {
            let width = f64::from(f32::from(self.size.width));
            let height = f64::from(f32::from(self.size.height));
            let x = f64::from(f32::from(anchor.x));
            let y = f64::from(f32::from(anchor.y)) - f64::from(GAP) - height;
            native.set_frame([x, y], [width, height], cx);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (anchor, cx);
    }
}

/// Resizes the window to the card's measured `size`, keeping it above the
/// anchor.
fn fit(size: Size<Pixels>, cx: &mut App) {
    if !cx.has_global::<HoverCard>() {
        return;
    }
    let state = cx.global_mut::<HoverCard>();
    if state.size == size {
        return;
    }
    state.size = size;
    let state = cx.global::<HoverCard>();
    if let Some((_, anchor)) = &state.showing {
        state.place(*anchor, cx);
    }
}

impl Global for HoverCard {}

/// Shows `card` above `anchor`, a rectangle in global screen points with
/// the origin at the top left of the main display, as `Window::bounds`
/// counts them.
pub fn show(card: Card, anchor: Bounds<Pixels>, cx: &mut App) {
    let origin = anchor.origin;
    if !cx.has_global::<HoverCard>() && !open(cx) {
        return;
    }
    let state = cx.global_mut::<HoverCard>();
    if state
        .showing
        .as_ref()
        .is_some_and(|(shown, at)| *shown == card && *at == origin)
    {
        return;
    }
    let changed = state.showing.as_ref().map(|(shown, _)| shown) != Some(&card);
    let appearing = state.showing.is_none();
    state.showing = Some((card.clone(), origin));
    let view = state.view.clone();
    let state = cx.global::<HoverCard>();
    state.place(origin, cx);
    #[cfg(target_os = "macos")]
    if let Some(native) = &state.native {
        native.set_visible(true, cx);
    }
    if changed {
        view.update(cx, |view, cx| {
            view.card = Some(card);
            view.shown += 1;
            cx.notify();
        });
    }
    // GPUI skips drawing a window that is ordered out, and the window comes
    // in on the next main-thread turn, so draw again once it is on screen.
    if appearing {
        let view = view.downgrade();
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(16))
                .await;
            let _ = view.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }
}

/// Hides the card, if one is showing.
pub fn hide(cx: &mut App) {
    if !cx.has_global::<HoverCard>() {
        return;
    }
    let state = cx.global_mut::<HoverCard>();
    if state.showing.take().is_none() {
        return;
    }
    #[cfg(target_os = "macos")]
    if let Some(native) = state.native.clone() {
        native.set_visible(false, cx);
    }
}

fn open(cx: &mut App) -> bool {
    let view = cx.new(|_| HoverCardView {
        card: None,
        shown: 0,
    });
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            gpui::point(px(0.), px(0.)),
            size(px(WINDOW_W), px(WINDOW_H)),
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
    let window = match cx.open_window(options, move |window, _| {
        super::platform::prepare_overlay_window(window);
        root
    }) {
        Ok(window) => window,
        Err(e) => {
            log::error!("Failed to open the hover card: {e}");
            return false;
        }
    };
    #[cfg(not(target_os = "macos"))]
    let _ = window;
    #[cfg(target_os = "macos")]
    let native = window
        .update(cx, |_, window, _| {
            super::platform::capsule::CapsuleWindow::of(window)
        })
        .ok()
        .flatten();
    #[cfg(target_os = "macos")]
    if let Some(native) = &native {
        native.set_ignores_mouse(true);
        // The frame's shadow and hairline follow the card's corners.
        native.set_shadow(true);
        native.set_corner_radius(f64::from(RADIUS));
    }
    cx.set_global(HoverCard {
        view,
        #[cfg(target_os = "macos")]
        native,
        showing: None,
        size: size(px(WINDOW_W), px(WINDOW_H)),
    });
    true
}
