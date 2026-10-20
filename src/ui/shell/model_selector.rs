//! The model status line in the sidebar footer and its switcher menu
//! (`model-selector/`). The menu opens upward and lists downloaded models.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    anchored, canvas, deferred, div, point, prelude::*, px, rems, AnimationExt, App, Bounds,
    Context, Corner, Entity, FontWeight, Hsla, Pixels, SharedString, Window,
};

use crate::app_handle::{AppEvent, AppHandle};
use crate::i18n::{t, t_with};
use crate::managers::transcription::{ModelStateEvent, TranscriptionManager};
use crate::ui::components::{
    icon, pulse_animation, pulse_opacity, KeyboardFocus, RowDirection, Typography,
};
use crate::ui::event_bus::event_bus;
use crate::ui::format::{model_description, model_name};
use crate::ui::stores::ModelStore;
use crate::ui::theme::{palette, shadow_lg, tw, Alpha};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModelStatus {
    Ready,
    Loading,
    Downloading,
    Verifying,
    Extracting,
    Error,
    Unloaded,
    None,
}

pub struct ModelSelector {
    store: Entity<ModelStore>,
    status: ModelStatus,
    error: Option<SharedString>,
    open: bool,
    /// Optimistic selection shown until the switch finishes.
    pending_model: Option<String>,
    trigger: Bounds<Pixels>,
    checked_model: Option<String>,
}

impl ModelSelector {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let store = ModelStore::global(cx);
        cx.observe(&store, |this, store, cx| {
            let current = store.read(cx).current_model.clone();
            if this.checked_model.as_ref() != Some(&current) {
                this.check_status(cx);
            }
            cx.notify();
        })
        .detach();
        cx.subscribe(&event_bus(cx), |this, _, event: &AppEvent, cx| {
            this.handle_event(event, cx)
        })
        .detach();

