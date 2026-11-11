use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, FontWeight, ScrollHandle, SharedString, Task,
    Window, div, point, prelude::*, px, rems,
};
use gpui_component::scroll::ScrollableElement;

use crate::app_handle::{AppEvent, AppHandle};
use crate::i18n::t;
use crate::managers::transcription::AvailableAccelerators;
use crate::settings::{AppSettings, TranscribeAcceleratorSetting};
use crate::shortcut;
use crate::ui::components::{
    DialogFrame, DialogMaterial, Dropdown, DropdownOption, KeyboardFocus, RowDirection,
    SettingContainer, SettingsGroup, Typography, icon,
};
use crate::ui::event_bus::event_bus;
use crate::ui::glass::{GlassStyle, glass};
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{Alpha, palette, radius, tw};

use super::annotate::{AnnotateSettings, AnnotateSettingsPage};
use super::dictation_settings::{DictatePage, DictationSettings};
use super::post_processing::{PostProcessingApi, PostProcessingPrompts};
use super::rows::{self, set};
use super::shortcut_input::ShortcutInput;

const DIALOG_WIDTH: f32 = 980.;
const NAV_WIDTH: f32 = 210.;
const DEVICE_POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// A page of the Settings dialog, listed in its sidebar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsSection {
    General,
    Appearance,
    Sound,
    DictateShortcut,
    DictateLanguage,
    DictatePostProcessing,
    DictateHistory,
    AnnotateShortcut,
    AnnotateHistory,
    System,
    Experimental,
}

impl SettingsSection {
    const ALL: [Self; 11] = [
        Self::General,
        Self::Appearance,
        Self::Sound,
        Self::DictateShortcut,
        Self::DictateLanguage,
        Self::DictatePostProcessing,
        Self::DictateHistory,
        Self::AnnotateShortcut,
        Self::AnnotateHistory,
        Self::System,
        Self::Experimental,
    ];

    fn id(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Appearance => "appearance",
            Self::Sound => "sound",
            Self::DictateShortcut => "dictate-shortcut",
            Self::DictateLanguage => "dictate-language",
            Self::DictatePostProcessing => "dictate-post-processing",
            Self::DictateHistory => "dictate-history",
            Self::AnnotateShortcut => "annotate-shortcut",
            Self::AnnotateHistory => "annotate-history",
            Self::System => "system",
            Self::Experimental => "experimental",
        }
    }

    /// The page named `id`, as in `settings:<id>` for dev snapshots.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|section| section.id() == id)
    }

    fn label_key(self) -> &'static str {
        match self {
            Self::General => "settings.nav.general",
            Self::Appearance => "settings.advanced.groups.appearance",
            Self::Sound => "settings.nav.sound",
            Self::DictateShortcut | Self::AnnotateShortcut => "settings.advanced.groups.shortcut",
            Self::DictateLanguage => "settings.general.language.title",
            Self::DictatePostProcessing => "settings.nav.postProcessing",
            Self::DictateHistory | Self::AnnotateHistory => "settings.dictionary.tabs.history",
            Self::System => "settings.advanced.groups.system",
            Self::Experimental => "settings.advanced.groups.experimental",
        }
    }

    /// The feature a page belongs to, shown before it in the header.
    fn parent_key(self) -> Option<&'static str> {
        match self {
            Self::DictateShortcut
            | Self::DictateLanguage
            | Self::DictatePostProcessing
            | Self::DictateHistory => Some("sidebar.dictate"),
            Self::AnnotateShortcut | Self::AnnotateHistory => Some("sidebar.annotate"),
            _ => None,
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::General => "sliders-horizontal",
            Self::Appearance => "monitor",
            Self::Sound => "audio-lines",
            Self::DictateShortcut | Self::AnnotateShortcut => "keyboard",
            Self::DictateLanguage => "languages",
            Self::DictatePostProcessing => "pencil-ruler",
            Self::DictateHistory | Self::AnnotateHistory => "history",
            Self::System => "hard-drive",
            Self::Experimental => "flask-conical",
        }
    }

    fn dictate_page(self) -> Option<DictatePage> {
        match self {
            Self::Sound => Some(DictatePage::Sound),
            Self::DictateShortcut => Some(DictatePage::Shortcut),
            Self::DictateLanguage => Some(DictatePage::Language),
            Self::DictatePostProcessing => Some(DictatePage::PostProcessing),
            Self::DictateHistory => Some(DictatePage::History),
            _ => None,
        }
    }

    /// The sidebar: the app-wide pages first, then a headed group per feature, then the rest.
    fn groups(experimental: bool) -> Vec<(Option<&'static str>, Vec<Self>)> {
        let mut groups = vec![
            (None, vec![Self::General, Self::Appearance, Self::Sound]),
            (
                Some("sidebar.dictate"),
                vec![
                    Self::DictateShortcut,
                    Self::DictateLanguage,
                    Self::DictatePostProcessing,
                    Self::DictateHistory,
                ],
            ),
        ];
        groups.push((
            Some("sidebar.annotate"),
            vec![Self::AnnotateShortcut, Self::AnnotateHistory],
        ));
        let mut app = vec![Self::System];
        if experimental {
            app.push(Self::Experimental);
        }
        groups.push((Some("settings.nav.app"), app));
        groups
    }
}

