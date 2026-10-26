//! Small primitives: spinners, badges, alerts, the reset button, path display,
//! progress bars, tooltips and the width tracker.

use std::time::Duration;

use gpui::{
    canvas, div, percentage, prelude::*, px, rems, Animation, AnimationExt, AnyElement, AnyView,
    App, ClickEvent, Context, ElementId, FontWeight, Hsla, Pixels, Rems, SharedString, WeakEntity,
    Window,
};

use super::RowDirection;
use super::{icon, Button, ButtonSize, ButtonVariant, KeyboardFocus, Typography};
use crate::i18n::t;
use crate::ui::theme::{palette, shadow_lg, tw, Alpha};

/// `animate-spin` spinners.
pub struct Spinner;

impl Spinner {
    /// `border-2 border-{color} border-t-transparent rounded-full animate-spin`.
    pub fn ring(size: Rems, color: Hsla) -> impl IntoElement {
        icon("spinner-ring")
            .size(size)
            .text_color(color)
            .with_animation(
                "spinner-ring",
                Animation::new(Duration::from_secs(1)).repeat(),
                |icon, delta| icon.rotate(percentage(delta)),
            )
    }

    /// Lucide `Loader2` with `animate-spin`.
    pub fn loader(size: Rems, color: Hsla) -> impl IntoElement {
        icon("loader-circle")
            .size(size)
            .text_color(color)
            .with_animation(
                "spinner-loader",
                Animation::new(Duration::from_secs(1)).repeat(),
                |icon, delta| icon.rotate(percentage(delta)),
            )
    }
}

/// Opacity curve of Tailwind's `animate-pulse` (2s, 1 → 0.5 → 1).
pub fn pulse_opacity(delta: f32) -> f32 {
    1. - 0.5 * (1. - (delta * std::f32::consts::TAU).cos()) / 2.
}

pub fn pulse_animation() -> Animation {
    Animation::new(Duration::from_secs(2)).repeat()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BadgeVariant {
    Primary,
    Secondary,
}

/// `ui/Badge.tsx`: a small rounded pill.
pub fn badge(variant: BadgeVariant, cx: &App) -> gpui::Div {
    let p = palette(cx);
    let (bg, fg) = match variant {
        BadgeVariant::Primary => (p.accent, p.accent_contrast),
        BadgeVariant::Secondary => (p.mid_gray.a(0.2), p.text.a(0.7)),
    };
    div()
        .flex()
        .row_dir()
        .items_center()
        .px_3()
        .py_1()
        .rounded_full()
        .xs_text()
        .font_weight(FontWeight::MEDIUM)
        .bg(bg)
        .text_color(fg)
}

/// `ui/Alert.tsx` with `variant="error"`, the only variant the app uses.
/// `contained` drops the rounded corners for use inside a card.
pub fn error_alert(contained: bool, text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .flex()
        .row_dir()
        .items_start()
        .gap_3()
        .p_4()
        .bg(tw::red_500().a(0.1))
        .when(!contained, |el| el.rounded_lg())
        .child(
            icon("circle-alert")
                .size(rems(1.25))
                .flex_none()
                .mt(rems(0.125))
                .text_color(tw::red_500()),
        )
        .child(div().sm_text().text_color(tw::red_400()).child(text.into()))
}

/// `ui/ResetButton.tsx`: the circular-arrow icon button next to a control.
pub fn reset_button(
    id: impl Into<ElementId>,
    disabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let p = palette(cx);
    div()
        .id(id)
        .p_1()
        .rounded_lg()
        .flex()
        .row_dir()
        .items_center()
        .justify_center()
        .when(disabled, |el| {
            el.opacity(0.5)
                .cursor_not_allowed()
                .text_color(p.text.a(0.4))
        })
        .when(!disabled, |el| {
            el.cursor_pointer()
                .keyboard_focus(cx)
                .text_color(p.text.a(0.6))
                .hover(|s| s.bg(p.mid_gray.a(0.15)).text_color(p.text))
                .active(|s| s.bg(p.mid_gray.a(0.25)))
                .on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    on_click(event, window, cx)
                })
        })
        .child(icon("reset").size(px(20.)))
}

