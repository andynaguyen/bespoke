//! The Settings page (`settings-page/SettingsPage.tsx`): three tabs of
//! setting groups. Experimental appears only when enabled. Dictation's
//! settings live on the Dictate page (`dictation_settings.rs`).

use gpui::{div, prelude::*, AnyElement, App, Context, Entity, SharedString, Window};

use crate::app_handle::{AppEvent, AppHandle};
use crate::i18n::t;
use crate::managers::transcription::AvailableAccelerators;
use crate::settings::{AppSettings, TranscribeAcceleratorSetting};
use crate::shortcut;
use crate::ui::components::{
    Dropdown, DropdownOption, SegmentedControl, SettingContainer, SettingsGroup,
};
use crate::ui::event_bus::event_bus;
use crate::ui::shell::Toolbar;
use crate::ui::stores::SettingsStore;

use super::post_processing::{PostProcessingApi, PostProcessingPrompts};
use super::rows::{self, set};
use super::shortcut_input::ShortcutInput;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Appearance,
    System,
    Experimental,
}

impl Tab {
    fn id(self) -> &'static str {
        match self {
            Tab::Appearance => "appearance",
            Tab::System => "system",
            Tab::Experimental => "experimental",
        }
    }
}

pub struct SettingsPage {
    store: Entity<SettingsStore>,
    tab: Tab,
    post_process_shortcut: Entity<ShortcutInput>,
    post_processing_api: Entity<PostProcessingApi>,
    post_processing_prompts: Entity<PostProcessingPrompts>,
    accelerators: Option<AvailableAccelerators>,
    app_dir: Result<String, String>,
    log_dir: Result<String, String>,
}

impl SettingsPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = SettingsStore::global(cx);
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&event_bus(cx), |_, _, event: &AppEvent, cx| {
            if event.name == crate::updater::STATE_CHANGED {
                cx.notify();
            }
        })
        .detach();

        let app = AppHandle::global().clone();
        let this = Self {
            post_process_shortcut: cx
                .new(|cx| ShortcutInput::new("transcribe_with_post_process", cx)),
            post_processing_api: cx.new(|cx| PostProcessingApi::new(window, cx)),
            post_processing_prompts: cx.new(|cx| PostProcessingPrompts::new(window, cx)),
            accelerators: None,
            app_dir: crate::commands::get_app_dir_path(app.clone()),
            log_dir: crate::commands::get_log_dir_path(app),
            store,
            tab: Tab::Appearance,
        };

        cx.spawn(async move |this, cx| {
            let available = crate::runtime::run(shortcut::get_available_accelerators()).await;
            let _ = this.update(cx, |this, cx| {
                this.accelerators = Some(available);
                cx.notify();
            });
        })
        .detach();
        this
    }

    pub fn select_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        self.tab = match id {
            "appearance" => Tab::Appearance,
            "system" => Tab::System,
            "experimental" => Tab::Experimental,
            _ => Tab::Appearance,
        };
        cx.notify();
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
                        .child(rows::update_status())
                        .children(crate::updater::is_enabled().then(rows::automatic_updates))
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
        // Fall back to Appearance if the Experimental tab disappears while selected.
        if self.tab == Tab::Experimental && !experimental {
            self.tab = Tab::Appearance;
        }
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_5()
            .children(self.render_tab(cx))
    }
}
