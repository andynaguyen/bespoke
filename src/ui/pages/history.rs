use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Local, TimeZone};
use gpui::{
    Animation, AnimationExt, AnyElement, Context, Entity, FontWeight, ScrollHandle, SharedString,
    Subscription, Task, Window, canvas, div, prelude::*, px, relative,
};
use gpui_component::scroll::ScrollableElement;

use crate::app_handle::{AppEvent, AppHandle};
use crate::i18n::t;
use crate::managers::history::{HistoryEntry, HistoryManager, HistoryUpdatePayload};
use crate::managers::transcription::TranscriptionManager;
use crate::ui::audio_player::{self, PlayerState, audio_player};
use crate::ui::components::{
    Button, ButtonSize, ButtonVariant, KeyboardFocus, RowDirection, Typography, icon,
};
use crate::ui::dates;
use crate::ui::event_bus::event_bus;
use crate::ui::shell::Toolbar;
use crate::ui::theme::{Alpha, CONTROL_HEIGHT, palette, radius};
use crate::ui::toast;

const PAGE_SIZE: usize = 30;
const INSPECTOR_WIDTH: f32 = 300.;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum CopyKind {
    Pasted,
    Raw,
}

pub struct HistoryPage {
    entries: Vec<HistoryEntry>,
    loading: bool,
    loading_more: bool,
    has_more: bool,
    selected: Option<i64>,
    last_inspected: Option<i64>,
    list_scroll: ScrollHandle,
    inspector_scroll: ScrollHandle,
    players: HashMap<i64, Entity<PlayerState>>,
    copied: HashSet<(i64, CopyKind)>,
    retrying: HashSet<i64>,
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
    pub fn new(cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.subscribe(&event_bus(cx), |this, _, event: &AppEvent, cx| {
                if event.name == HistoryUpdatePayload::EVENT
                    && let Some(update) = event.parse::<HistoryUpdatePayload>()
                {
                    this.apply_update(update, cx);
                }
            }),
        ];
        let mut this = Self {
            entries: Vec::new(),
            loading: true,
            loading_more: false,
            has_more: true,
            selected: None,
            last_inspected: None,
            list_scroll: ScrollHandle::new(),
            inspector_scroll: ScrollHandle::new(),
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
                manager().get_history_entries(cursor, PAGE_SIZE).await
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
        }
        cx.notify();
    }

    fn copy(&mut self, id: i64, kind: CopyKind, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.iter().find(|e| e.id == id) else {
            return;
        };
        let text = match kind {
            CopyKind::Raw => entry.transcription_text.trim(),
            CopyKind::Pasted => pasted_text(entry),
        }
        .to_string();
        if text.is_empty() {
            return;
        }
        if AppHandle::global().clipboard().write_text(&text).is_err() {
            toast::error(t("settings.history.copyError"), cx);
            return;
        }
        self.copied.insert((id, kind));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let _ = this.update(cx, |this, cx| {
                this.copied.remove(&(id, kind));
                cx.notify();
            });
        })
        .detach();
    }

    fn select(&mut self, id: Option<i64>, cx: &mut Context<Self>) {
        self.selected = id;
        cx.notify();
    }

    fn delete(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(index) = self.entries.iter().position(|e| e.id == id) else {
            return;
        };
        self.entries.remove(index);
        self.select(neighbor_of_removed(&self.entries, index), cx);
        self.players.remove(&id);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = crate::runtime::run(async move { manager().delete_entry(id).await }).await;
            if let Err(e) = result {
                log::error!("Failed to delete entry: {e}");
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

    fn player_state(
        &mut self,
        id: i64,
        path: &std::path::Path,
        cx: &mut Context<Self>,
    ) -> Entity<PlayerState> {
        self.players
            .entry(id)
            .or_insert_with(|| {
                let duration = audio_player::wav_duration(path);
                let state = cx.new(|_| {
                    let mut state = PlayerState::default();
                    state.duration = duration;
                    state
                });
                let path = path.to_path_buf();
                let loading = state.downgrade();
                cx.spawn(async move |_, cx| {
                    let peaks = cx
                        .background_executor()
                        .spawn(async move { audio_player::wav_peaks(&path, audio_player::BARS) })
                        .await;
                    let _ = loading.update(cx, |state, cx| {
                        state.peaks = peaks;
                        cx.notify();
                    });
                })
                .detach();
                state
            })
            .clone()
    }

    fn ensure_ticker(&mut self, cx: &mut Context<Self>) {
        if self.ticker.is_some() {
            return;
        }
        self.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let alive = this.update(cx, |_, cx| cx.notify()).is_ok();
                if !alive {
                    break;
                }
            }
        }));
    }

    pub fn toolbar_actions(&self, toolbar: Toolbar) -> Toolbar {
        // The actions sit in the inspector once entries load. While the first page loads, there's
        // no inspector yet, and putting them in the toolbar would flash them there for a frame.
        if self.loading || self.has_inspector() {
            return toolbar;
        }
        toolbar.action(open_folder_action())
    }

    fn has_inspector(&self) -> bool {
        !self.loading && !self.entries.is_empty()
    }

    fn inspected_entry(&self) -> Option<HistoryEntry> {
        self.has_inspector()
            .then(|| self.selected_entry().cloned())
            .flatten()
    }

    pub fn inspector(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let entry = self.inspected_entry();
        let inspected = entry.as_ref().map(|e| e.id);
        if self.last_inspected != inspected {
            self.last_inspected = inspected;
            audio_player::stop();
            self.inspector_scroll
                .set_offset(gpui::point(px(0.), px(0.)));
        }
        let entry = entry?;
        let actions = self.entry_actions(&entry, cx);
        let player = self.render_player(&entry, cx).into_any_element();
        let details = self.render_inspector(&entry, cx).into_any_element();
        Some(crate::ui::shell::inspector(
            "history-inspector",
            INSPECTOR_WIDTH,
            actions,
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex_none()
                        .px_4()
                        .pt(px(4.))
                        .pb(px(10.))
                        .child(player),
                )
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .child(
                            div()
                                .id("history-inspector")
                                .size_full()
                                .overflow_y_scroll()
                                .track_scroll(&self.inspector_scroll)
                                .px_4()
                                .pt(px(6.))
                                .pb(px(16.))
                                .child(details),
                        )
                        .vertical_scrollbar(&self.inspector_scroll),
                ),
        ))
    }

    fn selected_entry(&self) -> Option<&HistoryEntry> {
        self.selected
            .and_then(|id| self.entries.iter().find(|e| e.id == id))
            .or_else(|| self.entries.first())
    }

    fn render_row(
        &self,
        entry: &HistoryEntry,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let id = entry.id;
        let time = Local
            .timestamp_opt(entry.timestamp, 0)
            .single()
            .map(dates::time_label)
            .unwrap_or_default();
        let text = pasted_text(entry);
        let (text, failed) = if self.retrying.contains(&id) {
            (t("settings.history.transcribing"), true)
        } else if text.is_empty() {
            (t("settings.history.transcriptionFailed"), true)
        } else {
            (
                SharedString::from(text.split_whitespace().collect::<Vec<_>>().join(" ")),
                false,
            )
        };
        div()
            .id(SharedString::from(format!("entry-{id}")))
            .flex()
            .row_dir()
            .items_start()
            .gap_3()
            .mx(px(8.))
            .px(px(12.))
            .py(px(10.))
            .rounded(radius::ROW)
            .border_1()
            .cursor_pointer()
            .relative()
            .when(selected, |el| {
                el.bg(p.accent.a(0.16)).border_color(p.accent.a(0.3))
            })
            .when(!selected, |el| {
                el.border_color(gpui::transparent_black())
                    .hover(|s| s.bg(p.card))
            })
            .keyboard_focus(cx)
            .on_click(cx.listener(move |this, _, _, cx| this.select(Some(id), cx)))
            .child(
                div()
                    .w(px(64.))
                    .flex_none()
                    .text_px(12.)
                    .line_height(px(18.))
                    .text_color(p.mid_gray)
                    .child(time),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_px(13.)
                    .line_height(px(18.))
                    .line_clamp(2)
                    .text_ellipsis()
                    .text_color(if failed { p.mid_gray } else { p.text.a(0.85) })
                    .child(text),
            )
            .into_any_element()
    }

    fn entry_actions(&self, entry: &HistoryEntry, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let id = entry.id;
        let retrying = self.retrying.contains(&id);
        let has_text = !entry.transcription_text.trim().is_empty();
        let mut row = vec![
            icon_action(
                SharedString::from(format!("delete-{id}")),
                "trash-2",
                t("settings.history.delete"),
                false,
            )
            .disabled(retrying)
            .on_click(cx.listener(move |this, _, _, cx| this.delete(id, cx)))
            .into_any_element(),
        ];
        let file = manager().get_audio_file_path(&entry.file_name);
        row.push(
            icon_action(
                SharedString::from(format!("reveal-{id}")),
                "folder-open",
                t("annotate.library.showInFinder"),
                false,
            )
            .on_click(move |_, _, _| {
                if let Err(e) = std::process::Command::new("/usr/bin/open")
                    .arg("-R")
                    .arg(&file)
                    .spawn()
                {
                    log::error!("Failed to reveal {}: {e}", file.display());
                }
            })
            .into_any_element(),
        );
        let main = if has_text {
            row.push(
                icon_action(
                    SharedString::from(format!("retry-{id}")),
                    "rotate-ccw",
                    t("settings.history.retranscribe"),
                    retrying,
                )
                .disabled(retrying)
                .on_click(cx.listener(move |this, _, _, cx| this.retry(id, cx)))
                .into_any_element(),
            );
            let copied = self.copied.contains(&(id, CopyKind::Pasted));
            Button::new(SharedString::from(format!("copy-{id}")))
                .child(icon(if copied { "check" } else { "copy" }).size(px(13.)))
                .child(t("settings.history.copy"))
                .on_click(cx.listener(move |this, _, _, cx| this.copy(id, CopyKind::Pasted, cx)))
        } else {
            Button::new(SharedString::from(format!("retry-{id}")))
                .child(spinning("rotate-ccw", 13., format!("retry-{id}"), retrying))
                .child(t("settings.history.retranscribe"))
                .on_click(cx.listener(move |this, _, _, cx| this.retry(id, cx)))
        };
        row.push(
            main.gap(px(5.))
                .disabled(retrying)
                .when(retrying, |el| el.variant(ButtonVariant::Secondary))
                .into_any_element(),
        );
        row
    }

    fn render_player(&mut self, entry: &HistoryEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let id = entry.id;
        let path: PathBuf = manager().get_audio_file_path(&entry.file_name);
        if audio_player::is_playing(&path) {
            self.ensure_ticker(cx);
        } else {
            self.ticker = None;
        }
        let state = self.player_state(id, &path, cx);
        let file_name = entry.file_name.clone();
        audio_player(
            SharedString::from(format!("player-{id}")),
            Some(path),
            state,
            move |_| Some(manager().get_audio_file_path(&file_name)),
            cx,
        )
    }

    fn render_inspector(
        &mut self,
        entry: &HistoryEntry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = palette(cx);
        let id = entry.id;
        let retrying = self.retrying.contains(&id);
        let transcript = entry.transcription_text.trim();
        let post_processed = post_processed_text(entry);
        let duration = self
            .players
            .get(&id)
            .and_then(|state| state.read(cx).duration);

        let lead = div()
            .text_px(15.)
            .line_height(relative(1.5))
            .whitespace_normal();
        let lead = if retrying {
            lead.child(t("settings.history.transcribing"))
                .with_animation(
                    SharedString::from(format!("retry-pulse-{id}")),
                    gpui::Animation::new(Duration::from_secs(3)).repeat(),
                    move |el, delta| {
                        let wave = (1. - (delta * std::f32::consts::TAU).cos()) / 2.;
                        el.text_color(p.text.a(0.4 + 0.5 * wave))
                    },
                )
                .into_any_element()
        } else if transcript.is_empty() {
            lead.text_px(13.)
                .text_color(p.mid_gray)
                .child(t("settings.history.transcriptionFailed"))
                .into_any_element()
        } else {
            lead.text_color(p.text)
                .child(SharedString::from(pasted_text(entry).to_string()))
                .into_any_element()
        };

        let when = Local
            .timestamp_opt(entry.timestamp, 0)
            .single()
            .map(|recorded| {
                format!(
                    "{}, {}",
                    capitalize(&dates::short_day_label(recorded.date_naive())),
                    dates::time_label(recorded)
                )
            })
            .unwrap_or_default();
        let bundle_id = entry.bundle_id.clone().filter(|id| !id.is_empty());
        let app = entry
            .app_name
            .clone()
            .or_else(|| bundle_id.clone())
            .filter(|app| !app.is_empty());
        let header = div()
            .mb(px(14.))
            .flex()
            .row_dir()
            .items_center()
            .gap(px(10.))
            .when_some(app.clone(), |el, app| {
                // macOS icons carry a transparent margin, so a 36pt icon centered in a 30pt box
                // lines its visible edge up with the play button above, in either direction.
                el.child(
                    div()
                        .flex_none()
                        .size(px(30.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(crate::ui::app_icons::app_icon(
                            bundle_id.as_deref(),
                            &app,
                            px(36.),
                            cx,
                        )),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .when_some(app, |el, app| {
                        el.child(
                            div()
                                .text_px(13.)
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(app),
                        )
                    })
                    .child(
                        div()
                            .text_px(12.)
                            .text_color(p.mid_gray)
                            .truncate()
                            .child(when),
                    ),
            );

        let mut details: Vec<(SharedString, SharedString)> = Vec::new();
        if let Some(duration) = duration {
            details.push((
                t("settings.history.duration"),
                crate::annotate::model::format_clock(duration.as_millis() as u64).into(),
            ));
        }
        let words = crate::managers::history::count_words(pasted_text(entry));
        if words > 0 {
            details.push((
                t("settings.history.wordCount"),
                crate::ui::format::number(words).into(),
            ));
        }

        let pill = post_processed.filter(|_| !retrying).map(|_| {
            div().flex().row_dir().mb(px(10.)).child(
                div()
                    .min_w_0()
                    .h(px(22.))
                    .px(px(8.))
                    .flex()
                    .row_dir()
                    .items_center()
                    .gap(px(5.))
                    .rounded_full()
                    .bg(p.accent.a(0.12))
                    .text_color(accent_text(&p))
                    .text_px(11.5)
                    .font_weight(FontWeight::MEDIUM)
                    .child(icon("sparkles").size(px(11.)).flex_none())
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(t("settings.history.postProcessed")),
                    ),
            )
        });

        let raw_copied = self.copied.contains(&(id, CopyKind::Raw));
        let card = || {
            div()
                .rounded(px(10.))
                .bg(p.card)
                .border_1()
                .border_color(p.line)
        };
        let rows = details.len();
        div()
            .flex()
            .flex_col()
            .child(header)
            .children(pill)
            .child(lead)
            .when(
                post_processed.is_some_and(|text| text != transcript) && !retrying,
                |el| {
                    el.child(
                        card()
                            .mt(px(14.))
                            .px(px(10.))
                            .py(px(8.))
                            .child(
                                div()
                                    .mb(px(1.))
                                    .flex()
                                    .row_dir()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_px(11.)
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(p.mid_gray)
                                            .child(t("settings.history.transcript")),
                                    )
                                    .child(
                                        Button::new(SharedString::from(format!("copy-raw-{id}")))
                                            .variant(ButtonVariant::Ghost)
                                            .size(ButtonSize::Sm)
                                            .title(t("settings.history.copy"))
                                            .size_6()
                                            .px(px(0.))
                                            .pb(px(0.))
                                            .text_color(p.mid_gray)
                                            .child(
                                                icon(if raw_copied { "check" } else { "copy" })
                                                    .size(px(12.)),
                                            )
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.copy(id, CopyKind::Raw, cx)
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .text_px(13.)
                                    .line_height(relative(1.45))
                                    .text_color(p.text.a(0.7))
                                    .child(SharedString::from(transcript.to_string())),
                            ),
                    )
                },
            )
            .when(rows > 0, |el| {
                el.child(card().mt(px(18.)).flex().flex_col().text_px(12.5).children(
                    details.into_iter().enumerate().map(|(i, (label, value))| {
                        div()
                            .h(px(30.))
                            .px(px(10.))
                            .flex()
                            .row_dir()
                            .items_center()
                            .gap(px(10.))
                            .when(i + 1 < rows, |el| el.border_b_1().border_color(p.line))
                            .child(div().flex_none().text_color(p.mid_gray).child(label))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .row_dir()
                                    .justify_flex_end()
                                    .child(div().min_w_0().truncate().child(value)),
                            )
                    }),
                ))
            })
    }
}

fn post_processed_text(entry: &HistoryEntry) -> Option<&str> {
    entry
        .post_processed_text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty() && !entry.transcription_text.trim().is_empty())
}

