use std::time::Duration;

use gpui::{
    Animation, AnimationExt, App, Bounds, Context, Entity, FontWeight, Global, Pixels, Point,
    SharedString, Size, Window, WindowBackgroundAppearance, WindowBounds, WindowDecorations,
    WindowKind, WindowOptions, div, ease_out_quint, prelude::*, px, size,
};

use super::glass::{GlassStyle, glass};
use super::theme::{Alpha, palette};

const WINDOW_W: f32 = 300.;
const WINDOW_H: f32 = 220.;
const GAP: f32 = 6.;
const RADIUS: f32 = 16.;
const FADE_IN: Duration = Duration::from_millis(120);

#[derive(Clone, PartialEq)]
pub struct Card {
    pub title: SharedString,
    pub rows: Vec<(SharedString, SharedString)>,
    pub body: Option<SharedString>,
}

struct HoverCardView {
    card: Option<Card>,
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
                            .children(
                                card.body
                                    .clone()
                                    .map(|body| div().text_color(p.text.a(0.75)).child(body)),
                            )
                            .when(!card.rows.is_empty(), |el| {
                                el.child(
                                    div().flex().flex_col().children(card.rows.iter().map(row)),
                                )
                            })
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

struct HoverCard {
    view: Entity<HoverCardView>,
    native: Option<super::platform::capsule::CapsuleWindow>,
    showing: Option<(Card, Point<Pixels>)>,
    size: Size<Pixels>,
}

impl HoverCard {
    fn place(&self, anchor: Point<Pixels>, cx: &App) {
        if let Some(native) = &self.native {
            let width = f64::from(f32::from(self.size.width));
            let height = f64::from(f32::from(self.size.height));
            let x = f64::from(f32::from(anchor.x));
            let y = f64::from(f32::from(anchor.y)) - f64::from(GAP) - height;
            native.set_frame([x, y], [width, height], cx);
        }
    }
}

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

pub fn show(card: Card, anchor: Bounds<Pixels>, cx: &mut App) {
    if super::popup::popovers_hidden(cx) {
        hide(cx);
        return;
    }
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

pub fn hide(cx: &mut App) {
    if !cx.has_global::<HoverCard>() {
        return;
    }
    let state = cx.global_mut::<HoverCard>();
    if state.showing.take().is_none() {
        return;
    }
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
    let native = window
        .update(cx, |_, window, _| {
            super::platform::capsule::CapsuleWindow::of(window)
        })
        .ok()
        .flatten();
    if let Some(native) = &native {
        native.set_ignores_mouse(true);
        native.set_shadow(true);
        native.set_corner_radius(f64::from(RADIUS));
    }
    cx.set_global(HoverCard {
        view,
        native,
        showing: None,
        size: size(px(WINDOW_W), px(WINDOW_H)),
    });
    true
}
