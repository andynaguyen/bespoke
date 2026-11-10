use std::path::PathBuf;

use gpui::{
    AnyElement, App, Context, FocusHandle, FontWeight, Global, SharedString, Window, div,
    prelude::*, px,
};

use crate::i18n::t;
use crate::triage::Handoff;
use crate::ui::components::{
    Button, ButtonVariant, Dialog, KeyboardFocus, RowDirection, Tooltip, Typography, icon,
};
use crate::ui::shell::close_dialog;
use crate::ui::theme::{Alpha, palette, radius};

const DIALOG_WIDTH: f32 = 540.;

const ATTACH_FINISHED_WITHIN_MS: u64 = 10 * 60 * 1000;

#[derive(Default)]
struct Draft {
    agent: Option<Handoff>,
    reopen_after_recording: bool,
}

impl Global for Draft {}

/// Opens Triage again when the capture that Record started finishes. Call once at startup.
pub fn init(cx: &mut App) {
    cx.subscribe(
        &crate::ui::event_bus::event_bus(cx),
        |_, event: &crate::app_handle::AppEvent, cx| {
            let finished = event.name == crate::annotate::CAPTURE_FINISHED;
            if !finished && event.name != crate::annotate::CAPTURE_CANCELLED {
                return;
            }
            let draft = cx.default_global::<Draft>();
            let reopen = std::mem::take(&mut draft.reopen_after_recording);
            let open = crate::ui::main_view(cx)
                .is_some_and(|main| main.read(cx).dialog::<TriageDialog>().is_some());
            if finished && reopen && !open {
                crate::ui::show_triage(cx);
            }
        },
    )
    .detach();
}

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

/// Opens Triage over the main window, or hands it the keys when it is already open. It does
/// nothing during onboarding. Call it outside the main window's own update, or defer it.
pub fn open(window: &mut Window, cx: &mut App) {
    let Some(main) = crate::ui::main_view(cx) else {
        return;
    };
    let main = main.read(cx);
    if !main.shows_app() {
        return;
    }
    if main.dialog::<TriageDialog>().is_some() {
        main.focus_dialog(cx);
        return;
    }
    crate::ui::shell::open_dialog(window, cx, |window, cx| {
        cx.new(|cx| TriageDialog::new(window, cx)).into()
    });
}

struct Attached {
    dir: PathBuf,
    label: SharedString,
}

#[derive(Clone, Copy)]
struct Agent {
    handoff: Handoff,
    available: bool,
}

struct Step {
    done: bool,
    content: AnyElement,
}

impl Step {
    fn new(content: impl IntoElement) -> Self {
        Self {
            done: false,
            content: content.into_any_element(),
        }
    }
}

pub struct TriageDialog {
    focus: FocusHandle,
    agents: Vec<Agent>,
    agent: Handoff,
    capture: Option<Attached>,
}