        let mut this = Self {
            store,
            status: ModelStatus::Unloaded,
            error: None,
            open: false,
            pending_model: None,
            trigger: Bounds::default(),
            checked_model: None,
        };
        this.check_status(cx);
        this
    }

    /// Whether the selected model is the one loaded in memory.
    fn check_status(&mut self, cx: &mut Context<Self>) {
        let current = self.store.read(cx).current_model.clone();
        self.checked_model = Some(current.clone());
        if current.is_empty() {
            self.status = ModelStatus::None;
            return;
        }
        let loaded = AppHandle::global()
            .try_state::<Arc<TranscriptionManager>>()
            .and_then(|tm| tm.get_current_model());
        self.status = if loaded.as_deref() == Some(current.as_str()) {
            ModelStatus::Ready
        } else {
            ModelStatus::Unloaded
        };
    }

    fn handle_event(&mut self, event: &AppEvent, cx: &mut Context<Self>) {
        match event.name.as_str() {
            "model-state-changed" => {
                let Some(state) = event.parse::<ModelStateEvent>() else {
                    return;
                };
                match state.event_type.as_str() {
                    "loading_started" => {
                        self.status = ModelStatus::Loading;
                        self.error = None;
                    }
                    "loading_completed" => {
                        self.status = ModelStatus::Ready;
                        self.error = None;
                        self.pending_model = None;
                    }
                    "loading_failed" => {
                        self.status = ModelStatus::Error;
                        self.error = Some(
                            state
                                .error
                                .unwrap_or_else(|| "Failed to load model".into())
                                .into(),
                        );
                        self.pending_model = None;
                    }
                    "unloaded" => {
                        self.status = ModelStatus::Unloaded;
                        self.error = None;
                    }
                    _ => {}
                }
                cx.notify();
            }
            // Select a model automatically once its download (and any
            // extraction) finishes, unless the user is recording.
            "model-download-complete" => {
                let Some(model_id) = event.payload.as_str().map(str::to_string) else {
                    return;
                };
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(500))
                        .await;
                    let recording =
                        crate::commands::audio::is_recording(AppHandle::global().clone());
                    if recording {
                        return;
                    }
                    let _ = this.update(cx, |this, cx| {
                        this.pending_model = Some(model_id.clone());
                        this.error = None;
                        this.open = false;
                        let weak = cx.entity().downgrade();
                        this.store.update(cx, |store, cx| {
                            store.select_model(
                                model_id,
                                move |ok, cx| {
                                    if !ok {
                                        let _ = weak.update(cx, |this, cx| {
                                            this.pending_model = None;
                                            cx.notify();
                                        });
                                    }
                                },
                                cx,
                            )
                        });
                        cx.notify();
                    });
                })
                .detach();
            }
            _ => {}
        }
    }

    fn select(&mut self, model_id: String, cx: &mut Context<Self>) {
        self.pending_model = Some(model_id.clone());
        self.error = None;
        self.open = false;
        let weak = cx.entity().downgrade();
        self.store.update(cx, |store, cx| {
            store.select_model(
                model_id,
                move |ok, cx| {
                    if !ok {
                        let _ = weak.update(cx, |this, cx| {
                            this.pending_model = None;
                            this.status = ModelStatus::Error;
                            this.error = Some("Failed to switch model".into());
                            cx.notify();
                        });
                    }
                },
                cx,
            )
        });
        cx.notify();
    }

    fn display_status(&self, cx: &App) -> ModelStatus {
        let store = self.store.read(cx);
        if !store.verifying.is_empty() {
            ModelStatus::Verifying
        } else if !store.extracting.is_empty() {
            ModelStatus::Extracting
        } else if !store.progress.is_empty() {
            ModelStatus::Downloading
        } else {
            self.status
        }
    }

    fn display_text(&self, cx: &App) -> SharedString {
        let store = self.store.read(cx);
        let name_of = |id: &str| store.model(id).map(model_name);

        if !store.verifying.is_empty() {
            return if store.verifying.len() == 1 {
                let id = store.verifying.iter().next().unwrap();
                let name = name_of(id).unwrap_or_else(|| {
                    t("modelSelector.verifyingGeneric")
                        .replace("...", "")
                        .into()
                });
                t_with("modelSelector.verifying", &[("modelName", &name)])
            } else {
                t("modelSelector.verifyingGeneric")
            };
        }
        if !store.extracting.is_empty() {
            return if store.extracting.len() == 1 {
                let id = store.extracting.iter().next().unwrap();
                let name = name_of(id).unwrap_or_else(|| {
                    t("modelSelector.extractingGeneric")
                        .replace("...", "")
                        .into()
                });
                t_with("modelSelector.extracting", &[("modelName", &name)])
            } else {
                t_with(
                    "modelSelector.extractingMultiple",
                    &[("count", &store.extracting.len())],
                )
            };
        }
        if !store.progress.is_empty() {
            return if store.progress.len() == 1 {
                let progress = store.progress.values().next().unwrap();
                let percentage = progress.percentage.round().clamp(0., 100.) as i64;
                t_with("modelSelector.downloading", &[("percentage", &percentage)])
            } else {
                t_with(
                    "modelSelector.downloadingMultiple",
                    &[("count", &store.progress.len())],
                )
            };
        }

        let display_id = self
            .pending_model
            .clone()
            .unwrap_or_else(|| store.current_model.clone());
        let current = store.model(&display_id);
        match self.status {
            ModelStatus::Ready => current
                .map(model_name)
                .unwrap_or_else(|| t("modelSelector.modelReady")),
            ModelStatus::Loading => match current {
                Some(model) => t_with(
                    "modelSelector.loading",
                    &[("modelName", &model_name(model))],
                ),
                None => t("modelSelector.loadingGeneric"),
            },
            ModelStatus::Error => self
                .error
                .clone()
                .unwrap_or_else(|| t("modelSelector.modelError")),
            ModelStatus::None => t("modelSelector.noModelDownloadRequired"),
            _ => current
                .map(model_name)
                .unwrap_or_else(|| t("modelSelector.modelUnloaded")),
        }
    }

    fn render_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let store = self.store.read(cx);
        let display_id = self
            .pending_model
            .clone()
            .unwrap_or_else(|| store.current_model.clone());
        let downloaded: Vec<_> = store
            .models
            .iter()
            .filter(|m| m.is_downloaded)
            .cloned()
            .collect();
        let trigger = self.trigger;

        let menu = div()
            .id("model-menu")
            .occlude()
            .w(rems(16.))
            .max_h(px(600.))
            .overflow_y_scroll()
            .bg(p.background)
            .border_1()
            .border_color(p.mid_gray.a(0.2))
            .rounded_lg()
            .shadow(shadow_lg())
            .py_2()
            .on_mouse_down_out(
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    if !trigger.contains(&event.position) {
                        this.open = false;
                        cx.notify();
                    }
                }),
            )
            .when(downloaded.is_empty(), |el| {
                el.child(
                    div()
                        .px_3()
                        .py_2()
                        .sm_text()
                        .text_color(p.text.a(0.6))
                        .child(t("modelSelector.noModelsAvailable")),
                )
            })
            .children(downloaded.into_iter().map(|model| {
                let active = model.id == display_id;
                let id = model.id.clone();
                div()
                    .id(SharedString::from(format!("model-menu-{}", model.id)))
                    .w_full()
                    .px_3()
                    .py_2()
                    .cursor_pointer()
                    .hover(|s| s.bg(p.mid_gray.a(0.1)))
                    .when(active, |el| el.bg(p.mid_gray.a(0.1)).text_color(p.text))
                    .keyboard_focus(cx)
                    .on_click(cx.listener(move |this, _, _, cx| this.select(id.clone(), cx)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .row_dir()
                            .child(
                                div()
                                    .child(
                                        div()
                                            .sm_text()
                                            .text_color(p.text.a(0.8))
                                            .child(model_name(&model))
                                            .when(model.is_custom, |el| {
                                                el.child(badge_text(
                                                    t("modelSelector.custom"),
                                                    p.text.a(0.4),
                                                ))
                                            })
                                            .when(model.supports_streaming, |el| {
                                                el.child(badge_text(
                                                    t("modelSelector.streaming"),
                                                    p.accent.a(0.7),
                                                ))
                                            }),
                                    )
                                    .child(
                                        div()
                                            .xs_text()
                                            .text_color(p.text.a(0.4))
                                            .italic()
                                            .pe(rems(1.))
                                            .child(model_description(&model)),
                                    ),
                            )
                            .when(active, |el| {
                                el.child(
                                    div()
                                        .xs_text()
                                        .text_color(p.accent)
                                        .child(t("modelSelector.active")),
                                )
                            }),
                    )
            }));

        deferred(
            // `start-0`: the menu lines up with the trigger's inline start.
            anchored()
                .anchor(if crate::i18n::is_rtl() {
                    Corner::BottomRight
                } else {
                    Corner::BottomLeft
                })
                .position(point(
                    if crate::i18n::is_rtl() {
                        trigger.right()
                    } else {
                        trigger.left()
                    },
                    trigger.top() - px(7.5),
                ))
                .snap_to_window_with_margin(px(8.))
                .child(menu),
        )
        .with_priority(1)
    }
}

