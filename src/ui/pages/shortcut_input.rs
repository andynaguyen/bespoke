//! The shortcut recorder row (`ShortcutInput.tsx`).
//!
//! macOS, and anyone on the handy-keys backend, records through the backend
//! listener (`HandyKeysShortcutInput.tsx`): it reports key events as
//! `handy-keys-event` and the combo commits when its key is released, or when
//! every modifier is released for a modifier-only shortcut.
//!
//! The global-hotkey backend records keys in the window instead
//! (`GlobalShortcutInput.tsx`), with all shortcuts suspended meanwhile.
//!
//! In both, a plain left click outside the recorder cancels.

use std::collections::BTreeSet;

use gpui::{
    canvas, div, prelude::*, rems, App, Bounds, Context, Entity, FocusHandle, FontWeight,
    KeyDownEvent, KeyUpEvent, Modifiers, ModifiersChangedEvent, MouseButton, MouseDownEvent,
    MouseUpEvent, NavigationDirection, Pixels, SharedString, Window,
};

use crate::app_handle::{AppEvent, AppHandle};
use crate::i18n::{t, t_or, t_with};
use crate::settings::KeyboardImplementation;
use crate::shortcut::handy_keys::FrontendKeyEvent;
use crate::ui::components::{
    reset_button, KeyboardFocus, RowDirection, SettingContainer, Typography,
};
use crate::ui::event_bus::event_bus;
use crate::ui::keyboard::format_key_combination;
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{palette, shadow, Alpha};
use crate::ui::toast;

const MODIFIERS: &[&str] = &[
    "ctrl", "control", "shift", "alt", "option", "meta", "command", "cmd", "super", "win",
    "windows",
];

/// State of an in-window (global-hotkey) recording.
#[derive(Default)]
struct WindowCapture {
    pressed: BTreeSet<String>,
    captured: Vec<String>,
    mouse: Option<(MouseButton, String)>,
    modifiers: Modifiers,
}

pub struct ShortcutInput {
    binding_id: String,
    store: Entity<SettingsStore>,
    recording: bool,
    /// Keys shown while recording.
    current: String,
    original: String,
    /// Last combo that included a main key (handy-keys).
    keyed: String,
    /// Last modifier-only combo (handy-keys).
    modifier_only: String,
    committing: bool,
    capture: WindowCapture,
    focus: FocusHandle,
    chip: Bounds<Pixels>,
}

fn uses_handy_keys(cx: &App) -> bool {
    cfg!(target_os = "macos")
        || SettingsStore::get(cx).keyboard_implementation == KeyboardImplementation::HandyKeys
}

/// The stored token for a mouse button, or `None` when the click is not a
/// recordable shortcut (unmodified left and right clicks stay clicks).
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

/// GPUI key name → stored token (`getKeyName` in keyboard.ts).
fn key_token(key: &str) -> String {
    match key {
        "escape" => "esc".into(),
        "pageup" => "pageup".into(),
        "pagedown" => "pagedown".into(),
        "capslock" => "capslock".into(),
        other => other.to_lowercase(),
    }
}

fn modifier_tokens(modifiers: &Modifiers) -> Vec<&'static str> {
    let mac = cfg!(target_os = "macos");
    let mut tokens = Vec::new();
    if modifiers.shift {
        tokens.push("shift");
    }
    if modifiers.control {
        tokens.push("ctrl");
    }
    if modifiers.alt {
        tokens.push(if mac { "option" } else { "alt" });
    }
    if modifiers.platform {
        tokens.push(if mac { "command" } else { "super" });
    }
    tokens
}

