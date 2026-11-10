use gpui::{
    Bounds, Context, Entity, FontWeight, Modifiers, MouseButton, MouseDownEvent,
    NavigationDirection, Pixels, SharedString, Window, canvas, div, prelude::*, px,
};

use crate::app_handle::{AppEvent, AppHandle};
use crate::i18n::{t, t_or, t_with};
use crate::shortcut::handy_keys::FrontendKeyEvent;
use crate::ui::clay::Clay;
use crate::ui::components::{
    KeyboardFocus, RowDirection, SettingContainer, Typography, reset_button,
};
use crate::ui::event_bus::event_bus;
use crate::ui::keyboard::format_key_combination;
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{Alpha, mix, palette};
use crate::ui::toast;

pub struct ShortcutInput {
    binding_id: String,
    store: Entity<SettingsStore>,
    recording: bool,
    current: String,
    original: String,
    keyed: String,
    modifier_only: String,
    committing: bool,
    chip: Bounds<Pixels>,
}

fn mouse_shortcut(button: MouseButton, modifiers: &Modifiers) -> Option<String> {
    let name = match button {
        MouseButton::Left => "mouseleft",
        MouseButton::Middle => "mousemiddle",
        MouseButton::Right => "mouseright",
        MouseButton::Navigate(NavigationDirection::Back) => "mousex1",
        MouseButton::Navigate(NavigationDirection::Forward) => "mousex2",
    };
    let modified = modifiers.control || modifiers.alt || modifiers.shift || modifiers.platform;
    if matches!(button, MouseButton::Left | MouseButton::Right) && !modified {
        return None;
    }
    let mut parts = Vec::new();
    if modifiers.control {
        parts.push("ctrl");
    }
    if modifiers.alt {
        parts.push("alt");
    }
    if modifiers.shift {
        parts.push("shift");
    }
    if modifiers.platform {
        parts.push("super");
    }
    parts.push(name);
    Some(parts.join("+"))
}

