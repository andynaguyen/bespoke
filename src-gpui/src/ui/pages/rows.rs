//! Setting rows without local state, shared by the Settings and Debug pages.
//! Each mirrors one component in `src/components/settings/`.

use gpui::{div, prelude::*, rems, AnyElement, App, FontWeight, SharedString};

use crate::app_handle::AppHandle;
use crate::commands::audio::AudioDevice;
use crate::i18n::{t, t_with};
use crate::settings::{
    AppSettings, AutoSubmitKey, ClipboardHandling, LogLevel, ModelUnloadTimeout, OverlayPosition,
    OverlayStyle, PasteMethod, RecordingRetentionPeriod, ShortcutActivation, SoundTheme, Theme,
};
use crate::shortcut;
use crate::ui::components::{
    icon, reset_button, Button, ButtonSize, ButtonVariant, Dropdown, DropdownOption, RowDirection,
    SettingContainer, Slider, ToggleSwitchRow, Typography,
};
use crate::ui::stores::SettingsStore;
use crate::ui::theme::palette;

/// Writes one setting through the store: applied locally at once, persisted
/// by `command` on a worker thread.
pub fn set(
    cx: &mut App,
    key: &'static str,
    apply: impl FnOnce(&mut AppSettings) + 'static,
    command: impl FnOnce(AppHandle) -> Result<(), String> + Send + 'static,
) {
    SettingsStore::global(cx).update(cx, |store, cx| store.update(key, apply, command, cx));
}

fn store(cx: &App) -> &SettingsStore {
    SettingsStore::global(cx).read(cx)
}

/// A switch row bound to one boolean setting.
pub fn toggle(
    key: &'static str,
    title: &str,
    description: &str,
    value: bool,
    cx: &App,
    on_change: impl Fn(bool, &mut App) + 'static,
) -> AnyElement {
    ToggleSwitchRow::new(SharedString::from(key), t(title), t(description), value)
        .updating(store(cx).is_updating(key))
        .on_change(move |value, _, cx| on_change(value, cx))
        .into_any_element()
}

/// A dropdown row bound to one setting.
pub fn dropdown(
    key: &'static str,
    title: &str,
    description: &str,
    options: Vec<DropdownOption>,
    selected: impl Into<SharedString>,
    cx: &App,
    on_select: impl Fn(&SharedString, &mut App) + 'static,
) -> AnyElement {
    SettingContainer::new(t(title))
        .description(t(description))
        .child(
            Dropdown::new(SharedString::from(key), options)
                .selected(Some(selected.into()))
                .disabled(store(cx).is_updating(key))
                .on_select(move |value, _, cx| on_select(value, cx)),
        )
        .into_any_element()
}

pub fn shortcut_activation(s: &AppSettings, cx: &App) -> AnyElement {
    let option = |value: &str, label: &str| {
        DropdownOption::new(
            value.to_string(),
            t(&format!(
                "settings.general.shortcutActivation.options.{label}"
            )),
        )
        .description(t(&format!(
            "settings.general.shortcutActivation.descriptions.{value}"
        )))
    };
    let selected = match s.shortcut_activation {
        ShortcutActivation::HoldOrToggle => "hold_or_toggle",
        ShortcutActivation::PushToTalk => "push_to_talk",
        ShortcutActivation::Toggle => "toggle",
    };
    SettingContainer::new(t("settings.general.shortcutActivation.title"))
        .description(t("settings.general.shortcutActivation.description"))
        .child(
            Dropdown::new(
                "shortcut_activation",
                vec![
                    option("hold_or_toggle", "holdOrToggle"),
                    option("push_to_talk", "pushToTalk"),
                    option("toggle", "toggle"),
                ],
            )
            .selected(Some(selected))
            .menu_end_aligned(rems(20.))
            .disabled(store(cx).is_updating("shortcut_activation"))
            .on_select(|value, _, cx| {
                let activation = match value.as_ref() {
                    "push_to_talk" => ShortcutActivation::PushToTalk,
                    "toggle" => ShortcutActivation::Toggle,
                    _ => ShortcutActivation::HoldOrToggle,
                };
                set(
                    cx,
                    "shortcut_activation",
                    move |s| s.shortcut_activation = activation,
                    move |app| shortcut::change_shortcut_activation_setting(app, activation),
                );
            }),
        )
        .into_any_element()
}

/// Device options with "Default" first, as the React selectors list them.
fn device_options(devices: &[AudioDevice]) -> Vec<DropdownOption> {
    devices
        .iter()
        .map(|d| DropdownOption::new(d.name.clone(), d.name.clone()))
        .collect()
}

fn device_selected(value: &Option<String>) -> SharedString {
    match value.as_deref() {
        None | Some("default") | Some("Default") => "Default".into(),
        Some(name) => name.to_string().into(),
    }
}

/// `"Default"` is stored as `"default"`, which the backend treats as the
/// system device.
fn device_value(name: &str) -> String {
    if name == "Default" {
        "default".into()
    } else {
        name.into()
    }
}

