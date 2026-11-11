use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, App, Bounds, Div, FontFeatures, FontWeight, Hsla, Pixels,
    SharedString, Stateful, WeakEntity, div, prelude::*, px,
};

use super::clay::Clay;
use super::components::width_tracker;
use super::glass::{GlassSpec, GlassStyle, glass_with};
use super::platform::capsule as native;
use super::theme::{Alpha, palette, shadow};

pub const HEIGHT: f32 = 36.;
pub const RADIUS: f32 = HEIGHT / 2.;
pub const CHIP: f32 = 28.;
pub const INSET: f32 = (HEIGHT - CHIP) / 2.;
pub const KEYCAP: f32 = 16.;
pub const DIVIDER_W: f32 = 9.;
pub const METER_BARS: usize = 5;

const METER_BAR_W: f32 = 3.;
const METER_GAP: f32 = 2.;
pub const METER_W: f32 = METER_BARS as f32 * METER_BAR_W + (METER_BARS - 1) as f32 * METER_GAP;
const METER_H: f32 = 14.;
pub const DOT: f32 = 8.;
pub const SPINNER: f32 = 16.;
pub const DONE: f32 = 16.;
pub const WARN_ICON: f32 = 13.;
pub const LEAD_PAD: f32 = 8.;
pub const LEAD_GAP: f32 = 7.;
const SPEECH_BANDS: [usize; METER_BARS] = [0, 2, 4, 6, 8];

const FOLD_AFTER_START: Duration = Duration::from_millis(3_500);
const FOLD_LINGER: Duration = Duration::from_millis(1_200);
pub const MORPH: Duration = Duration::from_millis(460);

fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let curve = |a: f32, b: f32, s: f32| {
        let u = 1. - s;
        3. * u * u * s * a + 3. * u * s * s * b + s * s * s
    };
    let (mut lo, mut hi) = (0f32, 1f32);
    for _ in 0..20 {
        let mid = (lo + hi) / 2.;
        if curve(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    curve(y1, y2, (lo + hi) / 2.)
}

pub fn ease_morph(t: f32) -> f32 {
    cubic_bezier(0.22, 1., 0.36, 1., t)
}

pub fn ease(t: f32) -> f32 {
    cubic_bezier(0.25, 0.1, 0.25, 1., t)
}

#[derive(Clone, Copy)]
pub struct Tween {
    from: f32,
    to: f32,
    start: Option<Instant>,
    duration: Duration,
    ease: fn(f32) -> f32,
}

impl Tween {
    pub fn new(value: f32, duration: Duration, ease: fn(f32) -> f32) -> Self {
        Self {
            from: value,
            to: value,
            start: None,
            duration,
            ease,
        }
    }

    fn progress(&self) -> f32 {
        match self.start {
            Some(start) if !self.duration.is_zero() => {
                (start.elapsed().as_secs_f32() / self.duration.as_secs_f32()).min(1.)
            }
            _ => 1.,
        }
    }

    pub fn value(&self) -> f32 {
        self.from + (self.to - self.from) * (self.ease)(self.progress())
    }

    pub fn running(&self) -> bool {
        self.progress() < 1.
    }

    pub fn set(&mut self, to: f32, snap: bool) {
        if snap {
            self.from = to;
            self.to = to;
            self.start = None;
        } else if to != self.to {
            self.from = self.value();
            self.to = to;
            self.start = Some(Instant::now());
        }
    }
}

pub struct Fold {
    open_until: Option<Instant>,
    open: bool,
    progress: Tween,
    measured: Pixels,
}

impl Fold {
    pub fn new() -> Self {
        Self {
            open_until: None,
            open: false,
            progress: Tween::new(0., MORPH, ease_morph),
            measured: px(0.),
        }
    }

    pub fn start(&mut self, now: Instant) {
        self.open_until = Some(now + FOLD_AFTER_START);
        self.open = true;
        self.progress.set(1., true);
    }

    pub fn close(&mut self, snap: bool) {
        self.open_until = None;
        self.open = false;
        self.progress.set(0., snap);
    }

    pub fn tick(&mut self, now: Instant, hovered: bool, snap: bool) -> bool {
        if hovered {
            let linger = now + FOLD_LINGER;
            self.open_until = Some(self.open_until.map_or(linger, |until| until.max(linger)));
        }
        let open = self.open_until.is_some_and(|until| now < until);
        if !open {
            self.open_until = None;
        }
        let toggled = open != self.open;
        if toggled {
            self.open = open;
            self.progress.set(if open { 1. } else { 0. }, snap);
        }
        toggled
    }

    pub fn running(&self) -> bool {
        self.progress.running()
    }

    pub fn width(&self) -> Pixels {
        self.measured * self.progress.value()
    }

    pub fn element<V: 'static>(
        &self,
        buttons: impl IntoElement,
        owner: WeakEntity<V>,
        fold: fn(&mut V) -> &mut Fold,
    ) -> impl IntoElement {
        let tracker = width_tracker(owner, move |this: &mut V| &mut fold(this).measured);
        div()
            .flex_none()
            .flex()
            .overflow_hidden()
            .when(crate::i18n::is_rtl(), |el| el.justify_end())
            .opacity(self.progress.value())
            .w(self.width())
            .child(
                div()
                    .relative()
                    .flex_none()
                    .flex()
                    .items_center()
                    .child(buttons)
                    .child(tracker),
            )
    }
}

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
    pub warn: Hsla,
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
            warn: p.warning,
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
    div()
        .relative()
        .size(px(DOT))
        .flex_none()
        .when(ready, |el| el.clay(c.live, DOT, px(DOT / 2.), c.dark))
        .when(!ready, |el| el.rounded_full().bg(c.text.a(0.3)))
}

