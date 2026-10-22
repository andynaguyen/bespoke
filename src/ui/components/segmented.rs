//! A segmented control: a few options side by side in a track, the selected
//! one raised. Reads as a filter, where `Tabs` reads as navigation.
//!
//! Picking another option slides the thumb over like Liquid Glass: it lifts
//! into a clear lens that swells and stretches as it travels, overshoots a
//! little, and settles back into the raised thumb.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    div, linear_color_stop, linear_gradient, prelude::*, px, App, Bounds, ElementId, FontWeight,
    Hsla, Pixels, SharedString, Window,
};

use super::RowDirection;
use super::{KeyboardFocus, Typography};
use crate::ui::theme::{palette, shadow, Alpha, Palette, CONTROL_HEIGHT};

type SelectHandler = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;

/// The gap between the track's edge and the thumb.
const TRACK_PADDING: f32 = 3.;
/// The thumb's height when it rests.
const THUMB_HEIGHT: f32 = 26.;
/// How long the thumb takes to move to another option.
const SLIDE: Duration = Duration::from_millis(300);

#[derive(IntoElement)]
pub struct SegmentedControl {
    id: ElementId,
    options: Vec<(SharedString, SharedString)>,
    selected: SharedString,
    on_select: SelectHandler,
}

impl SegmentedControl {
    /// `options` is `(id, label)` pairs in display order.
    pub fn new(
        id: impl Into<ElementId>,
        options: Vec<(SharedString, SharedString)>,
        selected: impl Into<SharedString>,
        on_select: impl Fn(&SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            options,
            selected: selected.into(),
            on_select: Rc::new(on_select),
        }
    }
}

/// A thumb's place in the row of options: its left edge and width.
#[derive(Clone, Copy, Default, PartialEq)]
struct Span {
    left: f32,
    width: f32,
}

/// What the control remembers between frames to animate the thumb.
#[derive(Default)]
struct Slide {
    selected: SharedString,
    /// Each option's span, measured on the last frame.
    spans: Vec<Span>,
    /// Where the thumb set out from, and when.
    from: Option<(Span, Instant)>,
}

impl Slide {
    /// The right edge of the last option.
    fn end(&self) -> f32 {
        self.spans.last().map_or(0., |span| span.left + span.width)
    }

    /// Where the thumb is `progress` of the way from `from` to `to`, with
    /// how much it has lifted off the track (0 at rest, 1 at the peak). It
    /// may swell a few points past the row's ends, `0..end`, but no further.
    fn at(from: Span, to: Span, end: f32, progress: f32) -> (Span, f32) {
        let travel = travel(progress);
        // Lifts while it travels and is back on the track before it settles.
        let lift = (std::f32::consts::PI * (progress / 0.85).min(1.))
            .sin()
            .max(0.);
        let distance = (to.left - from.left).abs();
        let width = from.width + (to.width - from.width) * travel;
        // Swells, and stretches along its path while it moves fastest.
        let width = width * (1. + 0.14 * lift) + distance * 0.18 * lift;
        let center = from.left
            + from.width / 2.
            + (to.left + to.width / 2. - from.left - from.width / 2.) * travel;
        let reach = 5. * lift;
        let left = (center - width / 2.).max(-reach);
        let right = (center + width / 2.).min(end + reach);
        (
            Span {
                left,
                width: (right - left).max(0.),
            },
            lift,
        )
    }
}

/// Debug builds: `BESPOKE_DEV_SLIDE=<0..1>` holds every thumb that far along
/// a slide from the first option to the selected one, for dev snapshots.
#[cfg(debug_assertions)]
fn dev_slide(
    state: &gpui::Entity<Slide>,
    selected: Option<usize>,
    cx: &App,
) -> Option<(Span, f32)> {
    let progress: f32 = std::env::var("BESPOKE_DEV_SLIDE").ok()?.parse().ok()?;
    let spans = &state.read(cx).spans;
    let to = *spans.get(selected?)?;
    let from = *spans.first()?;
    Some(Slide::at(
        from,
        to,
        state.read(cx).end(),
        progress.clamp(0., 0.999),
    ))
}

/// How far along its path the thumb is: it speeds up, slows into the new
/// option by 80% of the slide, then drifts a little past it and back, like
/// a spring with some give.
fn travel(t: f32) -> f32 {
    let u = (t / 0.8).clamp(0., 1.);
    let smooth = u * u * u * (u * (u * 6. - 15.) + 10.);
    let give = ((t - 0.6) / 0.4).clamp(0., 1.);
    smooth + 0.035 * (std::f32::consts::PI * give).sin()
}

