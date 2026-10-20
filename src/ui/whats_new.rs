//! "What's New" after an update (`whats-new/`): the bundled release note for
//! the newest version not seen yet, shown once in a dialog.

use std::ops::Range;

use gpui::{
    div, img, prelude::*, px, rems, AnyElement, App, Context, FocusHandle, FontStyle, FontWeight,
    HighlightStyle, ImageSource, InteractiveText, Resource, SharedString, StyledText, Subscription,
    UnderlineStyle, Window,
};
use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};

use crate::i18n::t_with;
use crate::shortcut;
use crate::ui::components::{Dialog, RowDirection, Typography};
use crate::ui::shell::{close_dialog, open_dialog};
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{palette, Alpha, MONO_FONT};

include!(concat!(env!("OUT_DIR"), "/release_notes.rs"));

#[derive(Clone, Debug)]
pub struct ReleaseNote {
    pub version: String,
    pub markdown: &'static str,
}

fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let v = version.trim().trim_start_matches(['v', 'V']);
    let mut parts = v.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch_part = parts.next()?;
    let digits: String = patch_part
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    Some((major, minor, digits.parse().ok()?))
}

fn notes() -> impl Iterator<Item = ReleaseNote> {
    RELEASE_NOTES.iter().map(|(version, markdown)| ReleaseNote {
        version: version.to_string(),
        markdown,
    })
}

/// The newest bundled note at or below `current` and above `last_seen` (when
/// `last_seen` is a version).
pub fn release_note_to_show(current: &str, last_seen: &str) -> Option<ReleaseNote> {
    newest_unseen(notes(), current, last_seen)
}

fn newest_unseen(
    notes: impl Iterator<Item = ReleaseNote>,
    current: &str,
    last_seen: &str,
) -> Option<ReleaseNote> {
    let current = parse_version(current)?;
    let last_seen = parse_version(last_seen);
    notes
        .filter(|note| parse_version(&note.version).is_some_and(|v| v <= current))
        .filter(|note| match last_seen {
            Some(seen) => parse_version(&note.version).is_some_and(|v| v > seen),
            None => true,
        })
        .max_by_key(|note| parse_version(&note.version))
}

/// The newest bundled note, for the debug preview.
pub fn latest_release_note() -> Option<ReleaseNote> {
    notes().max_by_key(|note| parse_version(&note.version))
}

/// Opens the dialog. `mark_seen` records the version as seen on dismiss.
pub fn open_modal(note: ReleaseNote, mark_seen: bool, window: &mut Window, cx: &mut App) {
    open_dialog(window, cx, move |window, cx| {
        cx.new(|cx| WhatsNewModal::new(note, mark_seen, window, cx))
            .into()
    });
}

/// Opens the newest unseen release note while "show What's New" is on, like
/// `WhatsNewGate.tsx`. It re-checks whenever settings change, so turning the
/// setting on shows a pending note at once.
pub struct WhatsNewGate {
    /// The note this session already opened. Dismissing marks it seen, and
    /// this keeps it closed even if saving that fails.
    shown: Option<String>,
    _settings: Subscription,
}

impl WhatsNewGate {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = SettingsStore::global(cx);
        let settings = cx.observe_in(&store, window, |this, _, window, cx| this.check(window, cx));
        cx.defer_in(window, |this, window, cx| this.check(window, cx));
        Self {
            shown: None,
            _settings: settings,
        }
    }

    fn check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = SettingsStore::get(cx);
        if !settings.show_whats_new_on_update {
            return;
        }
        let Some(note) = release_note_to_show(
            env!("CARGO_PKG_VERSION"),
            &settings.whats_new_last_seen_version,
        ) else {
            return;
        };
        if self.shown.as_deref() == Some(note.version.as_str()) {
            return;
        }
        self.shown = Some(note.version.clone());
        window.defer(cx, move |window, cx| open_modal(note, true, window, cx));
    }
}

impl Render for WhatsNewGate {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

pub struct WhatsNewModal {
    note: ReleaseNote,
    mark_seen: bool,
    focus: FocusHandle,
    blocks: Vec<Block>,
}

impl WhatsNewModal {
    fn new(
        note: ReleaseNote,
        mark_seen: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window);
        let blocks = parse_markdown(note.markdown);
        Self {
            note,
            mark_seen,
            focus,
            blocks,
        }
    }
}