pub fn microphone(s: &AppSettings, cx: &App) -> AnyElement {
    let store = store(cx);
    let loading = store.audio_devices.is_empty();
    let disabled = store.is_updating("selected_microphone") || loading;
    SettingContainer::new(t("settings.sound.microphone.title"))
        .description(t("settings.sound.microphone.description"))
        .child(
            div()
                .flex()
                .row_dir()
                .items_center()
                .gap_1()
                .child(
                    Dropdown::new("selected_microphone", device_options(&store.audio_devices))
                        .selected(Some(device_selected(&s.selected_microphone)))
                        .placeholder(if loading {
                            t("settings.sound.microphone.loading")
                        } else {
                            t("settings.sound.microphone.placeholder")
                        })
                        .disabled(disabled)
                        .on_open(|_, cx| {
                            SettingsStore::global(cx)
                                .update(cx, |s, cx| s.refresh_audio_devices(cx))
                        })
                        .on_select(|name, _, cx| set_microphone(name.to_string(), cx)),
                )
                .child(reset_button(
                    "reset-microphone",
                    disabled,
                    |_, _, cx| set_microphone("Default".into(), cx),
                    cx,
                )),
        )
        .into_any_element()
}

fn set_microphone(name: String, cx: &mut App) {
    let value = device_value(&name);
    let stored = (value != "default").then(|| value.clone());
    SettingsStore::global(cx).update(cx, |store, cx| {
        store.update_async(
            "selected_microphone",
            move |s| s.selected_microphone = stored,
            {
                let app = store.app().clone();
                async move { crate::commands::audio::set_selected_microphone(app, value).await }
            },
            cx,
        )
    });
}

pub fn output_device(s: &AppSettings, disabled: bool, cx: &App) -> AnyElement {
    let store = store(cx);
    let loading = store.output_devices.is_empty();
    let blocked = disabled || store.is_updating("selected_output_device") || loading;
    let select = |name: String, cx: &mut App| {
        let value = device_value(&name);
        let stored = (value != "default").then(|| value.clone());
        set(
            cx,
            "selected_output_device",
            move |s| s.selected_output_device = stored,
            move |app| crate::commands::audio::set_selected_output_device(app, value),
        );
    };
    SettingContainer::new(t("settings.sound.outputDevice.title"))
        .description(t("settings.sound.outputDevice.description"))
        .disabled(disabled)
        .child(
            div()
                .flex()
                .row_dir()
                .items_center()
                .gap_1()
                .child(
                    Dropdown::new(
                        "selected_output_device",
                        device_options(&store.output_devices),
                    )
                    .selected(Some(device_selected(&s.selected_output_device)))
                    .placeholder(if loading {
                        t("settings.sound.outputDevice.loading")
                    } else {
                        t("settings.sound.outputDevice.placeholder")
                    })
                    .disabled(blocked)
                    .on_open(|_, cx| {
                        SettingsStore::global(cx).update(cx, |s, cx| s.refresh_output_devices(cx))
                    })
                    .on_select(move |name, _, cx| select(name.to_string(), cx)),
                )
                .child(reset_button(
                    "reset-output",
                    blocked,
                    move |_, _, cx| select("Default".into(), cx),
                    cx,
                )),
        )
        .into_any_element()
}

pub fn clamshell_microphone(s: &AppSettings, cx: &App) -> AnyElement {
    let store = store(cx);
    let loading = store.audio_devices.is_empty();
    let disabled = store.is_updating("clamshell_microphone") || loading;
    let select = |name: String, cx: &mut App| {
        let value = device_value(&name);
        let stored = (value != "default").then(|| value.clone());
        set(
            cx,
            "clamshell_microphone",
            move |s| s.clamshell_microphone = stored,
            move |app| crate::commands::audio::set_clamshell_microphone(app, value),
        );
    };
    SettingContainer::new(t("settings.debug.clamshellMicrophone.title"))
        .description(t("settings.debug.clamshellMicrophone.description"))
        .child(
            div()
                .flex()
                .row_dir()
                .items_center()
                .gap_1()
                .child(
                    Dropdown::new("clamshell_microphone", device_options(&store.audio_devices))
                        .selected(Some(device_selected(&s.clamshell_microphone)))
                        .placeholder(if loading {
                            t("common.loading")
                        } else {
                            t("settings.sound.microphone.placeholder")
                        })
                        .disabled(disabled)
                        .on_open(|_, cx| {
                            SettingsStore::global(cx)
                                .update(cx, |s, cx| s.refresh_audio_devices(cx))
                        })
                        .on_select(move |name, _, cx| select(name.to_string(), cx)),
                )
                .child(reset_button(
                    "reset-clamshell",
                    disabled,
                    move |_, _, cx| select("Default".into(), cx),
                    cx,
                )),
        )
        .into_any_element()
}

pub fn mute_while_recording(s: &AppSettings, cx: &App) -> AnyElement {
    toggle(
        "mute_while_recording",
        "settings.debug.muteWhileRecording.label",
        "settings.debug.muteWhileRecording.description",
        s.mute_while_recording,
        cx,
        |v, cx| {
            set(
                cx,
                "mute_while_recording",
                move |s| s.mute_while_recording = v,
                move |app| shortcut::change_mute_while_recording_setting(app, v),
            )
        },
    )
}

pub fn translate_to_english(s: &AppSettings, cx: &App) -> AnyElement {
    toggle(
        "translate_to_english",
        "settings.advanced.translateToEnglish.label",
        "settings.advanced.translateToEnglish.description",
        s.translate_to_english,
        cx,
        |v, cx| {
            set(
                cx,
                "translate_to_english",
                move |s| s.translate_to_english = v,
                move |app| shortcut::change_translate_to_english_setting(app, v),
            )
        },
    )
}

pub fn vad_enabled(s: &AppSettings, cx: &App) -> AnyElement {
    toggle(
        "vad_enabled",
        "settings.advanced.voiceActivityDetection.title",
        "settings.advanced.voiceActivityDetection.description",
        s.vad_enabled,
        cx,
        |v, cx| {
            set(
                cx,
                "vad_enabled",
                move |s| s.vad_enabled = v,
                move |app| shortcut::change_vad_enabled_setting(app, v),
            )
        },
    )
}