/// Opens Settings over the main window at `section`, or at General for `None`. With Settings
/// already open, it switches to `section` and takes the keys back from the main window, which
/// the tray and Scrawl bring forward first. It does nothing during onboarding. It reads and
/// updates the main window, so call it outside the main window's own update, or defer it.
pub fn open(section: Option<SettingsSection>, window: &mut Window, cx: &mut App) {
    let Some(main) = crate::ui::main_view(cx) else {
        return;
    };
    if !main.read(cx).shows_app() {
        return;
    }
    if let Some(open) = main.read(cx).dialog::<SettingsDialog>() {
        if let Some(section) = section {
            open.update(cx, |dialog, cx| dialog.select(section, cx));
        }
        main.read(cx).focus_dialog(cx);
        return;
    }
    crate::ui::shell::open_dialog(window, cx, move |window, cx| {
        cx.new(|cx| SettingsDialog::new(section.unwrap_or(SettingsSection::General), window, cx))
            .into()
    });
}

pub struct SettingsDialog {
    store: Entity<SettingsStore>,
    section: SettingsSection,
    focus: FocusHandle,
    scroll: ScrollHandle,
    dictation: Entity<DictationSettings>,
    annotate: Entity<AnnotateSettings>,
    post_process_shortcut: Entity<ShortcutInput>,
    post_processing_api: Entity<PostProcessingApi>,
    post_processing_prompts: Entity<PostProcessingPrompts>,
    accelerators: Option<AvailableAccelerators>,
    accelerators_loading: bool,
    app_dir: Result<String, String>,
    log_dir: Result<String, String>,
    device_poll: Option<Task<()>>,
}

impl SettingsDialog {
    fn new(section: SettingsSection, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = SettingsStore::global(cx);
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&event_bus(cx), |_, _, event: &AppEvent, cx| {
            if event.name == crate::updater::STATE_CHANGED {
                cx.notify();
            }
        })
        .detach();
        let focus = cx.focus_handle();
        focus.focus(window);

        // Both stay alive while the dialog is open: their fields keep what you typed between
        // pages, and Annotate's permission poll marks the Permissions row from any page.
        let dictation = cx.new(|cx| DictationSettings::new(window, cx));
        cx.observe(&dictation, |_, _, cx| cx.notify()).detach();
        let annotate = cx.new(AnnotateSettings::new);
        cx.observe(&annotate, |_, _, cx| cx.notify()).detach();

        let app = AppHandle::global().clone();
        let mut this = Self {
            device_poll: None,
            post_process_shortcut: cx
                .new(|cx| ShortcutInput::new("transcribe_with_post_process", cx)),
            post_processing_api: cx.new(|cx| PostProcessingApi::new(window, cx)),
            post_processing_prompts: cx.new(|cx| PostProcessingPrompts::new(window, cx)),
            accelerators: None,
            accelerators_loading: false,
            app_dir: crate::commands::get_app_dir_path(app.clone()),
            log_dir: crate::commands::get_log_dir_path(app),
            store,
            section,
            focus,
            scroll: ScrollHandle::new(),
            dictation,
            annotate,
        };