impl Render for WhatsNewModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let version = self.note.version.clone();
        let mark_seen = self.mark_seen;
        let on_close = move |window: &mut Window, cx: &mut App| {
            if mark_seen {
                let seen = version.clone();
                let stored = seen.clone();
                super::pages::rows::set(
                    cx,
                    "whats_new_last_seen_version",
                    move |s| s.whats_new_last_seen_version = stored,
                    move |app| shortcut::change_whats_new_last_seen_version_setting(app, seen),
                );
            }
            close_dialog(window, cx);
        };
        let mut body = div().flex().flex_col().gap_3();
        for (index, block) in self.blocks.iter().enumerate() {
            body = body.child(render_block(block, index, cx));
        }
        Dialog::new(
            t_with("whatsNew.title", &[("version", &self.note.version)]),
            self.focus.clone(),
            on_close,
        )
        .child(body)
    }
}

// ---- Markdown ----------------------------------------------------------------

#[derive(Clone, Default, Debug)]
struct Inline {
    text: String,
    styles: Vec<(Range<usize>, Span)>,
    links: Vec<(Range<usize>, String)>,
}

#[derive(Clone, Copy, Debug, Default)]
struct Span {
    bold: bool,
    italic: bool,
    code: bool,
    link: bool,
}

#[derive(Debug)]
enum Block {
    Heading(u8, Inline),
    Paragraph(Inline),
    List {
        ordered: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Quote(Vec<Block>),
    Code(String),
    Rule,
    Image {
        src: String,
    },
}

/// Only http(s) and mailto links open; anything else renders as text.
fn is_safe_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:")
}

/// Images must be bundled release-note images.
fn is_safe_image(src: &str) -> bool {
    src.starts_with("/release-notes/") && !src.contains("..") && !src.contains('\\')
}

