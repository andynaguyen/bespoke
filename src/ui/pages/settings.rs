//! The Settings page (`settings-page/SettingsPage.tsx`): five tabs of
//! setting groups. Experimental appears only when enabled.

use gpui::{div, prelude::*, rems, AnyElement, App, Context, Entity, SharedString, Window};
use gpui_component::input::{InputEvent, InputState};

use crate::app_handle::AppHandle;
use crate::i18n::{t, t_with};
use crate::managers::transcription::AvailableAccelerators;
use crate::settings::{AppSettings, PasteMethod, TranscribeAcceleratorSetting, TypingTool};
use crate::shortcut;
use crate::ui::components::{
    reset_button, Dropdown, DropdownOption, RowDirection, SegmentedControl, SettingContainer,
    SettingsGroup, Spinner, TextField,
};
use crate::ui::languages::{
    language_label, recognition_language, selectable_languages, supports_language_code,
    unique_capability_languages, CHINESE_LANGUAGE_CODE,
};
use crate::ui::shell::Toolbar;
use crate::ui::stores::{ModelStore, SettingsStore};
use crate::ui::theme::{palette, Alpha};

use super::keyword_field::{KeywordField, KeywordSetting};
use super::language_picker::{LanguagePicker, PickerStyle};
use super::post_processing::{PostProcessingApi, PostProcessingPrompts};
use super::rows::{self, set};
use super::shortcut_input::ShortcutInput;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Dictation,
    Transcription,
    Appearance,
    System,
    Experimental,
}

impl Tab {
    fn id(self) -> &'static str {
        match self {
            Tab::Dictation => "dictation",
            Tab::Transcription => "transcription",
            Tab::Appearance => "appearance",
            Tab::System => "system",
            Tab::Experimental => "experimental",
        }
    }
}

pub struct SettingsPage {
    store: Entity<SettingsStore>,
    models: Entity<ModelStore>,
    tab: Tab,
    transcribe_shortcut: Entity<ShortcutInput>,
    cancel_shortcut: Entity<ShortcutInput>,
    post_process_shortcut: Entity<ShortcutInput>,
    language: Entity<LanguagePicker>,
    scrub_keyword: Entity<KeywordField>,
    submit_keyword: Entity<KeywordField>,
    external_script: Entity<InputState>,
    post_processing_api: Entity<PostProcessingApi>,
    post_processing_prompts: Entity<PostProcessingPrompts>,
    /// Channel count of the selected microphone, and which device it is for.
    channels: (String, u16),
    typing_tools: Vec<String>,
    accelerators: Option<AvailableAccelerators>,
    app_dir: Result<String, String>,
    log_dir: Result<String, String>,
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

impl SettingsPage {
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

        let app = AppHandle::global().clone();
        let mut this = Self {
            transcribe_shortcut: cx.new(|cx| ShortcutInput::new("transcribe", cx)),
            cancel_shortcut: cx.new(|cx| ShortcutInput::new("cancel", cx)),
            post_process_shortcut: cx
                .new(|cx| ShortcutInput::new("transcribe_with_post_process", cx)),
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
            post_processing_api: cx.new(|cx| PostProcessingApi::new(window, cx)),
            post_processing_prompts: cx.new(|cx| PostProcessingPrompts::new(window, cx)),
            channels: (String::new(), 1),
            // `TypingTool.tsx` starts from "auto" while the probe runs.
            typing_tools: if cfg!(target_os = "linux") {
                vec!["auto".into()]
            } else {
                Vec::new()
            },
            accelerators: None,
            app_dir: crate::commands::get_app_dir_path(app.clone()),
            log_dir: crate::commands::get_log_dir_path(app),
            store,
            models,
            tab: Tab::Dictation,
        };
        this.refresh_channels(cx);
        this.sync_language(cx);

        cx.spawn(async move |this, cx| {
            let available = crate::runtime::run(shortcut::get_available_accelerators()).await;
            let _ = this.update(cx, |this, cx| {
                this.accelerators = Some(available);
                cx.notify();
            });
        })
        .detach();
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

    pub fn select_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        self.tab = match id {
            "transcription" => Tab::Transcription,
            "appearance" => Tab::Appearance,
            "system" => Tab::System,
            "experimental" => Tab::Experimental,
            _ => Tab::Dictation,
        };
        cx.notify();
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

