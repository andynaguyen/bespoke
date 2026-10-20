//! The settings window (`App.tsx`): onboarding until it is done, then the
//! sidebar and the content panel for the selected section. Also hosts the
//! dialog layer and toasts.

mod banners;
mod model_selector;
mod sidebar;

use gpui::{
    div, point, prelude::*, px, rems, AnyView, App, Context, Entity, FocusHandle, Focusable,
    FontWeight, ScrollHandle, SharedString, Window,
};
use gpui_component::scroll::ScrollableElement;

use crate::app_handle::{AppEvent, AppHandle};
use crate::i18n::{t, t_or, t_with};
use crate::managers::transcription::ModelStateEvent;
use crate::ui::components::{KeyboardFocus, RowDirection};
use crate::ui::event_bus::event_bus;
use crate::ui::onboarding::{AccessibilityOnboarding, ModelOnboarding, OnboardingPreview};
use crate::ui::pages;
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{palette, shadow, Alpha, SERIF_FALLBACKS, SERIF_FONT};
use crate::ui::toast;
use crate::ui::whats_new::WhatsNewGate;

pub use banners::SECURE_INPUT_HELP_URL;
use banners::{AccessibilityBanner, SecureInputWarning};
use model_selector::ModelSelector;
pub use sidebar::logo;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    History,
    Stats,
    Customize,
    Models,
    Debug,
    Settings,
}

impl Section {
    pub const PRIMARY: [Section; 4] = [
        Section::History,
        Section::Stats,
        Section::Customize,
        Section::Models,
    ];

    pub fn label_key(self) -> &'static str {
        match self {
            Section::History => "sidebar.history",
            Section::Stats => "sidebar.stats",
            Section::Customize => "sidebar.customize",
            Section::Models => "sidebar.models",
            Section::Debug => "sidebar.debug",
            Section::Settings => "sidebar.settings",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Section::History => "nav-history",
            Section::Stats => "nav-chart-column",
            Section::Customize => "nav-pencil-ruler",
            Section::Models => "nav-cpu",
            Section::Debug => "nav-flask-conical",
            Section::Settings => "nav-settings",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum OnboardingStep {
    Accessibility,
    Model,
    Done,
}

/// macOS window chrome height reserved above the content.
pub const TITLEBAR_HEIGHT: f32 = if cfg!(target_os = "macos") { 28. } else { 0. };

pub struct MainWindow {
    step: OnboardingStep,
    is_returning_user: bool,
    post_onboarding_done: bool,
    preview: Option<OnboardingPreview>,
    section: Section,
    page: AnyView,
    scroll: ScrollHandle,
    focus: FocusHandle,
    model_selector: Entity<ModelSelector>,
    accessibility_banner: Entity<AccessibilityBanner>,
    secure_input_warning: Entity<SecureInputWarning>,
    whats_new: Entity<WhatsNewGate>,
    accessibility_onboarding: Option<Entity<AccessibilityOnboarding>>,
    model_onboarding: Option<Entity<ModelOnboarding>>,
    dialog: Option<AnyView>,
    previous_focus: Option<FocusHandle>,
}

impl MainWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = SettingsStore::global(cx);
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&event_bus(cx), |this, _, event: &AppEvent, cx| {
            this.handle_event(event, cx)
        })
        .detach();
        cx.observe_window_appearance(window, |_, window, cx| {
            crate::ui::theme::system_appearance_changed(window, cx);
        })
        .detach();

        let (step, is_returning_user) = initial_onboarding_step(cx);
        #[cfg(debug_assertions)]
        let step = if crate::ui::dev_snapshot::requested() {
            OnboardingStep::Done
        } else {
            step
        };
        // A returning user who lost a permission sees the permission step
        // even when Handy starts hidden.
        if is_returning_user && step == OnboardingStep::Accessibility {
            cx.defer(crate::ui::show_main_window);
        }
        let focus = cx.focus_handle();
        focus.focus(window);