impl ShortcutInput {
    pub fn new(binding_id: &str, cx: &mut Context<Self>) -> Self {
        let store = SettingsStore::global(cx);
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&event_bus(cx), |this, _, event: &AppEvent, cx| {
            if event.name == "handy-keys-event"
                && let Some(key_event) = event.parse::<FrontendKeyEvent>()
            {
                this.on_handy_keys_event(key_event, cx);
            }
        })
        .detach();
        cx.on_release(|this, _| {
            if this.recording && !this.committing {
                this.stop_backend_recording();
            }
        })
        .detach();
        Self {
            binding_id: binding_id.to_string(),
            store,
            recording: false,
            current: String::new(),
            original: String::new(),
            keyed: String::new(),
            modifier_only: String::new(),
            committing: false,
            chip: Bounds::default(),
        }
    }

    fn app() -> AppHandle {
        AppHandle::global().clone()
    }

    /// Records the next shortcut through the key tap, which sees keys that other apps and
    /// macOS would otherwise take.
    fn start(&mut self, cx: &mut Context<Self>) {
        if self.recording {
            return;
        }
        self.committing = false;
        self.original = self
            .store
            .read(cx)
            .settings
            .bindings
            .get(&self.binding_id)
            .map(|b| b.current_binding.clone())
            .unwrap_or_default();

        if let Err(error) = crate::shortcut::handy_keys::start_handy_keys_recording(Self::app()) {
            if error.contains("secure-input-active") {
                toast::error(t("secureInput.recorderBlocked"), cx);
            } else {
                toast::error(
                    t_with("settings.general.shortcut.errors.set", &[("error", &error)]),
                    cx,
                );
            }
            return;
        }
        self.recording = true;
        self.current.clear();
        self.keyed.clear();
        self.modifier_only.clear();
        cx.notify();
    }

    fn stop_backend_recording(&self) {
        if let Err(e) = crate::shortcut::handy_keys::stop_handy_keys_recording(Self::app()) {
            log::error!("{e}");
        }
    }

    fn reset_state(&mut self, cx: &mut Context<Self>) {
        self.recording = false;
        self.current.clear();
        self.keyed.clear();
        self.modifier_only.clear();
        self.original.clear();
        cx.notify();
    }

    fn commit(&mut self, keys: String, cx: &mut Context<Self>) {
        if self.committing {
            return;
        }
        self.committing = true;
        let original = self.original.clone();
        let id = self.binding_id.clone();
        let store = self.store.clone();
        self.store.update(cx, |s, cx| {
            s.update_binding(
                id.clone(),
                keys,
                move |result, cx| {
                    if let Err(error) = result {
                        toast::error(
                            t_with("settings.general.shortcut.errors.set", &[("error", &error)]),
                            cx,
                        );
                        if !original.is_empty() {
                            store.update(cx, |s, cx| {
                                s.update_binding(
                                    id,
                                    original,
                                    |result, cx| {
                                        if result.is_err() {
                                            toast::error(
                                                t("settings.general.shortcut.errors.reset"),
                                                cx,
                                            );
                                        }
                                    },
                                    cx,
                                )
                            });
                        }
                    }
                },
                cx,
            )
        });
        self.stop_backend_recording();
        self.reset_state(cx);
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.recording || self.committing {
            return;
        }
        self.committing = true;
        self.stop_backend_recording();
        if !self.original.is_empty() {
            let original = self.original.clone();
            let id = self.binding_id.clone();
            self.store.update(cx, |s, cx| {
                s.update_binding(
                    id,
                    original,
                    |result, cx| {
                        if result.is_err() {
                            toast::error(t("settings.general.shortcut.errors.restore"), cx);
                        }
                    },
                    cx,
                )
            });
        }
        self.reset_state(cx);
    }

    fn on_handy_keys_event(&mut self, event: FrontendKeyEvent, cx: &mut Context<Self>) {
        if !self.recording || self.committing {
            return;
        }
        if event.is_key_down && !event.hotkey_string.is_empty() {
            if event.key.is_some() {
                self.keyed = event.hotkey_string.clone();
            } else {
                self.modifier_only = event.hotkey_string.clone();
            }
            self.current = event.hotkey_string;
            cx.notify();
        } else if !event.is_key_down && event.key.is_some() {
            // The main key was released: commit the keyed combo. Never fall
            // back to a modifier-only capture here (issue #1578).
            let keys = if self.keyed.is_empty() {
                event.hotkey_string
            } else {
                self.keyed.clone()
            };
            if !keys.is_empty() {
                self.commit(keys, cx);
            }
        } else if !event.is_key_down && event.key.is_none() && event.modifiers.is_empty() {
            let primary_button = self
                .keyed
                .rsplit('+')
                .next()
                .is_some_and(|last| last == "mouseleft" || last == "mouseright");
            if primary_button {
                self.commit(self.keyed.clone(), cx);
            } else if self.keyed.is_empty() && !self.modifier_only.is_empty() {
                self.commit(self.modifier_only.clone(), cx);
            }
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        if !self.recording {
            return;
        }
        let shortcut = mouse_shortcut(event.button, &event.modifiers);
        let keyed_has_mouse = self.keyed.split('+').any(|k| k.starts_with("mouse"));
        let key_tap_records_it = shortcut.is_some() || keyed_has_mouse;
        if self.committing || key_tap_records_it {
            return;
        }
        if event.button == MouseButton::Left && !self.chip.contains(&event.position) {
            self.cancel(cx);
        }
    }

    fn render_row(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = palette(cx);
        let store = self.store.read(cx);
        let Some(binding) = store.settings.bindings.get(&self.binding_id).cloned() else {
            return div().into_any_element();
        };
        let updating = store.is_updating(&format!("binding_{}", self.binding_id));
        let title = t_or(
            &format!(
                "settings.general.shortcut.bindings.{}.name",
                self.binding_id
            ),
            &binding.name,
        );
        let description = t_or(
            &format!(
                "settings.general.shortcut.bindings.{}.description",
                self.binding_id
            ),
            &binding.description,
        );

        let chip = div()
            .id(SharedString::from(format!("shortcut-{}", self.binding_id)))
            .h(px(26.))
            .px(px(9.))
            .flex()
            .items_center()
            .text_px(12.5)
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap();
        let chip = if self.recording {
            let text: SharedString = if self.current.is_empty() {
                t("settings.general.shortcut.pressKeysOrMouse")
            } else {
                format_key_combination(&self.current).into()
            };
            chip.rounded(px(6.))
                .bg(p.background)
                .border_1()
                .border_color(p.accent)
                .shadow(vec![gpui::BoxShadow {
                    color: p.accent.a(0.15),
                    offset: gpui::point(gpui::px(0.), gpui::px(0.)),
                    blur_radius: gpui::px(0.),
                    spread_radius: gpui::px(2.),
                }])
                .child(text)
        } else {
            chip.muted_clay(p.clay, 26., px(6.), p.is_dark)
                .hover(|s| s.bg(mix(p.clay, p.text, 0.05)))
                .cursor_pointer()
                .keyboard_focus(cx)
                .on_click(cx.listener(move |this, _, _, cx| this.start(cx)))
                .child(format_key_combination(&binding.current_binding))
        };

        let entity = cx.entity().downgrade();
        let paint_entity = entity.clone();
        let recording = self.recording;
        let tracker = canvas(
            move |bounds, _, cx| {
                let _ = entity.update(cx, |this, _| this.chip = bounds);
            },
            move |_, _, window, _| {
                if !recording {
                    return;
                }
                let down = paint_entity.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        let _ = down.update(cx, |this, cx| this.on_mouse_down(event, cx));
                    }
                });
            },
        )
        .absolute()
        .size_full();

        let id = self.binding_id.clone();
        let store = self.store.clone();
        SettingContainer::new(title)
            .description(description)
            .child(
                div()
                    .flex()
                    .row_dir()
                    .items_center()
                    .gap_1()
                    .child(div().relative().child(chip).child(tracker))
                    .child(reset_button(
                        SharedString::from(format!("reset-{}", self.binding_id)),
                        updating,
                        move |_, _, cx| {
                            let id = id.clone();
                            store.update(cx, |s, cx| s.reset_binding(id, cx));
                        },
                        cx,
                    )),
            )
            .into_any_element()
    }
}

impl Render for ShortcutInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_row(cx)
    }
}