fn parse_markdown(markdown: &str) -> Vec<Block> {
    let mut stack: Vec<Vec<Block>> = vec![Vec::new()];
    let mut lists: Vec<(Option<u64>, Vec<Vec<Block>>)> = Vec::new();
    let mut inline: Option<Inline> = None;
    let mut span = Span::default();
    let mut link: Option<(usize, String)> = None;
    let mut heading: Option<u8> = None;
    let mut code: Option<String> = None;

    let flush_inline =
        |inline: &mut Option<Inline>, stack: &mut Vec<Vec<Block>>, heading: Option<u8>| {
            if let Some(text) = inline.take() {
                if text.text.trim().is_empty() {
                    return;
                }
                let block = match heading {
                    Some(level) => Block::Heading(level, text),
                    None => Block::Paragraph(text),
                };
                stack.last_mut().unwrap().push(block);
            }
        };

    for event in Parser::new(markdown) {
        match event {
            Event::Start(Tag::Paragraph) => inline = Some(Inline::default()),
            Event::End(TagEnd::Paragraph) => flush_inline(&mut inline, &mut stack, None),
            Event::Start(Tag::Heading { level, .. }) => {
                heading = Some(match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    _ => 3,
                });
                inline = Some(Inline::default());
            }
            Event::End(TagEnd::Heading(_)) => {
                flush_inline(&mut inline, &mut stack, heading);
                heading = None;
            }
            Event::Start(Tag::List(start)) => {
                flush_inline(&mut inline, &mut stack, None);
                lists.push((start, Vec::new()));
            }
            Event::End(TagEnd::List(_)) => {
                if let Some((ordered, items)) = lists.pop() {
                    stack
                        .last_mut()
                        .unwrap()
                        .push(Block::List { ordered, items });
                }
            }
            Event::Start(Tag::Item) => {
                stack.push(Vec::new());
                inline = Some(Inline::default());
            }
            Event::End(TagEnd::Item) => {
                flush_inline(&mut inline, &mut stack, None);
                let blocks = stack.pop().unwrap_or_default();
                if let Some((_, items)) = lists.last_mut() {
                    items.push(blocks);
                }
            }
            Event::Start(Tag::BlockQuote(_)) => stack.push(Vec::new()),
            Event::End(TagEnd::BlockQuote(_)) => {
                let blocks = stack.pop().unwrap_or_default();
                stack.last_mut().unwrap().push(Block::Quote(blocks));
            }
            Event::Start(Tag::CodeBlock(_)) => code = Some(String::new()),
            Event::End(TagEnd::CodeBlock) => {
                if let Some(code) = code.take() {
                    stack.last_mut().unwrap().push(Block::Code(code));
                }
            }
            Event::Rule => stack.last_mut().unwrap().push(Block::Rule),
            Event::Start(Tag::Image { dest_url, .. }) => {
                if is_safe_image(&dest_url) {
                    flush_inline(&mut inline, &mut stack, heading);
                    stack.last_mut().unwrap().push(Block::Image {
                        src: dest_url.to_string(),
                    });
                    inline = Some(Inline::default());
                }
                // Alt text is dropped; GPUI images have no alt rendering.
                span.code = false;
            }
            Event::End(TagEnd::Image) => {
                if let Some(inline) = inline.as_mut() {
                    inline.text.clear();
                    inline.styles.clear();
                }
            }
            Event::Start(Tag::Emphasis) => span.italic = true,
            Event::End(TagEnd::Emphasis) => span.italic = false,
            Event::Start(Tag::Strong) => span.bold = true,
            Event::End(TagEnd::Strong) => span.bold = false,
            Event::Start(Tag::Link { dest_url, .. }) => {
                let start = inline.as_ref().map(|i| i.text.len()).unwrap_or(0);
                if is_safe_url(&dest_url) {
                    link = Some((start, dest_url.to_string()));
                    span.link = true;
                }
            }
            Event::End(TagEnd::Link) => {
                if let (Some((start, url)), Some(inline)) = (link.take(), inline.as_mut()) {
                    let end = inline.text.len();
                    inline.links.push((start..end, url));
                }
                span.link = false;
            }
            Event::Text(text) => {
                if let Some(code) = code.as_mut() {
                    code.push_str(&text);
                } else if let Some(inline) = inline.as_mut() {
                    let start = inline.text.len();
                    inline.text.push_str(&text);
                    inline.styles.push((start..inline.text.len(), span));
                }
            }
            Event::Code(text) => {
                if let Some(inline) = inline.as_mut() {
                    let start = inline.text.len();
                    inline.text.push_str(&text);
                    inline
                        .styles
                        .push((start..inline.text.len(), Span { code: true, ..span }));
                }
            }
            Event::SoftBreak => {
                if let Some(inline) = inline.as_mut() {
                    inline.text.push(' ');
                }
            }
            Event::HardBreak => {
                if let Some(inline) = inline.as_mut() {
                    inline.text.push('\n');
                }
            }
            _ => {}
        }
    }
    flush_inline(&mut inline, &mut stack, None);
    stack.pop().unwrap_or_default()
}

fn render_inline(inline: &Inline, id: String, cx: &App) -> AnyElement {
    let p = palette(cx);
    let highlights: Vec<(Range<usize>, HighlightStyle)> = inline
        .styles
        .iter()
        .filter(|(range, span)| {
            !range.is_empty() && (span.bold || span.italic || span.code || span.link)
        })
        .map(|(range, span)| {
            let mut style = HighlightStyle::default();
            if span.bold {
                style.font_weight = Some(FontWeight::BOLD);
            }
            if span.italic {
                style.font_style = Some(FontStyle::Italic);
            }
            if span.code {
                style.background_color = Some(p.mid_gray.a(0.1));
            }
            if span.link {
                style.color = Some(p.accent);
                style.underline = Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(p.accent.a(0.4)),
                    wavy: false,
                });
            }
            (range.clone(), style)
        })
        .collect();
    let text = StyledText::new(SharedString::from(inline.text.clone())).with_highlights(highlights);
    if inline.links.is_empty() {
        return text.into_any_element();
    }
    let links = inline.links.clone();
    InteractiveText::new(SharedString::from(id), text)
        .on_click(
            links.iter().map(|(range, _)| range.clone()).collect(),
            move |index, _, cx| cx.open_url(&links[index].1),
        )
        .into_any_element()
}