    fn acceleration_rows(&self, s: &AppSettings, cx: &App) -> Vec<AnyElement> {
        let mut transcribe = Vec::new();
        let mut ort: Vec<String> = Vec::new();
        if let Some(available) = &self.accelerators {
            if available.transcribe.iter().any(|a| a == "auto") {
                transcribe.push(DropdownOption::new(
                    "auto",
                    t("settings.advanced.acceleration.gpuDevice.auto"),
                ));
            }
            if available.transcribe.iter().any(|a| a == "gpu") {
                for device in &available.gpu_devices {
                    let vram = if device.total_vram_mb >= 1024 {
                        format!("{:.1} GB", device.total_vram_mb as f64 / 1024.)
                    } else {
                        format!("{} MB", device.total_vram_mb)
                    };
                    transcribe.push(DropdownOption::new(
                        format!("gpu:{}", device.id),
                        format!("{} ({vram})", device.name),
                    ));
                }
            }
            if available.transcribe.iter().any(|a| a == "cpu") {
                transcribe.push(DropdownOption::new("cpu", "CPU"));
            }
            ort = available.ort.clone();
            if !ort.iter().any(|o| o == "auto") {
                ort.insert(0, "auto".into());
            }
        }

        let current = match (s.transcribe_accelerator, &s.transcribe_gpu_device) {
            (TranscribeAcceleratorSetting::Cpu, _) => "cpu".to_string(),
            (TranscribeAcceleratorSetting::Gpu, Some(device)) => format!("gpu:{device}"),
            _ => "auto".to_string(),
        };
        let displayed = if transcribe.iter().any(|o| o.value.as_ref() == current) {
            Some(current)
        } else {
            transcribe.first().map(|o| o.value.to_string())
        };
        let store = self.store.read(cx);
        let mut rows =
            vec![
                SettingContainer::new(t("settings.advanced.acceleration.transcribe.title"))
                    .description(t("settings.advanced.acceleration.transcribe.description"))
                    .child(
                        Dropdown::new("transcribe_accelerator", transcribe)
                            .selected(displayed)
                            .disabled(
                                store.is_updating("transcribe_accelerator")
                                    || store.is_updating("transcribe_gpu_device"),
                            )
                            .on_select(|value, _, cx| {
                                let (accelerator, device) = if value.as_ref() == "cpu" {
                                    (TranscribeAcceleratorSetting::Cpu, None)
                                } else if let Some(device) = value.strip_prefix("gpu:") {
                                    (TranscribeAcceleratorSetting::Gpu, Some(device.to_string()))
                                } else {
                                    (TranscribeAcceleratorSetting::Auto, None)
                                };
                                // Save the device first so `gpu + null` is never
                                // normalized to Auto.
                                let stored = device.clone();
                                set(
                                    cx,
                                    "transcribe_accelerator",
                                    move |s| {
                                        s.transcribe_gpu_device = stored;
                                        s.transcribe_accelerator = accelerator;
                                    },
                                    move |app| {
                                        shortcut::change_transcribe_gpu_device(
                                            app.clone(),
                                            device,
                                        )?;
                                        shortcut::change_transcribe_accelerator_setting(
                                            app,
                                            accelerator,
                                        )
                                    },
                                );
                            }),
                    )
                    .into_any_element(),
            ];

        if ort.len() > 2 {
            use crate::settings::OrtAcceleratorSetting;
            let label = |v: &str| -> SharedString {
                match v {
                    "auto" => "Auto".into(),
                    "cpu" => "CPU".into(),
                    "cuda" => "CUDA".into(),
                    "directml" => "DirectML".into(),
                    "rocm" => "ROCm".into(),
                    other => other.to_string().into(),
                }
            };
            let selected = match s.ort_accelerator {
                OrtAcceleratorSetting::Auto => "auto",
                OrtAcceleratorSetting::Cpu => "cpu",
                OrtAcceleratorSetting::Cuda => "cuda",
                OrtAcceleratorSetting::DirectMl => "directml",
                OrtAcceleratorSetting::Rocm => "rocm",
            };
            rows.push(rows::dropdown(
                "ort_accelerator",
                "settings.advanced.acceleration.ort.title",
                "settings.advanced.acceleration.ort.description",
                ort.iter()
                    .map(|v| DropdownOption::new(v.clone(), label(v)))
                    .collect(),
                selected,
                cx,
                |value, cx| {
                    let accelerator = match value.as_ref() {
                        "cpu" => OrtAcceleratorSetting::Cpu,
                        "cuda" => OrtAcceleratorSetting::Cuda,
                        "directml" => OrtAcceleratorSetting::DirectMl,
                        "rocm" => OrtAcceleratorSetting::Rocm,
                        _ => OrtAcceleratorSetting::Auto,
                    };
                    set(
                        cx,
                        "ort_accelerator",
                        move |s| s.ort_accelerator = accelerator,
                        move |app| shortcut::change_ort_accelerator_setting(app, accelerator),
                    )
                },
            ));
        }
        rows
    }

