//! The capsule: the one floating control that dictation and Annotate share.
//!
//! Both features show the same shape while they work: a status lead (the
//! live dot, a spinner, or a check), a body, then actions after a divider.
//! The capsule sits on clear glass (`ui::glass`) tinted toward the theme, so
//! the app under it stays visible and its text stays readable.

use std::time::Duration;

use gpui::{
    div, prelude::*, px, Animation, AnimationExt, App, Div, Hsla, Pixels, SharedString, Stateful,
};

use super::clay::Clay;
use super::glass::{glass_with, GlassSpec, GlassStyle};
use super::theme::{palette, shadow, Alpha};

/// The capsule's height.
pub const HEIGHT: f32 = 48.;
/// Fully round.
pub const RADIUS: f32 = HEIGHT / 2.;
/// A chip inside the capsule: a button or the close control.
pub const CHIP: f32 = 36.;
/// The gap between the capsule's edge and a chip, so their corners nest.
pub const INSET: f32 = (HEIGHT - CHIP) / 2.;

/// The capsule's colors in the current theme.
#[derive(Clone, Copy)]
pub struct Colors {
    pub text: Hsla,
    /// The timer, labels after the main one, and the resting close icon.
    pub secondary: Hsla,
    pub divider: Hsla,
    /// A chip's fill, and a hovered icon chip's.
    pub chip: Hsla,
    pub chip_hover: Hsla,
    /// Live recording and ink.
    pub live: Hsla,
    pub accent: Hsla,
    pub done: Hsla,
    /// The tint over the glass; also the flat fill without native glass.
    pub tint: Hsla,
    /// The hairline rim.
    pub rim: Hsla,
    /// A keycap hint's clay.
    pub keycap: Hsla,
    /// Whether the theme is dark, which deepens clay's shadows.
    pub dark: bool,
}

impl Colors {
    pub fn current(cx: &App) -> Self {
        let p = palette(cx);
        if p.is_dark {
            Self {
                text: gpui::rgb(0xf5f5f7).into(),
                secondary: gpui::rgba(0xebebf59e).into(),
                divider: gpui::white().a(0.14),
                chip: gpui::white().a(0.12),
                chip_hover: gpui::white().a(0.2),
                live: p.ink,
                accent: p.accent,
                done: p.done,
                tint: gpui::rgba(0x14141873).into(),
                rim: gpui::white().a(0.16),
                keycap: p.clay,
                dark: true,
            }
        } else {
            Self {
                text: gpui::rgb(0x1d1d1f).into(),
                secondary: gpui::rgba(0x3c3c43a6).into(),
                divider: gpui::black().a(0.1),
                chip: gpui::black().a(0.06),
                chip_hover: gpui::black().a(0.11),
                live: p.ink,
                accent: p.accent,
                done: p.done,
                tint: gpui::rgba(0xffffff80).into(),
                rim: gpui::white().a(0.6),
                keycap: p.clay,
                dark: false,
            }
        }
    }
}

/// The capsule's surface: tinted clear glass with a hairline rim, and a
/// drop shadow where there is no native glass to draw one. `id` must be
/// unique within the window. Children draw over the glass.
pub fn surface(id: impl Into<SharedString>, radius: Pixels, c: &Colors) -> Div {
    let native = super::glass::native_available();
    div()
        .relative()
        .flex_none()
        .flex()
        .rounded(radius)
        .overflow_hidden()
        .border_1()
        .border_color(c.rim)
        .when(!native, |el| {
            el.bg(c.tint.a(1.))
                .shadow(vec![shadow(0., 6., 18., 0., 0.28)])
        })
        .child(glass_with(
            id,
            GlassSpec {
                style: GlassStyle::Clear,
                radius,
                tint: Some(c.tint),
            },
        ))
}

