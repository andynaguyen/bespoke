//! The Annotate page's Captures tab: every capture in Annotate's folder as a
//! card, and one capture open in place of Scrawl's Inspect window. An open
//! capture shows its contact sheet or one keyframe on the content surface,
//! and its keyframes as a timeline in a glass panel, like History's
//! inspector.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Local, TimeZone};
use gpui::{
    div, img, prelude::*, px, AnyElement, App, Context, FontWeight, Global, ObjectFit,
    ScrollHandle, SharedString, Task,
};
use gpui_component::scroll::ScrollableElement;
use scrawl::host::KeyframeRow;

use crate::i18n::{t, t_count};
use crate::ui::components::{
    icon, Button, ButtonSize, ButtonVariant, KeyboardFocus, RowDirection, SegmentedControl,
    Typography,
};
use crate::ui::dates;
use crate::ui::shell::Toolbar;
use crate::ui::theme::{palette, radius, Alpha};

/// The most captures the page lists.
const LIMIT: usize = 120;
const INSPECTOR_WIDTH: f32 = 320.;
const COLUMNS: usize = 3;

/// A capture Scrawl asked to inspect, waiting for the page to open it.
#[derive(Default)]
struct PendingInspect(Option<PathBuf>);

impl Global for PendingInspect {}

/// Set by `inspect` so the Annotate page switches to its Captures tab.
#[derive(Default)]
pub struct PendingInspectFlag(pub bool);

impl Global for PendingInspectFlag {}

/// Shows the capture in `dir` on the Annotate page.
pub fn inspect(dir: PathBuf, cx: &mut App) {
    cx.default_global::<PendingInspect>().0 = Some(dir);
    cx.default_global::<PendingInspectFlag>().0 = true;
    crate::ui::show_section(crate::ui::shell::Section::Annotate, cx);
    cx.refresh_windows();
}

/// One capture, read from its manifest.
#[derive(Clone)]
struct Item {
    dir: PathBuf,
    title: SharedString,
    created_at_ms: u64,
    duration: SharedString,
    marks: usize,
    sheet: Option<PathBuf>,
    rows: Vec<KeyframeRow>,
    /// Width over height of the contact sheet, then of each keyframe.
    sheet_aspect: f32,
    row_aspects: Vec<f32>,
}

impl Item {
    fn read(capture: &scrawl::host::Capture) -> Self {
        let sheet = capture.contact_sheet(false).ok();
        let rows = capture.keyframe_rows();
        Self {
            dir: capture.dir().to_path_buf(),
            title: capture.target().into(),
            created_at_ms: capture.created_at_ms(),
            duration: capture.duration().into(),
            marks: capture.marks(),
            sheet_aspect: sheet.as_deref().map_or(1.6, png_aspect),
            row_aspects: rows.iter().map(|row| png_aspect(&row.path)).collect(),
            sheet,
            rows,
        }
    }

    /// The first keyframe, else the contact sheet.
    fn thumbnail(&self) -> Option<PathBuf> {
        self.rows
            .first()
            .map(|row| row.path.clone())
            .or_else(|| self.sheet.clone())
    }

    fn when(&self) -> String {
        let Some(time) = Local
            .timestamp_millis_opt(self.created_at_ms as i64)
            .single()
        else {
            return String::new();
        };
        format!(
            "{} {}",
            dates::day_label(time.date_naive()),
            dates::time_label(time)
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Sheet,
    Keyframe,
}

pub struct Captures {
    /// `None` until the first read finishes.
    items: Option<Vec<Item>>,
    open: Option<Item>,
    stage: Stage,
    keyframe: usize,
    copied: bool,
    /// The capture a reload is looking for.
    retried: Option<PathBuf>,
    stage_scroll: ScrollHandle,
    timeline_scroll: ScrollHandle,
    _load: Task<()>,
}

impl Captures {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            items: None,
            open: None,
            stage: Stage::Sheet,
            keyframe: 0,
            copied: false,
            retried: None,
            stage_scroll: ScrollHandle::new(),
            timeline_scroll: ScrollHandle::new(),
            _load: Task::ready(()),
        };
        this.reload(cx);
        this
    }

