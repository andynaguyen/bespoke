//! The Dictate page's Settings tab: how a dictation starts, records,
//! cleans up and pastes, in that order, with feedback sounds last.

use gpui::{div, prelude::*, rems, AnyElement, App, Context, Entity, SharedString, Window};
use gpui_component::input::{InputEvent, InputState};

use crate::i18n::{t, t_with};
use crate::settings::{AppSettings, PasteMethod, TypingTool};
use crate::shortcut;
use crate::ui::components::{
    reset_button, Dropdown, DropdownOption, RowDirection, SettingContainer, SettingsGroup, Spinner,
    TextField,
};
use crate::ui::languages::{
    language_label, recognition_language, selectable_languages, supports_language_code,
    unique_capability_languages, CHINESE_LANGUAGE_CODE,
};
use crate::ui::stores::{ModelStore, SettingsStore};
use crate::ui::theme::{palette, Alpha};

use super::keyword_field::{KeywordField, KeywordSetting};
use super::language_picker::{LanguagePicker, PickerStyle};
use super::rows::{self, set};
use super::shortcut_input::ShortcutInput;

pub struct DictationSettings {
    store: Entity<SettingsStore>,
    models: Entity<ModelStore>,
    transcribe_shortcut: Entity<ShortcutInput>,
    cancel_shortcut: Entity<ShortcutInput>,
    language: Entity<LanguagePicker>,
    scrub_keyword: Entity<KeywordField>,
    submit_keyword: Entity<KeywordField>,
    external_script: Entity<InputState>,
    /// Channel count of the selected microphone, and which device it is for.
    channels: (String, u16),
    typing_tools: Vec<String>,
}

/// The canonical picker entry for a language code (`pickerLanguage`).
fn picker_language(code: &str) -> String {
    if selectable_languages().any(|(c, _)| *c == code) {
        code.to_string()
    } else {
        recognition_language(code).to_string()
    }
}

/// The language actually used: the stored intent resolved against what the
/// model supports (`effectiveLanguage` in LanguageSelector.tsx).
fn effective_language(intent: &str, supported: &[String], detection: bool) -> String {
    if supported.is_empty() {
        return picker_language(intent);
    }
    if intent != "auto" && supports_language_code(supported, intent) {
        return picker_language(intent);
    }
    if detection {
        return "auto".into();
    }
    if supports_language_code(supported, "en") {
        return "en".into();
    }
    picker_language(&supported[0])
}