impl ShortcutInput {
    pub fn new(binding_id: &str, cx: &mut Context<Self>) -> Self {
        let store = SettingsStore::global(cx);
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&event_bus(cx), |this, _, event: &AppEvent, cx| {
            if event.name == "handy-keys-event" {
                if let Some(key_event) = event.parse::<FrontendKeyEvent>() {
                    this.on_handy_keys_event(key_event, cx);
                }
            }
        })
        .detach();
        cx.on_release(|this, cx| {
            if this.recording && !this.committing {
                this.stop_backend_recording(cx);
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
            capture: WindowCapture::default(),
            focus: cx.focus_handle(),
            chip: Bounds::default(),
        }
    }

    fn app() -> AppHandle {
        AppHandle::global().clone()
    }

    fn start(&mut self, disabled: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording || disabled {
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

        if uses_handy_keys(cx) {
            let id = self.binding_id.clone();
            match crate::shortcut::handy_keys::start_handy_keys_recording(Self::app(), id) {
                Ok(()) => {}
                Err(error) => {
                    if error.contains("secure-input-active") {
                        toast::show(
                            toast::ToastKind::Error,
                            t("secureInput.recorderBlocked"),
                            None,
                            None,
                            cx,
                        );
                    } else {
                        toast::error(
                            t_with("settings.general.shortcut.errors.set", &[("error", &error)]),
                            cx,
                        );
                    }
                    return;
                }
            }
        } else {
            if let Err(e) = crate::shortcut::suspend_all_bindings(Self::app()) {
                log::error!("Failed to suspend bindings: {e}");
            }
            self.capture = WindowCapture::default();
            self.focus.focus(window);
        }
        self.recording = true;
        self.current.clear();
        self.keyed.clear();
        self.modifier_only.clear();
        cx.notify();
    }

    fn stop_backend_recording(&self, cx: &App) {
        if uses_handy_keys(cx) {
            if let Err(e) = crate::shortcut::handy_keys::stop_handy_keys_recording(Self::app()) {
                log::error!("{e}");
            }
        } else if let Err(e) = crate::shortcut::resume_all_bindings(Self::app()) {
            log::error!("{e}");
        }
    }

    fn reset_state(&mut self, cx: &mut Context<Self>) {
        self.recording = false;
        self.current.clear();
        self.keyed.clear();
        self.modifier_only.clear();
        self.original.clear();
        self.capture = WindowCapture::default();
        cx.notify();
    }

    /// Saves `keys`, restoring the previous binding if the backend refuses.
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
        self.stop_backend_recording(cx);
        self.reset_state(cx);
    }

    /// A plain click outside: stop and keep the old binding.
    fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.recording || self.committing {
            return;
        }
        self.committing = true;
        self.stop_backend_recording(cx);
        if uses_handy_keys(cx) && !self.original.is_empty() {
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
        if !self.recording || self.committing || !uses_handy_keys(cx) {
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
                // The native listener omits primary button releases once all
                // modifiers are up.
                self.commit(self.keyed.clone(), cx);
            } else if self.keyed.is_empty() && !self.modifier_only.is_empty() {
                self.commit(self.modifier_only.clone(), cx);
            }
        }
    }

    fn sorted_capture(&self) -> String {
        let mut keys = self.capture.captured.clone();
        keys.sort_by_key(|k| !MODIFIERS.contains(&k.as_str()));
        keys.join("+")
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if !self.recording || uses_handy_keys(cx) || event.is_held {
            return;
        }
        cx.stop_propagation();
        let key = key_token(&event.keystroke.key);
        self.capture.pressed.insert(key.clone());
        if !self.capture.captured.contains(&key) {
            self.capture.captured.push(key);
        }
        if self.capture.mouse.is_none() {
            self.current = self.capture.captured.join("+");
        }
        cx.notify();
    }

    fn on_key_up(&mut self, event: &KeyUpEvent, cx: &mut Context<Self>) {
        if !self.recording || uses_handy_keys(cx) {
            return;
        }
        cx.stop_propagation();
        self.capture
            .pressed
            .remove(&key_token(&event.keystroke.key));
        self.maybe_finish_keys(cx);
    }

    fn on_modifiers_changed(&mut self, event: &ModifiersChangedEvent, cx: &mut Context<Self>) {
        if !self.recording || uses_handy_keys(cx) {
            return;
        }
        let before: BTreeSet<&str> = modifier_tokens(&self.capture.modifiers)
            .into_iter()
            .collect();
        let after: BTreeSet<&str> = modifier_tokens(&event.modifiers).into_iter().collect();
        self.capture.modifiers = event.modifiers;
        for added in after.difference(&before) {
            self.capture.pressed.insert(added.to_string());
            if !self.capture.captured.iter().any(|k| k == added) {
                self.capture.captured.push(added.to_string());
            }
        }
        for removed in before.difference(&after) {
            self.capture.pressed.remove(*removed);
        }
        if self.capture.mouse.is_none() {
            self.current = self.capture.captured.join("+");
        }
        self.maybe_finish_keys(cx);
        cx.notify();
    }

