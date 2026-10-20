//! `ui/Slider.tsx`: a setting row with a range track, the formatted value
//! and an optional reset button. Dragging reports every step, like the
//! React `onChange`.

use std::rc::Rc;

use gpui::{
    canvas, div, prelude::*, px, rems, App, Bounds, DispatchPhase, ElementId, FontWeight,
    KeyDownEvent, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, SharedString, Window,
};

use super::RowDirection;
use super::{misc::reset_button, KeyboardFocus, SettingContainer, Typography};
use crate::ui::theme::{palette, shadow, Alpha};

/// The WebKit default width of `<input type="range">`.
const TRACK_WIDTH: f32 = 129.;
const THUMB: f32 = 16.;

#[derive(Default)]
struct SliderState {
    track: Bounds<Pixels>,
    dragging: bool,
}

type ChangeHandler = Rc<dyn Fn(f32, &mut Window, &mut App)>;
type ResetHandler = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Slider {
    id: ElementId,
    title: SharedString,
    description: SharedString,
    value: f32,
    min: f32,
    max: f32,
    step: f32,
    disabled: bool,
    format: Rc<dyn Fn(f32) -> String>,
    on_change: Option<ChangeHandler>,
    on_reset: Option<ResetHandler>,
}

impl Slider {
    pub fn new(
        id: impl Into<ElementId>,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
        value: f32,
        range: std::ops::RangeInclusive<f32>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            description: description.into(),
            value,
            min: *range.start(),
            max: *range.end(),
            step: 0.01,
            disabled: false,
            format: Rc::new(|v| format!("{v:.2}")),
            on_change: None,
            on_reset: None,
        }
    }

    pub fn step(mut self, step: f32) -> Self {
        self.step = step;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn format(mut self, format: impl Fn(f32) -> String + 'static) -> Self {
        self.format = Rc::new(format);
        self
    }

    pub fn on_change(mut self, handler: impl Fn(f32, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    pub fn on_reset(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_reset = Some(Rc::new(handler));
        self
    }
}

/// Maps a window x position to a stepped value inside the track.
fn value_at(x: Pixels, track: Bounds<Pixels>, min: f32, max: f32, step: f32) -> f32 {
    let usable = (track.size.width - px(THUMB)).max(px(1.));
    let fraction = ((x - track.left() - px(THUMB / 2.)) / usable).clamp(0., 1.);
    let raw = min + fraction * (max - min);
    let stepped = min + ((raw - min) / step).round() * step;
    stepped.clamp(min, max)
}

/// The value a key press moves to, like `<input type="range">`: arrows step,
/// Page Up/Down move a tenth of the range, Home and End jump to the ends.
fn value_for_key(key: &str, value: f32, min: f32, max: f32, step: f32) -> Option<f32> {
    let page = ((max - min) / 10.).max(step);
    let next = match key {
        "left" | "down" => value - step,
        "right" | "up" => value + step,
        "pagedown" => value - page,
        "pageup" => value + page,
        "home" => min,
        "end" => max,
        _ => return None,
    };
    let stepped = min + ((next - min) / step).round() * step;
    Some(stepped.clamp(min, max))
}

impl RenderOnce for Slider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| SliderState::default());
        let fraction = if self.max > self.min {
            ((self.value - self.min) / (self.max - self.min)).clamp(0., 1.)
        } else {
            0.
        };
        let (min, max, step, disabled) = (self.min, self.max, self.step, self.disabled);

        let tracker_state = state.clone();
        let drag_state = state.clone();
        let drag_change = self.on_change.clone();
        let tracker = canvas(
            move |bounds, _, cx| {
                tracker_state.update(cx, |state, _| state.track = bounds);
            },
            move |_, _, window, cx| {
                if !drag_state.read(cx).dragging {
                    return;
                }
                let move_state = drag_state.clone();
                let on_change = drag_change.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble
                        || event.pressed_button != Some(MouseButton::Left)
                    {
                        return;
                    }
                    let track = move_state.read(cx).track;
                    if let Some(on_change) = &on_change {
                        on_change(
                            value_at(event.position.x, track, min, max, step),
                            window,
                            cx,
                        );
                    }
                });
                let up_state = drag_state.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        up_state.update(cx, |state, cx| {
                            state.dragging = false;
                            cx.notify();
                        });
                    }
                });
            },
        )
        .absolute()
        .size_full();

        let down_state = state.clone();
        let key_change = self.on_change.clone();
        let value = self.value;
        let on_change = self.on_change.clone();
        let track = div()
            .id(self.id.clone())
            .relative()
            .w(px(TRACK_WIDTH))
            .h(px(THUMB))
            .flex()
            .items_center()
            .when(disabled, |el| el.opacity(0.5).cursor_not_allowed())
            .when(!disabled, |el| {
                el.cursor_pointer()
                    .keyboard_focus(cx)
                    .rounded_full()
                    .on_key_down(move |event: &KeyDownEvent, window, cx| {
                        let key = event.keystroke.key.as_str();
                        let Some(next) = value_for_key(key, value, min, max, step) else {
                            return;
                        };
                        cx.stop_propagation();
                        if let Some(on_change) = &key_change {
                            if next != value {
                                on_change(next, window, cx);
                            }
                        }
                    })
                    .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                        cx.stop_propagation();
                        let track = down_state.read(cx).track;
                        down_state.update(cx, |state, cx| {
                            state.dragging = true;
                            cx.notify();
                        });
                        if let Some(on_change) = &on_change {
                            on_change(
                                value_at(event.position.x, track, min, max, step),
                                window,
                                cx,
                            );
                        }
                    })
            })
            .child(
                div()
                    .w_full()
                    .h(rems(0.375))
                    .rounded_full()
                    .overflow_hidden()
                    .flex()
                    .bg(p.mid_gray.a(0.3))
                    .child(div().h_full().w(gpui::relative(fraction)).bg(p.accent)),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left((px(TRACK_WIDTH) - px(THUMB)) * fraction)
                    .size(px(THUMB))
                    .rounded_full()
                    .bg(gpui::white())
                    .shadow(vec![
                        shadow(0., 0., 0., 1., 0.12),
                        shadow(0., 1., 3., 0., 0.2),
                    ]),
            )
            .child(tracker);

        let on_reset = self.on_reset.clone();
        SettingContainer::new(self.title)
            .description(self.description)
            .disabled(disabled)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .h(rems(1.5))
                    .child(track)
                    .child(
                        div()
                            .w(rems(3.))
                            .flex()
                            .justify_flex_end()
                            .sm_text()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.text.a(0.9))
                            .child((self.format)(self.value)),
                    )
                    .when_some(on_reset, |el, on_reset| {
                        el.child(reset_button(
                            (self.id, SharedString::from("reset")),
                            disabled,
                            move |_, window, cx| on_reset(window, cx),
                            cx,
                        ))
                    }),
            )
    }
}