fn render_block(block: &Block, index: usize, cx: &App) -> AnyElement {
    let p = palette(cx);
    let id = format!("md-{index}");
    match block {
        Block::Heading(level, inline) => {
            let base = div()
                .font_weight(FontWeight::SEMIBOLD)
                .leading_snug()
                .text_color(p.text);
            let base = match level {
                1 => base.base_text(),
                2 => base.text_px(15.),
                _ => base.sm_text(),
            };
            base.leading_snug()
                .child(render_inline(inline, id, cx))
                .into_any_element()
        }
        Block::Paragraph(inline) => div()
            .sm_text()
            .leading_relaxed()
            .text_color(p.text.a(0.8))
            .child(render_inline(inline, id, cx))
            .into_any_element(),
        Block::List { ordered, items } => div()
            .flex()
            .flex_col()
            .gap_1()
            .sm_text()
            .leading_relaxed()
            .text_color(p.text.a(0.8))
            .children(items.iter().enumerate().map(|(i, item)| {
                let marker = match ordered {
                    Some(start) => format!("{}.", start + i as u64),
                    None => "•".to_string(),
                };
                div()
                    .flex()
                    .row_dir()
                    .gap_2()
                    .ps(rems(0.5))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(p.text.a(0.5))
                            .child(marker),
                    )
                    .child(
                        div().flex_1().min_w_0().flex().flex_col().gap_1().children(
                            item.iter()
                                .enumerate()
                                .map(|(j, b)| render_block(b, index * 100 + i * 10 + j, cx)),
                        ),
                    )
            }))
            .into_any_element(),
        Block::Quote(blocks) => div()
            // `border-s-2 ps-3`.
            .when(crate::i18n::is_rtl(), |el| el.border_r_2())
            .when(!crate::i18n::is_rtl(), |el| el.border_l_2())
            .border_color(p.accent.a(0.5))
            .ps(rems(0.75))
            .sm_text()
            .leading_relaxed()
            .text_color(p.text.a(0.7))
            .children(
                blocks
                    .iter()
                    .enumerate()
                    .map(|(j, b)| render_block(b, index * 100 + j, cx)),
            )
            .into_any_element(),
        Block::Code(code) => div()
            .rounded_md()
            .bg(p.mid_gray.a(0.1))
            .p_3()
            .xs_text()
            .leading_relaxed()
            .text_color(p.text.a(0.8))
            .font_family(MONO_FONT)
            .child(code.clone())
            .into_any_element(),
        Block::Rule => div().h(px(1.)).bg(p.mid_gray.a(0.2)).into_any_element(),
        Block::Image { src } => div()
            .flex()
            .row_dir()
            .justify_center()
            .child(
                img(ImageSource::Resource(Resource::Embedded(
                    SharedString::from(src.clone()),
                )))
                .max_h(rems(18.))
                .max_w_full()
                .object_fit(gpui::ObjectFit::Contain),
            )
            .into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_newest_unseen_note() {
        let notes = || {
            ["0.9.0", "0.9.7", "1.0.0"].map(|version| ReleaseNote {
                version: version.to_string(),
                markdown: "",
            })
        };
        let pick = |current, last_seen| newest_unseen(notes().into_iter(), current, last_seen);
        assert!(pick("0.9.7", "0.9.7").is_none());
        assert_eq!(pick("0.9.7", "0.9.0").unwrap().version, "0.9.7");
        assert_eq!(pick("0.9.7", "").unwrap().version, "0.9.7");
    }

    #[test]
    fn parses_release_markdown() {
        let blocks = parse_markdown(
            "## Title\n\nSome **bold** text.\n\n- a\n- b\n\n![x](/release-notes/0.9.7/a.png)\n",
        );
        assert!(matches!(blocks[0], Block::Heading(2, _)));
        assert!(matches!(blocks[1], Block::Paragraph(_)));
        assert!(matches!(blocks[2], Block::List { .. }));
        assert!(blocks.iter().any(|b| matches!(b, Block::Image { .. })));
    }
}