pub fn audio_feedback(s: &AppSettings, cx: &App) -> AnyElement {
    toggle(
        "audio_feedback",
        "settings.sound.audioFeedback.label",
        "settings.sound.audioFeedback.description",
        s.audio_feedback,
        cx,
        |v, cx| {
            set(
                cx,
                "audio_feedback",
                move |s| s.audio_feedback = v,
                move |app| shortcut::change_audio_feedback_setting(app, v),
            )
        },
    )
}

pub fn volume(s: &AppSettings, disabled: bool) -> AnyElement {
    Slider::new(
        "audio_feedback_volume",
        t("settings.sound.volume.title"),
        t("settings.sound.volume.description"),
        s.audio_feedback_volume,
        0.0..=1.0,
    )
    .disabled(disabled)
    .format(|v| format!("{}%", (v * 100.).round() as i64))
    .on_change(|v, _, cx| {
        set(
            cx,
            "audio_feedback_volume",
            move |s| s.audio_feedback_volume = v,
            move |app| shortcut::change_audio_feedback_volume_setting(app, v),
        )
    })
    .into_any_element()
}

pub fn filler_word_removal(s: &AppSettings, cx: &App) -> AnyElement {
    toggle(
        "filler_word_removal_enabled",
        "settings.advanced.fillerWordRemoval.title",
        "settings.advanced.fillerWordRemoval.description",
        s.filler_word_removal_enabled,
        cx,
        |v, cx| {
            set(
                cx,
                "filler_word_removal_enabled",
                move |s| s.filler_word_removal_enabled = v,
                move |app| shortcut::change_filler_word_removal_enabled_setting(app, v),
            )
        },
    )
}

pub fn append_trailing_space(s: &AppSettings, cx: &App) -> AnyElement {
    toggle(
        "append_trailing_space",
        "settings.debug.appendTrailingSpace.label",
        "settings.debug.appendTrailingSpace.description",
        s.append_trailing_space,
        cx,
        |v, cx| {
            set(
                cx,
                "append_trailing_space",
                move |s| s.append_trailing_space = v,
                move |app| shortcut::change_append_trailing_space_setting(app, v),
            )
        },
    )
}

pub fn paste_method_value(method: PasteMethod) -> &'static str {
    match method {
        PasteMethod::CtrlV => "ctrl_v",
        PasteMethod::Direct => "direct",
        PasteMethod::None => "none",
        PasteMethod::ShiftInsert => "shift_insert",
        PasteMethod::CtrlShiftV => "ctrl_shift_v",
        PasteMethod::ExternalScript => "external_script",
    }
}

pub fn paste_method_options(current: PasteMethod) -> Vec<DropdownOption> {
    let mac = cfg!(target_os = "macos");
    let modifier = if mac { "Cmd" } else { "Ctrl" };
    let mut options = vec![DropdownOption::new(
        "ctrl_v",
        t_with(
            "settings.advanced.pasteMethod.options.clipboard",
            &[("modifier", &modifier)],
        ),
    )];
    // Direct typing is unsupported on macOS; it stays listed (disabled) only
    // when an older install still has it selected.
    if !mac || current == PasteMethod::Direct {
        options.push(
            DropdownOption::new("direct", t("settings.advanced.pasteMethod.options.direct"))
                .disabled(mac),
        );
    }
    options.push(DropdownOption::new(
        "none",
        t("settings.advanced.pasteMethod.options.none"),
    ));
    if cfg!(any(target_os = "windows", target_os = "linux")) {
        options.push(DropdownOption::new(
            "ctrl_shift_v",
            t("settings.advanced.pasteMethod.options.clipboardCtrlShiftV"),
        ));
        options.push(DropdownOption::new(
            "shift_insert",
            t("settings.advanced.pasteMethod.options.clipboardShiftInsert"),
        ));
    }
    if cfg!(target_os = "linux") {
        options.push(DropdownOption::new(
            "external_script",
            t("settings.advanced.pasteMethod.options.externalScript"),
        ));
    }
    options
}

pub fn clipboard_handling(s: &AppSettings, cx: &App) -> AnyElement {
    let selected = match s.clipboard_handling {
        ClipboardHandling::DontModify => "dont_modify",
        ClipboardHandling::CopyToClipboard => "copy_to_clipboard",
    };
    dropdown(
        "clipboard_handling",
        "settings.advanced.clipboardHandling.title",
        "settings.advanced.clipboardHandling.description",
        vec![
            DropdownOption::new(
                "dont_modify",
                t("settings.advanced.clipboardHandling.options.dontModify"),
            ),
            DropdownOption::new(
                "copy_to_clipboard",
                t("settings.advanced.clipboardHandling.options.copyToClipboard"),
            ),
        ],
        selected,
        cx,
        |value, cx| {
            let handling = if value.as_ref() == "copy_to_clipboard" {
                ClipboardHandling::CopyToClipboard
            } else {
                ClipboardHandling::DontModify
            };
            let value = value.to_string();
            set(
                cx,
                "clipboard_handling",
                move |s| s.clipboard_handling = handling,
                move |app| shortcut::change_clipboard_handling_setting(app, value),
            )
        },
    )
}