        let mut this = Self {
            step,
            is_returning_user,
            post_onboarding_done: false,
            preview: None,
            section: Section::History,
            page: pages::build(Section::History, window, cx),
            scroll: ScrollHandle::new(),
            focus,
            model_selector: cx.new(ModelSelector::new),
            accessibility_banner: cx.new(|_| AccessibilityBanner::new()),
            secure_input_warning: cx.new(SecureInputWarning::new),
            whats_new: cx.new(|cx| WhatsNewGate::new(window, cx)),
            accessibility_onboarding: None,
            model_onboarding: None,
            dialog: None,
            previous_focus: None,
        };
        this.enter_step(step, window, cx);
        this
    }

    fn enter_step(&mut self, step: OnboardingStep, window: &mut Window, cx: &mut Context<Self>) {
        self.step = step;
        match step {
            OnboardingStep::Accessibility => {
                let weak = cx.entity().downgrade();
                self.accessibility_onboarding = Some(cx.new(|cx| {
                    AccessibilityOnboarding::new(
                        false,
                        move |window, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                let next = if this.is_returning_user {
                                    OnboardingStep::Done
                                } else {
                                    OnboardingStep::Model
                                };
                                this.enter_step(next, window, cx);
                            });
                        },
                        window,
                        cx,
                    )
                }));
            }
            OnboardingStep::Model => {
                self.accessibility_onboarding = None;
                let weak = cx.entity().downgrade();
                self.model_onboarding = Some(cx.new(|cx| {
                    ModelOnboarding::new(
                        false,
                        move |window, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.enter_step(OnboardingStep::Done, window, cx);
                            });
                        },
                        window,
                        cx,
                    )
                }));
            }
            OnboardingStep::Done => {
                self.accessibility_onboarding = None;
                self.model_onboarding = None;
                self.run_post_onboarding_init(cx);
            }
        }
        cx.notify();
    }

    /// Enigo and shortcuts start only after onboarding, so the macOS
    /// permission prompts never appear before the user is ready.
    fn run_post_onboarding_init(&mut self, cx: &mut Context<Self>) {
        if self.post_onboarding_done {
            return;
        }
        self.post_onboarding_done = true;
        let app = AppHandle::global().clone();
        std::thread::spawn(move || {
            if let Err(e) = crate::commands::initialize_enigo(app.clone()) {
                log::warn!("Failed to initialize: {e}");
            }
            if let Err(e) = crate::commands::initialize_shortcuts(app) {
                log::warn!("Failed to initialize: {e}");
            }
        });
        SettingsStore::global(cx).update(cx, |store, cx| {
            store.refresh_audio_devices(cx);
            store.refresh_output_devices(cx);
        });
    }

    pub fn select_section(
        &mut self,
        section: Section,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.section != section {
            self.section = section;
            self.page = pages::build(section, window, cx);
        }
        self.scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    pub fn preview_onboarding(
        &mut self,
        preview: OnboardingPreview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preview = Some(preview);
        match preview {
            OnboardingPreview::Accessibility => {
                self.accessibility_onboarding =
                    Some(cx.new(|cx| AccessibilityOnboarding::new(true, |_, _| {}, window, cx)));
            }
            OnboardingPreview::Model => {
                self.model_onboarding =
                    Some(cx.new(|cx| ModelOnboarding::new(true, |_, _| {}, window, cx)));
            }
        }
        cx.notify();
    }

    /// Dev snapshots: opens `page` (see `ui::dev_snapshot`).
    #[cfg(debug_assertions)]
    pub fn dev_navigate(&mut self, page: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (page, scroll) = page.split_once('@').unwrap_or((page, ""));
        let (name, detail) = page.split_once(':').unwrap_or((page, ""));
        self.preview = None;
        let section = match name {
            "onboarding" => {
                let preview = if detail == "model" {
                    OnboardingPreview::Model
                } else {
                    OnboardingPreview::Accessibility
                };
                self.preview_onboarding(preview, window, cx);
                return;
            }
            "stats" => Section::Stats,
            "customize" => Section::Customize,
            "models" => Section::Models,
            "debug" => Section::Debug,
            "settings" => Section::Settings,
            _ => Section::History,
        };
        self.select_section(section, window, cx);
        if !detail.is_empty() {
            pages::select_tab(&self.page, detail, window, cx);
        }
        if let Ok(offset) = scroll.parse::<f32>() {
            self.scroll.set_offset(gpui::point(px(0.), px(-offset)));
        }
    }

    fn exit_preview(&mut self, cx: &mut Context<Self>) {
        self.preview = None;
        if self.step == OnboardingStep::Done {
            self.accessibility_onboarding = None;
            self.model_onboarding = None;
        }
        cx.notify();
    }

    /// Shows a modal dialog over the window. The view closes it with
    /// [`close_dialog`].
    pub fn open_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        build: impl FnOnce(&mut Window, &mut App) -> AnyView,
    ) {
        self.previous_focus = window.focused(cx);
        self.dialog = Some(build(window, cx));
        cx.notify();
    }

    pub fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog = None;
        match self.previous_focus.take() {
            Some(focus) => focus.focus(window),
            None => self.focus.focus(window),
        }
        cx.notify();
    }

    fn handle_event(&mut self, event: &AppEvent, cx: &mut Context<Self>) {
        match event.name.as_str() {
            "recording-error" => {
                let error_type = event.payload.get("error_type").and_then(|v| v.as_str());
                let detail = event.payload.get("detail").and_then(|v| v.as_str());
                match error_type {
                    Some("microphone_permission_denied") => {
                        let platform = if cfg!(target_os = "macos") {
                            "macos"
                        } else if cfg!(target_os = "windows") {
                            "windows"
                        } else {
                            "linux"
                        };
                        let generic = t("errors.micPermissionDenied.generic");
                        let description =
                            t_or(&format!("errors.micPermissionDenied.{platform}"), &generic);
                        toast::error_with(t("errors.micPermissionDeniedTitle"), description, cx);
                    }
                    Some("no_input_device") => toast::error_with(
                        t("errors.noInputDeviceTitle"),
                        t("errors.noInputDevice"),
                        cx,
                    ),
                    _ => toast::error(
                        t_with(
                            "errors.recordingFailed",
                            &[("error", &detail.unwrap_or("Unknown error"))],
                        ),
                        cx,
                    ),
                }
            }
            "paste-error" => {
                toast::error_with(t("errors.pasteFailedTitle"), t("errors.pasteFailed"), cx)
            }
            "transcription-error" => {
                let detail = event.payload.as_str().unwrap_or_default().to_string();
                toast::error_with(t("errors.transcriptionFailedTitle"), detail, cx)
            }
            "model-state-changed" => {
                if let Some(state) = event.parse::<ModelStateEvent>() {
                    if state.event_type == "loading_failed" {
                        let model = state
                            .model_name
                            .map(SharedString::from)
                            .unwrap_or_else(|| t("errors.modelLoadFailedUnknown"));
                        toast::show(
                            toast::ToastKind::Error,
                            t_with("errors.modelLoadFailed", &[("model", &model)]),
                            state.error.map(Into::into),
                            None,
                            cx,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    fn toggle_debug_mode(&mut self, cx: &mut Context<Self>) {
        let store = SettingsStore::global(cx);
        store.update(cx, |store, cx| {
            let enabled = !store.settings.debug_mode;
            store.update(
                "debug_mode",
                |s| s.debug_mode = enabled,
                move |app| crate::shortcut::change_debug_mode_setting(app, enabled),
                cx,
            );
        });
    }

    fn render_app(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let sidebar = sidebar::render(self, window, cx);
        let rtl = crate::i18n::is_rtl();
        div()
            .size_full()
            .flex()
            .flex_col()
            .pt(px(TITLEBAR_HEIGHT))
            .cursor_default()
            .child(self.whats_new.clone())
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(rtl, |el| el.flex_row_reverse())
                    .child(sidebar)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .my_2()
                            .when(rtl, |el| el.ml_2())
                            .when(!rtl, |el| el.mr_2())
                            .rounded_2xl()
                            .border_1()
                            .border_color(p.mid_gray.a(0.15))
                            .bg(p.background)
                            .shadow(vec![shadow(0., 1., 3., 0., 0.04)])
                            .child(
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_h_0()
                                    .child(
                                        div()
                                            .id("content-scroll")
                                            .size_full()
                                            .overflow_y_scroll()
                                            .track_scroll(&self.scroll)
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .items_center()
                                                    .px_7()
                                                    .pt_7()
                                                    .pb_8()
                                                    .gap_5()
                                                    .child(
                                                        div()
                                                            .max_w(rems(48.))
                                                            .w_full()
                                                            .font(serif_font())
                                                            .text_size(px(28.))
                                                            .line_height(px(35.))
                                                            .child(t(self.section.label_key())),
                                                    )
                                                    .child(self.accessibility_banner.clone())
                                                    .child(self.secure_input_warning.clone())
                                                    .child(self.page.clone()),
                                            ),
                                    )
                                    .vertical_scrollbar(&self.scroll),
                            ),
                    ),
            )
    }
}

/// The page-title serif (`--font-serif`), with the CSS fallbacks.
fn serif_font() -> gpui::Font {
    gpui::Font {
        family: SERIF_FONT.into(),
        features: Default::default(),
        fallbacks: Some(gpui::FontFallbacks::from_fonts(
            SERIF_FALLBACKS.iter().map(|f| f.to_string()).collect(),
        )),
        weight: FontWeight::NORMAL,
        style: gpui::FontStyle::Normal,
    }
}

/// Decides where onboarding starts, like `checkOnboardingStatus` in App.tsx.
/// Returns the step and whether this is a returning user.
fn initial_onboarding_step(cx: &App) -> (OnboardingStep, bool) {
    let settings = SettingsStore::get(cx);
    if !settings.onboarding_completed {
        return (OnboardingStep::Accessibility, false);
    }

    #[cfg(target_os = "macos")]
    if !crate::ui::platform::accessibility_granted() || !crate::ui::platform::microphone_granted() {
        return (OnboardingStep::Accessibility, true);
    }

    #[cfg(target_os = "windows")]
    {
        let status = crate::commands::audio::get_windows_microphone_permission_status();
        if status.supported
            && status.overall_access == crate::commands::audio::PermissionAccess::Denied
        {
            return (OnboardingStep::Accessibility, true);
        }
    }

    (OnboardingStep::Done, true)
}

/// Closes the open dialog, if any.
pub fn close_dialog(window: &mut Window, cx: &mut App) {
    if let Some(main) = crate::ui::main_view(cx) {
        main.update(cx, |main, cx| main.close_dialog(window, cx));
    }
}

/// Opens `build`'s view as a modal dialog in the settings window.
pub fn open_dialog(
    window: &mut Window,
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut App) -> AnyView,
) {
    if let Some(main) = crate::ui::main_view(cx) {
        main.update(cx, |main, cx| main.open_dialog(window, cx, build));
    }
}

impl Focusable for MainWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let content = if let Some(preview) = self.preview {
            let view: AnyView = match preview {
                OnboardingPreview::Accessibility => self
                    .accessibility_onboarding
                    .clone()
                    .map(Into::into)
                    .unwrap_or_else(|| cx.new(|_| gpui::Empty).into()),
                OnboardingPreview::Model => self
                    .model_onboarding
                    .clone()
                    .map(Into::into)
                    .unwrap_or_else(|| cx.new(|_| gpui::Empty).into()),
            };
            div()
                .size_full()
                .relative()
                .child(view)
                .child(
                    div()
                        .id("exit-preview")
                        .absolute()
                        .top_4()
                        .inset_end(rems(1.))
                        .rounded_lg()
                        .border_1()
                        .border_color(p.mid_gray.a(0.2))
                        .bg(p.background)
                        .px_4()
                        .py_2()
                        .text_size(rems(0.875))
                        .font_weight(FontWeight::MEDIUM)
                        .shadow(crate::ui::theme::shadow_lg())
                        .hover(|s| s.bg(p.mid_gray.a(0.1)))
                        .cursor_pointer()
                        .keyboard_focus(cx)
                        .on_click(cx.listener(|this, _, _, cx| this.exit_preview(cx)))
                        .child(t("settings.debug.onboardingPreview.exitButton")),
                )
                .into_any_element()
        } else {
            match self.step {
                OnboardingStep::Accessibility => self
                    .accessibility_onboarding
                    .clone()
                    .map(|v| v.into_any_element())
                    .unwrap_or_else(|| div().into_any_element()),
                OnboardingStep::Model => self
                    .model_onboarding
                    .clone()
                    .map(|v| v.into_any_element())
                    .unwrap_or_else(|| div().into_any_element()),
                OnboardingStep::Done => self.render_app(window, cx).into_any_element(),
            }
        };

        div()
            .id("handy-main")
            .key_context("HandyMain")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .bg(p.shell)
            .text_color(p.text)
            .text_size(px(15.))
            .line_height(px(24.))
            // `dir="rtl"` makes start-aligned text right-aligned.
            .when(crate::i18n::is_rtl(), |el| el.text_right())
            .on_action(
                cx.listener(|this, _: &crate::ui::ToggleDebugMode, _, cx| {
                    this.toggle_debug_mode(cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::ui::OpenSettings, window, cx| {
                    // During an onboarding preview this picks the section
                    // shown once the preview closes, like App.tsx.
                    if this.step == OnboardingStep::Done {
                        this.select_section(Section::Settings, window, cx);
                    }
                }),
            )
            // `focus-visible`: rings show after keyboard input only.
            .capture_key_down(|_, window, cx| {
                crate::ui::components::set_keyboard_modality(true, window, cx)
            })
            .capture_any_mouse_down(|_, window, cx| {
                crate::ui::components::set_keyboard_modality(false, window, cx)
            })
            .on_action(|_: &crate::ui::Minimize, window, _| window.minimize_window())
            .on_action(|_: &crate::ui::CloseWindow, window, cx| {
                if crate::ui::hide_main_window(window, cx) {
                    window.remove_window();
                }
            })
            .when(cfg!(target_os = "macos"), |el| {
                // The title strip (`data-tauri-drag-region`): AppKit drags
                // the window from it, and a double click zooms or minimizes
                // per the system setting. It sits under the content so
                // controls drawn over it keep their clicks.
                el.child(
                    div()
                        .id("title-strip")
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(TITLEBAR_HEIGHT))
                        .on_click(|event, window, _| {
                            if event.click_count() == 2 {
                                window.titlebar_double_click();
                            }
                        }),
                )
            })
            .child(content)
            .children(self.dialog.clone())
            .child(crate::ui::toast::toasts(cx))
    }
}