/// The live dot, in clay. Once capture starts, a halo ripples out and fades
/// while the dot breathes, both on one 1.6s cycle. Before that it rests
/// muted and flat.
pub fn live_dot(ready: bool, c: &Colors) -> impl IntoElement {
    let (live, dark) = (c.live, c.dark);
    let cycle = Duration::from_millis(1600);
    div()
        .relative()
        .size(px(10.))
        .flex_none()
        .when(ready, |el| {
            el.child(div().absolute().rounded_full().with_animation(
                "capsule-dot-halo",
                Animation::new(cycle).repeat(),
                move |halo, ripple| {
                    let r = 5. + 7. * ripple;
                    halo.left(px(5. - r))
                        .top(px(5. - r))
                        .size(px(2. * r))
                        .bg(live.a(0.34 * (1. - ripple).powi(2)))
                },
            ))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .clay(live, 10., px(5.), dark)
                    .with_animation(
                        "capsule-dot-breath",
                        Animation::new(cycle).repeat(),
                        move |dot, t| {
                            let breath = 0.5 + 0.5 * (std::f32::consts::TAU * t).cos();
                            dot.opacity(0.8 + 0.2 * breath)
                        },
                    ),
            )
        })
        .when(!ready, |el| {
            el.child(div().absolute().inset_0().rounded_full().bg(c.text.a(0.3)))
        })
}

/// Eight dots on a ring, brightest at a head that goes around once a second.
pub fn spinner(c: &Colors) -> impl IntoElement {
    let color = c.text;
    div()
        .relative()
        .size(px(16.))
        .flex_none()
        .children((0..8).map(move |k| {
            let angle = k as f32 * std::f32::consts::FRAC_PI_4 - std::f32::consts::FRAC_PI_2;
            div()
                .absolute()
                .left(px(8. + 6.2 * angle.cos() - 1.35))
                .top(px(8. + 6.2 * angle.sin() - 1.35))
                .size(px(2.7))
                .rounded_full()
                .bg(color)
                .with_animation(
                    SharedString::from(format!("capsule-spin-{k}")),
                    Animation::new(Duration::from_secs(1)).repeat(),
                    move |dot, t| {
                        let behind = (t * 8. - k as f32).rem_euclid(8.);
                        dot.opacity((1. - behind / 7.5).clamp(0.18, 1.))
                    },
                )
        }))
}

/// A white check on a green clay disc, for a finished paste or capture.
pub fn done_mark(c: &Colors) -> impl IntoElement {
    div()
        .size(px(22.))
        .flex_none()
        .clay(c.done, 22., px(11.), c.dark)
        .flex()
        .items_center()
        .justify_center()
        .text_color(gpui::white())
        .child(super::components::icon("check").size(px(13.)))
}

/// A thin vertical rule between the body and the actions.
pub fn divider(c: &Colors) -> impl IntoElement {
    div()
        .w(px(13.))
        .flex_none()
        .flex()
        .justify_center()
        .child(div().w(px(1.)).h(px(20.)).bg(c.divider))
}

/// How a chip is filled.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ChipKind {
    /// A neutral fill.
    Plain,
    /// The primary action, tinted blue.
    Primary,
    /// Starts a recording, tinted red.
    Live,
    /// A bare icon that gains a circle on hover, like the close control.
    Icon,
}

/// A capsule-shaped button inside the capsule.
pub fn chip(id: impl Into<SharedString>, kind: ChipKind, c: &Colors) -> Stateful<Div> {
    let (fill, hover, text) = match kind {
        ChipKind::Plain => (c.chip, c.chip_hover, c.text),
        ChipKind::Primary => (c.accent, c.accent.a(0.85), gpui::white()),
        ChipKind::Live => (c.live, c.live.a(0.85), gpui::white()),
        ChipKind::Icon => (gpui::transparent_black(), c.chip_hover, c.secondary),
    };
    let hover_text = if kind == ChipKind::Icon { c.text } else { text };
    div()
        .id(id.into())
        .flex_none()
        .h(px(CHIP))
        .when(kind == ChipKind::Icon, |el| el.w(px(CHIP)).justify_center())
        .when(kind != ChipKind::Icon, |el| el.px(px(14.)).gap(px(6.)))
        .rounded_full()
        .flex()
        .items_center()
        .bg(fill)
        .text_color(text)
        .text_size(px(13.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .cursor_pointer()
        .when(matches!(kind, ChipKind::Primary | ChipKind::Live), |el| {
            el.border_1().border_color(gpui::white().a(0.2))
        })
        .hover(move |s| s.bg(hover).text_color(hover_text))
}