pub fn submit_key_value(key: AutoSubmitKey) -> &'static str {
    match key {
        AutoSubmitKey::Enter => "enter",
        AutoSubmitKey::CtrlEnter => "ctrl_enter",
        AutoSubmitKey::CmdEnter => "cmd_enter",
    }
}

fn parse_submit_key(value: &str) -> AutoSubmitKey {
    match value {
        "ctrl_enter" => AutoSubmitKey::CtrlEnter,
        "cmd_enter" => AutoSubmitKey::CmdEnter,
        _ => AutoSubmitKey::Enter,
    }
}

fn submit_key_options() -> Vec<DropdownOption> {
    vec![
        DropdownOption::new("enter", t("settings.advanced.autoSubmit.options.enter")),
        DropdownOption::new(
            "ctrl_enter",
            t("settings.advanced.autoSubmit.options.ctrlEnter"),
        ),
        DropdownOption::new(
            "cmd_enter",
            if cfg!(target_os = "macos") {
                t("settings.advanced.autoSubmit.options.cmdEnter")
            } else {
                t("settings.advanced.autoSubmit.options.superEnter")
            },
        ),
    ]
}

pub fn set_submit_key(value: &str, cx: &mut App) {
    let key = parse_submit_key(value);
    let value = value.to_string();
    set(
        cx,
        "auto_submit_key",
        move |s| s.auto_submit_key = key,
        move |app| shortcut::change_auto_submit_key_setting(app, value),
    );
}

pub fn auto_submit(s: &AppSettings, cx: &App) -> AnyElement {
    let mut options = vec![DropdownOption::new(
        "off",
        t("settings.advanced.autoSubmit.options.off"),
    )];
    options.extend(submit_key_options());
    let selected = if s.auto_submit {
        submit_key_value(s.auto_submit_key)
    } else {
        "off"
    };
    let was_enabled = s.auto_submit;
    let store = store(cx);
    SettingContainer::new(t("settings.advanced.autoSubmit.title"))
        .description(t("settings.advanced.autoSubmit.description"))
        .child(
            Dropdown::new("auto_submit", options)
                .selected(Some(selected))
                .disabled(store.is_updating("auto_submit") || store.is_updating("auto_submit_key"))
                .on_select(move |value, _, cx| {
                    if value.as_ref() == "off" {
                        set(
                            cx,
                            "auto_submit",
                            |s| s.auto_submit = false,
                            |app| shortcut::change_auto_submit_setting(app, false),
                        );
                        return;
                    }
                    set_submit_key(value, cx);
                    if !was_enabled {
                        set(
                            cx,
                            "auto_submit",
                            |s| s.auto_submit = true,
                            |app| shortcut::change_auto_submit_setting(app, true),
                        );
                    }
                }),
        )
        .into_any_element()
}

pub fn submit_key(s: &AppSettings, cx: &App) -> AnyElement {
    dropdown(
        "auto_submit_key",
        "settings.advanced.voiceSubmit.key.title",
        "settings.advanced.voiceSubmit.key.description",
        submit_key_options(),
        submit_key_value(s.auto_submit_key),
        cx,
        |value, cx| set_submit_key(value, cx),
    )
}

pub fn app_language(s: &AppSettings, cx: &App) -> AnyElement {
    let options = crate::i18n::LANGUAGES
        .iter()
        .map(|l| DropdownOption::new(l.code, format!("{} ({})", l.native_name, l.name)))
        .collect();
    let selected =
        crate::i18n::supported_language(&s.app_language).unwrap_or(crate::i18n::language());
    dropdown(
        "app_language",
        "appLanguage.title",
        "appLanguage.description",
        options,
        selected,
        cx,
        |code, cx| {
            crate::i18n::set_language(code);
            cx.refresh_windows();
            let code = code.to_string();
            let stored = code.clone();
            set(
                cx,
                "app_language",
                move |s| s.app_language = stored,
                move |app| shortcut::change_app_language_setting(app, code),
            )
        },
    )
}

pub fn theme(s: &AppSettings, cx: &App) -> AnyElement {
    let selected = match s.theme {
        Theme::System => "system",
        Theme::Light => "light",
        Theme::Dark => "dark",
    };
    dropdown(
        "theme",
        "theme.title",
        "theme.description",
        vec![
            DropdownOption::new("system", t("theme.options.system")),
            DropdownOption::new("light", t("theme.options.light")),
            DropdownOption::new("dark", t("theme.options.dark")),
        ],
        selected,
        cx,
        |value, cx| {
            let theme = match value.as_ref() {
                "light" => Theme::Light,
                "dark" => Theme::Dark,
                _ => Theme::System,
            };
            // Repaint right away, before the write lands.
            crate::ui::theme::apply_theme_setting(theme, cx);
            let value = value.to_string();
            set(
                cx,
                "theme",
                move |s| s.theme = theme,
                move |app| shortcut::change_theme_setting(app, value),
            )
        },
    )
}