    fn maybe_finish_keys(&mut self, cx: &mut Context<Self>) {
        if self.capture.pressed.is_empty()
            && !self.capture.captured.is_empty()
            && self.capture.mouse.is_none()
        {
            let keys = self.sorted_capture();
            self.commit(keys, cx);
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        if !self.recording {
            return;
        }
        let shortcut = mouse_shortcut(event.button, &event.modifiers);
        let keyed_has_mouse = self.keyed.split('+').any(|k| k.starts_with("mouse"));
        if self.committing || shortcut.is_some() || keyed_has_mouse {
            if !uses_handy_keys(cx) && self.capture.mouse.is_none() {
                if let Some(shortcut) = shortcut {
                    self.current = shortcut.clone();
                    self.capture.mouse = Some((event.button, shortcut));
                    cx.notify();
                }
            }
            return;
        }
        if event.button == MouseButton::Left && !self.chip.contains(&event.position) {
            self.cancel(cx);
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if !self.recording || uses_handy_keys(cx) {
            return;
        }
        if let Some((button, shortcut)) = self.capture.mouse.clone() {
            if button == event.button {
                self.commit(shortcut, cx);
            }
        }
    }

    fn render_row(&self, disabled: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = palette(cx);
        let store = self.store.read(cx);
        // Settings load before the first frame, so React's loading state
        // never shows here; an empty binding list still can.
        let none = |description: SharedString| {
            SettingContainer::new(t("settings.general.shortcut.title"))
                .description(description)
                .child(
                    div()
                        .sm_text()
                        .text_color(p.mid_gray)
                        .child(t("settings.general.shortcut.none")),
                )
                .into_any_element()
        };
        if store.settings.bindings.is_empty() {
            return none(t("settings.general.shortcut.description"));
        }
        let Some(binding) = store.settings.bindings.get(&self.binding_id).cloned() else {
            return SettingContainer::new(t("settings.general.shortcut.title"))
                .description(t("settings.general.shortcut.notFound"))
                .child(
                    div()
                        .sm_text()
                        .text_color(p.mid_gray)
                        .child(t("settings.general.shortcut.none")),
                )
                .into_any_element();
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
            .px(rems(0.625))
            .py_1()
            .sm_text()
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .rounded_lg()
            .bg(p.background)
            .border_1();
        let chip = if self.recording {
            let text: SharedString = if self.current.is_empty() {
                t("settings.general.shortcut.pressKeysOrMouse")
            } else {
                format_key_combination(&self.current).into()
            };
            chip.border_color(p.accent)
                .shadow(vec![gpui::BoxShadow {
                    color: p.accent.a(0.15),
                    offset: gpui::point(gpui::px(0.), gpui::px(0.)),
                    blur_radius: gpui::px(0.),
                    spread_radius: gpui::px(2.),
                }])
                .track_focus(&self.focus)
                .capture_key_down(cx.listener(|this, e, _, cx| this.on_key_down(e, cx)))
                .on_key_up(cx.listener(|this, e, _, cx| this.on_key_up(e, cx)))
                .on_modifiers_changed(
                    cx.listener(|this, e, _, cx| this.on_modifiers_changed(e, cx)),
                )
                .child(text)
        } else {
            chip.border_color(p.mid_gray.a(0.25))
                .shadow(vec![shadow(0., 1., 1., 0., 0.03)])
                .hover(|s| s.border_color(p.mid_gray.a(0.5)))
                .cursor_pointer()
                .keyboard_focus(cx)
                .on_click(cx.listener(move |this, _, window, cx| this.start(disabled, window, cx)))
                .child(format_key_combination(&binding.current_binding))
        };

        let entity = cx.entity().downgrade();
        let paint_entity = entity.clone();
        let recording = self.recording;
        // While recording, every click in the window goes through the
        // recorder: mouse shortcuts are captured, a plain left click cancels.
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
                let up = paint_entity.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        let _ = up.update(cx, |this, cx| this.on_mouse_up(event, cx));
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
            .disabled(disabled)
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
        self.render_row(false, cx)
    }
}
