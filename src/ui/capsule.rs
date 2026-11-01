use std::time::Duration;

use gpui::{
    div, prelude::*, px, Animation, AnimationExt, App, Div, Hsla, Pixels, SharedString, Stateful,
};

use super::clay::Clay;
use super::glass::{glass_with, GlassSpec, GlassStyle};
use super::theme::{palette, shadow, Alpha};

pub const HEIGHT: f32 = 48.;
pub const RADIUS: f32 = HEIGHT / 2.;
pub const CHIP: f32 = 36.;
pub const INSET: f32 = (HEIGHT - CHIP) / 2.;

#[derive(Clone, Copy)]
pub struct Colors {
    pub text: Hsla,
    pub secondary: Hsla,
    pub divider: Hsla,
    pub chip: Hsla,
    pub chip_hover: Hsla,
    pub live: Hsla,
    pub accent: Hsla,
    pub done: Hsla,
    pub tint: Hsla,
    pub rim: Hsla,
    pub keycap: Hsla,
    pub dark: bool,
}

impl Colors {
    pub fn current(cx: &App) -> Self {
        let p = palette(cx);
        let (secondary, divider, chip, chip_hover, tint, rim) = if p.is_dark {
            (
                gpui::rgba(0xebebf59e).into(),
                gpui::white().a(0.14),
                gpui::white().a(0.12),
                gpui::white().a(0.2),
                gpui::rgba(0x14141873).into(),
                gpui::white().a(0.16),
            )
        } else {
            (
                gpui::rgba(0x3c3c43a6).into(),
                gpui::black().a(0.1),
                gpui::black().a(0.06),
                gpui::black().a(0.11),
                gpui::rgba(0xffffff80).into(),
                gpui::white().a(0.6),
            )
        };
        Self {
            text: p.text,
            secondary,
            divider,
            chip,
            chip_hover,
            live: p.ink,
            accent: p.accent,
            done: p.done,
            tint,
            rim,
            keycap: p.clay,
            dark: p.is_dark,
        }
    }
}

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

pub fn divider(c: &Colors) -> impl IntoElement {
    div()
        .w(px(13.))
        .flex_none()
        .flex()
        .justify_center()
        .child(div().w(px(1.)).h(px(20.)).bg(c.divider))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ChipKind {
    Plain,
    Primary,
    Live,
    Icon,
}

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