pub fn overlay(s: &AppSettings, cx: &App) -> Vec<AnyElement> {
    let style = match s.overlay_style {
        OverlayStyle::None => "none",
        OverlayStyle::Minimal => "minimal",
        OverlayStyle::Live => "live",
    };
    let mut rows = vec![dropdown(
        "overlay_style",
        "settings.advanced.overlay.style.title",
        "settings.advanced.overlay.style.description",
        vec![
            DropdownOption::new("none", t("settings.advanced.overlay.style.options.none")),
            DropdownOption::new(
                "minimal",
                t("settings.advanced.overlay.style.options.minimal"),
            ),
            DropdownOption::new("live", t("settings.advanced.overlay.style.options.live")),
        ],
        style,
        cx,
        |value, cx| {
            let style = match value.as_ref() {
                "none" => OverlayStyle::None,
                "minimal" => OverlayStyle::Minimal,
                _ => OverlayStyle::Live,
            };
            let value = value.to_string();
            set(
                cx,
                "overlay_style",
                move |s| s.overlay_style = style,
                move |app| shortcut::change_overlay_style_setting(app, value),
            )
        },
    )];
    if s.overlay_style != OverlayStyle::None {
        let position = if s.overlay_position == OverlayPosition::Top {
            "top"
        } else {
            "bottom"
        };
        rows.push(dropdown(
            "overlay_position",
            "settings.advanced.overlay.position.title",
            "settings.advanced.overlay.position.description",
            vec![
                DropdownOption::new(
                    "bottom",
                    t("settings.advanced.overlay.position.options.bottom"),
                ),
                DropdownOption::new("top", t("settings.advanced.overlay.position.options.top")),
            ],
            position,
            cx,
            |value, cx| {
                let position = if value.as_ref() == "top" {
                    OverlayPosition::Top
                } else {
                    OverlayPosition::Bottom
                };
                let value = value.to_string();
                set(
                    cx,
                    "overlay_position",
                    move |s| s.overlay_position = position,
                    move |app| shortcut::change_overlay_position_setting(app, value),
                )
            },
        ));
    }
    rows
}

pub fn start_hidden(s: &AppSettings, cx: &App) -> AnyElement {
    toggle(
        "start_hidden",
        "settings.advanced.startHidden.label",
        "settings.advanced.startHidden.description",
        s.start_hidden,
        cx,
        |v, cx| {
            set(
                cx,
                "start_hidden",
                move |s| s.start_hidden = v,
                move |app| shortcut::change_start_hidden_setting(app, v),
            )
        },
    )
}

pub fn autostart(s: &AppSettings, cx: &App) -> AnyElement {
    toggle(
        "autostart_enabled",
        "settings.advanced.autostart.label",
        "settings.advanced.autostart.description",
        s.autostart_enabled,
        cx,
        |v, cx| {
            set(
                cx,
                "autostart_enabled",
                move |s| s.autostart_enabled = v,
                move |app| shortcut::change_autostart_setting(app, v),
            )
        },
    )
}

pub fn show_tray_icon(s: &AppSettings, cx: &App) -> AnyElement {
    toggle(
        "show_tray_icon",
        "settings.advanced.showTrayIcon.label",
        "settings.advanced.showTrayIcon.description",
        s.show_tray_icon,
        cx,
        |v, cx| {
            set(
                cx,
                "show_tray_icon",
                move |s| s.show_tray_icon = v,
                move |app| shortcut::change_show_tray_icon_setting(app, v),
            )
        },
    )
}

pub fn model_unload_timeout(s: &AppSettings, cx: &App) -> AnyElement {
    let mut options: Vec<DropdownOption> = [
        "never",
        "immediately",
        "min2",
        "min5",
        "min10",
        "min15",
        "hour1",
    ]
    .iter()
    .map(|v| DropdownOption::new(*v, t(&format!("settings.advanced.modelUnload.options.{v}"))))
    .collect();
    if s.debug_mode {
        options.push(DropdownOption::new(
            "sec15",
            t("settings.advanced.modelUnload.options.sec15"),
        ));
    }
    let selected = match s.model_unload_timeout {
        ModelUnloadTimeout::Never => "never",
        ModelUnloadTimeout::Immediately => "immediately",
        ModelUnloadTimeout::Min2 => "min2",
        ModelUnloadTimeout::Min5 => "min5",
        ModelUnloadTimeout::Min10 => "min10",
        ModelUnloadTimeout::Min15 => "min15",
        ModelUnloadTimeout::Hour1 => "hour1",
        ModelUnloadTimeout::Sec15 => "sec15",
    };
    dropdown(
        "model_unload_timeout",
        "settings.advanced.modelUnload.title",
        "settings.advanced.modelUnload.description",
        options,
        selected,
        cx,
        |value, cx| {
            let timeout = match value.as_ref() {
                "never" => ModelUnloadTimeout::Never,
                "immediately" => ModelUnloadTimeout::Immediately,
                "min2" => ModelUnloadTimeout::Min2,
                "min10" => ModelUnloadTimeout::Min10,
                "min15" => ModelUnloadTimeout::Min15,
                "hour1" => ModelUnloadTimeout::Hour1,
                "sec15" => ModelUnloadTimeout::Sec15,
                _ => ModelUnloadTimeout::Min5,
            };
            set(
                cx,
                "model_unload_timeout",
                move |s| s.model_unload_timeout = timeout,
                move |app| {
                    crate::commands::transcription::set_model_unload_timeout(app, timeout);
                    Ok(())
                },
            )
        },
    )
}

pub fn show_whats_new(s: &AppSettings, cx: &App) -> AnyElement {
    toggle(
        "show_whats_new_on_update",
        "settings.about.whatsNewUpdates.label",
        "settings.about.whatsNewUpdates.description",
        s.show_whats_new_on_update,
        cx,
        |v, cx| {
            set(
                cx,
                "show_whats_new_on_update",
                move |s| s.show_whats_new_on_update = v,
                move |app| shortcut::change_show_whats_new_on_update_setting(app, v),
            )
        },
    )
}