pub fn pointer_on(
    window: &native::CapsuleWindow,
    card: Bounds<Pixels>,
    screen_point: [f64; 2],
) -> Option<bool> {
    let [x, y, _, _] = window.frame()?;
    Some(card.contains(&gpui::point(
        px((screen_point[0] - x) as f32),
        px((screen_point[1] - y) as f32),
    )))
}

pub fn smooth_levels(smoothed: &mut [f32; METER_BARS], levels: &[f32]) {
    for (level, band) in smoothed.iter_mut().zip(SPEECH_BANDS) {
        *level = *level * 0.7 + levels.get(band).copied().unwrap_or(0.) * 0.3;
    }
}

pub fn meter(levels: &[f32; METER_BARS], ready: bool, c: &Colors) -> impl IntoElement {
    let color = c.text.a(if ready { 0.6 } else { 0.3 });
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(METER_GAP))
        .h(px(METER_H))
        .children(levels.iter().map(move |level| {
            let height = if ready {
                (3. + level.max(0.).powf(0.7) * (METER_H - 3.)).clamp(3., METER_H)
            } else {
                3.
            };
            div()
                .w(px(METER_BAR_W))
                .h(px(height))
                .rounded(px(METER_BAR_W / 2.))
                .flex_none()
                .bg(color)
        }))
}

pub fn status_row() -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(LEAD_GAP))
        .px(px(LEAD_PAD))
        .when(crate::i18n::is_rtl(), |el| el.flex_row_reverse())
}

pub fn clock(text: impl Into<SharedString>, c: &Colors) -> impl IntoElement {
    let mut clock = div()
        .flex_none()
        .text_size(px(12.))
        .line_height(px(16.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c.secondary)
        .whitespace_nowrap()
        .child(text.into());
    clock
        .text_style()
        .get_or_insert_with(Default::default)
        .font_features = Some(FontFeatures(Arc::new(vec![("tnum".into(), 1)])));
    clock
}

pub fn label(text: impl Into<SharedString>, c: &Colors) -> Div {
    div()
        .text_size(px(12.5))
        .line_height(px(16.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c.text)
        .whitespace_nowrap()
        .child(text.into())
}

pub fn spinner(c: &Colors) -> impl IntoElement {
    let color = c.text;
    div()
        .relative()
        .size(px(SPINNER))
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
        .size(px(DONE))
        .flex_none()
        .clay(c.done, DONE, px(DONE / 2.), c.dark)
        .flex()
        .items_center()
        .justify_center()
        .text_color(gpui::white())
        .child(super::components::icon("check").size(px(10.)))
}

pub fn warn_icon(c: &Colors) -> impl IntoElement {
    div()
        .flex_none()
        .text_color(c.warn)
        .child(super::components::icon("triangle-alert").size(px(WARN_ICON)))
}

pub fn divider(c: &Colors) -> impl IntoElement {
    div()
        .w(px(DIVIDER_W))
        .flex_none()
        .flex()
        .justify_center()
        .child(div().w(px(1.)).h(px(16.)).bg(c.divider))
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
        .when(kind != ChipKind::Icon, |el| el.px(px(9.)).gap(px(5.)))
        .when(crate::i18n::is_rtl(), |el| el.flex_row_reverse())
        .rounded_full()
        .flex()
        .items_center()
        .bg(fill)
        .text_color(text)
        .text_size(px(12.5))
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .cursor_pointer()
        .when(matches!(kind, ChipKind::Primary | ChipKind::Live), |el| {
            el.border_1().border_color(gpui::white().a(0.2))
        })
        .hover(move |s| s.bg(hover).text_color(hover_text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_opens_at_start_and_folds_after() {
        let start = Instant::now();
        let mut fold = Fold::new();
        fold.start(start);
        assert!(!fold.tick(start + Duration::from_secs(3), false, true));
        assert!(fold.open);
        assert!(fold.tick(start + FOLD_AFTER_START, false, true));
        assert!(!fold.open);
    }

    #[test]
    fn hover_opens_and_lingers() {
        let start = Instant::now();
        let mut fold = Fold::new();
        assert!(fold.tick(start, true, true));
        assert!(fold.open);
        assert!(!fold.tick(start + FOLD_LINGER / 2, false, true));
        assert!(fold.tick(start + FOLD_LINGER, false, true));
        assert!(!fold.open);
    }

    #[test]
    fn hover_keeps_the_start_window_open() {
        let start = Instant::now();
        let mut fold = Fold::new();
        fold.start(start);
        let late = start + FOLD_AFTER_START - Duration::from_millis(1);
        fold.tick(late, true, true);
        assert!(!fold.tick(late + FOLD_LINGER / 2, false, true));
        assert!(fold.open);
    }
}
