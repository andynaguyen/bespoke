//! A transcription model card (`onboarding/ModelCard.tsx`), used by
//! onboarding and the Models page.

use std::rc::Rc;
use std::sync::LazyLock;

use gpui::{div, prelude::*, px, rems, AnimationExt, App, FontWeight, SharedString, Window};

use crate::i18n::{t, t_with};
use crate::managers::model::{ModelInfo, ModelSource};
use crate::ui::components::{
    badge, icon, pulse_animation, pulse_opacity, BadgeVariant, Button, ButtonSize, ButtonVariant,
    KeyboardFocus, RowDirection, Spinner, Tooltip, Typography,
};
use crate::ui::format::{model_description, model_name, model_size};
use crate::ui::languages::{language_label, unique_capability_languages};
use crate::ui::theme::{palette, Alpha};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CardStatus {
    Downloadable,
    Downloading,
    Verifying,
    Extracting,
    Switching,
    Active,
    Available,
}

/// A blob (Url-sourced) model, kept runnable but no longer the advertised
/// download.
pub fn is_legacy(model: &ModelInfo) -> bool {
    matches!(model.source, ModelSource::Url { .. })
}

/// GGUF quantization label from the filename, e.g. `Q8_0`.
fn quant_label(filename: &str) -> Option<String> {
    static PATTERN: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)[._-](IQ\d+_\w+|Q\d+(?:_\w+)?|F16|BF16|F32)\.gguf$").unwrap()
    });
    PATTERN.captures(filename).map(|c| c[1].to_uppercase())
}

