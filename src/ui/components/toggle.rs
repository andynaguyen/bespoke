use std::time::Duration;

use gpui::{
    div, prelude::*, px, rems, Animation, AnimationExt, App, ElementId, SharedString, Window,
};

use super::KeyboardFocus;
use super::RowDirection;
use super::{misc::Spinner, SettingContainer};
use crate::ui::clay::Clay;
use crate::ui::theme::{mix, palette, Alpha};

type ChangeHandler = Box<dyn Fn(bool, &mut Window, &mut App)>;

struct SwitchMemory {
    checked: bool,
    changes: u64,
}

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

    pub fn updating(mut self, updating: bool) -> Self {
        self.updating = updating;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Box::new(handler));
        self
    }
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

        let rtl = crate::i18n::is_rtl();
        let paint = move |el: gpui::Div, t: f32| {
            let offset = if rtl { 1. - t } else { t };
            el.bg(mix(off_track, on_track, t)).child(
                div()
                    .absolute()
                    .top(px(2.))
                    .left(px(2. + 14. * offset))
                    .size(px(14.))
                    .clay(
                        if checked { on_knob } else { off_knob },
                        14.,
                        px(7.),
                        p.is_dark,
                    ),
            )
        };
        let track = div()
            .relative()
            .w(px(32.))
            .h(px(18.))
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

#[derive(IntoElement)]
pub struct ToggleSwitchRow {
    toggle: ToggleSwitch,
    title: SharedString,
    description: SharedString,
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
            description: description.into(),
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
        SettingContainer::new(self.title)
            .description(self.description)
            .child(self.toggle)
    }
}