/// `ms-1.5 text-[10px] font-medium uppercase` label after a model name.
fn badge_text(text: SharedString, color: Hsla) -> gpui::Div {
    div()
        .ms(rems(0.375))
        .text_px(10.)
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .child(text.to_uppercase())
}

impl Render for ModelSelector {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let status = self.display_status(cx);
        let text = self.display_text(cx);
        let (dot, pulsing) = match status {
            ModelStatus::Ready => (tw::green_400(), false),
            ModelStatus::Loading => (tw::yellow_400(), true),
            ModelStatus::Downloading => (p.accent, true),
            ModelStatus::Verifying | ModelStatus::Extracting => (tw::orange_400(), true),
            ModelStatus::Error | ModelStatus::None => (tw::red_400(), false),
            ModelStatus::Unloaded => (p.mid_gray.a(0.6), false),
        };
        let dot = div()
            .size(rems(0.5))
            .mt_1()
            .flex_none()
            .rounded_full()
            .bg(dot);
        let dot = if pulsing {
            dot.with_animation("model-dot", pulse_animation(), |el, delta| {
                el.opacity(pulse_opacity(delta))
            })
            .into_any_element()
        } else {
            dot.into_any_element()
        };

        let store = self.store.read(cx);
        let single_download = (store.progress.len() == 1)
            .then(|| store.progress.values().next().cloned())
            .flatten();
        let download_count = store.progress.len();
        let speed = single_download
            .as_ref()
            .and_then(|progress| store.stats.get(&progress.model_id))
            .map(|stats| stats.speed)
            .unwrap_or(0.);
        let all_progress: Vec<f32> = store
            .progress
            .values()
            .map(|progress| progress.percentage as f32)
            .collect();

