//! The Debug page (`debug/DebugSettings.tsx`), shown in debug mode: log
//! level, previews, timing knobs, diagnostics and the live log viewer.

use std::time::Duration;

use gpui::{
    div, prelude::*, px, rems, AnimationExt, App, Context, FontWeight, Hsla, ScrollHandle, Task,
    Window,
};

use crate::app_handle::{AppEvent, AppHandle};
use crate::i18n::{t, t_with};
use crate::logging::{LogEventPayload, LOG_EVENT};
use crate::secure_input::KeyboardDiagnosticReport;
use crate::ui::components::{
    pulse_animation, pulse_opacity, Button, ButtonSize, ButtonVariant, KeyboardFocus, RowDirection,
    SettingContainer, SettingLayout, SettingsGroup, Typography,
};
use crate::ui::event_bus::event_bus;
use crate::ui::onboarding::OnboardingPreview;
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{palette, tw, Alpha};

use super::rows;

const MAX_LINES: usize = 1000;
const FLUSH_INTERVAL: Duration = Duration::from_millis(250);

struct LogLine {
    level: u8,
    time: String,
    message: String,
}

fn level_tag(level: u8) -> &'static str {
    match level {
        1 => "TRACE",
        2 => "DEBUG",
        3 => "INFO",
        4 => "WARN",
        5 => "ERROR",
        _ => "LOG",
    }
}

pub struct DebugPage {
    is_laptop: bool,
    diagnostic_running: bool,
    diagnostic: Option<Result<KeyboardDiagnosticReport, String>>,
    logs: Vec<LogLine>,
    pending: Vec<LogLine>,
    paused: bool,
    copied: bool,
    pinned: bool,
    log_scroll: ScrollHandle,
    _flush: Task<()>,
}

