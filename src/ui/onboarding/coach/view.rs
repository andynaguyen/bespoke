use gpui::{
    AnyElement, App, Context, FontWeight, ObjectFit, SharedString, Window, div, img, prelude::*,
    px, rems,
};

use super::{KeycapCoach, Review, binding, scratch_keyword, scratch_on};
use crate::i18n::{t, t_with};
use crate::ui::clay::Clay;
use crate::ui::components::{Button, ButtonVariant, RowDirection, Spinner, Typography, icon};
use crate::ui::onboarding::top_padding;
use crate::ui::onboarding::tour::{self, Attempt, Stage};
use crate::ui::theme::{Alpha, palette};

const WRONG_TOTAL: &str = "$60.00";
const INK_READABLE_BACKGROUND: u32 = 0xf4f4f6;

impl KeycapCoach {
    fn key_is_lit(&self, label: &str, all: bool) -> bool {
        all || tour::modifier_mask(label).is_some_and(|mask| self.flags & mask != 0)
    }

    fn keys(&self, labels: &[String], height: f32, all: bool, cx: &App) -> AnyElement {
        let p = palette(cx);
        let cap = |label: &String| {
            let lit = self.key_is_lit(label, all);
            let color = if lit { p.accent } else { p.clay };
            let text = if lit { p.accent_contrast } else { p.text };
            let wide = label.chars().count() > 2;
            div()
                .flex_none()
                .h(px(height))
                .when(wide, |el| el.w(px(height * 3.1)))
                .when(!wide, |el| el.w(px(height)))
                .mt(px(if lit { 3. } else { 0. }))
                .clay(color, height, px(height * 0.24), p.is_dark)
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(height * if wide { 0.3 } else { 0.42 }))
                .font_weight(FontWeight::MEDIUM)
                .text_color(text)
                .whitespace_nowrap()
                .child(SharedString::from(label.clone()))
        };
        div()
            .h(px(height + 3.))
            .flex()
            .row_dir()
            .items_start()
            .justify_center()
            .gap(px(height * 0.13))
            .children(labels.iter().map(cap))
            .into_any_element()
    }

    fn hero(&self, glyph: &'static str, cx: &App) -> AnyElement {
        let p = palette(cx);
        div()
            .size(px(92.))
            .flex_none()
            .clay(p.clay, 92., px(26.), p.is_dark)
            .flex()
            .items_center()
            .justify_center()
            .text_color(p.text)
            .child(icon(glyph).size(px(40.)))
            .into_any_element()
    }

    fn headline(&self, title: SharedString, body: SharedString, cx: &App) -> AnyElement {
        let p = palette(cx);
        div()
            .max_w(rems(32.))
            .flex()
            .flex_col()
            .items_center()
            .text_center()
            .gap_2()
            .child(
                div()
                    .text_size(px(24.))
                    .line_height(px(30.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .text_color(p.text.a(0.65))
                    .child(body),
            )
            .into_any_element()
    }

    fn waiting_line(&self, label: SharedString, cx: &App) -> AnyElement {
        let p = palette(cx);
        div()
            .flex()
            .row_dir()
            .items_center()
            .gap_2()
            .sm_text()
            .text_color(p.text.a(0.6))
            .child(Spinner::loader(rems(1.), p.text.a(0.6)))
            .child(label)
            .into_any_element()
    }

    fn column(&self) -> gpui::Div {
        div().flex().flex_col().items_center().gap_5()
    }

    fn microphone(&self, cx: &mut Context<Self>) -> AnyElement {
        let (body, label) = if self.microphone_denied {
            (
                t("onboarding.coach.microphone.denied"),
                t("onboarding.coach.openSettings"),
            )
        } else {
            (
                t("onboarding.coach.microphone.description"),
                t("onboarding.coach.microphone.button"),
            )
        };
        self.column()
            .child(self.hero("mic", cx))
            .child(self.headline(t("onboarding.coach.microphone.title"), body, cx))
            .child(
                Button::new("coach-microphone")
                    .child(label)
                    .on_click(cx.listener(|this, _, _, cx| this.ask_microphone(cx))),
            )
            .into_any_element()
    }

    fn accessibility(&self, cx: &mut Context<Self>) -> AnyElement {
        self.column()
            .child(self.hero("keyboard", cx))
            .child(self.headline(
                t("onboarding.coach.accessibility.title"),
                t("onboarding.coach.accessibility.description"),
                cx,
            ))
            .children(
                self.accessibility_asked
                    .then(|| self.waiting_line(t("onboarding.coach.dragHint"), cx)),
            )
            .child(
                Button::new("coach-accessibility")
                    .variant(asked_variant(self.accessibility_asked))
                    .child(asked_label(
                        self.accessibility_asked,
                        "onboarding.coach.accessibility.button",
                    ))
                    .on_click(cx.listener(|this, _, _, cx| this.ask_accessibility(cx))),
            )
            .into_any_element()
    }

    fn preparing(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let store = self.models.read(cx);
        let id = self.starter.as_deref().unwrap_or_default();
        let model = store.model(id);
        let name = model.map(crate::ui::format::model_name).unwrap_or_default();
        let size = model
            .map(|model| crate::ui::format::model_size(model.size_mb))
            .unwrap_or_default();
        let percent = store.progress.get(id).map_or(0., |p| p.percentage) as f32;
        self.column()
            .child(self.headline(
                t("onboarding.coach.preparing.title"),
                t_with(
                    "onboarding.coach.preparing.description",
                    &[("model", &name), ("size", &size)],
                ),
                cx,
            ))
            .child(
                div()
                    .w(px(300.))
                    .h(px(6.))
                    .rounded_full()
                    .bg(p.text.a(0.12))
                    .child(
                        div()
                            .h_full()
                            .rounded_full()
                            .bg(p.accent)
                            .w(px(300. * (percent / 100.).clamp(0., 1.))),
                    ),
            )
            .child(
                Button::new("coach-pick-model")
                    .variant(ButtonVariant::Ghost)
                    .child(t("onboarding.coach.preparing.other"))
                    .on_click(cx.listener(|this, _, window, cx| this.pick_model(window, cx))),
            )
            .into_any_element()
    }

    fn lesson_pill(&self, label: SharedString, done: bool, active: bool, cx: &App) -> AnyElement {
        let p = palette(cx);
        div()
            .px_3()
            .py_1()
            .rounded_full()
            .bg(p.text.a(if active { 0.12 } else { 0.06 }))
            .xs_text()
            .text_color(if done { p.done } else { p.text.a(0.7) })
            .child(label)
            .into_any_element()
    }

    fn practice(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let scratch = self.stage == Stage::Scratch;
        let keyword = scratch_keyword(cx);
        let script = if scratch {
            t_with(
                "onboarding.coach.scratch.line",
                &[("keyword", &capitalize(&keyword))],
            )
        } else {
            t("onboarding.coach.hold.line")
        };
        let lesson = if scratch { "scratch" } else { "hold" };
        let hint_key = |state: &str| format!("onboarding.coach.{lesson}.{state}");
        let keyword_arg: [(&str, &dyn std::fmt::Display); 1] = [("keyword", &keyword)];
        let hint = match &self.attempt {
            Attempt::Ready => t_with(&hint_key("ready"), &keyword_arg),
            Attempt::Talking => t_with(&hint_key("talking"), &keyword_arg),
            Attempt::Working(_) => t("onboarding.coach.working"),
            Attempt::Passed(text) if text.is_empty() => t("onboarding.coach.scratch.passedEmpty"),
            Attempt::Passed(_) => t(&hint_key("passed")),
            Attempt::Retry(None) => t("onboarding.coach.notHeard"),
            Attempt::Retry(Some(_)) => t_with(&hint_key("retry"), &keyword_arg),
        };
        let (line, line_color) = match &self.attempt {
            Attempt::Passed(text) if text.is_empty() => {
                (t("onboarding.coach.scratch.emptyLine"), p.done)
            }
            Attempt::Passed(text) => (SharedString::from(text.clone()), p.done),
            Attempt::Retry(Some(text)) => (SharedString::from(text.clone()), p.warning),
            Attempt::Talking | Attempt::Working(_) => (script, p.text),
            _ => (script, p.text.a(0.5)),
        };
        let passed = matches!(self.attempt, Attempt::Passed(_));
        let labels = binding("transcribe", cx);
        self.column()
            .child(
                div()
                    .w_full()
                    .max_w(px(720.))
                    .min_h(px(84.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_center()
                    .text_size(px(27.))
                    .line_height(px(36.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(line_color)
                    .child(line),
            )
            .child(self.keys(&labels, 96., self.dictating, cx))
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(hint),
            )
            .child(
                div()
                    .flex()
                    .row_dir()
                    .gap_2()
                    .child(self.lesson_pill(
                        t("onboarding.coach.hold.pill"),
                        scratch || passed,
                        !scratch,
                        cx,
                    ))
                    .child(self.lesson_pill(
                        t("onboarding.coach.scratch.pill"),
                        scratch && passed,
                        scratch,
                        cx,
                    )),
            )
            .child(div().h(px(32.)).when(passed, |el| {
                el.child(
                    Button::new("coach-next")
                        .child(t("onboarding.coach.next"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.preview {
                                return;
                            }
                            if !scratch && scratch_on(cx) {
                                this.enter(Stage::Scratch, cx);
                            } else {
                                this.start_annotate(cx);
                            }
                        })),
                )
            }))
            .into_any_element()
    }

    fn screen(&self, cx: &mut Context<Self>) -> AnyElement {
        self.column()
            .child(self.hero("monitor", cx))
            .child(self.headline(
                t("onboarding.coach.screen.title"),
                t("onboarding.coach.screen.description"),
                cx,
            ))
            .children(
                self.screen_asked
                    .then(|| self.waiting_line(t("onboarding.coach.dragHint"), cx)),
            )
            .child(
                Button::new("coach-screen")
                    .variant(asked_variant(self.screen_asked))
                    .child(asked_label(
                        self.screen_asked,
                        "onboarding.coach.screen.button",
                    ))
                    .on_click(cx.listener(|this, _, _, cx| this.ask_screen(cx))),
            )
            .into_any_element()
    }

    fn restart(&self, cx: &mut Context<Self>) -> AnyElement {
        self.column()
            .child(self.hero("monitor", cx))
            .child(self.headline(
                t("onboarding.coach.restart.title"),
                t("onboarding.coach.restart.description"),
                cx,
            ))
            .child(
                Button::new("coach-restart")
                    .child(t("onboarding.coach.restart.button"))
                    .on_click(cx.listener(|this, _, _, _| this.relaunch())),
            )
            .into_any_element()
    }

    fn receipt_with_wrong_total(&self) -> AnyElement {
        let dark = gpui::rgb(0x1d1d1f);
        let rule = gpui::rgb(0xdcdce0);
        let item = |name: SharedString, price: &'static str| {
            div()
                .flex()
                .row_dir()
                .justify_between()
                .py(px(6.))
                .border_b_1()
                .border_color(rule)
                .child(name)
                .child(price)
        };
        div()
            .w(px(300.))
            .p_4()
            .rounded(px(11.))
            .bg(gpui::rgb(INK_READABLE_BACKGROUND))
            .text_color(dark)
            .text_size(px(14.))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(15.))
                    .mb_2()
                    .child(t("onboarding.coach.record.receiptTitle")),
            )
            .child(item(t("onboarding.coach.record.itemOne"), "$36.00"))
            .child(item(t("onboarding.coach.record.itemTwo"), "$14.00"))
            .child(
                div()
                    .flex()
                    .row_dir()
                    .justify_between()
                    .pt_2()
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(16.))
                    .child(t("onboarding.coach.record.total"))
                    .child(WRONG_TOTAL),
            )
            .into_any_element()
    }

    fn record(&self, cx: &mut Context<Self>) -> AnyElement {
        let shortcut = binding(crate::annotate::RECORD_BINDING, cx);
        let keys_text = shortcut.join("");
        let (hint, labels) = if self.capturing {
            (
                t_with("onboarding.coach.record.live", &[("keys", &keys_text)]),
                vec!["⇧".to_string()],
            )
        } else {
            (
                t_with("onboarding.coach.record.ready", &[("keys", &keys_text)]),
                shortcut,
            )
        };
        self.column()
            .child(self.receipt_with_wrong_total())
            .child(
                div()
                    .max_w(rems(46.))
                    .text_center()
                    .text_size(px(16.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(hint),
            )
            .child(self.keys(&labels, 72., false, cx))
            .into_any_element()
    }

    fn review(&self, cx: &mut Context<Self>) -> AnyElement {
        let paste = binding(crate::annotate::PASTE_BINDING, cx).join("");
        self.column()
            .child(self.headline(
                t("onboarding.coach.review.title"),
                t_with("onboarding.coach.review.description", &[("keys", &paste)]),
                cx,
            ))
            .child(self.review_card(cx))
            .child(
                Button::new("coach-finish")
                    .child(t("onboarding.coach.review.button"))
                    .on_click(cx.listener(|this, _, window, cx| this.finish(window, cx))),
            )
            .into_any_element()
    }

    fn review_card(&self, cx: &App) -> AnyElement {
        let p = palette(cx);
        let card = div()
            .w(px(460.))
            .p_3()
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .flex()
            .flex_col()
            .gap_2();
        let Some(Review { sheet, lines }) = self.review.clone() else {
            return card
                .sm_text()
                .text_center()
                .text_color(p.text.a(0.7))
                .child(t("onboarding.coach.review.empty"))
                .into_any_element();
        };
        card.children(sheet.map(|path| {
            img(path)
                .w_full()
                .h(px(200.))
                .rounded(px(7.))
                .object_fit(ObjectFit::Contain)
        }))
        .children(
            lines
                .into_iter()
                .map(|line| div().xs_text().text_color(p.text.a(0.85)).child(line)),
        )
        .into_any_element()
    }

    fn dots(&self, cx: &App) -> AnyElement {
        let p = palette(cx);
        let current = self.stage.dot();
        div()
            .absolute()
            .top(px(20.))
            .left_0()
            .right_0()
            .flex()
            .row_dir()
            .justify_center()
            .gap(px(7.))
            .children((0..4).map(|index| {
                div()
                    .h(px(6.))
                    .w(px(if index == current { 26. } else { 6. }))
                    .rounded_full()
                    .bg(if index <= current {
                        p.text
                    } else {
                        p.text.a(0.2)
                    })
            }))
            .into_any_element()
    }

    fn skip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let practicing = self.stage.is_practicing_dictation();
        let can_leave_annotate =
            matches!(self.stage, Stage::Screen | Stage::Restart | Stage::Record) && !self.capturing;
        (practicing || can_leave_annotate).then(|| {
            div()
                .absolute()
                .top(px(14.))
                .inset_end(px(16.))
                .child(
                    Button::new("coach-skip")
                        .variant(ButtonVariant::Ghost)
                        .child(t("onboarding.coach.skip"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if practicing {
                                this.start_annotate(cx);
                            } else {
                                this.finish(window, cx);
                            }
                        })),
                )
                .into_any_element()
        })
    }
}

fn asked_variant(asked: bool) -> ButtonVariant {
    if asked {
        ButtonVariant::Ghost
    } else {
        ButtonVariant::Primary
    }
}

fn asked_label(asked: bool, key: &str) -> SharedString {
    t(if asked {
        "onboarding.coach.openAgain"
    } else {
        key
    })
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

impl Render for KeycapCoach {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.stage {
            Stage::Microphone => self.microphone(cx),
            Stage::Accessibility => self.accessibility(cx),
            Stage::Preparing => self.preparing(cx),
            Stage::Hold | Stage::Scratch => self.practice(cx),
            Stage::Screen => self.screen(cx),
            Stage::Restart => self.restart(cx),
            Stage::Record => self.record(cx),
            Stage::Review => self.review(cx),
        };
        div()
            .id("keycap-coach")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .p_6()
            .pt(top_padding())
            .child(self.dots(cx))
            .children(self.skip(cx))
            .child(body)
    }
}