type ModelHandler = Rc<dyn Fn(&str, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct ModelCard {
    model: ModelInfo,
    status: CardStatus,
    featured: bool,
    disabled: bool,
    show_recommended: bool,
    progress: Option<f64>,
    speed: Option<f64>,
    on_select: Option<ModelHandler>,
    on_download: Option<ModelHandler>,
    on_delete: Option<ModelHandler>,
    on_cancel: Option<ModelHandler>,
}

impl ModelCard {
    pub fn new(model: ModelInfo, status: CardStatus) -> Self {
        Self {
            model,
            status,
            featured: false,
            disabled: false,
            show_recommended: true,
            progress: None,
            speed: None,
            on_select: None,
            on_download: None,
            on_delete: None,
            on_cancel: None,
        }
    }

    pub fn featured(mut self, featured: bool) -> Self {
        self.featured = featured;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn show_recommended(mut self, show: bool) -> Self {
        self.show_recommended = show;
        self
    }

    /// Download percentage and speed in MiB/s while downloading.
    pub fn progress(mut self, percent: Option<f64>, speed: Option<f64>) -> Self {
        self.progress = percent;
        self.speed = speed;
        self
    }

    pub fn on_select(mut self, f: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }

    pub fn on_download(mut self, f: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_download = Some(Rc::new(f));
        self
    }

    pub fn on_delete(mut self, f: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_delete = Some(Rc::new(f));
        self
    }

    pub fn on_cancel(mut self, f: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_cancel = Some(Rc::new(f));
        self
    }
}

fn score_bar(label: SharedString, score: f32, cx: &App) -> impl IntoElement {
    let p = palette(cx);
    div()
        .flex()
        .row_dir()
        .items_center()
        .gap_2()
        .child(
            div()
                .xs_text()
                .text_color(p.text.a(0.6))
                .w(rems(6.))
                .flex()
                .row_dir()
                .justify_flex_end()
                .child(label),
        )
        .child(
            div()
                .w(rems(4.))
                .h(rems(0.375))
                .bg(p.mid_gray.a(0.2))
                .rounded_full()
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .bg(p.data)
                        .rounded_full()
                        .w(gpui::relative(score.clamp(0., 1.))),
                ),
        )
}

fn chip(
    id: String,
    glyph: &'static str,
    text: SharedString,
    title: SharedString,
    cx: &App,
) -> impl IntoElement {
    let p = palette(cx);
    div()
        .id(SharedString::from(id))
        .flex()
        .row_dir()
        .items_center()
        .gap_1()
        .xs_text()
        .text_color(p.text.a(0.5))
        .tooltip(move |window, cx| Tooltip::text(title.clone(), window, cx))
        .child(icon(glyph).size(rems(0.875)))
        .child(text)
}

/// A full-width accent bar pulsing while verifying or extracting.
fn busy_block(label: SharedString, id: String, cx: &App) -> impl IntoElement {
    let p = palette(cx);
    div()
        .w_full()
        .mt_3()
        .child(
            div()
                .w_full()
                .h(rems(0.375))
                .bg(p.mid_gray.a(0.2))
                .rounded_full()
                .overflow_hidden()
                .child(
                    div()
                        .size_full()
                        .bg(p.accent)
                        .rounded_full()
                        .with_animation(SharedString::from(id), pulse_animation(), |el, delta| {
                            el.opacity(pulse_opacity(delta))
                        }),
                ),
        )
        .child(
            div()
                .xs_text()
                .text_color(p.text.a(0.5))
                .mt_1()
                .child(label),
        )
}

impl RenderOnce for ModelCard {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let model = self.model;
        let status = self.status;
        let clickable = matches!(status, CardStatus::Available | CardStatus::Downloadable);
        let name = model_name(&model);
        let debug_mode = crate::ui::stores::SettingsStore::get(cx).debug_mode;
        let group = SharedString::from(format!("model-card-{}", model.id));

        let mut card = div()
            .id(group.clone())
            .group(group.clone())
            .flex()
            .flex_col()
            .rounded(px(10.))
            .px_5()
            .py_4()
            .gap_2()
            .border_1();
        card = if status == CardStatus::Active {
            card.border_color(p.accent.a(0.6)).bg(p.card)
        } else if self.featured {
            card.border_color(p.mid_gray.a(0.3)).bg(p.card)
        } else {
            card.border_color(p.mid_gray.a(0.2))
        };
        if clickable {
            if self.disabled {
                card = card.opacity(0.5).cursor_not_allowed();
            } else {
                card = card
                    .cursor_pointer()
                    .keyboard_focus(cx)
                    .hover(|s| s.border_color(p.mid_gray.a(0.4)).bg(p.card));
                let id = model.id.clone();
                let on_download = self.on_download.clone();
                let on_select = self.on_select.clone();
                card = card.on_click(move |_, window, cx| {
                    if status == CardStatus::Downloadable {
                        if let Some(f) = &on_download {
                            return f(&id, window, cx);
                        }
                    }
                    if let Some(f) = &on_select {
                        f(&id, window, cx);
                    }
                });
            }
        }

        let mut badges: Vec<gpui::AnyElement> = Vec::new();
        if self.show_recommended && model.is_recommended {
            badges.push(
                badge(BadgeVariant::Primary, cx)
                    .child(t("onboarding.recommended"))
                    .into_any_element(),
            );
        }
        if status == CardStatus::Active {
            badges.push(
                badge(BadgeVariant::Primary, cx)
                    .child(icon("check").size(rems(0.75)).mr_1())
                    .child(t("modelSelector.active"))
                    .into_any_element(),
            );
        }
        if model.is_custom {
            badges.push(
                badge(BadgeVariant::Secondary, cx)
                    .child(t("modelSelector.custom"))
                    .into_any_element(),
            );
        }
        if is_legacy(&model) {
            badges.push(
                badge(BadgeVariant::Secondary, cx)
                    .child(t("modelSelector.legacy"))
                    .into_any_element(),
            );
        }
        if status == CardStatus::Switching {
            badges.push(
                badge(BadgeVariant::Secondary, cx)
                    .child(
                        div()
                            .mr_1()
                            .child(Spinner::loader(rems(0.75), p.text.a(0.7))),
                    )
                    .child(t("modelSelector.switching"))
                    .into_any_element(),
            );
        }

        let top = div()
            .flex()
            .justify_between()
            .row_dir()
            .items_center()
            .w_full()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .row_dir()
                            .items_center()
                            .gap_3()
                            .flex_wrap()
                            .child(
                                div()
                                    .text_px(14.)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.text)
                                    .when(clickable && !self.disabled, |el| {
                                        el.group_hover(group.clone(), |s| s.text_color(p.accent))
                                    })
                                    .child(name.clone()),
                            )
                            .children(badges),
                    )
                    .child(
                        div()
                            .sm_text()
                            .leading_relaxed()
                            .text_color(p.text.a(0.6))
                            .child(model_description(&model)),
                    ),
            )
            .when(model.accuracy_score > 0. || model.speed_score > 0., |el| {
                el.child(
                    div().flex().items_center().ms(rems(1.)).child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(score_bar(
                                t("onboarding.modelCard.accuracy"),
                                model.accuracy_score,
                                cx,
                            ))
                            .child(score_bar(
                                t("onboarding.modelCard.speed"),
                                model.speed_score,
                                cx,
                            )),
                    ),
                )
            });

        let languages = unique_capability_languages(&model.supported_languages);
        let mut chips: Vec<gpui::AnyElement> = Vec::new();
        if !languages.is_empty() {
            let (text, title) = if languages.len() == 1 {
                let label = language_label(&languages[0])
                    .unwrap_or(&languages[0])
                    .to_string();
                (
                    t_with(
                        "modelSelector.capabilities.languageOnly",
                        &[("language", &label)],
                    ),
                    t("modelSelector.capabilities.singleLanguage"),
                )
            } else {
                (
                    t_with(
                        "modelSelector.capabilities.languageCount",
                        &[("total", &languages.len())],
                    ),
                    t("modelSelector.capabilities.languageSelection"),
                )
            };
            chips.push(
                chip(format!("lang-{}", model.id), "globe", text, title, cx).into_any_element(),
            );
        }
        if model.supports_translation {
            chips.push(
                chip(
                    format!("translate-{}", model.id),
                    "languages",
                    t("modelSelector.capabilities.translate"),
                    t("modelSelector.capabilities.translation"),
                    cx,
                )
                .into_any_element(),
            );
        }
        if model.supports_streaming {
            chips.push(
                chip(
                    format!("stream-{}", model.id),
                    "audio-lines",
                    t("modelSelector.streaming"),
                    t("modelSelector.capabilities.streaming"),
                    cx,
                )
                .into_any_element(),
            );
        }

        let show_size = matches!(
            status,
            CardStatus::Downloadable | CardStatus::Available | CardStatus::Active
        );
        let quant = debug_mode.then(|| quant_label(&model.filename)).flatten();
        let bottom = div()
            .flex()
            .row_dir()
            .items_center()
            .gap_3()
            .w_full()
            .h(rems(1.25))
            .mt(rems(0.125))
            .mb(rems(-0.125))
            .child(div().flex().items_center().gap_3().children(chips))
            .child(div().flex_1())
            .when(show_size, |el| {
                el.child(
                    div()
                        .flex()
                        .row_dir()
                        .items_center()
                        .gap(rems(0.375))
                        .xs_text()
                        .text_color(p.text.a(0.5))
                        .child(
                            icon(if status == CardStatus::Downloadable {
                                "download"
                            } else {
                                "hard-drive"
                            })
                            .size(rems(0.875)),
                        )
                        .child(model_size(model.size_mb))
                        .children(quant.map(|q| div().text_color(p.text.a(0.4)).child(q))),
                )
            })
            .when_some(
                self.on_delete
                    .clone()
                    .filter(|_| matches!(status, CardStatus::Available | CardStatus::Active)),
                |el, on_delete| {
                    let id = model.id.clone();
                    el.child(
                        Button::new(SharedString::from(format!("delete-{}", model.id)))
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Sm)
                            .gap(rems(0.375))
                            .text_color(p.accent.a(0.85))
                            .hover(move |s| s.text_color(p.accent).bg(p.mid_gray.a(0.1)))
                            .title(t_with("modelSelector.deleteModel", &[("modelName", &name)]))
                            .child(icon("trash-2").size(rems(0.875)))
                            .child(t("common.delete"))
                            .on_click(move |_, window, cx| on_delete(&id, window, cx)),
                    )
                },
            );

        let progress = match status {
            CardStatus::Downloading => self.progress.map(|percent| {
                let on_cancel = self.on_cancel.clone();
                let id = model.id.clone();
                div()
                    .w_full()
                    .mt_3()
                    .child(crate::ui::components::progress_track(
                        percent as f32,
                        gpui::relative(1.),
                        rems(0.375).to_pixels(crate::ui::theme::REM),
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .row_dir()
                            .xs_text()
                            .mt_1()
                            .child(div().text_color(p.text.a(0.5)).child(t_with(
                                "modelSelector.downloading",
                                &[("percentage", &(percent.round() as i64))],
                            )))
                            .child(
                                div()
                                    .flex()
                                    .row_dir()
                                    .items_center()
                                    .gap_2()
                                    .when(self.speed.is_some_and(|s| s > 0.), |el| {
                                        el.child(div().text_color(p.text.a(0.5)).child(t_with(
                                            "modelSelector.downloadSpeed",
                                            &[(
                                                "speed",
                                                &format!("{:.1}", self.speed.unwrap_or(0.)),
                                            )],
                                        )))
                                    })
                                    .when_some(on_cancel, |el, on_cancel| {
                                        el.child(
                                            Button::new(SharedString::from(format!(
                                                "cancel-{}",
                                                model.id
                                            )))
                                            .variant(ButtonVariant::DangerGhost)
                                            .size(ButtonSize::Sm)
                                            .child(t("modelSelector.cancel"))
                                            .on_click(move |_, window, cx| {
                                                on_cancel(&id, window, cx)
                                            }),
                                        )
                                    }),
                            ),
                    )
                    .into_any_element()
            }),
            CardStatus::Verifying => Some(
                busy_block(
                    t("modelSelector.verifyingGeneric"),
                    format!("verify-{}", model.id),
                    cx,
                )
                .into_any_element(),
            ),
            CardStatus::Extracting => Some(
                busy_block(
                    t("modelSelector.extractingGeneric"),
                    format!("extract-{}", model.id),
                    cx,
                )
                .into_any_element(),
            ),
            _ => None,
        };

        card.child(top)
            .child(div().w_full().h(px(1.)).bg(p.mid_gray.a(0.2)))
            .child(bottom)
            .children(progress)
    }
}
