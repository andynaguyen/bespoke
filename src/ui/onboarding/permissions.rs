//! The permission step (`AccessibilityOnboarding.tsx`). macOS asks for the
//! microphone and Accessibility, Windows for the microphone only, and other
//! platforms skip the step.

use std::rc::Rc;
use std::time::Duration;

use gpui::{div, prelude::*, rems, App, Context, SharedString, Task, Window};

use super::top_padding;
use crate::app_handle::AppHandle;
use crate::i18n::t;
use crate::ui::components::{icon, KeyboardFocus, RowDirection, Spinner, Typography};
use crate::ui::shell::logo;
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{palette, tw, Alpha};
use crate::ui::WindowCallback;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Platform {
    MacOs,
    Windows,
    Other,
}

const PLATFORM: Platform = if cfg!(target_os = "macos") {
    Platform::MacOs
} else if cfg!(target_os = "windows") {
    Platform::Windows
} else {
    Platform::Other
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Needed,
    Waiting,
    Granted,
}

fn accessibility_granted() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::ui::platform::accessibility_granted()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

fn microphone_granted() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::ui::platform::microphone_granted()
    }
    #[cfg(target_os = "windows")]
    {
        let status = crate::commands::audio::get_windows_microphone_permission_status();
        !status.supported
            || status.overall_access != crate::commands::audio::PermissionAccess::Denied
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        true
    }
}

/// Starts the paste engine and shortcuts once Accessibility is granted.
fn initialize_input() {
    let app = AppHandle::global().clone();
    std::thread::spawn(move || {
        if let Err(e) = crate::commands::initialize_enigo(app.clone()) {
            log::warn!("Failed to initialize after permission grant: {e}");
        }
        if let Err(e) = crate::commands::initialize_shortcuts(app) {
            log::warn!("Failed to initialize after permission grant: {e}");
        }
    });
}

pub struct AccessibilityOnboarding {
    preview: bool,
    accessibility: Status,
    microphone: Status,
    on_complete: WindowCallback,
    completing: bool,
    polling: Option<Task<()>>,
}

impl AccessibilityOnboarding {
    pub fn new(
        preview: bool,
        on_complete: impl Fn(&mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            preview,
            accessibility: Status::Needed,
            microphone: Status::Needed,
            on_complete: Rc::new(on_complete),
            completing: false,
            polling: None,
        };
        // Previews show the request screen without checking or changing
        // operating-system permissions.
        if preview {
            if PLATFORM != Platform::MacOs {
                this.accessibility = Status::Granted;
            }
            if PLATFORM == Platform::Other {
                this.microphone = Status::Granted;
            }
            return this;
        }
        match PLATFORM {
            Platform::Other => {
                this.accessibility = Status::Granted;
                this.microphone = Status::Granted;
            }
            Platform::Windows => {
                this.accessibility = Status::Granted;
                if microphone_granted() {
                    this.microphone = Status::Granted;
                }
            }
            Platform::MacOs => {
                if accessibility_granted() {
                    this.accessibility = Status::Granted;
                    initialize_input();
                }
                if microphone_granted() {
                    this.microphone = Status::Granted;
                }
            }
        }
        if this.all_granted() {
            this.complete(window, cx);
        }
        this
    }

    fn all_granted(&self) -> bool {
        self.accessibility == Status::Granted && self.microphone == Status::Granted
    }