pub fn experimental(s: &AppSettings, cx: &App) -> AnyElement {
    toggle(
        "experimental_enabled",
        "settings.advanced.experimentalToggle.label",
        "settings.advanced.experimentalToggle.description",
        s.experimental_enabled,
        cx,
        |v, cx| {
            set(
                cx,
                "experimental_enabled",
                move |s| s.experimental_enabled = v,
                move |app| shortcut::change_experimental_enabled_setting(app, v),
            )
        },
    )
}

pub fn post_processing_toggle(s: &AppSettings, cx: &App) -> AnyElement {
    toggle(
        "post_process_enabled",
        "settings.debug.postProcessingToggle.label",
        "settings.debug.postProcessingToggle.description",
        s.post_process_enabled,
        cx,
        |v, cx| {
            set(
                cx,
                "post_process_enabled",
                move |s| s.post_process_enabled = v,
                move |app| shortcut::change_post_process_enabled_setting(app, v),
            )
        },
    )
}

pub fn lazy_stream_close(s: &AppSettings, cx: &App) -> AnyElement {
    toggle(
        "lazy_stream_close",
        "settings.advanced.lazyStreamClose.label",
        "settings.advanced.lazyStreamClose.description",
        s.lazy_stream_close,
        cx,
        |v, cx| {
            set(
                cx,
                "lazy_stream_close",
                move |s| s.lazy_stream_close = v,
                move |app| shortcut::change_lazy_stream_close_setting(app, v),
            )
        },
    )
}

pub fn vad_backend(s: &AppSettings, cx: &App) -> AnyElement {
    use crate::settings::VadBackend;
    let selected = match s.vad_backend {
        VadBackend::Silero => "silero",
        VadBackend::Earshot => "earshot",
    };
    dropdown(
        "vad_backend",
        "settings.advanced.vadBackend.title",
        "settings.advanced.vadBackend.description",
        vec![
            DropdownOption::new("silero", t("settings.advanced.vadBackend.options.silero")),
            DropdownOption::new("earshot", t("settings.advanced.vadBackend.options.earshot")),
        ],
        selected,
        cx,
        |value, cx| {
            let backend = if value.as_ref() == "earshot" {
                VadBackend::Earshot
            } else {
                VadBackend::Silero
            };
            SettingsStore::global(cx).update(cx, |store, cx| {
                let app = store.app().clone();
                store.update_async(
                    "vad_backend",
                    move |s| s.vad_backend = backend,
                    async move {
                        let result =
                            shortcut::change_vad_backend_setting(app.clone(), backend).await;
                        if let Err(error) = &result {
                            let error = error.clone();
                            app.with_ui(move |cx| crate::ui::toast::error(error, cx));
                        }
                        result
                    },
                    cx,
                )
            });
        },
    )
}

pub fn keyboard_implementation(s: &AppSettings, cx: &App) -> AnyElement {
    use crate::settings::KeyboardImplementation;
    let selected = match s.keyboard_implementation {
        KeyboardImplementation::Tauri => "tauri",
        KeyboardImplementation::HandyKeys => "handy_keys",
    };
    dropdown(
        "keyboard_implementation",
        "settings.debug.keyboardImplementation.title",
        "settings.debug.keyboardImplementation.description",
        vec![
            DropdownOption::new("tauri", "Tauri Global Shortcut"),
            DropdownOption::new("handy_keys", "Handy Keys"),
        ],
        selected,
        cx,
        |value, cx| {
            let implementation = value.to_string();
            let app = AppHandle::global().clone();
            cx.spawn(async move |cx| {
                let result = crate::runtime::spawn_blocking(move || {
                    shortcut::change_keyboard_implementation_setting(app, implementation)
                })
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
                let _ = cx.update(|cx| {
                    match result {
                        Err(error) => crate::ui::toast::error(error, cx),
                        Ok(result) => {
                            if !result.reset_bindings.is_empty() {
                                crate::ui::toast::warning(
                                    t("settings.debug.keyboardImplementation.bindingsReset"),
                                    cx,
                                );
                            }
                        }
                    }
                    SettingsStore::global(cx).update(cx, |s, cx| s.refresh(cx));
                });
            })
            .detach();
        },
    )
}

pub fn version_row(cx: &App) -> AnyElement {
    let _ = palette(cx);
    SettingContainer::new(t("settings.about.version.title"))
        .description(t("settings.about.version.description"))
        .child(
            div()
                .sm_text()
                .font_family(crate::ui::theme::MONO_FONT)
                .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
        )
        .into_any_element()
}

/// App data or log directory with an Open button. `AppDataDirectory.tsx`
/// replaces the whole row with its error box; `LogDirectory.tsx` keeps the
/// title and shows the error in place of the path.
pub fn directory_row(
    title: &str,
    description: &str,
    path: Result<String, String>,
    open: fn(AppHandle) -> Result<(), String>,
    error_replaces_row: bool,
    cx: &App,
) -> AnyElement {
    if let (Err(error), true) = (&path, error_replaces_row) {
        return div()
            .p_4()
            .bg(crate::ui::theme::tw::red_50())
            .border_1()
            .border_color(crate::ui::theme::tw::red_200())
            .rounded_lg()
            .sm_text()
            .text_color(crate::ui::theme::tw::red_600())
            .child(t_with("errors.loadDirectory", &[("error", error)]))
            .into_any_element();
    }
    let body = match path {
        Ok(path) => crate::ui::components::path_display(
            SharedString::from(format!("open-{title}")),
            path.into(),
            move |_, _, _| {
                if let Err(e) = open(AppHandle::global().clone()) {
                    log::error!("{e}");
                }
            },
            cx,
        )
        .into_any_element(),
        Err(error) => div()
            .p_3()
            .bg(crate::ui::theme::tw::red_50())
            .border_1()
            .border_color(crate::ui::theme::tw::red_200())
            .rounded_sm()
            .xs_text()
            .text_color(crate::ui::theme::tw::red_600())
            .child(t_with("errors.loadDirectory", &[("error", &error)]))
            .into_any_element(),
    };
    SettingContainer::new(t(title))
        .description(t(description))
        .layout(crate::ui::components::SettingLayout::Stacked)
        .child(body)
        .into_any_element()
}