/// `ui/PathDisplay.tsx`: a monospace path with an Open button.
pub fn path_display(
    id: impl Into<ElementId>,
    path: SharedString,
    on_open: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let p = palette(cx);
    let empty = path.is_empty();
    div()
        .flex()
        .row_dir()
        .items_center()
        .gap_2()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .px_2()
                .py_2()
                .bg(p.background)
                .border_1()
                .border_color(p.mid_gray.a(0.25))
                .rounded_lg()
                .xs_text()
                .font_family(crate::ui::theme::MONO_FONT)
                .child(path),
        )
        .child(
            Button::new(id)
                .variant(ButtonVariant::Secondary)
                .size(ButtonSize::Sm)
                .px_3()
                .py_2()
                .disabled(empty)
                .child(t("common.open"))
                .on_click(on_open),
        )
}

/// A rounded progress track with an accent fill, `percent` in 0..=100.
pub fn progress_track(
    percent: f32,
    width: impl Into<gpui::Length>,
    height: Pixels,
    cx: &App,
) -> gpui::Div {
    let p = palette(cx);
    div()
        .w(width.into())
        .h(height)
        .rounded_full()
        .overflow_hidden()
        .bg(p.mid_gray.a(0.2))
        .child(
            div()
                .h_full()
                .rounded_full()
                .bg(p.accent)
                .w(gpui::relative(percent.clamp(0., 100.) / 100.)),
        )
}

/// Stores the width of its parent in `slot` and re-renders the owner when it
/// changes. GPUI has no container queries, so layouts that switch or size on
/// their own width read the stored value. Add it as the last child of a
/// `relative()` parent.
pub fn width_tracker<T: 'static>(
    entity: WeakEntity<T>,
    slot: fn(&mut T) -> &mut Pixels,
) -> impl IntoElement {
    canvas(
        move |bounds, _, cx| {
            let changed = entity
                .update(cx, |this, _| {
                    let width = slot(this);
                    let changed = *width != bounds.size.width;
                    *width = bounds.size.width;
                    changed
                })
                .unwrap_or(false);
            // `Window::refresh` is a no-op mid-draw, so re-render after this
            // frame finishes.
            if changed {
                cx.defer(move |cx| {
                    let _ = entity.update(cx, |_, cx| cx.notify());
                });
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

type TooltipBuilder = std::rc::Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// Hover tooltip, styled like `ui/Tooltip.tsx`. In the main window it is a
/// glass popup (`ui::popup`).
pub struct Tooltip {
    build: TooltipBuilder,
}

impl Tooltip {
    pub fn text(text: impl Into<SharedString>, _window: &mut Window, cx: &mut App) -> AnyView {
        let text = text.into();
        Self::element(
            move |_, _| div().sm_text().child(text.clone()).into_any_element(),
            cx,
        )
    }

    pub fn element(
        build: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
        cx: &mut App,
    ) -> AnyView {
        cx.new(|_| Self {
            build: std::rc::Rc::new(build),
        })
        .into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        if crate::ui::popup::hosts(window, cx) {
            // GPUI places this element at the pointer; the tooltip itself is a
            // glass popup there that lets clicks through.
            let build = self.build.clone();
            let spec = crate::ui::popup::Spec {
                placement: crate::ui::popup::Placement::At,
                width: None,
                frame_radius: Some(crate::ui::theme::radius::ROW),
                interactive: false,
            };
            return div()
                .size(px(1.))
                .child(crate::ui::popup::anchor(
                    "tooltip",
                    spec,
                    move |window, cx| {
                        crate::ui::popup::surface("tooltip", crate::ui::theme::radius::ROW)
                            .max_w(px(200.))
                            .px_3()
                            .py_2()
                            .child(build(window, cx))
                            .into_any_element()
                    },
                ))
                .into_any_element();
        }
        let content = (self.build)(window, cx);
        div()
            .max_w(px(200.))
            .px_3()
            .py_2()
            .bg(p.background)
            .border_1()
            .border_color(p.mid_gray.a(0.25))
            .rounded_lg()
            .shadow(shadow_lg())
            .text_color(p.text)
            .child(content)
            .into_any_element()
    }
}
