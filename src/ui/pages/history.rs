//! The History page (`history/HistorySettings.tsx`): recordings grouped by
//! day, newest first, loaded 30 at a time as the list scrolls.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Local, TimeZone};
use gpui::{
    canvas, div, prelude::*, px, rems, Animation, AnimationExt, App, Context, Entity, Focusable,
    FontWeight, SharedString, Subscription, Task, Window,
};
use gpui_component::input::{InputEvent, InputState};

use crate::app_handle::{AppEvent, AppHandle};
use crate::i18n::t;
use crate::managers::history::{HistoryEntry, HistoryManager, HistoryUpdatePayload};
use crate::managers::transcription::TranscriptionManager;
use crate::ui::audio_player::{self, audio_player, PlayerState};
use crate::ui::components::{
    focus_handle_for, icon, Button, ButtonSize, ButtonVariant, FocusVisible, RowDirection,
    SettingContainer, SettingsGroup, TextField, Tooltip, Typography,
};
use crate::ui::dates;
use crate::ui::event_bus::event_bus;
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{palette, Alpha};
use crate::ui::toast;

use super::rows;

const PAGE_SIZE: usize = 30;

pub struct HistoryPage {
    entries: Vec<HistoryEntry>,
    loading: bool,
    loading_more: bool,
    has_more: bool,
    show_settings: bool,
    history_limit: Entity<InputState>,
    players: HashMap<i64, Entity<PlayerState>>,
    copied: HashSet<i64>,
    retrying: HashSet<i64>,
    /// Re-renders while a recording plays, to move the playhead.
    ticker: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

fn manager() -> Arc<HistoryManager> {
    AppHandle::global()
        .state::<Arc<HistoryManager>>()
        .inner()
        .clone()
}

impl HistoryPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let limit = SettingsStore::get(cx).history_limit.to_string();
        // `<input type="number" min="0">`: digits only.
        let history_limit = cx.new(|cx| {
            InputState::new(window, cx)
                .pattern(regex::Regex::new(r"^\d*$").expect("valid pattern"))
                .default_value(limit)
        });
        let subscriptions = vec![
            // The field follows the stored value while it is not being edited.
            cx.observe_in(
                &SettingsStore::global(cx),
                window,
                |this, store, window, cx| {
                    let stored = store.read(cx).settings.history_limit.to_string();
                    let input = this.history_limit.clone();
                    let editing = input.read(cx).focus_handle(cx).is_focused(window);
                    if !editing && input.read(cx).value().as_ref() != stored {
                        input.update(cx, |input, cx| input.set_value(stored, window, cx));
                    }
                },
            ),
            cx.subscribe(&event_bus(cx), |this, _, event: &AppEvent, cx| {
                if event.name == HistoryUpdatePayload::EVENT {
                    if let Some(update) = event.parse::<HistoryUpdatePayload>() {
                        this.apply_update(update, cx);
                    }
                }
            }),
            cx.subscribe(&history_limit, |_, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    // Ignore anything that is not a non-negative number.
                    if let Ok(limit) = input.read(cx).value().trim().parse::<usize>() {
                        save_history_limit(limit, cx);
                    }
                }
            }),
        ];
        let mut this = Self {
            entries: Vec::new(),
            loading: true,
            loading_more: false,
            has_more: true,
            show_settings: false,
            history_limit,
            players: HashMap::new(),
            copied: HashSet::new(),
            retrying: HashSet::new(),
            ticker: None,
            _subscriptions: subscriptions,
        };
        this.load_page(None, cx);
        this
    }

    fn load_page(&mut self, cursor: Option<i64>, cx: &mut Context<Self>) {
        if cursor.is_some() && self.loading_more {
            return;
        }
        if cursor.is_none() {
            self.loading = true;
        } else {
            self.loading_more = true;
        }
        cx.spawn(async move |this, cx| {
            let result = crate::runtime::run(async move {
                manager().get_history_entries(cursor, Some(PAGE_SIZE)).await
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(page) => {
                        if cursor.is_none() {
                            this.entries = page.entries;
                        } else {
                            this.entries.extend(page.entries);
                        }
                        this.has_more = page.has_more;
                    }
                    Err(e) => log::error!("Failed to load history: {e}"),
                }
                this.loading = false;
                this.loading_more = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_update(&mut self, update: HistoryUpdatePayload, cx: &mut Context<Self>) {
        match update {
            HistoryUpdatePayload::Added { entry } => self.entries.insert(0, entry),
            HistoryUpdatePayload::Updated { entry } => {
                if let Some(existing) = self.entries.iter_mut().find(|e| e.id == entry.id) {
                    *existing = entry;
                }
            }
            // Applied optimistically when the user acted.
            HistoryUpdatePayload::Deleted { .. } | HistoryUpdatePayload::Toggled { .. } => {}
        }
        cx.notify();
    }

    fn copy(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.iter().find(|e| e.id == id) else {
            return;
        };
        if entry.transcription_text.trim().is_empty() {
            return;
        }
        if AppHandle::global()
            .clipboard()
            .write_text(&entry.transcription_text)
            .is_err()
        {
            toast::error(t("settings.history.copyError"), cx);
            return;
        }
        self.copied.insert(id);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let _ = this.update(cx, |this, cx| {
                this.copied.remove(&id);
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_saved(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) else {
            return;
        };
        let original = entry.saved;
        entry.saved = !original;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result =
                crate::runtime::run(async move { manager().toggle_saved_status(id).await }).await;
            if result.is_err() {
                let _ = this.update(cx, |this, cx| {
                    if let Some(entry) = this.entries.iter_mut().find(|e| e.id == id) {
                        entry.saved = original;
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn delete(&mut self, id: i64, cx: &mut Context<Self>) {
        self.entries.retain(|e| e.id != id);
        self.players.remove(&id);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = crate::runtime::run(async move { manager().delete_entry(id).await }).await;
            if let Err(e) = result {
                log::error!("Failed to delete entry: {e}");
                // Reload so the list matches what is stored.
                let _ = this.update(cx, |this, cx| this.load_page(None, cx));
            }
        })
        .detach();
    }

    fn retry(&mut self, id: i64, cx: &mut Context<Self>) {
        self.retrying.insert(id);
        cx.notify();
        let app = AppHandle::global().clone();
        cx.spawn(async move |this, cx| {
            let result = crate::runtime::run(async move {
                let hm = app.state::<Arc<HistoryManager>>();
                let tm = app.state::<Arc<TranscriptionManager>>();
                crate::commands::history::retry_history_entry_transcription(app.clone(), hm, tm, id)
                    .await
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                this.retrying.remove(&id);
                if let Err(e) = result {
                    log::error!("Failed to re-transcribe: {e}");
                    toast::error(t("settings.history.retranscribeError"), cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn player_state(&mut self, id: i64, cx: &mut Context<Self>) -> Entity<PlayerState> {
        self.players
            .entry(id)
            .or_insert_with(|| cx.new(|_| PlayerState::default()))
            .clone()
    }

    /// Keeps the playhead moving while any recording plays.
    fn ensure_ticker(&mut self, cx: &mut Context<Self>) {
        if self.ticker.is_some() {
            return;
        }
        self.ticker = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
            let alive = this.update(cx, |_, cx| cx.notify()).is_ok();
            if !alive {
                break;
            }
        }));
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let settings = SettingsStore::get(cx).clone();
        let limit_updating = SettingsStore::global(cx)
            .read(cx)
            .is_updating("history_limit");
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .row_dir()
                    .justify_flex_end()
                    .gap_1()
                    .child(
                        Button::new("history-settings")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Sm)
                            .gap(rems(0.375))
                            .text_color(p.text.a(0.6))
                            .hover(move |s| s.text_color(p.text))
                            .child(icon("sliders-horizontal").size(rems(0.875)))
                            .child(t("sidebar.settings"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_settings = !this.show_settings;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("open-recordings")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Sm)
                            .gap(rems(0.375))
                            .text_color(p.text.a(0.6))
                            .hover(move |s| s.text_color(p.text))
                            .title(t("settings.history.openFolder"))
                            .child(icon("folder-open").size(rems(0.875)))
                            .child(t("settings.history.openFolder"))
                            .on_click(|_, _, _| {
                                if let Err(e) = crate::commands::open_recordings_folder(
                                    AppHandle::global().clone(),
                                ) {
                                    log::error!("Failed to open recordings folder: {e}");
                                }
                            }),
                    ),
            )
            .when(self.show_settings, |el| {
                el.child(
                    SettingsGroup::new()
                        .child(
                            SettingContainer::new(t("settings.debug.historyLimit.title"))
                                .description(t("settings.debug.historyLimit.description"))
                                .child(
                                    div()
                                        .flex()
                                        .row_dir()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                // Arrow keys step like a number input's
                                                // spinner, within `min="0" max="1000"`.
                                                .capture_key_down(cx.listener(
                                                    |this, event: &gpui::KeyDownEvent, window, cx| {
                                                        let step: i64 = match event.keystroke.key.as_str() {
                                                            "up" => 1,
                                                            "down" => -1,
                                                            _ => return,
                                                        };
                                                        cx.stop_propagation();
                                                        let current = this
                                                            .history_limit
                                                            .read(cx)
                                                            .value()
                                                            .trim()
                                                            .parse::<i64>()
                                                            .unwrap_or(0);
                                                        let next = (current + step).clamp(0, 1000);
                                                        this.history_limit.update(cx, |input, cx| {
                                                            input.set_value(next.to_string(), window, cx)
                                                        });
                                                        save_history_limit(next as usize, cx);
                                                    },
                                                ))
                                                .child(
                                                    TextField::new(&self.history_limit)
                                                        .disabled(limit_updating)
                                                        .w(rems(5.)),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .sm_text()
                                                .text_color(p.text)
                                                .child(t("settings.debug.historyLimit.entries")),
                                        ),
                                ),
                        )
                        .child(rows::recording_retention(&settings, cx)),
                )
            })
    }

    fn render_entry(
        &mut self,
        entry: &HistoryEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = palette(cx);
        let id = entry.id;
        let has_text = !entry.transcription_text.trim().is_empty();
        let retrying = self.retrying.contains(&id);
        let time = Local
            .timestamp_opt(entry.timestamp, 0)
            .single()
            .map(dates::time_label)
            .unwrap_or_default();
        let path: PathBuf = manager().get_audio_file_path(&entry.file_name);
        if audio_player::is_playing(&path) {
            self.ensure_ticker(cx);
        }
        let player_state = self.player_state(id, cx);
        let file_name = entry.file_name.clone();
        let player = audio_player(
            SharedString::from(format!("player-{id}")),
            Some(path),
            player_state,
            move |_| Some(manager().get_audio_file_path(&file_name)),
            window,
            cx,
        );

        let text = if retrying {
            div()
                .sm_text()
                .line_height(px(24.))
                .child(t("settings.history.transcribing"))
                .with_animation(
                    SharedString::from(format!("retry-pulse-{id}")),
                    gpui::Animation::new(Duration::from_secs(3)).repeat(),
                    move |el, delta| {
                        let wave = (1. - (delta * std::f32::consts::TAU).cos()) / 2.;
                        el.text_color(p.text.a(0.4 + 0.5 * wave))
                    },
                )
                .into_any_element()
        } else if has_text {
            div()
                .sm_text()
                .line_height(px(24.))
                .whitespace_normal()
                .child(entry.transcription_text.clone())
                .into_any_element()
        } else {
            div()
                .sm_text()
                .line_height(px(24.))
                .text_color(p.text.a(0.4))
                .child(t("settings.history.transcriptionFailed"))
                .into_any_element()
        };

        let focus = FocusVisible::ring(cx);
        let handles: HashMap<&'static str, gpui::FocusHandle> = ["copy", "save", "retry", "delete"]
            .into_iter()
            .map(|name| {
                let key = SharedString::from(format!("{name}-{id}"));
                (name, focus_handle_for(key.into(), cx))
            })
            .collect();
        let icon_button = |name: &'static str,
                           glyph: &'static str,
                           title: SharedString,
                           active: bool,
                           disabled: bool,
                           spin: bool| {
            let button = div()
                .id(SharedString::from(format!("{name}-{id}")))
                .p(rems(0.375))
                .rounded_lg()
                .flex()
                .row_dir()
                .items_center()
                .justify_center();
            let button = if disabled {
                button.text_color(p.text.a(0.2)).cursor_not_allowed()
            } else {
                focus
                    .apply_with(button, handles[name].clone())
                    .cursor_pointer()
                    .hover(|s| s.bg(p.mid_gray.a(0.15)))
                    .when(active, |el| el.text_color(p.accent))
                    .when(!active, |el| {
                        el.text_color(p.text.a(0.45))
                            .hover(|s| s.text_color(p.text))
                    })
            };
            button
                .tooltip(move |window, cx| Tooltip::text(title.clone(), window, cx))
                .child(if spin {
                    // `spin 1s linear infinite reverse`.
                    icon(glyph)
                        .size(px(16.))
                        .with_animation(
                            SharedString::from(format!("{name}-spin-{id}")),
                            Animation::new(Duration::from_secs(1)).repeat(),
                            |icon, delta| icon.rotate(gpui::percentage(-delta)),
                        )
                        .into_any_element()
                } else {
                    icon(glyph).size(px(16.)).into_any_element()
                })
        };

        let copied = self.copied.contains(&id);
        div()
            .id(SharedString::from(format!("entry-{id}")))
            .flex()
            .row_dir()
            .gap_4()
            .px_5()
            .py_4()
            .hover(|s| s.bg(p.card.a(0.6)))
            .child(
                div()
                    .w(rems(4.))
                    .flex_shrink_0()
                    .text_px(13.)
                    .line_height(px(24.))
                    .text_color(p.text.a(0.5))
                    .child(time),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(text)
                    .child(
                        div()
                            .flex()
                            .row_dir()
                            .items_center()
                            .gap_3()
                            .child(div().flex_1().min_w_0().child(player))
                            .child(
                                div()
                                    .flex()
                                    .row_dir()
                                    .items_center()
                                    .flex_shrink_0()
                                    .me(rems(-0.375))
                                    .child(
                                        icon_button(
                                            "copy",
                                            if copied { "check" } else { "copy" },
                                            t("settings.history.copyToClipboard"),
                                            false,
                                            !has_text || retrying,
                                            false,
                                        )
                                        .when(
                                            has_text && !retrying,
                                            |el| {
                                                el.on_click(cx.listener(move |this, _, _, cx| {
                                                    this.copy(id, cx)
                                                }))
                                            },
                                        ),
                                    )
                                    .child(
                                        icon_button(
                                            "save",
                                            if entry.saved { "star-filled" } else { "star" },
                                            if entry.saved {
                                                t("settings.history.unsave")
                                            } else {
                                                t("settings.history.save")
                                            },
                                            entry.saved,
                                            retrying,
                                            false,
                                        )
                                        .when(
                                            !retrying,
                                            |el| {
                                                el.on_click(cx.listener(move |this, _, _, cx| {
                                                    this.toggle_saved(id, cx)
                                                }))
                                            },
                                        ),
                                    )
                                    .child(
                                        icon_button(
                                            "retry",
                                            "rotate-ccw",
                                            t("settings.history.retranscribe"),
                                            false,
                                            retrying,
                                            retrying,
                                        )
                                        .when(
                                            !retrying,
                                            |el| {
                                                el.on_click(cx.listener(move |this, _, _, cx| {
                                                    this.retry(id, cx)
                                                }))
                                            },
                                        ),
                                    )
                                    .child(
                                        icon_button(
                                            "delete",
                                            "trash-2",
                                            t("settings.history.delete"),
                                            false,
                                            retrying,
                                            false,
                                        )
                                        .when(
                                            !retrying,
                                            |el| {
                                                el.on_click(cx.listener(move |this, _, _, cx| {
                                                    this.delete(id, cx)
                                                }))
                                            },
                                        ),
                                    ),
                            ),
                    ),
            )
    }
}

/// Saves the history limit and trims old entries to it.
fn save_history_limit(limit: usize, cx: &mut App) {
    SettingsStore::global(cx).update(cx, |store, cx| {
        let app = store.app().clone();
        store.update_async(
            "history_limit",
            move |s| s.history_limit = limit,
            async move {
                let hm = app.state::<Arc<HistoryManager>>();
                crate::commands::history::update_history_limit(app.clone(), hm, limit).await
            },
            cx,
        )
    });
}

impl Render for HistoryPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let header = self.render_header(cx);
        let root = div()
            .max_w(rems(48.))
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(header);

        if self.loading || self.entries.is_empty() {
            return root.child(
                div()
                    .px_4()
                    .py_10()
                    .flex()
                    .row_dir()
                    .justify_center()
                    .sm_text()
                    .text_color(p.text.a(0.55))
                    .border_1()
                    .border_color(p.mid_gray.a(0.15))
                    .rounded_2xl()
                    .child(if self.loading {
                        t("settings.history.loading")
                    } else {
                        t("settings.history.empty")
                    }),
            );
        }

        // Entries arrive newest first, so each day's entries are contiguous.
        let mut groups: Vec<(chrono::NaiveDate, Vec<HistoryEntry>)> = Vec::new();
        for entry in &self.entries {
            let date = Local
                .timestamp_opt(entry.timestamp, 0)
                .single()
                .map(|d| d.date_naive())
                .unwrap_or_default();
            match groups.last_mut() {
                Some((day, list)) if *day == date => list.push(entry.clone()),
                _ => groups.push((date, vec![entry.clone()])),
            }
        }

        let mut sections = Vec::new();
        for (date, entries) in groups {
            let mut rendered = Vec::new();
            for (index, entry) in entries.iter().enumerate() {
                let row = self.render_entry(entry, window, cx);
                rendered.push(
                    div()
                        .flex()
                        .flex_col()
                        .when(index > 0, |el| {
                            el.border_t_1().border_color(p.mid_gray.a(0.15))
                        })
                        .child(row),
                );
            }
            sections.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .px_1()
                            .xs_text()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.text.a(0.55))
                            .child(dates::day_label(date).to_uppercase()),
                    )
                    .child(
                        div()
                            .border_1()
                            .border_color(p.mid_gray.a(0.15))
                            .rounded_2xl()
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .children(rendered),
                    ),
            );
        }

        // Load the next page once the end of the list scrolls into view.
        let has_more = self.has_more && !self.loading_more;
        let last_id = self.entries.last().map(|e| e.id);
        let entity = cx.entity().downgrade();
        let sentinel = canvas(
            move |bounds, window, cx| {
                if has_more && bounds.top() < window.viewport_size().height {
                    let entity = entity.clone();
                    cx.defer(move |cx| {
                        let _ = entity.update(cx, |this, cx| this.load_page(last_id, cx));
                    });
                }
            },
            |_, _, _, _| {},
        )
        .h_1()
        .w_full();

        root.child(div().flex().flex_col().gap_6().children(sections))
            .child(sentinel)
    }
}