    /// Re-reads the captures folder, then opens a capture Scrawl asked for.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self._load = cx.spawn(async move |this, cx| {
            let items = crate::runtime::spawn_blocking(|| {
                scrawl::host::recent_captures(LIMIT)
                    .iter()
                    .map(Item::read)
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.items = Some(items);
                this.take_pending(cx);
                cx.notify();
            });
        });
    }

    /// Opens the capture `inspect` asked for. A capture that just finished
    /// may be newer than the list, so the folder is read once more before
    /// the request is dropped.
    pub fn take_pending(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = cx.default_global::<PendingInspect>().0.clone() else {
            return;
        };
        let Some(items) = &self.items else {
            return;
        };
        if let Some(item) = items.iter().find(|item| same_dir(&item.dir, &dir)).cloned() {
            cx.global_mut::<PendingInspect>().0 = None;
            self.retried = None;
            self.open_item(item, cx);
        } else if self.retried.as_deref() == Some(dir.as_path()) {
            cx.global_mut::<PendingInspect>().0 = None;
            self.retried = None;
        } else {
            self.retried = Some(dir);
            self.reload(cx);
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    fn open_item(&mut self, item: Item, cx: &mut Context<Self>) {
        self.open = Some(item);
        self.stage = Stage::Sheet;
        self.keyframe = 0;
        self.copied = false;
        self.stage_scroll = ScrollHandle::new();
        self.timeline_scroll = ScrollHandle::new();
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.open = None;
        cx.notify();
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        let Some(item) = &self.open else {
            return;
        };
        match scrawl::host::copy_capture(&item.dir) {
            Ok(()) => {
                self.copied = true;
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_secs(2)).await;
                    let _ = this.update(cx, |this, cx| {
                        this.copied = false;
                        cx.notify();
                    });
                })
                .detach();
            }
            Err(e) => {
                log::warn!("Copying the capture failed: {e}");
                crate::ui::toast::error(t("annotate.library.copyFailed"), cx);
            }
        }
        cx.notify();
    }

    /// The toolbar for the open capture: back, the stage tabs and the
    /// capture's summary.
    pub fn toolbar(&self, cx: &mut Context<Self>) -> Option<Toolbar> {
        let item = self.open.as_ref()?;
        let back = cx.entity().downgrade();
        let tabs = cx.entity().downgrade();
        let rows = item.rows.len();
        let meta = format!(
            "{} · {} · {}",
            item.title,
            item.duration,
            t_count("annotate.library.keyframes", rows as i64, &[])
        );
        Some(
            Toolbar::new()
                .tabs(
                    div()
                        .flex()
                        .row_dir()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("capture-back")
                                .variant(ButtonVariant::Secondary)
                                .w(px(32.))
                                .px(px(0.))
                                .title(t("annotate.library.back"))
                                .on_click(move |_, _, cx| {
                                    let _ = back.update(cx, |this, cx| this.close(cx));
                                })
                                .child(icon("chevron-left").size(px(16.))),
                        )
                        .child(SegmentedControl::new(
                            "capture-stage",
                            vec![
                                ("sheet".into(), t("annotate.library.contactSheet")),
                                ("keyframe".into(), t("annotate.library.keyframe")),
                            ],
                            match self.stage {
                                Stage::Sheet => "sheet",
                                Stage::Keyframe => "keyframe",
                            },
                            move |id, _, cx| {
                                let stage = if id.as_ref() == "sheet" {
                                    Stage::Sheet
                                } else {
                                    Stage::Keyframe
                                };
                                let _ = tabs.update(cx, |this, cx| {
                                    this.stage = stage;
                                    cx.notify();
                                });
                            },
                        )),
                )
                .meta(meta),
        )
    }

    /// Show in Finder and Copy for the open capture, at the top of its
    /// timeline.
    fn actions(&self, item: &Item, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let reveal_dir = item.dir.clone();
        let copy = cx.entity().downgrade();
        vec![
            super::annotate::icon_button(
                "capture-reveal",
                "folder-open",
                t("annotate.library.showInFinder"),
            )
            .size(ButtonSize::Md)
            .on_click(move |_, _, _| scrawl::host::reveal_capture(&reveal_dir))
            .into_any_element(),
            Button::new("capture-copy")
                .gap(px(6.))
                .on_click(move |_, _, cx| {
                    let _ = copy.update(cx, |this, cx| this.copy(cx));
                })
                .child(icon(if self.copied { "check" } else { "copy" }).size(px(14.)))
                .child(if self.copied {
                    t("annotate.library.copied")
                } else {
                    t("annotate.library.copy")
                })
                .into_any_element(),
        ]
    }

    fn card(&self, index: usize, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let open = item.clone();
        div()
            .id(("capture-card", index))
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .p(px(8.))
            .rounded(radius::CARD)
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .cursor_pointer()
            .hover(|s| s.bg(p.text.a(if p.is_dark { 0.1 } else { 0.06 })))
            .keyboard_focus(cx)
            .on_click(cx.listener(move |this, _, _, cx| this.open_item(open.clone(), cx)))
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(132.))
                    .rounded(px(10.))
                    .overflow_hidden()
                    .bg(p.text.a(0.06))
                    // Out of the layout, so the image's own size never widens the card.
                    .children(item.thumbnail().map(|path| {
                        img(path)
                            .absolute()
                            .inset_0()
                            .size_full()
                            .object_fit(ObjectFit::Cover)
                            .into_any_element()
                    }))
                    // Keyframes carry their own `#N` badge at the top left.
                    .child(
                        div()
                            .absolute()
                            .bottom(px(8.))
                            .right(px(8.))
                            .h(px(22.))
                            .px(px(8.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .bg(gpui::rgba(0x16161ab8))
                            .border_1()
                            .border_color(gpui::white().a(0.2))
                            .text_px(11.)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(gpui::white())
                            .child(item.duration.clone()),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .px(px(4.))
                    .pt(px(9.))
                    .pb(px(2.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_px(13.)
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(item.title.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .row_dir()
                            .gap_2()
                            .text_px(11.5)
                            .text_color(p.mid_gray)
                            .child(item.when())
                            .when(item.marks > 0, |el| {
                                el.child(t_count("annotate.capsule.marks", item.marks as i64, &[]))
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_library(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let Some(items) = self.items.clone() else {
            return div().into_any_element();
        };
        if items.is_empty() {
            return div()
                .w_full()
                .py_16()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_px(15.)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t("annotate.library.empty")),
                )
                .child(
                    div()
                        .text_px(13.)
                        .text_color(p.mid_gray)
                        .child(t("annotate.library.emptyHint")),
                )
                .into_any_element();
        }
        let rows: Vec<AnyElement> = items
            .chunks(COLUMNS)
            .enumerate()
            .map(|(row, chunk)| {
                let mut cards: Vec<AnyElement> = chunk
                    .iter()
                    .enumerate()
                    .map(|(column, item)| self.card(row * COLUMNS + column, item, cx))
                    .collect();
                // Keeps a short last row's cards as wide as the others.
                while cards.len() < COLUMNS {
                    cards.push(div().flex_1().into_any_element());
                }
                div()
                    .w_full()
                    .flex()
                    .row_dir()
                    .gap(px(14.))
                    .children(cards)
                    .into_any_element()
            })
            .collect();
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(14.))
            .children(rows)
            .into_any_element()
    }

    fn render_stage(&self, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let (image, aspect) = match self.stage {
            Stage::Sheet => (item.sheet.clone(), item.sheet_aspect),
            Stage::Keyframe => (
                item.rows.get(self.keyframe).map(|row| row.path.clone()),
                item.row_aspects.get(self.keyframe).copied().unwrap_or(1.6),
            ),
        };
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .id("capture-stage")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.stage_scroll)
                    .p(px(14.))
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .rounded(px(12.))
                            .overflow_hidden()
                            .border_1()
                            .border_color(p.line)
                            .map(|mut el| {
                                el.style().aspect_ratio = Some(aspect);
                                el
                            })
                            .children(image.map(|path| {
                                img(path)
                                    .absolute()
                                    .inset_0()
                                    .size_full()
                                    .object_fit(ObjectFit::Contain)
                                    .into_any_element()
                            })),
                    ),
            )
            .vertical_scrollbar(&self.stage_scroll)
            .into_any_element()
    }

    fn render_timeline(&self, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let rows: Vec<AnyElement> = item
            .rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let selected = self.stage == Stage::Keyframe && self.keyframe == i;
                div()
                    .id(("capture-keyframe", i))
                    .flex()
                    .row_dir()
                    .gap(px(10.))
                    .px(px(10.))
                    .py(px(9.))
                    .rounded(radius::ROW)
                    .border_1()
                    .cursor_pointer()
                    .when(selected, |el| {
                        el.bg(p.accent.a(0.16)).border_color(p.accent.a(0.3))
                    })
                    .when(!selected, |el| {
                        el.border_color(gpui::transparent_black())
                            .hover(|s| s.bg(p.card))
                    })
                    .keyboard_focus(cx)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.stage = Stage::Keyframe;
                        this.keyframe = i;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(40.))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .text_px(12.)
                            .font_weight(FontWeight::BOLD)
                            .child(format!("#{}", row.index))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(p.mid_gray)
                                    .child(row.clock.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .when(!row.action.is_empty(), |el| {
                                el.child(
                                    div().flex().row_dir().child(
                                        div()
                                            .px(px(7.))
                                            .rounded(px(8.))
                                            .text_px(11.5)
                                            .when(row.mark, |el| {
                                                el.bg(p.ink.a(0.16)).text_color(p.ink)
                                            })
                                            .when(!row.mark, |el| {
                                                el.bg(p.text.a(0.08)).text_color(p.mid_gray)
                                            })
                                            .truncate()
                                            .child(row.action.clone()),
                                    ),
                                )
                            })
                            .children(row.said.clone().map(|said| {
                                div()
                                    .text_px(13.)
                                    .line_height(px(18.))
                                    .child(format!("“{said}”"))
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        crate::ui::shell::inspector(
            "capture-timeline",
            INSPECTOR_WIDTH,
            self.actions(item, cx),
            div()
                .relative()
                .size_full()
                .child(
                    div()
                        .id("capture-timeline")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.timeline_scroll)
                        .px(px(8.))
                        .py(px(12.))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .px(px(10.))
                                .pb(px(6.))
                                .xs_text()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(p.mid_gray)
                                .child(t("annotate.library.timeline")),
                        )
                        .children(rows),
                )
                .vertical_scrollbar(&self.timeline_scroll),
        )
    }

    /// The open capture's timeline, for the shell to show along the
    /// window's edge (`Page::inspector`).
    pub fn inspector(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let item = self.open.clone()?;
        Some(self.render_timeline(&item, cx))
    }
}

/// A PNG's width over its height, from its header, else 16:10.
fn png_aspect(path: &Path) -> f32 {
    use std::io::Read;
    let mut header = [0u8; 24];
    let read = std::fs::File::open(path).and_then(|mut file| file.read_exact(&mut header));
    if read.is_err() || &header[1..4] != b"PNG" {
        return 1.6;
    }
    let width = u32::from_be_bytes([header[16], header[17], header[18], header[19]]);
    let height = u32::from_be_bytes([header[20], header[21], header[22], header[23]]);
    if width == 0 || height == 0 {
        return 1.6;
    }
    width as f32 / height as f32
}

/// Whether two paths name the same bundle, ignoring a trailing separator.
fn same_dir(a: &Path, b: &Path) -> bool {
    a.components().eq(b.components())
}

impl Render for Captures {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.take_pending(cx);
        if let Some(item) = self.open.clone() {
            let rtl = crate::i18n::is_rtl();
            return div()
                .size_full()
                .flex()
                .when(rtl, |el| el.flex_row_reverse())
                .child(self.render_stage(&item, cx))
                .into_any_element();
        }
        self.render_library(cx)
    }
}