// ---- Debug page rows -------------------------------------------------------

pub fn log_level(s: &AppSettings, cx: &App) -> AnyElement {
    let selected = match s.log_level {
        LogLevel::Error => "error",
        LogLevel::Warn => "warn",
        LogLevel::Info => "info",
        LogLevel::Debug => "debug",
        LogLevel::Trace => "trace",
    };
    dropdown(
        "log_level",
        "settings.debug.logLevel.title",
        "settings.debug.logLevel.description",
        vec![
            DropdownOption::new("error", "Error"),
            DropdownOption::new("warn", "Warn"),
            DropdownOption::new("info", "Info"),
            DropdownOption::new("debug", "Debug"),
            DropdownOption::new("trace", "Trace"),
        ],
        selected,
        cx,
        |value, cx| {
            let level = match value.as_ref() {
                "error" => LogLevel::Error,
                "warn" => LogLevel::Warn,
                "info" => LogLevel::Info,
                "trace" => LogLevel::Trace,
                _ => LogLevel::Debug,
            };
            set(
                cx,
                "log_level",
                move |s| s.log_level = level,
                move |app| crate::commands::set_log_level(app, level),
            )
        },
    )
}

pub fn sound_theme(s: &AppSettings, cx: &App) -> AnyElement {
    let p = palette(cx);
    let custom = store(cx).custom_sounds;
    let mut options = vec![
        DropdownOption::new("marimba", "Marimba"),
        DropdownOption::new("pop", "Pop"),
    ];
    if custom.start && custom.stop {
        options.push(DropdownOption::new("custom", "Custom"));
    }
    let selected = match s.sound_theme {
        SoundTheme::Marimba => "marimba",
        SoundTheme::Pop => "pop",
        SoundTheme::Custom => "custom",
    };
    SettingContainer::new(t("settings.debug.soundTheme.label"))
        .description(t("settings.debug.soundTheme.description"))
        .child(
            div()
                .flex()
                .row_dir()
                .items_center()
                .gap_2()
                .child(
                    Dropdown::new("sound_theme", options)
                        .selected(Some(selected))
                        .disabled(store(cx).is_updating("sound_theme"))
                        .on_select(|value, _, cx| {
                            let theme = match value.as_ref() {
                                "pop" => SoundTheme::Pop,
                                "custom" => SoundTheme::Custom,
                                _ => SoundTheme::Marimba,
                            };
                            let value = value.to_string();
                            set(
                                cx,
                                "sound_theme",
                                move |s| s.sound_theme = theme,
                                move |app| shortcut::change_sound_theme_setting(app, value),
                            )
                        }),
                )
                .child(
                    Button::new("preview-sound")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::Sm)
                        .title("Preview sound theme (plays start then stop)")
                        .child(icon("play").size(rems(1.)).text_color(p.text))
                        .on_click(|_, _, _| {
                            let app = AppHandle::global().clone();
                            crate::runtime::spawn(async move {
                                crate::commands::audio::play_test_sound(
                                    app.clone(),
                                    "start".into(),
                                )
                                .await;
                                crate::commands::audio::play_test_sound(app, "stop".into()).await;
                            });
                        }),
                ),
        )
        .into_any_element()
}