        this.load_accelerators_if_needed(cx);
        this.sync_device_poll(cx);
        this
    }

    /// Scrolls the open dialog's page `offset` pixels down, for `settings:<page>@<px>` in dev
    /// snapshots.
    #[cfg(debug_assertions)]
    pub fn dev_scroll(offset: f32, cx: &mut App) {
        let open = crate::ui::main_view(cx).and_then(|main| main.read(cx).dialog::<Self>());
        if let Some(dialog) = open {
            dialog.update(cx, |dialog, cx| {
                dialog.scroll.set_offset(point(px(0.), px(-offset)));
                cx.notify();
            });
        }
    }

    /// Shows `section` from its top.
    pub fn select(&mut self, section: SettingsSection, cx: &mut Context<Self>) {
        if self.section != section {
            self.section = section;
            self.load_accelerators_if_needed(cx);
            self.sync_device_poll(cx);
            self.scroll.set_offset(point(px(0.), px(0.)));
        }
        cx.notify();
    }

    fn load_accelerators_if_needed(&mut self, cx: &mut Context<Self>) {
        if self.section != SettingsSection::Experimental
            || self.accelerators.is_some()
            || self.accelerators_loading
        {
            return;
        }
        self.accelerators_loading = true;
        cx.spawn(async move |this, cx| {
            let available = crate::runtime::run(shortcut::get_available_accelerators()).await;
            let _ = this.update(cx, |this, cx| {
                this.accelerators_loading = false;
                this.accelerators = Some(available);
                cx.notify();
            });
        })
        .detach();
    }

    fn sync_device_poll(&mut self, cx: &mut Context<Self>) {
        if self.section != SettingsSection::Sound {
            self.device_poll = None;
            return;
        }
        if self.device_poll.is_some() {
            return;
        }
        let store = self.store.clone();
        self.device_poll = Some(cx.spawn(async move |_, cx| {
            loop {
                if store
                    .update(cx, |store, cx| store.refresh_audio_devices(cx))
                    .is_err()
                {
                    return;
                }
                cx.background_executor().timer(DEVICE_POLL).await;
            }
        }));
    }

    fn permission_missing(&self, cx: &App) -> bool {
        self.annotate.read(cx).permission_missing()
    }

    fn permission_groups(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.annotate.clone().update(cx, |annotate, cx| {
            annotate.groups(AnnotateSettingsPage::Permissions, cx)
        })
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
        let mut rows = vec![
            SettingContainer::new(t("settings.advanced.acceleration.transcribe.title"))
                .description(t("settings.advanced.acceleration.transcribe.description"))
                .child(
                    Dropdown::new("transcribe_accelerator", transcribe)
                        .selected(displayed)
                        .disabled(store.is_updating("transcribe_accelerator"))
                        .on_select(|value, _, cx| {
                            let (accelerator, device) = if value.as_ref() == "cpu" {
                                (TranscribeAcceleratorSetting::Cpu, None)
                            } else if let Some(device) = value.strip_prefix("gpu:") {
                                (TranscribeAcceleratorSetting::Gpu, Some(device.to_string()))
                            } else {
                                (TranscribeAcceleratorSetting::Auto, None)
                            };
                            let stored = device.clone();
                            set(
                                cx,
                                "transcribe_accelerator",
                                move |s| {
                                    s.transcribe_gpu_device = stored;
                                    s.transcribe_accelerator = accelerator;
                                },
                                move |app| {
                                    shortcut::change_transcribe_accelerator_setting(
                                        app,
                                        accelerator,
                                        device,
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
                    "auto" => t("settings.advanced.acceleration.gpuDevice.auto"),
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

    fn render_section(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let s = self.store.read(cx).settings.clone();
        if let Some(page) = self.section.dictate_page() {
            return self
                .dictation
                .clone()
                .update(cx, |dictation, cx| dictation.groups(page, cx));
        }
        match self.section {
            SettingsSection::General => vec![
                SettingsGroup::new()
                    .title(t("settings.advanced.groups.startup"))
                    .child(rows::start_hidden(&s, cx))
                    .child(rows::autostart(&s, cx))
                    .child(rows::show_tray_icon(&s, cx))
                    .into_any_element(),
                SettingsGroup::new()
                    .title(t("settings.advanced.groups.updates"))
                    .child(rows::update_status())
                    .children(crate::updater::is_enabled().then(rows::automatic_updates))
                    .child(rows::show_whats_new(&s, cx))
                    .into_any_element(),
            ],
            SettingsSection::Appearance => vec![
                SettingsGroup::new()
                    .title(t("settings.advanced.groups.application"))
                    .child(rows::app_language(&s, cx))
                    .child(rows::theme(&s, cx))
                    .into_any_element(),
                SettingsGroup::new()
                    .title(t("settings.advanced.groups.overlay"))
                    .children(rows::overlay(&s, cx))
                    .into_any_element(),
            ],
            SettingsSection::AnnotateShortcut | SettingsSection::AnnotateHistory => {
                let page = if self.section == SettingsSection::AnnotateHistory {
                    AnnotateSettingsPage::History
                } else {
                    AnnotateSettingsPage::Shortcut
                };
                self.annotate
                    .clone()
                    .update(cx, |annotate, cx| annotate.groups(page, cx))
            }
            SettingsSection::System => {
                let mut groups = vec![
                    SettingsGroup::new()
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
                        ))
                        .into_any_element(),
                ];
                // Annotate's permissions sit above Experimental, or first while one is missing,
                // since that blocks every recording and Settings opens here to show it.
                let permissions = self.permission_groups(cx);
                if self.permission_missing(cx) {
                    groups.splice(0..0, permissions);
                } else {
                    groups.extend(permissions);
                }
                groups.push(
                    SettingsGroup::new()
                        .title(t("settings.advanced.groups.experimental"))
                        .child(rows::experimental(&s, cx))
                        .into_any_element(),
                );
                groups
            }
            SettingsSection::Experimental => {
                let mut post = SettingsGroup::new()
                    .title(t("settings.debug.postProcessingToggle.label"))
                    .child(rows::post_processing_toggle(&s, cx));
                if s.post_process_enabled {
                    post = post
                        .child(self.post_process_shortcut.clone())
                        .child(self.post_processing_api.clone())
                        .child(self.post_processing_prompts.clone());
                }
                vec![
                    post.into_any_element(),
                    SettingsGroup::new()
                        .title(t("settings.advanced.groups.keyboard"))
                        .child(rows::keyboard_implementation(&s, cx))
                        .into_any_element(),
                    SettingsGroup::new()
                        .title(t("settings.advanced.groups.acceleration"))
                        .children(self.acceleration_rows(&s, cx))
                        .into_any_element(),
                    SettingsGroup::new()
                        .title(t("settings.advanced.groups.recording"))
                        .child(rows::lazy_stream_close(&s, cx))
                        .child(rows::vad_backend(&s, cx))
                        .into_any_element(),
                ]
            }
            _ => Vec::new(),
        }
    }

    fn nav_item(&self, section: SettingsSection, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let active = self.section == section;
        let warn = section == SettingsSection::System && self.permission_missing(cx);
        div()
            .id(SharedString::from(format!("settings-nav-{}", section.id())))
            .flex()
            .row_dir()
            .items_center()
            .gap(px(10.))
            .h(px(36.))
            .px(px(10.))
            .w_full()
            .flex_none()
            .rounded(radius::ROW)
            .cursor_pointer()
            .text_px(15.)
            .when(active, |el| {
                el.bg(p.text.a(if p.is_dark { 0.12 } else { 0.08 }))
                    .text_color(p.text)
                    .font_weight(FontWeight::SEMIBOLD)
            })
            .when(!active, |el| {
                el.text_color(p.text.a(0.85))
                    .hover(|s| s.bg(p.text.a(0.06)).text_color(p.text))
            })
            .keyboard_focus(cx)
            .on_click(cx.listener(move |this, _, _, cx| this.select(section, cx)))
            .child(icon(section.icon()).size(px(18.)).flex_none())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(t(section.label_key())),
            )
            .when(warn, |el| {
                el.child(icon("triangle-alert").size(px(14.)).flex_none().text_color(
                    if p.is_dark {
                        tw::amber_400()
                    } else {
                        tw::amber_600()
                    },
                ))
            })
    }

    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let experimental = self.store.read(cx).settings.experimental_enabled;
        let mut items: Vec<AnyElement> = Vec::new();
        for (heading, sections) in SettingsSection::groups(experimental) {
            items.push(match heading {
                Some(key) => div()
                    .flex_none()
                    .px(px(10.))
                    .pt(px(14.))
                    .pb(px(4.))
                    .text_px(11.)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.mid_gray)
                    .child(t(key))
                    .into_any_element(),
                None => div().flex_none().h(px(4.)).into_any_element(),
            });
            for section in sections {
                items.push(self.nav_item(section, cx).into_any_element());
            }
        }
        // The main window's sidebar: a glass panel inset from the edges, with no divider.
        div().flex_none().h_full().p(radius::INSET).child(
            div()
                .relative()
                .w(px(NAV_WIDTH))
                .h_full()
                .flex()
                .flex_col()
                .rounded(radius::PANEL)
                .overflow_hidden()
                .child(glass("settings-nav", GlassStyle::Regular, radius::PANEL))
                .child(
                    div()
                        .flex_none()
                        .px(px(20.))
                        .pt(px(18.))
                        .pb_1()
                        .text_px(17.)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t("sidebar.settings")),
                )
                .child(
                    div()
                        .id("settings-nav")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .px(px(10.))
                        .pt_1()
                        .pb_3()
                        .children(items),
                ),
        )
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let crumb = |label: SharedString| {
            div()
                .flex_none()
                .flex()
                .row_dir()
                .gap_2()
                .text_color(p.mid_gray)
                .font_weight(FontWeight::MEDIUM)
                .child(label)
                .child("/")
        };
        div()
            .flex_none()
            .h(px(52.))
            .flex()
            .row_dir()
            .items_center()
            .justify_between()
            .gap_3()
            .pl_6()
            .pr_3()
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .row_dir()
                    .items_center()
                    .gap_2()
                    .text_px(14.)
                    .child(crumb(t("sidebar.settings")))
                    .children(self.section.parent_key().map(|key| crumb(t(key))))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t(self.section.label_key())),
                    ),
            )
            .child(
                div()
                    .id("settings-close")
                    .flex_none()
                    .p_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .keyboard_focus_ring(0.5, cx)
                    .text_color(p.mid_gray)
                    .hover(|s| s.bg(p.mid_gray.a(0.1)).text_color(p.text))
                    .on_click(|_, window, cx| crate::ui::shell::close_dialog(window, cx))
                    .child(icon("x").size(rems(1.))),
            )
    }
}

impl Render for SettingsDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let experimental = self.store.read(cx).settings.experimental_enabled;
        if self.section == SettingsSection::Experimental && !experimental {
            self.section = SettingsSection::System;
        }
        // Centered by a row, like the main window's padded pages, so text wraps at the width
        // its height was measured at.
        let content = div().flex().justify_center().px_6().pt_5().pb_8().child(
            div()
                .w_full()
                .max_w(rems(44.))
                .flex()
                .flex_col()
                .gap_5()
                .children(self.render_section(cx)),
        );
        let main = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(self.render_header(cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("settings-scroll")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .child(content),
                    )
                    .vertical_scrollbar(&self.scroll),
            );
        DialogFrame::new(self.focus.clone(), crate::ui::shell::close_dialog)
            .width(px(DIALOG_WIDTH))
            .material(DialogMaterial::Window)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .row_dir()
                    .child(self.render_nav(cx))
                    .child(main),
            )
    }
}