fn pasted_text(entry: &HistoryEntry) -> &str {
    post_processed_text(entry).unwrap_or(entry.transcription_text.trim())
}

fn neighbor_of_removed(entries: &[HistoryEntry], removed_index: usize) -> Option<i64> {
    entries
        .get(removed_index)
        .or_else(|| removed_index.checked_sub(1).and_then(|i| entries.get(i)))
        .map(|e| e.id)
}

fn accent_text(p: &crate::ui::theme::Palette) -> gpui::Hsla {
    if p.is_dark {
        crate::ui::theme::mix(p.accent, p.text, 0.45)
    } else {
        p.accent
    }
}

fn icon_action(id: SharedString, glyph: &'static str, title: SharedString, spin: bool) -> Button {
    let key = id.clone();
    Button::new(id)
        .variant(ButtonVariant::Secondary)
        .title(title)
        .w(CONTROL_HEIGHT)
        .px(px(0.))
        .child(spinning(glyph, 14., key, spin))
}

fn spinning(name: &'static str, size: f32, key: impl std::fmt::Display, spin: bool) -> AnyElement {
    let glyph = icon(name).size(px(size));
    if spin {
        glyph
            .with_animation(
                SharedString::from(format!("{key}-spin")),
                Animation::new(Duration::from_secs(1)).repeat(),
                |icon, delta| icon.rotate(gpui::percentage(-delta)),
            )
            .into_any_element()
    } else {
        glyph.into_any_element()
    }
}