impl DebugPage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.observe(&SettingsStore::global(cx), |_, _, cx| cx.notify())
            .detach();
        cx.subscribe(&event_bus(cx), |this, _, event: &AppEvent, _| {
            if event.name != LOG_EVENT {
                return;
            }
            if let Some(payload) = event.parse::<LogEventPayload>() {
                this.pending.push(LogLine {
                    level: payload.level,
                    time: chrono::Local::now().format("%H:%M:%S").to_string(),
                    message: payload.message,
                });
                if this.pending.len() > MAX_LINES {
                    let excess = this.pending.len() - MAX_LINES;
                    this.pending.drain(..excess);
                }
            }
        })
        .detach();

        // Batch incoming lines so a burst of logging never re-renders per line.
        let flush = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(FLUSH_INTERVAL).await;
            let alive = this
                .update(cx, |this, cx| {
                    if this.paused || this.pending.is_empty() {
                        return;
                    }
                    let pinned = this.pinned;
                    this.logs.append(&mut this.pending);
                    if this.logs.len() > MAX_LINES {
                        let excess = this.logs.len() - MAX_LINES;
                        this.logs.drain(..excess);
                    }
                    if pinned {
                        this.log_scroll.scroll_to_bottom();
                    }
                    cx.notify();
                })
                .is_ok();
            if !alive {
                break;
            }
        });

        Self {
            is_laptop: crate::helpers::clamshell::is_laptop().unwrap_or(false),
            diagnostic_running: false,
            diagnostic: None,
            logs: Vec::new(),
            pending: Vec::new(),
            paused: false,
            copied: false,
            pinned: true,
            log_scroll: ScrollHandle::new(),
            _flush: flush,
        }
    }

    fn run_diagnostic(&mut self, cx: &mut Context<Self>) {
        self.diagnostic_running = true;
        self.diagnostic = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let report =
                crate::runtime::run(crate::secure_input::run_keyboard_diagnostic(Some(10))).await;
            let _ = this.update(cx, |this, cx| {
                this.diagnostic_running = false;
                this.diagnostic = Some(report);
                cx.notify();
            });
        })
        .detach();
    }

    fn copy_logs(&mut self, cx: &mut Context<Self>) {
        let text = self
            .logs
            .iter()
            .map(|l| format!("{} {} {}", l.time, level_tag(l.level), l.message))
            .collect::<Vec<_>>()
            .join("\n");
        if AppHandle::global().clipboard().write_text(text).is_err() {
            log::error!("Failed to copy logs");
            return;
        }
        self.copied = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn preview_whats_new(window: &mut Window, cx: &mut App) {
        match crate::ui::whats_new::latest_release_note() {
            Some(note) => crate::ui::whats_new::open_modal(note, false, window, cx),
            None => crate::ui::toast::info(t("settings.debug.whatsNewPreview.noNotes"), cx),
        }
    }

    fn render_diagnostic(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let report = self.diagnostic.as_ref().map(|result| match result {
            Err(error) => div()
                .sm_text()
                .text_color(tw::red_500())
                .child(t_with(
                    "settings.debug.keyboardDiagnostic.failed",
                    &[("error", error)],
                ))
                .into_any_element(),
            Ok(report) => {
                let holder = if report.secure_input_enabled {
                    let who = match (&report.culprit_name, report.culprit_pid) {
                        (Some(name), Some(pid)) => t_with(
                            "settings.debug.keyboardDiagnostic.holder",
                            &[("name", name), ("pid", &pid)],
                        ),
                        _ => t("settings.debug.keyboardDiagnostic.holderUnknown"),
                    };
                    format!(
                        "{} — {}",
                        t("settings.debug.keyboardDiagnostic.enabled"),
                        who
                    )
                } else {
                    t("settings.debug.keyboardDiagnostic.disabled").to_string()
                };
                let verdict = if report.secure_input_enabled && report.key_down == 0 {
                    "verdictBlocked"
                } else if report.key_down == 0 && report.flags_changed > 0 {
                    "verdictSuspicious"
                } else if report.key_down == 0 && report.flags_changed == 0 && report.mouse == 0 {
                    "verdictNoEvents"
                } else {
                    "verdictOk"
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .sm_text()
                    .child(format!(
                        "{}: {}",
                        t("settings.debug.keyboardDiagnostic.secureInputLabel"),
                        holder
                    ))
                    .child(format!(
                        "{}: {} · {}: {} · {}: {} · {}: {}",
                        t("settings.debug.keyboardDiagnostic.keyDown"),
                        report.key_down,
                        t("settings.debug.keyboardDiagnostic.keyUp"),
                        report.key_up,
                        t("settings.debug.keyboardDiagnostic.flagsChanged"),
                        report.flags_changed,
                        t("settings.debug.keyboardDiagnostic.mouse"),
                        report.mouse
                    ))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(t(&format!("settings.debug.keyboardDiagnostic.{verdict}"))),
                    )
                    .into_any_element()
            }
        });

        div()
            .p_4()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .row_dir()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .sm_text()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(t("settings.debug.keyboardDiagnostic.title")),
                            )
                            .child(
                                div()
                                    .xs_text()
                                    .text_color(p.mid_gray)
                                    .child(t("settings.debug.keyboardDiagnostic.description")),
                            ),
                    )
                    .child(
                        div()
                            .id("keyboard-diagnostic")
                            .px_2()
                            .py_1()
                            .sm_text()
                            .font_weight(FontWeight::SEMIBOLD)
                            .bg(p.background)
                            .border_1()
                            .border_color(p.mid_gray.a(0.25))
                            .rounded_sm()
                            .whitespace_nowrap()
                            .when(self.diagnostic_running, |el| el.opacity(0.5))
                            .when(!self.diagnostic_running, |el| {
                                el.cursor_pointer()
                                    .hover(|s| {
                                        s.bg(p.mid_gray.a(0.1)).border_color(p.mid_gray.a(0.5))
                                    })
                                    .keyboard_focus(cx)
                                    .on_click(cx.listener(|this, _, _, cx| this.run_diagnostic(cx)))
                            })
                            .child(t("settings.debug.keyboardDiagnostic.run")),
                    ),
            )
            .when(self.diagnostic_running, |el| {
                el.child(
                    div()
                        .sm_text()
                        .child(t("settings.debug.keyboardDiagnostic.running"))
                        .with_animation("diagnostic-pulse", pulse_animation(), |el, delta| {
                            el.opacity(pulse_opacity(delta))
                        }),
                )
            })
            .children(report)
    }

    fn render_logs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let dark = p.is_dark;
        let level_colors = |level: u8| -> (Hsla, Hsla) {
            match level {
                1 => (p.mid_gray, p.mid_gray),
                2 => (
                    if dark { tw::sky_400() } else { tw::sky_600() },
                    p.text.a(0.8),
                ),
                3 => (
                    if dark {
                        tw::emerald_400()
                    } else {
                        tw::emerald_600()
                    },
                    p.text,
                ),
                4 => (
                    if dark {
                        tw::amber_400()
                    } else {
                        tw::amber_600()
                    },
                    if dark {
                        tw::amber_300()
                    } else {
                        tw::amber_700()
                    },
                ),
                5 => (
                    if dark { tw::red_400() } else { tw::red_600() },
                    if dark { tw::red_300() } else { tw::red_700() },
                ),
                _ => (p.mid_gray, p.text),
            }
        };
        let empty = self.logs.is_empty();
        let dot = div().size(rems(0.5)).rounded_full().flex_shrink_0();
        let dot = if self.paused {
            dot.bg(p.mid_gray).into_any_element()
        } else {
            dot.bg(tw::emerald_500())
                .with_animation("live-dot", pulse_animation(), |el, delta| {
                    el.opacity(pulse_opacity(delta))
                })
                .into_any_element()
        };

        let lines = self.logs.iter().map(|line| {
            let (tag_color, message_color) = level_colors(line.level);
            div()
                .flex()
                .row_dir()
                .gap_2()
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(p.mid_gray.a(0.8))
                        .child(line.time.clone()),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .w(rems(3.5))
                        .text_color(tag_color)
                        .child(level_tag(line.level)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(message_color)
                        .child(line.message.clone()),
                )
        });

        SettingContainer::new(t("settings.debug.liveLogs.title"))
            .description(t("settings.debug.liveLogs.description"))
            .layout(SettingLayout::Stacked)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .row_dir()
                    .mb_2()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .row_dir()
                            .items_center()
                            .gap_2()
                            .xs_text()
                            .text_color(p.mid_gray)
                            .min_w_0()
                            .child(dot)
                            .child(if self.paused {
                                t("settings.debug.liveLogs.paused")
                            } else {
                                t("settings.debug.liveLogs.live")
                            })
                            .child("·")
                            .child(div().truncate().child(t_with(
                                "settings.debug.liveLogs.lineCount",
                                &[("count", &self.logs.len())],
                            ))),
                    )
                    .child(
                        div()
                            .flex()
                            .row_dir()
                            .items_center()
                            .gap_2()
                            .flex_shrink_0()
                            .child(
                                Button::new("logs-pause")
                                    .variant(ButtonVariant::Secondary)
                                    .size(ButtonSize::Sm)
                                    .child(if self.paused {
                                        t("settings.debug.liveLogs.resume")
                                    } else {
                                        t("settings.debug.liveLogs.pause")
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.paused = !this.paused;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("logs-copy")
                                    .variant(ButtonVariant::Secondary)
                                    .size(ButtonSize::Sm)
                                    .disabled(empty)
                                    .child(if self.copied {
                                        t("settings.debug.liveLogs.copied")
                                    } else {
                                        t("common.copy")
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| this.copy_logs(cx))),
                            )
                            .child(
                                Button::new("logs-clear")
                                    .variant(ButtonVariant::Secondary)
                                    .size(ButtonSize::Sm)
                                    .disabled(empty)
                                    .child(t("common.clear"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.pending.clear();
                                        this.logs.clear();
                                        this.pinned = true;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .id("live-logs")
                    .h(rems(18.))
                    .overflow_y_scroll()
                    .track_scroll(&self.log_scroll)
                    .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                        let max = this.log_scroll.max_offset().height;
                        let offset = -this.log_scroll.offset().y;
                        this.pinned = max - offset < px(24.);
                        cx.notify();
                    }))
                    .rounded_lg()
                    .border_1()
                    .border_color(p.mid_gray.a(0.3))
                    .bg(p.log_surface)
                    .p_3()
                    .font_family(crate::ui::theme::MONO_FONT)
                    .xs_text()
                    .leading_relaxed()
                    .when(empty, |el| {
                        el.child(
                            div()
                                .text_color(p.mid_gray)
                                .child(t("settings.debug.liveLogs.empty")),
                        )
                    })
                    .children(lines),
            )
    }
}