    /// Refreshes the device lists, then moves on after the "All set!" screen
    /// has shown for 300ms.
    fn complete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.completing {
            return;
        }
        self.completing = true;
        SettingsStore::global(cx).update(cx, |store, cx| {
            store.refresh_audio_devices(cx);
            store.refresh_output_devices(cx);
        });
        let on_complete = self.on_complete.clone();
        cx.spawn_in(window, async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            let _ = cx.update(|window, cx| on_complete(window, cx));
        })
        .detach();
    }

    /// Checks every second until both permissions are granted.
    fn start_polling(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.polling.is_some() {
            return;
        }
        self.polling = Some(cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            match this.update_in(cx, |this, window, cx| this.poll(window, cx)) {
                Ok(false) => {}
                _ => break,
            }
        }));
    }

    /// One polling tick. Returns true once polling can stop.
    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if PLATFORM == Platform::MacOs
            && self.accessibility != Status::Granted
            && accessibility_granted()
        {
            self.accessibility = Status::Granted;
            initialize_input();
        }
        if self.microphone != Status::Granted && microphone_granted() {
            self.microphone = Status::Granted;
        }
        cx.notify();
        if self.all_granted() {
            self.complete(window, cx);
            return true;
        }
        false
    }

    fn grant_accessibility(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preview {
            return;
        }
        #[cfg(target_os = "macos")]
        crate::ui::platform::request_accessibility();
        self.accessibility = Status::Waiting;
        self.start_polling(window, cx);
        cx.notify();
    }

    fn grant_microphone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preview {
            return;
        }
        #[cfg(target_os = "windows")]
        {
            if let Err(e) = crate::commands::audio::open_microphone_privacy_settings() {
                log::error!("Failed to request microphone permission: {e}");
                crate::ui::toast::error(t("onboarding.permissions.errors.requestFailed"), cx);
                return;
            }
        }
        #[cfg(target_os = "macos")]
        crate::ui::platform::request_microphone();
        self.microphone = Status::Waiting;
        self.start_polling(window, cx);
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    fn render_card(
        &self,
        id: &'static str,
        glyph: &'static str,
        title: SharedString,
        description: SharedString,
        status: Status,
        button_label: SharedString,
        on_grant: fn(&mut Self, &mut Window, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = palette(cx);
        let action = match status {
            Status::Granted => div()
                .flex()
                .row_dir()
                .items_center()
                .gap_2()
                .text_color(tw::emerald_400())
                .sm_text()
                .child(icon("check").size(rems(1.)))
                .child(t("onboarding.permissions.granted"))
                .into_any_element(),
            Status::Waiting => div()
                .flex()
                .row_dir()
                .items_center()
                .gap_2()
                .text_color(p.text.a(0.5))
                .sm_text()
                .child(Spinner::loader(rems(1.), p.text.a(0.5)))
                .child(t("onboarding.permissions.waiting"))
                .into_any_element(),
            Status::Needed => div()
                .id(id)
                .flex()
                .row_dir()
                .px_4()
                .py_2()
                .rounded_lg()
                .bg(p.accent)
                .hover(|s| s.bg(p.accent.a(0.9)))
                .text_color(p.accent_contrast)
                .sm_text()
                .font_weight(gpui::FontWeight::MEDIUM)
                .cursor_pointer()
                .keyboard_focus(cx)
                .on_click(cx.listener(move |this, _, window, cx| on_grant(this, window, cx)))
                .child(button_label)
                .into_any_element(),
        };
        div()
            .w_full()
            .p_4()
            .rounded_lg()
            .bg(gpui::white().a(0.05))
            .border_1()
            .border_color(p.mid_gray.a(0.2))
            .child(
                div()
                    .flex()
                    .row_dir()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .p_3()
                            .rounded_full()
                            .bg(p.mid_gray.a(0.15))
                            .flex_shrink_0()
                            .text_color(p.accent)
                            .child(icon(glyph).size(rems(1.5))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .items_start()
                            .child(div().font_weight(gpui::FontWeight::MEDIUM).child(title))
                            .child(
                                div()
                                    .sm_text()
                                    .text_color(p.text.a(0.6))
                                    .mb_3()
                                    .child(description),
                            )
                            .child(action),
                    ),
            )
    }
}

impl Render for AccessibilityOnboarding {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        if self.all_granted() {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_4()
                .child(
                    div()
                        .p_4()
                        .rounded_full()
                        .bg(tw::emerald_500().a(0.2))
                        .text_color(tw::emerald_400())
                        .child(icon("check").size(rems(3.))),
                )
                .child(
                    div()
                        .lg_text()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(t("onboarding.permissions.allGranted")),
                )
                .into_any_element();
        }

        let microphone_label = if PLATFORM == Platform::Windows {
            t("accessibility.openSettings")
        } else {
            t("onboarding.permissions.grant")
        };
        div()
            .id("permissions-onboarding")
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .min_h_full()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_6()
                    .p_6()
                    .pt(top_padding())
                    .child(logo(200., cx))
                    .child(
                        div()
                            .max_w(rems(28.))
                            .w_full()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_4()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .text_center()
                                    .mb_2()
                                    .child(
                                        div()
                                            .xl_text()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .mb_2()
                                            .child(t("onboarding.permissions.title")),
                                    )
                                    .child(
                                        div()
                                            .text_color(p.text.a(0.7))
                                            .child(t("onboarding.permissions.description")),
                                    ),
                            )
                            .when(PLATFORM != Platform::Other, |el| {
                                el.child(self.render_card(
                                    "grant-microphone",
                                    "mic",
                                    t("onboarding.permissions.microphone.title"),
                                    t("onboarding.permissions.microphone.description"),
                                    self.microphone,
                                    microphone_label,
                                    Self::grant_microphone,
                                    cx,
                                ))
                            })
                            .when(PLATFORM == Platform::MacOs, |el| {
                                el.child(self.render_card(
                                    "grant-accessibility",
                                    "keyboard",
                                    t("onboarding.permissions.accessibility.title"),
                                    t("onboarding.permissions.accessibility.description"),
                                    self.accessibility,
                                    t("onboarding.permissions.grant"),
                                    Self::grant_accessibility,
                                    cx,
                                ))
                            }),
                    ),
            )
            .into_any_element()
    }
}