fn capitalize(label: &str) -> String {
    let mut chars = label.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn open_folder_action() -> AnyElement {
    let folder = t("settings.history.openFolder");
    Button::new("open-recordings")
        .variant(ButtonVariant::Secondary)
        .gap(px(5.))
        .title(folder.clone())
        .child(icon("folder-open").size(px(13.)))
        .child(folder)
        .on_click(|_, _, _| {
            if let Err(e) = crate::commands::open_recordings_folder(AppHandle::global().clone()) {
                log::error!("Failed to open recordings folder: {e}");
            }
        })
        .into_any_element()
}

impl Drop for HistoryPage {
    fn drop(&mut self) {
        audio_player::stop();
    }
}

impl Render for HistoryPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let root = div().size_full().flex().flex_col();

        if self.loading || self.entries.is_empty() {
            return root.child(
                div()
                    .px_8()
                    .py_16()
                    .flex()
                    .row_dir()
                    .justify_center()
                    .text_px(13.)
                    .text_color(p.mid_gray)
                    .child(if self.loading {
                        t("settings.history.loading")
                    } else {
                        t("settings.history.empty")
                    }),
            );
        }

        let selected_id = self.selected_entry().map(|e| e.id);
        let mut list: Vec<AnyElement> = Vec::new();
        let mut last_day = None;
        for entry in self.entries.clone() {
            let day = Local
                .timestamp_opt(entry.timestamp, 0)
                .single()
                .map(|d| d.date_naive())
                .unwrap_or_default();
            if last_day != Some(day) {
                last_day = Some(day);
                list.push(
                    div()
                        .px(px(20.))
                        .pt_4()
                        .pb(px(6.))
                        .text_px(11.)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.mid_gray)
                        .child(capitalize(&dates::day_label(day)))
                        .into_any_element(),
                );
            }
            list.push(self.render_row(&entry, Some(entry.id) == selected_id, cx));
        }

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

        root.child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .id("history-list")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.list_scroll)
                        .flex()
                        .flex_col()
                        .children(list)
                        .child(sentinel),
                )
                .vertical_scrollbar(&self.list_scroll),
        )
    }
}
