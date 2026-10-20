//! The Models page (`models/ModelsSettings.tsx`): search and filters, the
//! downloaded models, then models available to download.

use gpui::{
    div, prelude::*, rems, Context, Entity, FontWeight, PromptLevel, SharedString, Subscription,
    Window,
};
use gpui_component::input::{InputEvent, InputState};

use crate::i18n::{t, t_with};
use crate::managers::model::ModelInfo;
use crate::ui::components::{
    icon, KeyboardFocus, RowDirection, Spinner, TextField, Tooltip, Typography,
};
use crate::ui::languages::{model_capability_languages, supports_language_code};
use crate::ui::model_card::{is_legacy, CardStatus, ModelCard};
use crate::ui::stores::ModelStore;
use crate::ui::theme::{palette, Alpha};

use super::language_picker::{LanguagePicker, PickerStyle};

pub struct ModelsPage {
    store: Entity<ModelStore>,
    search: Entity<InputState>,
    language: Entity<LanguagePicker>,
    language_filter: SharedString,
    filter_streaming: bool,
    filter_translation: bool,
    switching: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl ModelsPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = ModelStore::global(cx);
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("settings.models.searchPlaceholder"))
        });
        let entity = cx.entity().downgrade();
        let language = cx.new(|cx| {
            LanguagePicker::new(
                PickerStyle::Filter,
                move |code, _, cx| {
                    let _ = entity.update(cx, |this, cx| {
                        this.language_filter = code;
                        this.sync_language(cx);
                        cx.notify();
                    });
                },
                window,
                cx,
            )
        });
        let subscriptions = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify()
                }
            }),
        ];
        let mut this = Self {
            store,
            search,
            language,
            language_filter: "all".into(),
            filter_streaming: false,
            filter_translation: false,
            switching: None,
            _subscriptions: subscriptions,
        };
        this.sync_language(cx);
        this
    }

    fn sync_language(&mut self, cx: &mut Context<Self>) {
        let options = model_capability_languages()
            .map(|(code, label)| {
                (
                    SharedString::new_static(code),
                    SharedString::new_static(label),
                )
            })
            .collect();
        let selected = self.language_filter.clone();
        self.language.update(cx, |picker, _| {
            picker.set_options(
                options,
                Some(("all".into(), t("settings.models.filters.allLanguages"))),
                Some(selected),
                false,
            )
        });
    }

    fn status(&self, model: &ModelInfo, store: &ModelStore) -> CardStatus {
        let id = &model.id;
        if store.extracting.contains(id) {
            CardStatus::Extracting
        } else if store.verifying.contains(id) {
            CardStatus::Verifying
        } else if store.downloading.contains(id) {
            CardStatus::Downloading
        } else if self.switching.as_ref() == Some(id) {
            CardStatus::Switching
        } else if !model.is_downloaded {
            CardStatus::Downloadable
        } else if *id == store.current_model {
            CardStatus::Active
        } else {
            CardStatus::Available
        }
    }

    fn select(&mut self, id: &str, cx: &mut Context<Self>) {
        self.switching = Some(id.to_string());
        cx.notify();
        let weak = cx.entity().downgrade();
        self.store.update(cx, |store, cx| {
            store.select_model(
                id.to_string(),
                move |_, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.switching = None;
                        cx.notify();
                    });
                },
                cx,
            )
        });
    }

    fn delete(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let name = store
            .model(id)
            .map(|m| m.name.clone())
            .unwrap_or_else(|| id.to_string());
        let message = if id == store.current_model {
            t_with(
                "settings.models.deleteActiveConfirm",
                &[("modelName", &name)],
            )
        } else {
            t_with("settings.models.deleteConfirm", &[("modelName", &name)])
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &t("settings.models.deleteTitle"),
            Some(&message),
            &["Yes", "No"],
            cx,
        );
        let id = id.to_string();
        let store = self.store.clone();
        cx.spawn(async move |_, cx| {
            if answer.await == Ok(0) {
                let _ = store.update(cx, |store, cx| store.delete_model(id, cx));
            }
        })
        .detach();
    }

    fn filter_button(
        &self,
        id: &'static str,
        glyph: &'static str,
        title: SharedString,
        active: bool,
        cx: &mut Context<Self>,
        toggle: fn(&mut Self),
    ) -> impl IntoElement {
        let p = palette(cx);
        div()
            .id(id)
            .flex()
            .row_dir()
            .items_center()
            .justify_center()
            .size_8()
            .rounded_lg()
            .cursor_pointer()
            .when(active, |el| {
                el.bg(p.accent)
                    .text_color(p.accent_contrast)
                    .hover(|s| s.bg(p.accent.a(0.9)))
            })
            .when(!active, |el| {
                el.bg(p.mid_gray.a(0.1))
                    .text_color(p.text.a(0.6))
                    .hover(|s| s.bg(p.mid_gray.a(0.2)))
            })
            .tooltip(move |window, cx| Tooltip::text(title.clone(), window, cx))
            .keyboard_focus(cx)
            .on_click(cx.listener(move |this, _, _, cx| {
                toggle(this);
                cx.notify();
            }))
            .child(icon(glyph).size(rems(0.875)))
    }

    fn card(&self, model: &ModelInfo, show_recommended: bool, cx: &mut Context<Self>) -> ModelCard {
        let store = self.store.read(cx);
        let status = self.status(model, store);
        let progress = store.progress.get(&model.id).map(|p| p.percentage);
        let speed = store.stats.get(&model.id).map(|s| s.speed);
        let entity = cx.entity().downgrade();
        let download_store = self.store.clone();
        let cancel_store = self.store.clone();
        let delete_entity = entity.clone();
        ModelCard::new(model.clone(), status)
            .show_recommended(show_recommended)
            .progress(progress, speed)
            .on_select(move |id, _, cx| {
                let _ = entity.update(cx, |this, cx| this.select(id, cx));
            })
            .on_download(move |id, _, cx| {
                download_store.update(cx, |s, cx| s.download_model(id.to_string(), |_, _| {}, cx));
            })
            .on_delete(move |id, window, cx| {
                let _ = delete_entity.update(cx, |this, cx| this.delete(id, window, cx));
            })
            .on_cancel(move |id, _, cx| {
                cancel_store.update(cx, |s, cx| s.cancel_download(id.to_string(), |_, _| {}, cx));
            })
    }
}