impl TriageDialog {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window);
        let agents: Vec<_> = Handoff::ALL
            .into_iter()
            .map(|handoff| Agent {
                handoff,
                available: handoff.available(),
            })
            .collect();
        let usable = |handoff: Handoff| {
            agents
                .iter()
                .any(|agent| agent.handoff == handoff && agent.available)
        };
        let agent = cx
            .default_global::<Draft>()
            .agent
            .filter(|handoff| usable(*handoff))
            .or_else(|| Handoff::ALL.into_iter().find(|handoff| usable(*handoff)))
            .unwrap_or(Handoff::Copy);
        let mut this = Self {
            focus,
            agents,
            agent,
            capture: None,
        };
        {
            cx.observe(&crate::ui::stores::SettingsStore::global(cx), |_, _, cx| {
                cx.notify()
            })
            .detach();
            this.attach_finished_captures(cx);
        }
        this
    }

    fn attach_finished_captures(&mut self, cx: &mut Context<Self>) {
        cx.subscribe(
            &crate::ui::event_bus::event_bus(cx),
            |this, _, event: &crate::app_handle::AppEvent, cx| {
                if event.name == crate::annotate::CAPTURE_FINISHED {
                    this.attach(crate::annotate::newest_capture());
                    cx.notify();
                }
            },
        )
        .detach();
        let since = now_ms().saturating_sub(ATTACH_FINISHED_WITHIN_MS);
        self.attach(
            crate::annotate::newest_capture()
                .filter(|capture| capture.created_at_ms + capture.duration_ms >= since),
        );
    }

    fn attach(&mut self, capture: Option<crate::annotate::handoff::Finished>) {
        if let Some(capture) = capture.filter(|capture| capture.dir.is_dir()) {
            self.capture = Some(Attached {
                label: capture_label(capture.created_at_ms, &capture.duration()),
                dir: capture.dir,
            });
        }
    }

    fn record(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = cx.default_global::<Draft>();
        draft.agent = Some(self.agent);
        draft.reopen_after_recording = true;
        close_dialog(window, cx);
        crate::annotate::coordinator::menu(crate::annotate::coordinator::MenuAction::ToggleCapture);
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let capture = self.capture.as_ref().map(|capture| capture.dir.clone());
        crate::triage::start(self.agent, capture, cx);
        close_dialog(window, cx);
    }

    fn show_step(&self, cx: &mut Context<Self>) -> Option<Step> {
        let p = palette(cx);
        let detail = match &self.capture {
            Some(capture) => div()
                .mt_2()
                .flex()
                .row_dir()
                .items_center()
                .gap_2()
                .pl(px(10.))
                .pr(px(4.))
                .py(px(4.))
                .rounded(radius::ROW)
                .bg(p.card)
                .child(
                    icon("scan-eye")
                        .size(px(16.))
                        .flex_none()
                        .text_color(p.done),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .sm_text()
                        .child(capture.label.clone()),
                )
                .child(
                    Button::new("triage-record-again")
                        .variant(ButtonVariant::Ghost)
                        .size(crate::ui::components::ButtonSize::Sm)
                        .on_click(cx.listener(|this, _, window, cx| this.record(window, cx)))
                        .child(t("triage.recordAgain")),
                )
                .child(
                    div()
                        .id("triage-remove-capture")
                        .flex_none()
                        .p_1()
                        .rounded_lg()
                        .cursor_pointer()
                        .keyboard_focus_ring(0.5, cx)
                        .text_color(p.mid_gray)
                        .hover(|s| s.bg(p.mid_gray.a(0.1)).text_color(p.text))
                        .tooltip(|window, cx| Tooltip::text(t("triage.removeCapture"), window, cx))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.capture = None;
                            cx.notify();
                        }))
                        .child(icon("x").size(px(14.))),
                )
                .into_any_element(),
            None => div().mt_1().child(show_hint(cx)).into_any_element(),
        };
        Some(Step {
            done: self.capture.is_some(),
            ..Step::new(
                div()
                    .flex()
                    .flex_col()
                    .child(step_title("triage.showTitle"))
                    .child(detail),
            )
        })
    }

    fn agent_step(&self, cx: &mut Context<Self>) -> Step {
        let p = palette(cx);
        let chips = self.agents.iter().map(|&Agent { handoff, available }| {
            let selected = self.agent == handoff;
            let key = agent_key(handoff);
            div()
                .id(SharedString::from(format!("triage-agent-{}", handoff.id())))
                .flex()
                .row_dir()
                .items_center()
                .gap(px(6.))
                .h(px(28.))
                .px(px(10.))
                .rounded(px(8.))
                .border_1()
                .text_px(13.)
                .when(selected, |el| {
                    el.border_color(p.accent)
                        .bg(p.accent.a(if p.is_dark { 0.2 } else { 0.1 }))
                        .font_weight(FontWeight::MEDIUM)
                })
                .when(!selected, |el| el.border_color(p.line).bg(p.card))
                .when(!available, |el| {
                    el.opacity(0.45)
                        .cursor_not_allowed()
                        .tooltip(move |window, cx| {
                            Tooltip::text(t(&format!("triage.{key}Missing")), window, cx)
                        })
                })
                .when(available, |el| {
                    el.cursor_pointer()
                        .keyboard_focus(cx)
                        .when(!selected, |el| el.hover(|s| s.bg(p.text.a(0.08))))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.agent = handoff;
                            cx.notify();
                        }))
                })
                .child(icon(agent_icon(handoff)).size(px(15.)).flex_none())
                .child(t(&format!("triage.{key}")))
        });
        let content = div()
            .flex()
            .flex_col()
            .child(step_title("triage.agentTitle"))
            .child(
                div()
                    .mt_2()
                    .flex()
                    .row_dir()
                    .flex_wrap()
                    .gap(px(6.))
                    .children(chips),
            )
            .child(
                div()
                    .mt(px(6.))
                    .sm_text()
                    .text_color(p.mid_gray)
                    .child(t(&format!("triage.{}Description", agent_key(self.agent)))),
            );
        Step::new(content)
    }
}