pub fn word_correction_threshold(s: &AppSettings) -> AnyElement {
    Slider::new(
        "word_correction_threshold",
        t("settings.debug.wordCorrectionThreshold.title"),
        t("settings.debug.wordCorrectionThreshold.description"),
        s.word_correction_threshold as f32,
        0.0..=1.0,
    )
    .on_change(|v, _, cx| {
        let v = v as f64;
        set(
            cx,
            "word_correction_threshold",
            move |s| s.word_correction_threshold = v,
            move |app| shortcut::change_word_correction_threshold_setting(app, v),
        )
    })
    .on_reset(|_, cx| {
        let v = SettingsStore::global(cx)
            .read(cx)
            .defaults
            .word_correction_threshold;
        set(
            cx,
            "word_correction_threshold",
            move |s| s.word_correction_threshold = v,
            move |app| shortcut::change_word_correction_threshold_setting(app, v),
        )
    })
    .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn ms_slider(
    key: &'static str,
    title: &str,
    description: &str,
    value: u64,
    range: std::ops::RangeInclusive<f32>,
    step: f32,
    apply: fn(&mut AppSettings, u64),
    default: fn(&AppSettings) -> u64,
    command: fn(AppHandle, u64) -> Result<(), String>,
) -> AnyElement {
    Slider::new(key, t(title), t(description), value as f32, range)
        .step(step)
        .format(|v| format!("{}ms", v.round() as i64))
        .on_change(move |v, _, cx| {
            let v = v.round() as u64;
            set(cx, key, move |s| apply(s, v), move |app| command(app, v))
        })
        .on_reset(move |_, cx| {
            let v = default(&SettingsStore::global(cx).read(cx).defaults);
            set(cx, key, move |s| apply(s, v), move |app| command(app, v))
        })
        .into_any_element()
}

pub fn paste_delay(s: &AppSettings) -> AnyElement {
    ms_slider(
        "paste_delay_ms",
        "settings.debug.pasteDelay.title",
        "settings.debug.pasteDelay.description",
        s.paste_delay_ms,
        10.0..=5000.0,
        10.,
        |s, v| s.paste_delay_ms = v,
        |d| d.paste_delay_ms,
        shortcut::change_paste_delay_ms_setting,
    )
}

pub fn paste_delay_after(s: &AppSettings) -> AnyElement {
    ms_slider(
        "paste_delay_after_ms",
        "settings.debug.pasteDelayAfter.title",
        "settings.debug.pasteDelayAfter.description",
        s.paste_delay_after_ms,
        10.0..=5000.0,
        10.,
        |s, v| s.paste_delay_after_ms = v,
        |d| d.paste_delay_after_ms,
        shortcut::change_paste_delay_after_ms_setting,
    )
}

pub fn hold_threshold(s: &AppSettings) -> AnyElement {
    ms_slider(
        "hold_threshold_ms",
        "settings.debug.holdThreshold.title",
        "settings.debug.holdThreshold.description",
        s.hold_threshold_ms,
        100.0..=1000.0,
        50.,
        |s, v| s.hold_threshold_ms = v,
        |d| d.hold_threshold_ms,
        shortcut::change_hold_threshold_ms_setting,
    )
}

pub fn recording_buffer(s: &AppSettings) -> AnyElement {
    ms_slider(
        "extra_recording_buffer_ms",
        "settings.debug.recordingBuffer.title",
        "settings.debug.recordingBuffer.description",
        s.extra_recording_buffer_ms,
        0.0..=1500.0,
        50.,
        |s, v| s.extra_recording_buffer_ms = v,
        |d| d.extra_recording_buffer_ms,
        shortcut::change_extra_recording_buffer_setting,
    )
}

pub fn reliable_paste(s: &AppSettings, cx: &App) -> AnyElement {
    toggle(
        "reliable_paste",
        "settings.debug.reliablePaste.title",
        "settings.debug.reliablePaste.description",
        s.reliable_paste,
        cx,
        |v, cx| {
            set(
                cx,
                "reliable_paste",
                move |s| s.reliable_paste = v,
                move |app| shortcut::change_reliable_paste_setting(app, v),
            )
        },
    )
}

pub fn always_on_microphone(s: &AppSettings, cx: &App) -> AnyElement {
    toggle(
        "always_on_microphone",
        "settings.debug.alwaysOnMicrophone.label",
        "settings.debug.alwaysOnMicrophone.description",
        s.always_on_microphone,
        cx,
        |v, cx| {
            SettingsStore::global(cx).update(cx, |store, cx| {
                let app = store.app().clone();
                store.update_async(
                    "always_on_microphone",
                    move |s| s.always_on_microphone = v,
                    async move { crate::commands::audio::update_microphone_mode(app, v).await },
                    cx,
                )
            })
        },
    )
}

pub fn recording_retention(s: &AppSettings, cx: &App) -> AnyElement {
    let selected = match s.recording_retention_period {
        RecordingRetentionPeriod::Never => "never",
        RecordingRetentionPeriod::PreserveLimit => "preserve_limit",
        RecordingRetentionPeriod::Days3 => "days3",
        RecordingRetentionPeriod::Weeks2 => "weeks2",
        RecordingRetentionPeriod::Months3 => "months3",
    };
    SettingContainer::new(t("settings.debug.recordingRetention.title"))
        .description(t("settings.debug.recordingRetention.description"))
        .child(
            Dropdown::new(
                "recording_retention_period",
                vec![
                    DropdownOption::new("never", t("settings.debug.recordingRetention.never")),
                    DropdownOption::new(
                        "preserve_limit",
                        t_with(
                            "settings.debug.recordingRetention.preserveLimit",
                            &[("count", &s.history_limit)],
                        ),
                    ),
                    DropdownOption::new("days3", t("settings.debug.recordingRetention.days3")),
                    DropdownOption::new("weeks2", t("settings.debug.recordingRetention.weeks2")),
                    DropdownOption::new("months3", t("settings.debug.recordingRetention.months3")),
                ],
            )
            .selected(Some(selected))
            .placeholder(t("settings.debug.recordingRetention.placeholder"))
            .disabled(store(cx).is_updating("recording_retention_period"))
            .on_select(|value, _, cx| {
                let period = match value.as_ref() {
                    "never" => RecordingRetentionPeriod::Never,
                    "days3" => RecordingRetentionPeriod::Days3,
                    "weeks2" => RecordingRetentionPeriod::Weeks2,
                    "months3" => RecordingRetentionPeriod::Months3,
                    _ => RecordingRetentionPeriod::PreserveLimit,
                };
                let value = value.to_string();
                SettingsStore::global(cx).update(cx, |store, cx| {
                    let app = store.app().clone();
                    store.update_async(
                        "recording_retention_period",
                        move |s| s.recording_retention_period = period,
                        async move {
                            let hm = app
                                .state::<std::sync::Arc<crate::managers::history::HistoryManager>>(
                                );
                            crate::commands::history::update_recording_retention_period(
                                app.clone(),
                                hm,
                                value,
                            )
                            .await
                        },
                        cx,
                    )
                });
            }),
        )
        .into_any_element()
}

/// A small header label used above groups of controls, e.g. "Prompt Label".
pub fn field_label(text: SharedString) -> impl IntoElement {
    div()
        .sm_text()
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}