impl Render for ModelsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let root = div().max_w(rems(48.)).w_full().flex().flex_col().gap_4();
        let store = self.store.read(cx);
        if store.loading {
            return root.child(
                div()
                    .flex()
                    .row_dir()
                    .items_center()
                    .justify_center()
                    .py_16()
                    .child(Spinner::ring(rems(2.), p.accent)),
            );
        }

        let query = self.search.read(cx).value().trim().to_lowercase();
        let language = self.language_filter.to_string();
        let filtered: Vec<ModelInfo> = store
            .models
            .iter()
            .filter(|m| !(is_legacy(m) && !m.is_downloaded))
            .filter(|m| {
                language == "all" || supports_language_code(&m.supported_languages, &language)
            })
            .filter(|m| !self.filter_streaming || m.supports_streaming)
            .filter(|m| !self.filter_translation || m.supports_translation)
            .filter(|m| {
                query.is_empty()
                    || format!("{} {}", m.name, m.description)
                        .to_lowercase()
                        .contains(&query)
            })
            .cloned()
            .collect();

        let (mut downloaded, available): (Vec<ModelInfo>, Vec<ModelInfo>) =
            filtered.iter().cloned().partition(|m| {
                m.is_custom
                    || m.is_downloaded
                    || store.downloading.contains(&m.id)
                    || store.extracting.contains(&m.id)
            });
        let current = store.current_model.clone();
        // Active first, then catalog models, then custom ones.
        downloaded.sort_by_key(|m| (m.id != current, m.is_custom));
        let rescanning = store.is_rescanning;

        let section_title = |text: SharedString| {
            div()
                .text_px(15.)
                .font_weight(FontWeight::SEMIBOLD)
                .child(text)
        };

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .row_dir()
            .child(section_title(t("settings.models.yourModels")))
            .child(
                div()
                    .flex()
                    .row_dir()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("rescan-models")
                            .flex()
                            .row_dir()
                            .items_center()
                            .justify_center()
                            .size_8()
                            .rounded_lg()
                            .bg(p.mid_gray.a(0.1))
                            .text_color(p.text.a(0.6))
                            .when(rescanning, |el| el.opacity(0.5).cursor_not_allowed())
                            .when(!rescanning, |el| {
                                el.cursor_pointer()
                                    .hover(|s| s.bg(p.mid_gray.a(0.2)))
                                    .keyboard_focus(cx)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.store.update(cx, |s, cx| s.rescan(cx))
                                    }))
                            })
                            .tooltip(|window, cx| {
                                Tooltip::text(t("settings.models.rescan.tooltip"), window, cx)
                            })
                            .child(if rescanning {
                                Spinner::loader(rems(0.875), p.text.a(0.6)).into_any_element()
                            } else {
                                icon("refresh-cw").size(rems(0.875)).into_any_element()
                            }),
                    )
                    .child(
                        div()
                            .h_4()
                            .w(gpui::px(1.))
                            .bg(p.mid_gray.a(0.3))
                            .mx(rems(0.125)),
                    )
                    .child(self.filter_button(
                        "filter-streaming",
                        "audio-lines",
                        t("settings.models.filters.streaming"),
                        self.filter_streaming,
                        cx,
                        |this| this.filter_streaming = !this.filter_streaming,
                    ))
                    .child(self.filter_button(
                        "filter-translation",
                        "languages",
                        t("settings.models.filters.translation"),
                        self.filter_translation,
                        cx,
                        |this| this.filter_translation = !this.filter_translation,
                    ))
                    .child(self.language.clone()),
            );

        let downloaded_cards: Vec<_> = downloaded.iter().map(|m| self.card(m, false, cx)).collect();
        let available_cards: Vec<_> = available.iter().map(|m| self.card(m, true, cx)).collect();
        let nothing = filtered.is_empty();

        root.child(
            div()
                .sm_text()
                .text_color(p.text.a(0.6))
                .mt(rems(-0.5))
                .mb_5()
                .child(t("settings.models.description")),
        )
        .child(
            TextField::new(&self.search)
                .regular_weight()
                .rounded_xl()
                .prefix(
                    icon("search")
                        .size(rems(1.))
                        .text_color(p.text.a(0.4))
                        .me(rems(0.5)),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_6()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(header)
                        .children(downloaded_cards),
                )
                .when(!available_cards.is_empty(), |el| {
                    el.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(section_title(t("settings.models.availableModels")))
                            .children(available_cards),
                    )
                })
                .when(nothing, |el| {
                    el.child(
                        div()
                            .py_8()
                            .flex()
                            .row_dir()
                            .justify_center()
                            .text_color(p.text.a(0.5))
                            .child(t("settings.models.noModelsMatch")),
                    )
                }),
        )
    }
}