fn send_step(cx: &App) -> Step {
    let p = palette(cx);
    let content = div()
        .flex()
        .flex_col()
        .child(step_title("triage.sendTitle"))
        .child(
            div()
                .mt_1()
                .sm_text()
                .leading_snug()
                .text_color(p.mid_gray)
                .child(t("triage.sendDescription")),
        );
    Step::new(content)
}

fn step_title(key: &str) -> impl IntoElement {
    div()
        .text_px(13.)
        .font_weight(FontWeight::SEMIBOLD)
        .child(t(key))
}

fn step(step: Step, number: usize, last: bool, cx: &App) -> impl IntoElement {
    let p = palette(cx);
    let badge = div()
        .size(px(22.))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .text_px(11.5)
        .font_weight(FontWeight::BOLD)
        .when(step.done, |el| {
            el.bg(p.done)
                .text_color(p.accent_contrast)
                .child(icon("check").size(px(13.)))
        })
        .when(!step.done, |el| {
            el.bg(p.text.a(0.1))
                .text_color(p.text)
                .child(number.to_string())
        });
    div()
        .flex()
        .row_dir()
        .gap_3()
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .child(badge)
                .when(!last, |el| {
                    el.child(div().flex_1().w(px(1.5)).my_1().bg(p.line))
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .pt(px(2.))
                .when(!last, |el| el.pb(px(18.)))
                .child(step.content),
        )
}

impl Render for TriageDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut steps = Vec::new();
        steps.extend(self.show_step(cx));
        steps.push(self.agent_step(cx));
        steps.push(send_step(cx));
        let count = steps.len();
        let steps = steps
            .into_iter()
            .enumerate()
            .map(|(index, each)| step(each, index + 1, index + 1 == count, cx));
        Dialog::new(t("triage.title"), self.focus.clone(), close_dialog)
            .width(px(DIALOG_WIDTH))
            .footer(
                Button::new("triage-cancel")
                    .variant(ButtonVariant::Secondary)
                    .on_click(|_, window, cx| close_dialog(window, cx))
                    .child(t("triage.cancel")),
            )
            .footer(
                Button::new("triage-start")
                    .on_click(cx.listener(|this, _, window, cx| this.start(window, cx)))
                    .child(t("triage.start")),
            )
            .child(div().pt_1().flex().flex_col().children(steps))
    }
}

fn agent_key(handoff: Handoff) -> &'static str {
    match handoff {
        Handoff::Claude => "claude",
        Handoff::ClaudeCode => "claudeCode",
        Handoff::Codex => "codex",
        Handoff::Copy => "copy",
    }
}

fn agent_icon(handoff: Handoff) -> &'static str {
    match handoff {
        Handoff::Claude => "nav-bot",
        Handoff::ClaudeCode => "terminal",
        Handoff::Codex => "code",
        Handoff::Copy => "copy",
    }
}

fn show_hint(cx: &App) -> impl IntoElement {
    let p = palette(cx);
    let hint = t("triage.showHint");
    let line = div()
        .flex()
        .row_dir()
        .items_center()
        .gap(px(5.))
        .sm_text()
        .text_color(p.mid_gray);
    let Some((before, after)) = hint.split_once("{{shortcut}}") else {
        return line.child(hint);
    };
    let shortcut = super::annotate::shortcut_label(
        crate::ui::stores::SettingsStore::get(cx),
        crate::annotate::RECORD_BINDING,
        crate::annotate::capture::ShortcutKind::Record,
    );
    let (before, after) = (before.trim_end(), after.trim_start());
    line.when(!before.is_empty(), |el| {
        el.child(SharedString::from(before.to_owned()))
    })
    .child(crate::ui::clay::keycap(
        shortcut, 18., p.clay, p.text, p.is_dark,
    ))
    .when(!after.is_empty(), |el| {
        el.child(SharedString::from(after.to_owned()))
    })
}

fn capture_label(created_at_ms: u64, duration: &str) -> SharedString {
    use chrono::TimeZone;
    let time = chrono::Local
        .timestamp_millis_opt(created_at_ms as i64)
        .single()
        .map(crate::ui::dates::time_label)
        .unwrap_or_default();
    crate::i18n::t_with(
        "triage.captured",
        &[("time", &time), ("duration", &duration)],
    )
}