    fn render_tab(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let s = self.store.read(cx).settings.clone();
        match self.tab {
            Tab::Dictation => {
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
                let sound = SettingsGroup::new()
                    .title(t("settings.sound.title"))
                    .child(rows::audio_feedback(&s, cx))
                    .child(rows::output_device(&s, !s.audio_feedback, cx))
                    .child(rows::volume(&s, !s.audio_feedback));
                vec![
                    shortcut_group.into_any_element(),
                    microphone.into_any_element(),
                    language.into_any_element(),
                    sound.into_any_element(),
                ]
            }
            Tab::Transcription => {
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
                vec![
                    cleanup.into_any_element(),
                    pasting.into_any_element(),
                    submitting.into_any_element(),
                ]
            }
            Tab::Appearance => vec![SettingsGroup::new()
                .child(rows::app_language(&s, cx))
                .child(rows::theme(&s, cx))
                .children(rows::overlay(&s, cx))
                .into_any_element()],
            Tab::System => {
                let about = SettingsGroup::new()
                    .title(t("settings.about.title"))
                    .child(rows::directory_row(
                        "settings.about.appDataDirectory.title",
                        "settings.about.appDataDirectory.description",
                        self.app_dir.clone(),
                        crate::commands::open_app_data_dir,
                        true,
                        cx,
                    ))
                    .child(rows::directory_row(
                        "settings.debug.logDirectory.title",
                        "settings.debug.logDirectory.description",
                        self.log_dir.clone(),
                        crate::commands::open_log_dir,
                        false,
                        cx,
                    ));
                vec![
                    SettingsGroup::new()
                        .title(t("settings.advanced.groups.startup"))
                        .child(rows::start_hidden(&s, cx))
                        .child(rows::autostart(&s, cx))
                        .child(rows::show_tray_icon(&s, cx))
                        .into_any_element(),
                    SettingsGroup::new()
                        .title(t("settings.advanced.groups.memory"))
                        .child(rows::model_unload_timeout(&s, cx))
                        .into_any_element(),
                    SettingsGroup::new()
                        .title(t("settings.advanced.groups.updates"))
                        .child(rows::show_whats_new(&s, cx))
                        .into_any_element(),
                    about.into_any_element(),
                    SettingsGroup::new()
                        .title(t("settings.advanced.groups.experimental"))
                        .child(rows::experimental(&s, cx))
                        .into_any_element(),
                ]
            }
            Tab::Experimental => {
                let mut post = SettingsGroup::new()
                    .title(t("settings.debug.postProcessingToggle.label"))
                    .child(rows::post_processing_toggle(&s, cx));
                if s.post_process_enabled {
                    post = post
                        .child(self.post_process_shortcut.clone())
                        .child(self.post_processing_api.clone())
                        .child(self.post_processing_prompts.clone());
                }
                let advanced = SettingsGroup::new()
                    .title(t("settings.advanced.groups.advanced"))
                    .child(rows::keyboard_implementation(&s, cx))
                    .children(self.acceleration_rows(&s, cx))
                    .child(rows::lazy_stream_close(&s, cx))
                    .child(rows::vad_backend(&s, cx));
                vec![post.into_any_element(), advanced.into_any_element()]
            }
        }
    }
}

impl super::Page for SettingsPage {
    fn toolbar(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Toolbar {
        let experimental = self.store.read(cx).settings.experimental_enabled;
        let mut tabs = vec![
            (Tab::Dictation, "settings.advanced.groups.dictation"),
            (Tab::Transcription, "settings.advanced.groups.transcription"),
            (Tab::Appearance, "settings.advanced.groups.appearance"),
            (Tab::System, "settings.advanced.groups.system"),
        ];
        if experimental {
            tabs.push((Tab::Experimental, "settings.advanced.groups.experimental"));
        }
        let entity = cx.entity().downgrade();
        Toolbar::new().tabs(SegmentedControl::new(
            "settings-tabs",
            tabs.iter()
                .map(|(tab, key)| (SharedString::from(tab.id()), t(key)))
                .collect(),
            self.tab.id(),
            move |id, _, cx| {
                let _ = entity.update(cx, |this, cx| this.select_tab(id, cx));
            },
        ))
    }
}

impl Render for SettingsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let experimental = self.store.read(cx).settings.experimental_enabled;
        // Fall back to Dictation if the Experimental tab disappears while selected.
        if self.tab == Tab::Experimental && !experimental {
            self.tab = Tab::Dictation;
        }
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_5()
            .children(self.render_tab(cx))
    }
}
