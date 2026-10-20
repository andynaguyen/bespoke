//! The model picker step (`Onboarding.tsx`). Picking a model downloads it,
//! and once it is downloaded, verified and extracted it becomes the active
//! model and onboarding ends.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    div, percentage, prelude::*, rems, Animation, AnimationExt, App, Context, Entity, FontWeight,
    ScrollHandle, SharedString, Subscription, Window,
};
use gpui_component::scroll::ScrollableElement;

use super::top_padding;
use crate::i18n::{t, t_with};
use crate::managers::model::ModelInfo;
use crate::ui::components::{icon, KeyboardFocus, RowDirection, Typography};
use crate::ui::model_card::{is_legacy, CardStatus, ModelCard};
use crate::ui::shell::logo;
use crate::ui::stores::ModelStore;
use crate::ui::theme::{palette, Alpha};
use crate::ui::toast;
use crate::ui::WindowCallback;

pub struct ModelOnboarding {
    store: Entity<ModelStore>,
    preview: bool,
    on_done: WindowCallback,
    /// The model being downloaded or switched to. Set means busy.
    selected: Option<String>,
    started_selection: bool,
    show_all: bool,
    /// Whether "Show all" was clicked, so the chevron only animates after
    /// a toggle and not on first paint.
    toggled: bool,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl ModelOnboarding {
    pub fn new(
        preview: bool,
        on_done: impl Fn(&mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = ModelStore::global(cx);
        let subscriptions = vec![cx.observe_in(&store, window, |this, _, window, cx| {
            this.check_ready(window, cx);
            cx.notify();
        })];
        Self {
            store,
            preview,
            on_done: Rc::new(on_done),
            selected: None,
            started_selection: false,
            show_all: false,
            toggled: false,
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    /// Switches to the selected model once it is fully on disk, then ends
    /// onboarding.
    fn check_ready(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Previews never switch the user's active model.
        if self.preview {
            return;
        }
        let Some(id) = self.selected.clone() else {
            self.started_selection = false;
            return;
        };
        let store = self.store.read(cx);
        let ready = store.model(&id).is_some_and(|m| m.is_downloaded)
            && !store.downloading.contains(&id)
            && !store.verifying.contains(&id)
            && !store.extracting.contains(&id);
        if !ready || self.started_selection {
            return;
        }
        self.started_selection = true;

        let weak = cx.entity().downgrade();
        let on_done = self.on_done.clone();
        let handle = window.window_handle();
        self.store.update(cx, |store, cx| {
            store.select_model(
                id,
                move |ok, cx| {
                    if ok {
                        let _ = handle.update(cx, |_, window, cx| on_done(window, cx));
                    } else {
                        toast::error(t("onboarding.errors.selectModel"), cx);
                        let _ = weak.update(cx, |this, cx| {
                            this.started_selection = false;
                            this.selected = None;
                            cx.notify();
                        });
                    }
                },
                cx,
            )
        });
    }

    fn download(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.preview {
            return;
        }
        self.selected = Some(id.to_string());
        // The store shows the error toast when a download fails.
        let weak = cx.entity().downgrade();
        self.store.update(cx, |store, cx| {
            store.download_model(
                id.to_string(),
                move |ok, cx| {
                    if !ok {
                        let _ = weak.update(cx, |this, cx| {
                            this.selected = None;
                            cx.notify();
                        });
                    }
                },
                cx,
            )
        });
        cx.notify();
    }

    fn cancel(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.preview {
            return;
        }
        let weak = cx.entity().downgrade();
        self.store.update(cx, |store, cx| {
            store.cancel_download(
                id.to_string(),
                move |ok, cx| {
                    if ok {
                        let _ = weak.update(cx, |this, cx| {
                            this.selected = None;
                            cx.notify();
                        });
                    }
                },
                cx,
            )
        });
    }

    fn select_existing(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.preview {
            return;
        }
        self.selected = Some(id.to_string());
        self.check_ready(window, cx);
        cx.notify();
    }

    fn download_status(&self, id: &str, cx: &App) -> CardStatus {
        let store = self.store.read(cx);
        if store.extracting.contains(id) {
            CardStatus::Extracting
        } else if store.verifying.contains(id) {
            CardStatus::Verifying
        } else if store.downloading.contains(id) {
            CardStatus::Downloading
        } else {
            CardStatus::Downloadable
        }
    }

    fn download_card(
        &self,
        model: &ModelInfo,
        featured: bool,
        cx: &mut Context<Self>,
    ) -> ModelCard {
        let store = self.store.read(cx);
        let progress = store.progress.get(&model.id).map(|p| p.percentage);
        let speed = store.stats.get(&model.id).map(|s| s.speed);
        let select = cx.entity().downgrade();
        let download = select.clone();
        let cancel = select.clone();
        ModelCard::new(model.clone(), self.download_status(&model.id, cx))
            .featured(featured)
            .disabled(self.selected.is_some())
            .show_recommended(false)
            .progress(progress, speed)
            .on_select(move |id, _, cx| {
                let _ = select.update(cx, |this, cx| this.download(id, cx));
            })
            .on_download(move |id, _, cx| {
                let _ = download.update(cx, |this, cx| this.download(id, cx));
            })
            .on_cancel(move |id, _, cx| {
                let _ = cancel.update(cx, |this, cx| this.cancel(id, cx));
            })
    }

    fn existing_card(&self, model: &ModelInfo, cx: &mut Context<Self>) -> ModelCard {
        let status = if self.selected.as_deref() == Some(model.id.as_str()) {
            CardStatus::Switching
        } else {
            CardStatus::Available
        };
        let entity = cx.entity().downgrade();
        ModelCard::new(model.clone(), status)
            .disabled(self.selected.is_some())
            .show_recommended(false)
            .on_select(move |id, window, cx| {
                let _ = entity.update(cx, |this, cx| this.select_existing(id, window, cx));
            })
    }
}

fn section_title(text: SharedString, cx: &App) -> impl IntoElement {
    div()
        .sm_text()
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette(cx).text.a(0.6))
        .child(text)
}

impl Render for ModelOnboarding {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let models = self.store.read(cx).models.clone();

        // Legacy (.bin/ONNX) downloads are deprecated and never offered here.
        // The catalog arrives in rank order, so the first two recommended
        // models are the featured picks and the rest hide behind "Show all".
        let downloadable: Vec<&ModelInfo> = models
            .iter()
            .filter(|m| !m.is_downloaded && !is_legacy(m))
            .collect();
        let recommended: Vec<&ModelInfo> = downloadable
            .iter()
            .copied()
            .filter(|m| m.is_recommended)
            .collect();
        let rest: Vec<&ModelInfo> = downloadable
            .iter()
            .copied()
            .filter(|m| !m.is_recommended)
            .collect();
        let top_picks = &recommended[..recommended.len().min(2)];
        let other_recommended = &recommended[top_picks.len()..];
        let has_recommended = !recommended.is_empty();
        let show_rest = self.show_all || !has_recommended;
        let existing: Vec<&ModelInfo> = models.iter().filter(|m| m.is_downloaded).collect();

        let mut sections = div().flex().flex_col().gap_6().pb_6();
        if !existing.is_empty() {
            sections = sections.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(section_title(t("onboarding.existingModelsTitle"), cx))
                    .children(existing.iter().map(|m| self.existing_card(m, cx))),
            );
        }
        if !downloadable.is_empty() {
            let show_all = self.show_all;
            // `transition-transform duration-200` to `rotate-180`.
            let chevron = icon("chevron-down").size(rems(1.));
            let chevron = if self.toggled {
                chevron
                    .with_animation(
                        SharedString::from(format!("toggle-chevron-{show_all}")),
                        Animation::new(Duration::from_millis(200)),
                        move |icon, delta| {
                            let turn = if show_all { delta } else { 1. - delta };
                            icon.rotate(percentage(turn / 2.))
                        },
                    )
                    .into_any_element()
            } else {
                chevron.into_any_element()
            };
            let toggle = (has_recommended && !rest.is_empty()).then(|| {
                div()
                    .id("toggle-all-models")
                    .flex()
                    .row_dir()
                    .items_center()
                    .justify_center()
                    .gap(rems(0.375))
                    .mx_auto()
                    .py_1()
                    .sm_text()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.text.a(0.6))
                    .hover(|s| s.text_color(p.text))
                    .cursor_pointer()
                    .keyboard_focus(cx)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_all = !this.show_all;
                        this.toggled = true;
                        cx.notify();
                    }))
                    .child(if show_all {
                        t("onboarding.showFewerModels")
                    } else {
                        t_with(
                            "onboarding.showAllModels",
                            &[("total", &downloadable.len())],
                        )
                    })
                    .child(chevron)
            });
            sections = sections.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(section_title(t("onboarding.downloadModelsTitle"), cx))
                    .children(top_picks.iter().map(|m| self.download_card(m, true, cx)))
                    .children(
                        other_recommended
                            .iter()
                            .map(|m| self.download_card(m, false, cx)),
                    )
                    .children(toggle)
                    .when(show_rest, |el| {
                        el.children(rest.iter().map(|m| self.download_card(m, false, cx)))
                    }),
            );
        }

        div()
            .size_full()
            .relative()
            .child(
                div()
                    .id("model-onboarding")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        div()
                            .min_h_full()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap_4()
                            .p_6()
                            .pt(top_padding())
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap_2()
                                    .flex_shrink_0()
                                    .child(logo(200., cx))
                                    .child(
                                        div()
                                            .max_w(rems(28.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(p.text.a(0.7))
                                            .child(t("onboarding.subtitle")),
                                    ),
                            )
                            .child(
                                div()
                                    .max_w(gpui::px(600.))
                                    .w_full()
                                    .mx_auto()
                                    .child(sections),
                            ),
                    ),
            )
            .vertical_scrollbar(&self.scroll)
    }
}