impl super::Page for DebugPage {}

impl Render for DebugPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = SettingsStore::get(cx).clone();
        let mut group = SettingsGroup::new()
            .child(rows::log_level(&s, cx))
            .child(
                SettingContainer::new(t("settings.debug.whatsNewPreview.title"))
                    .description(t("settings.debug.whatsNewPreview.description"))
                    .child(
                        Button::new("preview-whats-new")
                            .variant(ButtonVariant::Secondary)
                            .child(t("settings.debug.whatsNewPreview.button"))
                            .on_click(|_, window, cx| Self::preview_whats_new(window, cx)),
                    ),
            )
            .child(
                SettingContainer::new(t("settings.debug.onboardingPreview.title"))
                    .description(t("settings.debug.onboardingPreview.description"))
                    .child(
                        div()
                            .flex()
                            .row_dir()
                            .gap_2()
                            .child(
                                Button::new("preview-permissions")
                                    .variant(ButtonVariant::Secondary)
                                    .child(t("settings.debug.onboardingPreview.permissionsButton"))
                                    .on_click(|_, window, cx| {
                                        preview_onboarding(
                                            OnboardingPreview::Accessibility,
                                            window,
                                            cx,
                                        )
                                    }),
                            )
                            .child(
                                Button::new("preview-models")
                                    .variant(ButtonVariant::Secondary)
                                    .child(t("settings.debug.onboardingPreview.modelsButton"))
                                    .on_click(|_, window, cx| {
                                        preview_onboarding(OnboardingPreview::Model, window, cx)
                                    }),
                            ),
                    ),
            )
            .child(rows::sound_theme(&s, cx))
            .child(rows::word_correction_threshold(&s))
            .child(rows::paste_delay(&s))
            .child(rows::paste_delay_after(&s));
        if cfg!(any(target_os = "macos", target_os = "windows")) {
            group = group.child(rows::reliable_paste(&s, cx));
        }
        group = group
            .child(rows::hold_threshold(&s))
            .child(rows::recording_buffer(&s))
            .child(rows::always_on_microphone(&s, cx));
        if self.is_laptop {
            group = group.child(rows::clamshell_microphone(&s, cx));
        }
        if cfg!(target_os = "macos") {
            group = group.child(self.render_diagnostic(cx));
        }
        group = group.child(self.render_logs(cx));

        div().w_full().flex().flex_col().gap_6().child(group)
    }
}

fn preview_onboarding(preview: OnboardingPreview, window: &mut Window, cx: &mut App) {
    if let Some(main) = crate::ui::main_view(cx) {
        main.update(cx, |main, cx| main.preview_onboarding(preview, window, cx));
    }
}
