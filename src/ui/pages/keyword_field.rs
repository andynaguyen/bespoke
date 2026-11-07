use gpui::{Context, Entity, Subscription, Window, prelude::*, rems};
use gpui_component::input::{InputEvent, InputState};

use crate::app_handle::AppHandle;
use crate::i18n::t;
use crate::settings::AppSettings;
use crate::ui::components::{SettingContainer, TextField};
use crate::ui::stores::SettingsStore;

pub struct KeywordSetting {
    pub key: &'static str,
    pub i18n_prefix: &'static str,
    pub read: fn(&AppSettings) -> &str,
    pub apply: fn(&mut AppSettings, String),
    pub command: fn(AppHandle, String) -> Result<(), String>,
}

pub struct KeywordField {
    setting: KeywordSetting,
    input: Entity<InputState>,
    invalid: bool,
    saved: String,
    _subscriptions: Vec<Subscription>,
}

impl KeywordField {
    pub fn new(setting: KeywordSetting, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = SettingsStore::global(cx);
        let saved = (setting.read)(&store.read(cx).settings).to_string();
        let initial = saved.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(initial));
        let subscriptions =
            vec![
                cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::Change => {
                            if this.invalid {
                                this.invalid = false;
                                cx.notify();
                            }
                        }
                        InputEvent::Blur => this.commit(window, cx),
                        InputEvent::PressEnter { .. } => window.blur(),
                        _ => {}
                    }
                }),
                cx.observe_in(&store, window, |this, store, window, cx| {
                    let saved = (this.setting.read)(&store.read(cx).settings).to_string();
                    if saved != this.saved {
                        this.saved = saved.clone();
                        this.input
                            .update(cx, |input, cx| input.set_value(saved, window, cx));
                    }
                    cx.notify();
                }),
            ];
        Self {
            setting,
            input,
            invalid: false,
            saved,
            _subscriptions: subscriptions,
        }
    }

    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.input.read(cx).value().to_string();
        let Some(normalized) = crate::shortcut::normalize_keyword(&raw) else {
            self.invalid = true;
            cx.notify();
            return;
        };
        self.invalid = false;
        if normalized != raw {
            let value = normalized.clone();
            self.input
                .update(cx, |input, cx| input.set_value(value, window, cx));
        }
        if normalized != self.saved {
            let apply = self.setting.apply;
            let command = self.setting.command;
            let value = normalized.clone();
            SettingsStore::global(cx).update(cx, |store, cx| {
                store.update(
                    self.setting.key,
                    move |s| apply(s, normalized),
                    move |app| command(app, value),
                    cx,
                )
            });
        }
        cx.notify();
    }
}

impl Render for KeywordField {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prefix = self.setting.i18n_prefix;
        let updating = SettingsStore::global(cx)
            .read(cx)
            .is_updating(self.setting.key);
        SettingContainer::new(t(&format!("{prefix}.title")))
            .description(if self.invalid {
                t(&format!("{prefix}.invalid"))
            } else {
                t(&format!("{prefix}.description"))
            })
            .child(
                TextField::new(&self.input)
                    .compact()
                    .invalid(self.invalid)
                    .disabled(updating)
                    .w(rems(12.)),
            )
    }
}
