//! `ui/ToggleSwitch.tsx`: the switch control, and the setting row around it.

use std::time::Duration;

use gpui::{
    div, prelude::*, rems, Animation, AnimationExt, App, ElementId, Hsla, SharedString, Window,
};

use super::KeyboardFocus;
use super::RowDirection;
use super::{misc::Spinner, SettingContainer};
use crate::ui::theme::{palette, shadow_sm, Alpha};

type ChangeHandler = Box<dyn Fn(bool, &mut Window, &mut App)>;

/// Remembers the last rendered value so the knob only animates on change.
struct SwitchMemory {
    checked: bool,
    changes: u64,
}

/// The 37.5 x 22.5 switch: mid-gray track and white knob when off, accent
/// track and contrast knob when on. The knob slides over 150ms.
#[derive(IntoElement)]
pub struct ToggleSwitch {
    id: ElementId,
    checked: bool,
    updating: bool,
    on_change: Option<ChangeHandler>,
}

impl ToggleSwitch {
    pub fn new(id: impl Into<ElementId>, checked: bool) -> Self {
        Self {
            id: id.into(),
            checked,
            updating: false,
            on_change: None,
        }
    }

    /// Shows a spinner over the switch and blocks input while a write runs.
    pub fn updating(mut self, updating: bool) -> Self {
        self.updating = updating;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Box::new(handler));
        self
    }
}

fn lerp_color(from: Hsla, to: Hsla, t: f32) -> Hsla {
    let a: gpui::Rgba = from.into();
    let b: gpui::Rgba = to.into();
    gpui::Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
    .into()
}

impl RenderOnce for ToggleSwitch {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let checked = self.checked;
        let blocked = self.updating;
        let (off_track, on_track) = (p.mid_gray.a(0.3), p.accent);
        let (off_knob, on_knob) = (gpui::white(), p.accent_contrast);

        let memory = window.use_keyed_state(
            (self.id.clone(), SharedString::from("memory")),
            cx,
            |_, _| SwitchMemory {
                checked,
                changes: 0,
            },
        );
        let changes = memory.update(cx, |memory, _| {
            if memory.checked != checked {
                memory.checked = checked;
                memory.changes += 1;
            }
            memory.changes
        });

        // The knob starts at the inline start: the right edge in RTL.
        let rtl = crate::i18n::is_rtl();
        let paint = move |el: gpui::Div, t: f32| {
            let offset = if rtl { 1. - t } else { t };
            el.bg(lerp_color(off_track, on_track, t)).child(
                div()
                    .absolute()
                    .top(rems(0.125))
                    .left(rems(0.125 + offset))
                    .size(rems(1.25))
                    .rounded_full()
                    .bg(if checked { on_knob } else { off_knob })
                    .shadow(shadow_sm()),
            )
        };
        let track = div()
            .relative()
            .w(rems(2.5))
            .h(rems(1.5))
            .flex_shrink_0()
            .rounded_full();
        let track = if changes == 0 {
            paint(track, if checked { 1. } else { 0. }).into_any_element()
        } else {
            track
                .with_animation(
                    (
                        self.id.clone(),
                        SharedString::from(format!("anim-{changes}")),
                    ),
                    Animation::new(Duration::from_millis(150)),
                    move |el, delta| paint(el, if checked { delta } else { 1. - delta }),
                )
                .into_any_element()
        };

        let on_change = self.on_change;
        div()
            .id(self.id)
            .relative()
            .flex()
            .row_dir()
            .items_center()
            .rounded_full()
            .when(blocked, |el| el.cursor_not_allowed())
            .when(!blocked, |el| {
                el.cursor_pointer().keyboard_focus_ring(0.25, cx)
            })
            .child(track)
            .when(self.updating, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .row_dir()
                        .items_center()
                        .justify_center()
                        .child(Spinner::ring(rems(1.), p.accent)),
                )
            })
            .when(!blocked, |el| {
                el.on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    if let Some(handler) = &on_change {
                        handler(!checked, window, cx);
                    }
                })
            })
    }
}

/// A setting row with a switch on the right (`ToggleSwitch` with a label).
#[derive(IntoElement)]
pub struct ToggleSwitchRow {
    toggle: ToggleSwitch,
    title: SharedString,
    description: Option<SharedString>,
}

impl ToggleSwitchRow {
    pub fn new(
        id: impl Into<ElementId>,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
        checked: bool,
    ) -> Self {
        Self {
            toggle: ToggleSwitch::new(id, checked),
            title: title.into(),
            description: Some(description.into()),
        }
    }

    pub fn updating(mut self, updating: bool) -> Self {
        self.toggle = self.toggle.updating(updating);
        self
    }

    pub fn on_change(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.toggle = self.toggle.on_change(handler);
        self
    }
}

impl RenderOnce for ToggleSwitchRow {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut row = SettingContainer::new(self.title);
        if let Some(description) = self.description {
            row = row.description(description);
        }
        row.child(self.toggle)
    }
}
