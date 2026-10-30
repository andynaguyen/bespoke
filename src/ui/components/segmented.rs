//! A segmented control: a few options side by side in a track, the selected
//! one raised in muted clay. Reads as a filter, where `Tabs` reads as
//! navigation.
//!
//! Picking another option slides the thumb over to it, a little past it and
//! back, while it takes on the new option's width.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{div, prelude::*, px, App, Bounds, ElementId, FontWeight, Pixels, SharedString, Window};

use super::RowDirection;
use super::{KeyboardFocus, Typography};
use crate::ui::clay::Clay;
use crate::ui::theme::{palette, Alpha, CONTROL_HEIGHT};

type SelectHandler = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;

/// The gap between the track's edge and the thumb.
const TRACK_PADDING: f32 = 3.;
/// The thumb's height.
const THUMB_HEIGHT: f32 = 26.;
/// How long the thumb takes to move to another option.
const SLIDE: Duration = Duration::from_millis(250);

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

    /// Where the thumb is `progress` of the way from `from` to `to`. Its
    /// overshoot stays inside the row of options, `0..end`.
    fn at(from: Span, to: Span, end: f32, progress: f32) -> Span {
        let travel = travel(progress);
        let width = from.width + (to.width - from.width) * travel;
        let center = from.left
            + from.width / 2.
            + (to.left + to.width / 2. - from.left - from.width / 2.) * travel;
        let left = (center - width / 2.).max(0.);
        let right = (center + width / 2.).min(end);
        Span {
            left,
            width: (right - left).max(0.),
        }
    }
}

/// Debug builds: `BESPOKE_DEV_SLIDE=<0..1>` holds every thumb that far along
/// a slide from the first option to the selected one, for dev snapshots.
#[cfg(debug_assertions)]
fn dev_slide(state: &gpui::Entity<Slide>, selected: Option<usize>, cx: &App) -> Option<Span> {
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
                        (progress < 1.).then(|| Slide::at(from, previous, slide.end(), progress))
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
                        Some(span) => div()
                            .absolute()
                            .top_0()
                            .left(px(span.left))
                            .w(px(span.width))
                            .h(px(THUMB_HEIGHT))
                            .muted_clay(p.clay, THUMB_HEIGHT, px(THUMB_HEIGHT / 2.), p.is_dark)
                            .into_any_element(),
                        None => div().absolute().size_0().into_any_element(),
                    })
                    .children(
                        self.options
                            .into_iter()
                            .enumerate()
                            .map(|(index, (id, label))| {
                                let selected = id == self.selected;
                                // While the thumb slides, it draws apart from the options.
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
                                        el.muted_clay(
                                            p.clay,
                                            THUMB_HEIGHT,
                                            px(THUMB_HEIGHT / 2.),
                                            p.is_dark,
                                        )
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
        let start = Slide::at(FROM, TO, 210., 0.);
        assert!((start.left - FROM.left).abs() < 0.01);
        assert!((start.width - FROM.width).abs() < 0.01);
        let end = Slide::at(FROM, TO, 210., 1.);
        assert!((end.left - TO.left).abs() < 0.01);
        assert!((end.width - TO.width).abs() < 0.01);
    }

    #[test]
    fn overshoot_stays_inside_the_row() {
        for i in 0..=100 {
            let span = Slide::at(FROM, TO, 210., i as f32 / 100.);
            assert!(span.left >= 0. && span.left + span.width <= 210.01);
        }
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