        let open = self.open;
        div()
            .flex()
            .flex_col()
            .gap(rems(0.375))
            .child(
                div()
                    .relative()
                    .child(
                        div()
                            .id("model-status")
                            .flex()
                            .row_dir()
                            .items_start()
                            .gap_2()
                            .w_full()
                            .cursor_pointer()
                            .hover(|s| s.text_color(p.text.a(0.8)))
                            .tooltip({
                                let text = text.clone();
                                move |window, cx| {
                                    crate::ui::components::Tooltip::text(
                                        format!("Model status: {text}"),
                                        window,
                                        cx,
                                    )
                                }
                            })
                            .keyboard_focus(cx)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open = !this.open;
                                cx.notify();
                            }))
                            .child(dot)
                            .child(div().flex_1().min_w_0().line_height(rems(1.)).child(text))
                            .child(
                                icon("chevron-down-thin")
                                    .size(rems(0.75))
                                    .mt(rems(0.125))
                                    .flex_none()
                                    .text_color(p.text.a(0.6))
                                    .when(open, |svg| svg.rotate(gpui::percentage(0.5))),
                            ),
                    )
                    .child(
                        canvas(
                            {
                                let entity = cx.entity().downgrade();
                                move |bounds, _, cx| {
                                    let _ = entity.update(cx, |this, _| this.trigger = bounds);
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .when(open, |el| el.child(self.render_menu(cx))),
            )
            .when(download_count > 0, |el| {
                if download_count == 1 {
                    let percent = all_progress[0];
                    el.child(
                        div()
                            .flex()
                            .row_dir()
                            .items_center()
                            .gap_3()
                            .child(crate::ui::components::progress_track(
                                percent,
                                rems(5.),
                                rems(0.375).to_pixels(crate::ui::theme::REM),
                                cx,
                            ))
                            .child(div().xs_text().text_color(p.text.a(0.6)).child(
                                if speed > 0. {
                                    format!("{speed:.1}MB/s")
                                } else {
                                    "Downloading...".to_string()
                                },
                            )),
                    )
                } else {
                    el.child(
                        div()
                            .flex()
                            .row_dir()
                            .items_center()
                            .gap_2()
                            .children(all_progress.iter().map(|percent| {
                                crate::ui::components::progress_track(
                                    *percent,
                                    rems(0.75),
                                    rems(0.375).to_pixels(crate::ui::theme::REM),
                                    cx,
                                )
                            }))
                            .child(
                                div()
                                    .xs_text()
                                    .text_color(p.text.a(0.6))
                                    .child(format!("{download_count} downloading...")),
                            ),
                    )
                }
            })
    }
}