impl DictationSettings {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = SettingsStore::global(cx);
        let models = ModelStore::global(cx);
        cx.observe(&store, |this, _, cx| {
            this.refresh_channels(cx);
            this.sync_language(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&models, |this, _, cx| {
            this.sync_language(cx);
            cx.notify();
        })
        .detach();

        let external = store
            .read(cx)
            .settings
            .external_script_path
            .clone()
            .unwrap_or_default();
        let external_script = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("settings.advanced.pasteMethod.externalScriptPlaceholder"))
                .default_value(external)
        });
        cx.subscribe(&external_script, |_, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let path = input.read(cx).value().to_string();
                let stored = (!path.is_empty()).then(|| path.clone());
                let command = stored.clone();
                set(
                    cx,
                    "external_script_path",
                    move |s| s.external_script_path = stored,
                    move |app| shortcut::change_external_script_path_setting(app, command),
                );
            }
        })
        .detach();

        let mut this = Self {
            transcribe_shortcut: cx.new(|cx| ShortcutInput::new("transcribe", cx)),
            cancel_shortcut: cx.new(|cx| ShortcutInput::new("cancel", cx)),
            language: cx.new(|cx| {
                LanguagePicker::new(
                    PickerStyle::Setting,
                    |code, _, cx| {
                        let code = code.to_string();
                        let stored = code.clone();
                        set(
                            cx,
                            "selected_language",
                            move |s| s.selected_language = stored,
                            move |app| shortcut::change_selected_language_setting(app, code),
                        );
                    },
                    window,
                    cx,
                )
            }),
            scrub_keyword: cx.new(|cx| {
                KeywordField::new(
                    KeywordSetting {
                        key: "scrub_keyword",
                        i18n_prefix: "settings.advanced.voiceScrub.keyword",
                        read: |s| &s.scrub_keyword,
                        apply: |s, v| s.scrub_keyword = v,
                        command: shortcut::change_scrub_keyword_setting,
                    },
                    window,
                    cx,
                )
            }),
            submit_keyword: cx.new(|cx| {
                KeywordField::new(
                    KeywordSetting {
                        key: "voice_submit_keyword",
                        i18n_prefix: "settings.advanced.voiceSubmit.keyword",
                        read: |s| &s.voice_submit_keyword,
                        apply: |s, v| s.voice_submit_keyword = v,
                        command: shortcut::change_voice_submit_keyword_setting,
                    },
                    window,
                    cx,
                )
            }),
            external_script,
            channels: (String::new(), 1),
            // `TypingTool.tsx` starts from "auto" while the probe runs.
            typing_tools: if cfg!(target_os = "linux") {
                vec!["auto".into()]
            } else {
                Vec::new()
            },
            store,
            models,
        };
        this.refresh_channels(cx);
        this.sync_language(cx);
        if cfg!(target_os = "linux") {
            // The probe runs the typing tools' binaries; keep it off the UI
            // thread.
            cx.spawn(async move |this, cx| {
                let Ok(tools) =
                    crate::runtime::spawn_blocking(shortcut::get_available_typing_tools).await
                else {
                    return;
                };
                let _ = this.update(cx, |this, cx| {
                    this.typing_tools = tools;
                    cx.notify();
                });
            })
            .detach();
        }
        this
    }

    /// Reloads the channel count whenever the selected microphone changes.
    fn refresh_channels(&mut self, cx: &mut Context<Self>) {
        let device = self
            .store
            .read(cx)
            .settings
            .selected_microphone
            .clone()
            .unwrap_or_else(|| "default".into());
        if device == self.channels.0 {
            return;
        }
        // The count resets to 1 on every device change, like the React row.
        self.channels = (device.clone(), 1);
        cx.spawn(async move |this, cx| {
            let lookup = device.clone();
            let count =
                crate::runtime::run(crate::commands::audio::get_microphone_channels(lookup))
                    .await
                    .unwrap_or(1);
            let _ = this.update(cx, |this, cx| {
                if this.channels.0 == device {
                    this.channels.1 = count;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn sync_language(&mut self, cx: &mut Context<Self>) {
        let Some(model) = self.models.read(cx).current_model_info().cloned() else {
            return;
        };
        let settings = &self.store.read(cx).settings;
        let selected = effective_language(
            &settings.selected_language,
            &model.supported_languages,
            model.supports_language_detection,
        );
        let options: Vec<(SharedString, SharedString)> = selectable_languages()
            .filter(|(code, _)| {
                model.supported_languages.is_empty()
                    || if *code == "auto" {
                        model.supports_language_detection
                    } else {
                        supports_language_code(&model.supported_languages, code)
                    }
            })
            .map(|(code, label)| {
                (
                    SharedString::new_static(code),
                    SharedString::new_static(label),
                )
            })
            .collect();
        let label_selected = language_label(&selected).map(|_| selected.clone());
        let updating = self.store.read(cx).is_updating("selected_language");
        self.language.update(cx, |picker, _| {
            picker.set_options(options, None, label_selected.map(Into::into), updating);
        });
    }

    fn language_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(model) = self.models.read(cx).current_model_info().cloned() else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        let capability = unique_capability_languages(&model.supported_languages);
        let chinese_only = capability.len() == 1 && capability[0] == CHINESE_LANGUAGE_CODE;
        if model.supports_language_selection || chinese_only {
            let updating = self.store.read(cx).is_updating("selected_language");
            rows.push(
                SettingContainer::new(t("settings.general.language.title"))
                    .description(t("settings.general.language.description"))
                    .child(
                        div()
                            .flex()
                            .row_dir()
                            .items_center()
                            .gap_1()
                            .child(self.language.clone())
                            .child(reset_button(
                                "reset-language",
                                updating,
                                |_, _, cx| {
                                    let default = SettingsStore::global(cx)
                                        .read(cx)
                                        .defaults
                                        .selected_language
                                        .clone();
                                    let stored = default.clone();
                                    set(
                                        cx,
                                        "selected_language",
                                        move |s| s.selected_language = stored,
                                        move |app| {
                                            shortcut::change_selected_language_setting(app, default)
                                        },
                                    );
                                },
                                cx,
                            )),
                    )
                    // The saving overlay over the control column.
                    .when(updating, |el| {
                        el.child(
                            div()
                                .absolute()
                                .inset_0()
                                .rounded_sm()
                                .bg(palette(cx).mid_gray.a(0.1))
                                .flex()
                                .row_dir()
                                .items_center()
                                .justify_center()
                                .child(Spinner::ring(rems(1.), palette(cx).accent)),
                        )
                    })
                    .into_any_element(),
            );
        }
        if model.supports_translation {
            rows.push(rows::translate_to_english(
                &self.store.read(cx).settings,
                cx,
            ));
        }
        rows
    }

    fn channel_row(&self, s: &AppSettings, cx: &App) -> Option<AnyElement> {
        let count = self.channels.1;
        if count <= 1 {
            return None;
        }
        let mut options = vec![DropdownOption::new(
            "average",
            t("settings.sound.channel.average"),
        )];
        for i in 0..count {
            options.push(DropdownOption::new(
                i.to_string(),
                t_with("settings.sound.channel.channel", &[("n", &(i + 1))]),
            ));
        }
        let selected = match s.selected_channel {
            Some(c) if c < count => c.to_string(),
            _ => "average".into(),
        };
        let store = self.store.read(cx);
        Some(
            SettingContainer::new(t("settings.sound.channel.title"))
                .description(t("settings.sound.channel.description"))
                .child(
                    Dropdown::new("selected_channel", options)
                        .selected(Some(selected))
                        .disabled(store.is_updating("selected_channel"))
                        .on_select(|value, _, cx| {
                            let channel = value.parse::<u16>().ok();
                            SettingsStore::global(cx).update(cx, |store, cx| {
                                let app = store.app().clone();
                                store.update_async(
                                    "selected_channel",
                                    move |s| s.selected_channel = channel,
                                    async move {
                                        crate::commands::audio::set_selected_channel(app, channel)
                                            .await
                                    },
                                    cx,
                                )
                            });
                        }),
                )
                .into_any_element(),
        )
    }

    fn paste_method_row(&self, s: &AppSettings, cx: &App) -> AnyElement {
        let store = self.store.read(cx);
        let method = s.paste_method;
        SettingContainer::new(t("settings.advanced.pasteMethod.title"))
            .description(t("settings.advanced.pasteMethod.description"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        Dropdown::new("paste_method", rows::paste_method_options(method))
                            .selected(Some(rows::paste_method_value(method)))
                            .disabled(store.is_updating("paste_method"))
                            .on_select(|value, _, cx| {
                                let method = match value.as_ref() {
                                    "direct" => PasteMethod::Direct,
                                    "none" => PasteMethod::None,
                                    "shift_insert" => PasteMethod::ShiftInsert,
                                    "ctrl_shift_v" => PasteMethod::CtrlShiftV,
                                    "external_script" => PasteMethod::ExternalScript,
                                    _ => PasteMethod::CtrlV,
                                };
                                let value = value.to_string();
                                set(
                                    cx,
                                    "paste_method",
                                    move |s| s.paste_method = method,
                                    move |app| shortcut::change_paste_method_setting(app, value),
                                );
                            }),
                    )
                    .when(method == PasteMethod::ExternalScript, |el| {
                        el.child(
                            TextField::new(&self.external_script)
                                .disabled(store.is_updating("external_script_path")),
                        )
                    }),
            )
            .into_any_element()
    }

    fn typing_tool_row(&self, s: &AppSettings, cx: &App) -> Option<AnyElement> {
        if !cfg!(target_os = "linux") || s.paste_method != PasteMethod::Direct {
            return None;
        }
        let tools = if self.typing_tools.is_empty() {
            vec!["auto".to_string()]
        } else {
            self.typing_tools.clone()
        };
        let options = tools
            .iter()
            .map(|tool| {
                let label: SharedString = if tool == "auto" {
                    t("settings.advanced.typingTool.options.auto")
                } else {
                    tool.clone().into()
                };
                DropdownOption::new(tool.clone(), label)
            })
            .collect();
        let selected = match s.typing_tool {
            TypingTool::Auto => "auto",
            TypingTool::Wtype => "wtype",
            TypingTool::Kwtype => "kwtype",
            TypingTool::Dotool => "dotool",
            TypingTool::Ydotool => "ydotool",
            TypingTool::Xdotool => "xdotool",
        };
        Some(rows::dropdown(
            "typing_tool",
            "settings.advanced.typingTool.title",
            "settings.advanced.typingTool.description",
            options,
            selected,
            cx,
            |value, cx| {
                let tool = match value.as_ref() {
                    "wtype" => TypingTool::Wtype,
                    "kwtype" => TypingTool::Kwtype,
                    "dotool" => TypingTool::Dotool,
                    "ydotool" => TypingTool::Ydotool,
                    "xdotool" => TypingTool::Xdotool,
                    _ => TypingTool::Auto,
                };
                let value = value.to_string();
                set(
                    cx,
                    "typing_tool",
                    move |s| s.typing_tool = tool,
                    move |app| shortcut::change_typing_tool_setting(app, value),
                )
            },
        ))
    }
}

impl Render for DictationSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.store.read(cx).settings.clone();
        let mut shortcut_group = SettingsGroup::new()
            .title(t("settings.advanced.groups.shortcut"))
            .child(self.transcribe_shortcut.clone())
            .child(rows::shortcut_activation(&s, cx));
        if !cfg!(target_os = "linux") {
            // Dynamic cancel shortcut registration is unstable on Linux.
            shortcut_group = shortcut_group.child(self.cancel_shortcut.clone());
        }
        let microphone = SettingsGroup::new()
            .title(t("settings.sound.microphone.title"))
            .child(rows::microphone(&s, cx))
            .children(self.channel_row(&s, cx))
            .child(rows::mute_while_recording(&s, cx));
        let language = SettingsGroup::new()
            .title(t("settings.general.language.title"))
            .children(self.language_rows(cx))
            .child(rows::vad_enabled(&s, cx));
        let mut cleanup = SettingsGroup::new()
            .title(t("settings.advanced.groups.cleanup"))
            .child(rows::filler_word_removal(&s, cx))
            .child(rows::append_trailing_space(&s, cx))
            .child(rows::toggle(
                "scrub_enabled",
                "settings.advanced.voiceScrub.title",
                "settings.advanced.voiceScrub.description",
                s.scrub_enabled,
                cx,
                |v, cx| {
                    set(
                        cx,
                        "scrub_enabled",
                        move |s| s.scrub_enabled = v,
                        move |app| shortcut::change_scrub_enabled_setting(app, v),
                    )
                },
            ));
        if s.scrub_enabled {
            cleanup = cleanup.child(self.scrub_keyword.clone());
        }
        let pasting = SettingsGroup::new()
            .title(t("settings.advanced.groups.pasting"))
            .child(self.paste_method_row(&s, cx))
            .children(self.typing_tool_row(&s, cx))
            .child(rows::clipboard_handling(&s, cx));
        let mut submitting = SettingsGroup::new()
            .title(t("settings.advanced.groups.submitting"))
            .child(rows::auto_submit(&s, cx))
            .child(rows::toggle(
                "voice_submit_enabled",
                "settings.advanced.voiceSubmit.title",
                "settings.advanced.voiceSubmit.description",
                s.voice_submit_enabled,
                cx,
                |v, cx| {
                    set(
                        cx,
                        "voice_submit_enabled",
                        move |s| s.voice_submit_enabled = v,
                        move |app| shortcut::change_voice_submit_enabled_setting(app, v),
                    )
                },
            ));
        if s.voice_submit_enabled {
            submitting = submitting
                .child(self.submit_keyword.clone())
                .child(rows::submit_key(&s, cx));
        }
        let sound = SettingsGroup::new()
            .title(t("settings.sound.title"))
            .child(rows::audio_feedback(&s, cx))
            .child(rows::output_device(&s, !s.audio_feedback, cx))
            .child(rows::volume(&s, !s.audio_feedback));
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_5()
            .child(shortcut_group)
            .child(microphone)
            .child(language)
            .child(cleanup)
            .child(pasting)
            .child(submitting)
            .child(sound)
    }
}