impl RenderOnce for SegmentedControl {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| Slide {
            selected: self.selected.clone(),
            ..Default::default()
        });
        let selected_index = self.options.iter().position(|(id, _)| *id == self.selected);

        // Set the thumb off when the selection changes, from wherever it is.
        let now = Instant::now();
        let thumb = state.update(cx, |slide, _| {
            let to = selected_index.and_then(|index| slide.spans.get(index).copied());
            if slide.selected != self.selected {
                let previous = self
                    .options
                    .iter()
                    .position(|(id, _)| *id == slide.selected)
                    .and_then(|index| slide.spans.get(index).copied());
                let current = match (slide.from, previous) {
                    (Some((from, started)), Some(previous)) => {
                        let progress =
                            now.duration_since(started).as_secs_f32() / SLIDE.as_secs_f32();
                        (progress < 1.).then(|| Slide::at(from, previous, slide.end(), progress).0)
                    }
                    _ => None,
                }
                .or(previous);
                slide.selected = self.selected.clone();
                slide.from = if crate::ui::platform::reduce_motion() {
                    None
                } else {
                    current.map(|span| (span, now))
                };
            }
            let (from, started) = slide.from?;
            let progress = now.duration_since(started).as_secs_f32() / SLIDE.as_secs_f32();
            if progress >= 1. {
                slide.from = None;
                return None;
            }
            let end = slide.end();
            to.map(|to| Slide::at(from, to, end, progress))
        });
        if thumb.is_some() {
            window.request_animation_frame();
        }
        #[cfg(debug_assertions)]
        let thumb = thumb.or_else(|| dev_slide(&state, selected_index, cx));

        let measured = state.downgrade();
        div()
            .flex()
            .row_dir()
            .items_center()
            .h(CONTROL_HEIGHT)
            .p(px(TRACK_PADDING))
            .rounded_full()
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .text_px(13.)
            .font_weight(FontWeight::MEDIUM)
            .child(
                div()
                    .relative()
                    .flex()
                    .row_dir()
                    .items_center()
                    .gap(px(2.))
                    // The first child is the thumb; the rest are the options.
                    .on_children_prepainted(move |bounds: Vec<Bounds<Pixels>>, _, cx| {
                        let Some(first) = bounds.get(1) else {
                            return;
                        };
                        let origin = f32::from(first.origin.x);
                        let spans: Vec<Span> = bounds[1..]
                            .iter()
                            .map(|b| Span {
                                left: f32::from(b.origin.x) - origin,
                                width: f32::from(b.size.width),
                            })
                            .collect();
                        let _ = measured.update(cx, |slide, _| {
                            if slide.spans != spans {
                                slide.spans = spans;
                            }
                        });
                    })
                    .child(match thumb {
                        Some((span, lift)) => lens(span, lift, &p).into_any_element(),
                        None => div().absolute().size_0().into_any_element(),
                    })
                    .children(
                        self.options
                            .into_iter()
                            .enumerate()
                            .map(|(index, (id, label))| {
                                let selected = id == self.selected;
                                // While the lens travels it stands in for the thumb.
                                let raised = selected && thumb.is_none();
                                let on_select = self.on_select.clone();
                                div()
                                    .id((self.id.clone(), SharedString::from(index.to_string())))
                                    .relative()
                                    .h(px(THUMB_HEIGHT))
                                    .px(px(12.))
                                    .flex()
                                    .items_center()
                                    .rounded_full()
                                    .whitespace_nowrap()
                                    .cursor_pointer()
                                    .keyboard_focus(cx)
                                    .when(raised, |el| {
                                        el.bg(p.raised).shadow(vec![shadow(
                                            0.,
                                            1.,
                                            2.,
                                            0.,
                                            if p.is_dark { 0.25 } else { 0.08 },
                                        )])
                                    })
                                    .when(selected, |el| el.text_color(p.text))
                                    .when(!selected, |el| {
                                        el.text_color(p.mid_gray)
                                            .hover(|s| s.text_color(p.text.a(0.85)))
                                    })
                                    .on_click(move |_, window, cx| on_select(&id, window, cx))
                                    .child(label)
                            }),
                    ),
            )
    }
}

/// The thumb in flight: the raised fill thins into clear glass with a
/// bright rim and a sheen across its top, and it grows past the track.
fn lens(span: Span, lift: f32, p: &Palette) -> impl IntoElement {
    let height = THUMB_HEIGHT * (1. + 0.42 * lift);
    // Glass brightens what is under it, so the lens never goes darker than
    // the track. A drop shadow would show through it, so it fades out.
    let fill: Hsla = if p.is_dark {
        gpui::white().a(0.15 - 0.05 * lift)
    } else {
        gpui::white().a(0.92 - 0.55 * lift)
    };
    let rim = gpui::white().a(if p.is_dark {
        0.14 + 0.5 * lift
    } else {
        0.6 + 0.4 * lift
    });
    div()
        .absolute()
        .left(px(span.left))
        .top(px((THUMB_HEIGHT - height) / 2.))
        .w(px(span.width))
        .h(px(height))
        .rounded_full()
        .bg(fill)
        .border_1()
        .border_color(rim)
        .shadow(vec![shadow(
            0.,
            1.,
            2.,
            0.,
            (if p.is_dark { 0.25 } else { 0.08 }) * (1. - lift),
        )])
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded_full()
                .bg(linear_gradient(
                    180.,
                    linear_color_stop(gpui::white().a(0.35 * lift), 0.),
                    linear_color_stop(gpui::white().a(0.), 0.55),
                )),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    const FROM: Span = Span {
        left: 0.,
        width: 80.,
    };
    const TO: Span = Span {
        left: 150.,
        width: 60.,
    };

    #[test]
    fn slide_starts_and_ends_on_the_options() {
        let (start, lift) = Slide::at(FROM, TO, 210., 0.);
        assert!((start.left - FROM.left).abs() < 0.01 && lift.abs() < 0.01);
        let (end, lift) = Slide::at(FROM, TO, 210., 1.);
        assert!((end.left - TO.left).abs() < 0.01);
        assert!((end.width - TO.width).abs() < 0.01);
        assert!(lift.abs() < 0.01);
    }

    #[test]
    fn lens_swells_mid_slide_but_stays_near_the_track() {
        let (span, lift) = Slide::at(FROM, TO, 210., 0.4);
        assert!(lift > 0.9);
        assert!(span.width > TO.width.max(FROM.width));
        assert!(span.left >= -5. && span.left + span.width <= 215.);
    }

    #[test]
    fn travel_overshoots_a_little_then_settles() {
        let peak = (0..=100)
            .map(|i| travel(i as f32 / 100.))
            .fold(0., f32::max);
        assert!(peak > 1. && peak < 1.04);
        assert!((travel(1.) - 1.).abs() < 0.001);
    }
}
