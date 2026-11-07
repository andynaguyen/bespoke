use std::collections::HashSet;
use std::ops::RangeInclusive;

use gpui::{
    AnyElement, AnyView, App, Context, Div, Entity, FontWeight, Pixels, SharedString, Subscription,
    Window, div, prelude::*, px, rems,
};
use gpui_component::input::{InputEvent, InputState};

use crate::app_handle::AppEvent;
use crate::audio_toolkit::{apply_custom_words_with_fixes, build_match_key};
use crate::i18n::{t, t_count, t_with};
use crate::settings::{AppSettings, Snippet};
use crate::shortcut;
use crate::try_it::Take;
use crate::ui::clay::keycap;
use crate::ui::components::{
    Button, ButtonSize, ButtonVariant, Dialog, FocusVisible, KeyboardFocus, RowDirection,
    SettingsGroup, TextArea, TextField, Typography, focus_handle_for, icon,
};
use crate::ui::event_bus::event_bus;
use crate::ui::shell::{close_dialog, open_dialog};
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{Alpha, MONO_FONT, palette, radius, tw};

use super::rows::set;

const MAX_PHRASE_LENGTH: usize = 50;
const MAX_TEXT_LENGTH: usize = 4000;
const TRANSCRIBE_BINDING: &str = "transcribe";

fn normalize_word(word: &str) -> String {
    word.chars()
        .filter(|c| !matches!(c, '<' | '>' | '"' | '\''))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn trigger_key(trigger: &str) -> String {
    trigger
        .split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn set_words(words: Vec<String>, cx: &mut App) {
    let stored = words.clone();
    set(
        cx,
        "custom_words",
        move |s| s.custom_words = stored,
        move |app| shortcut::update_custom_words(app, words),
    );
}

fn set_snippets(snippets: Vec<Snippet>, cx: &mut App) {
    let stored = snippets.clone();
    set(
        cx,
        "snippets",
        move |s| s.snippets = stored,
        move |app| shortcut::update_snippets(app, snippets),
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum EntryRef {
    Word(usize),
    Snippet(usize),
}

struct Entry {
    at: EntryRef,
    phrase: String,
    text: Option<String>,
}

fn entries(settings: &AppSettings) -> Vec<Entry> {
    let words = settings
        .custom_words
        .iter()
        .enumerate()
        .map(|(index, word)| Entry {
            at: EntryRef::Word(index),
            phrase: word.clone(),
            text: None,
        });
    let snippets = settings
        .snippets
        .iter()
        .enumerate()
        .map(|(index, snippet)| Entry {
            at: EntryRef::Snippet(index),
            phrase: snippet.trigger.clone(),
            text: Some(snippet.text.clone()),
        });
    let mut all: Vec<Entry> = words.chain(snippets).collect();
    all.sort_by_cached_key(|entry| entry.phrase.to_lowercase());
    all
}

fn is_duplicate(
    phrase: &str,
    snippet: bool,
    editing: Option<EntryRef>,
    settings: &AppSettings,
) -> bool {
    let spelling = normalize_word(phrase).to_lowercase();
    let key = trigger_key(phrase);
    entries(settings).iter().any(|entry| {
        Some(entry.at) != editing
            && (normalize_word(&entry.phrase).to_lowercase() == spelling
                || (snippet && entry.text.is_some() && trigger_key(&entry.phrase) == key))
    })
}

#[derive(Debug, PartialEq)]
enum Draft {
    Word(String),
    Snippet(Snippet),
}

fn place(
    draft: Draft,
    editing: Option<EntryRef>,
    words: &mut Vec<String>,
    snippets: &mut Vec<Snippet>,
) {
    match (draft, editing) {
        (Draft::Word(word), Some(EntryRef::Word(index))) if index < words.len() => {
            words[index] = word
        }
        (Draft::Word(word), _) => words.push(word),
        (Draft::Snippet(snippet), Some(EntryRef::Snippet(index))) if index < snippets.len() => {
            snippets[index] = snippet
        }
        (Draft::Snippet(snippet), _) => snippets.push(snippet),
    }
}

fn remove_entry(at: EntryRef, cx: &mut App) {
    let settings = SettingsStore::get(cx);
    match at {
        EntryRef::Word(index) => {
            let mut words = settings.custom_words.clone();
            if index < words.len() {
                words.remove(index);
                set_words(words, cx);
            }
        }
        EntryRef::Snippet(index) => {
            let mut snippets = settings.snippets.clone();
            if index < snippets.len() {
                snippets.remove(index);
                set_snippets(snippets, cx);
            }
        }
    }
}

const INSPECTOR_WIDTH: f32 = 280.;
const CHIP_HEIGHT: f32 = 28.;
const PHRASE_COLUMN: f32 = 150.;
const ARROW_COLUMN: f32 = 22.;

/// GPUI doesn't clip children to rounded corners.
fn group_inner_radius() -> Pixels {
    radius::CARD - px(1.)
}

#[derive(Debug, PartialEq)]
enum Piece {
    Plain(String),
    Fix { heard: String, written: String },
}

fn pieces(take: &Take) -> Vec<Piece> {
    let text = &take.text;
    let mut out = Vec::new();
    let mut cursor = 0;
    for fix in &take.fixes {
        let Some(start) = find_ignoring_case(&text[cursor..], &fix.written).map(|i| cursor + i)
        else {
            continue;
        };
        let end = start + fix.written.len();
        if start > cursor {
            out.push(Piece::Plain(text[cursor..start].to_string()));
        }
        out.push(Piece::Fix {
            heard: fix.heard.clone(),
            written: text[start..end].to_string(),
        });
        cursor = end;
    }
    if cursor < text.len() {
        out.push(Piece::Plain(text[cursor..].to_string()));
    }
    out
}

fn find_ignoring_case(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    haystack.find(needle).or_else(|| {
        haystack.char_indices().map(|(i, _)| i).find(|&i| {
            haystack
                .get(i..i + needle.len())
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(needle))
        })
    })
}

fn used_entries(take: &Take, settings: &AppSettings) -> HashSet<EntryRef> {
    let tokens: Vec<String> = take.text.split_whitespace().map(build_match_key).collect();
    let words = settings
        .custom_words
        .iter()
        .enumerate()
        .filter_map(|(i, word)| {
            let key = build_match_key(word);
            let span = word.split_whitespace().count().max(1);
            let fixed = take
                .fixes
                .iter()
                .any(|fix| !fix.replacement && build_match_key(&fix.written) == key);
            let spelled = !key.is_empty() && tokens.windows(span).any(|run| run.concat() == key);
            (fixed || spelled).then_some(EntryRef::Word(i))
        });
    let snippets = settings
        .snippets
        .iter()
        .enumerate()
        .filter_map(|(i, snippet)| {
            let key = trigger_key(&snippet.trigger);
            take.fixes
                .iter()
                .any(|fix| fix.replacement && trigger_key(&fix.heard) == key)
                .then_some(EntryRef::Snippet(i))
        });
    words.chain(snippets).collect()
}

struct TakeItem {
    joins_previous: bool,
    element: AnyElement,
}

fn unspaced_runs(items: Vec<TakeItem>) -> Vec<Div> {
    let mut runs: Vec<Vec<AnyElement>> = Vec::new();
    for item in items {
        match runs.last_mut() {
            Some(run) if item.joins_previous => run.push(item.element),
            _ => runs.push(vec![item.element]),
        }
    }
    runs.into_iter()
        .map(|run| div().flex().row_dir().items_baseline().children(run))
        .collect()
}

fn word_catches(heard: &str, word: &str, threshold: f64) -> bool {
    build_match_key(heard) == build_match_key(word)
        || !apply_custom_words_with_fixes(heard, &[word.to_string()], threshold)
            .1
            .is_empty()
}

struct TakeWord {
    piece: usize,
    text: String,
}

enum TeachError {
    Incomplete,
    Duplicate(String),
}

pub struct DictionaryPage {
    store: Entity<SettingsStore>,
    take: Option<Take>,
    selection: Option<RangeInclusive<usize>>,
    spelling: Entity<InputState>,
    just_added: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl Drop for DictionaryPage {
    fn drop(&mut self) {
        crate::try_it::set_dictionary_active(false);
    }
}

impl DictionaryPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = SettingsStore::global(cx);
        let spelling = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("settings.dictionary.spellingPlaceholder"))
                .validate(|text, _| text.chars().count() <= MAX_PHRASE_LENGTH)
        });
        crate::try_it::set_dictionary_active(window.is_window_active());
        let subscriptions = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.observe_window_activation(window, |_, window, cx| {
                crate::try_it::set_dictionary_active(window.is_window_active());
                cx.notify();
            }),
            cx.subscribe(&event_bus(cx), |this, _, event: &AppEvent, cx| {
                if event.name == crate::try_it::TAKE_EVENT
                    && let Some(take) = event.parse::<Take>()
                {
                    this.take = Some(take);
                    this.selection = None;
                    this.just_added = None;
                    cx.notify();
                }
            }),
            cx.subscribe_in(
                &spelling,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => {
                        let replacement = !this.spelling_catches(cx);
                        this.teach(replacement, window, cx);
                    }
                    _ => {}
                },
            ),
        ];
        #[allow(unused_mut)]
        let mut this = Self {
            store,
            take: None,
            selection: None,
            spelling,
            just_added: None,
            _subscriptions: subscriptions,
        };
        #[cfg(debug_assertions)]
        this.seed_dev_take(window, cx);
        this
    }

    #[cfg(debug_assertions)]
    fn seed_dev_take(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(heard) = std::env::var("BESPOKE_DEV_TAKE") else {
            return;
        };
        let settings = SettingsStore::get(cx);
        let (text, mut fixes) = apply_custom_words_with_fixes(
            &heard,
            &settings.custom_words,
            settings.word_correction_threshold,
        );
        let (text, replacements) = crate::audio_toolkit::expand_snippets_with_fixes(
            &text,
            settings
                .snippets
                .iter()
                .map(|snippet| (snippet.trigger.as_str(), snippet.text.as_str())),
        );
        fixes.extend(replacements);
        self.take = Some(Take { text, fixes });
        if let Some(index) = std::env::var("BESPOKE_DEV_TAKE_PICK")
            .ok()
            .and_then(|pick| pick.parse().ok())
        {
            self.pick(index, false, window, cx);
        }
    }

    fn plain_words(&self) -> Vec<TakeWord> {
        let Some(take) = &self.take else {
            return Vec::new();
        };
        pieces(take)
            .into_iter()
            .enumerate()
            .filter_map(|(piece, part)| match part {
                Piece::Plain(text) => Some((piece, text)),
                Piece::Fix { .. } => None,
            })
            .flat_map(|(piece, text)| {
                text.split_whitespace()
                    .map(|word| TakeWord {
                        piece,
                        text: word.to_string(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn heard(&self) -> Option<String> {
        let selection = self.selection.clone()?;
        let words = self.plain_words();
        let picked = words.get(selection)?;
        let joined = picked
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let heard = joined.trim_matches(|c: char| !c.is_alphanumeric());
        (!heard.is_empty()).then(|| heard.to_string())
    }

    fn pick(&mut self, index: usize, extend: bool, window: &mut Window, cx: &mut Context<Self>) {
        let words = self.plain_words();
        let piece = |i: usize| words.get(i).map(|word| word.piece);
        self.selection = match self.selection.clone() {
            Some(range) if extend && piece(*range.start()) == piece(index) => {
                Some((*range.start()).min(index)..=(*range.end()).max(index))
            }
            Some(range) if !extend && range == (index..=index) => None,
            _ => Some(index..=index),
        };
        self.just_added = None;
        self.spelling.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn spelling_catches(&self, cx: &App) -> bool {
        let spelling = normalize_word(&self.spelling.read(cx).value());
        let threshold = SettingsStore::get(cx).word_correction_threshold;
        self.heard()
            .is_some_and(|heard| word_catches(&heard, &spelling, threshold))
    }

    fn teach_draft(&self, replacement: bool, cx: &App) -> Result<Draft, TeachError> {
        let heard = self.heard().ok_or(TeachError::Incomplete)?;
        let spelling = normalize_word(&self.spelling.read(cx).value());
        if spelling.is_empty() || spelling.chars().count() > MAX_PHRASE_LENGTH || spelling == heard
        {
            return Err(TeachError::Incomplete);
        }
        let settings = SettingsStore::get(cx);
        if replacement {
            if is_duplicate(&heard, true, None, settings) {
                return Err(TeachError::Duplicate(heard));
            }
            Ok(Draft::Snippet(Snippet {
                trigger: heard,
                text: spelling,
            }))
        } else {
            if is_duplicate(&spelling, false, None, settings) {
                return Err(TeachError::Duplicate(spelling));
            }
            Ok(Draft::Word(spelling))
        }
    }

    fn teach(&mut self, replacement: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(draft) = self.teach_draft(replacement, cx) else {
            return;
        };
        let added = match &draft {
            Draft::Word(word) => word.clone(),
            Draft::Snippet(snippet) => snippet.trigger.clone(),
        };
        let stored = SettingsStore::get(cx);
        let mut words = stored.custom_words.clone();
        let mut snippets = stored.snippets.clone();
        place(draft, None, &mut words, &mut snippets);
        if replacement {
            set_snippets(snippets, cx);
        } else {
            set_words(words, cx);
        }
        self.just_added = Some(added);
        self.selection = None;
        self.spelling
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    fn render_dictionary(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let store = self.store.read(cx);
        let updating = store.is_updating("custom_words") || store.is_updating("snippets");
        let fired = self
            .take
            .as_ref()
            .map(|take| used_entries(take, &store.settings))
            .unwrap_or_default();
        let added = self.just_added.as_deref().map(normalize_word);
        let is_added =
            |entry: &Entry| added.as_deref() == Some(normalize_word(&entry.phrase).as_str());
        let all = entries(&store.settings);
        let (replacements, words): (Vec<Entry>, Vec<Entry>) =
            all.into_iter().partition(|entry| entry.text.is_some());
        let entry_focus = FocusVisible::styled(cx, |s| s.underline());
        let lit = p.data;

        let first_row = replacements.first().map(|entry| entry.at);
        let rows = replacements.into_iter().map(|entry| {
            let at = entry.at;
            let id = entry_id(at);
            let group = SharedString::from(format!("entry-{id}"));
            let handle = focus_handle_for(SharedString::from(format!("edit-{id}")).into(), cx);
            let used = fired.contains(&at) || is_added(&entry);
            let preview = entry
                .text
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            div()
                .group(group.clone())
                .flex()
                .row_dir()
                .items_center()
                .gap_3()
                .pe(px(8.))
                .when(first_row == Some(at), |el| {
                    el.rounded_t(group_inner_radius())
                })
                .when(used, |el| el.bg(lit.a(0.14)))
                .when(!used, |el| el.hover(|s| s.bg(p.card)))
                .child(
                    entry_focus
                        .clone()
                        .apply_with(div().id(SharedString::from(format!("edit-{id}"))), handle)
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .row_dir()
                        .items_center()
                        .ps(px(14.))
                        .h(px(42.))
                        .text_px(13.)
                        .cursor_pointer()
                        .on_click(move |_, window, cx| {
                            open_entry_dialog(Target::Existing(at), window, cx)
                        })
                        .child(
                            div()
                                .w(px(PHRASE_COLUMN))
                                .flex_shrink_0()
                                .pe(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .child(entry.phrase.clone()),
                        )
                        .child(
                            div()
                                .w(px(ARROW_COLUMN))
                                .flex_shrink_0()
                                .text_color(p.mid_gray)
                                .child("→"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .px(px(8.))
                                .py(px(3.))
                                .rounded_md()
                                .bg(p.text.a(0.05))
                                .font_family(MONO_FONT)
                                .text_px(12.)
                                .text_color(p.text.a(0.8))
                                .truncate()
                                .child(preview),
                        ),
                )
                .child(remove_button(
                    SharedString::from(format!("remove-{id}")),
                    group,
                    t_with("settings.dictionary.remove", &[("word", &entry.phrase)]),
                    updating,
                    move |cx| remove_entry(at, cx),
                    cx,
                ))
                .into_any_element()
        });

        let chips = words.into_iter().map(|entry| {
            let at = entry.at;
            let id = entry_id(at);
            let group = SharedString::from(format!("chip-{id}"));
            let handle = focus_handle_for(SharedString::from(format!("edit-{id}")).into(), cx);
            let new = is_added(&entry);
            let used = fired.contains(&at);
            div()
                .group(group.clone())
                .relative()
                .child(
                    entry_focus
                        .clone()
                        .apply_with(div().id(SharedString::from(format!("edit-{id}"))), handle)
                        .h(px(CHIP_HEIGHT))
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .rounded_full()
                        .border_1()
                        .text_px(13.)
                        .whitespace_nowrap()
                        .cursor_pointer()
                        .map(|el| match (new, used) {
                            (true, _) => el.border_color(lit).bg(lit.a(0.12)),
                            (false, true) => el.border_color(lit.a(0.7)).bg(lit.a(0.2)),
                            (false, false) => el
                                .bg(p.card)
                                .border_color(p.line)
                                .hover(|s| s.bg(p.text.a(0.08))),
                        })
                        .on_click(move |_, window, cx| {
                            open_entry_dialog(Target::Existing(at), window, cx)
                        })
                        .child(entry.phrase.clone())
                        .when(new, |el| {
                            el.child(
                                div()
                                    .text_px(11.)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(lit)
                                    .child(t("settings.dictionary.addedTag")),
                            )
                        }),
                )
                .child(remove_badge(
                    SharedString::from(format!("remove-{id}")),
                    group,
                    t_with("settings.dictionary.remove", &[("word", &entry.phrase)]),
                    updating,
                    move |cx| remove_entry(at, cx),
                    cx,
                ))
                .into_any_element()
        });

        let section_header = |title: &str, caption: &str| {
            div()
                .flex()
                .row_dir()
                .items_baseline()
                .gap_2()
                .child(
                    div()
                        .text_px(11.)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.mid_gray)
                        .child(t(title)),
                )
                .child(
                    div()
                        .text_px(12.)
                        .text_color(p.text.a(0.55))
                        .child(t(caption)),
                )
        };

        div()
            .flex()
            .flex_col()
            .gap(px(24.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(section_header(
                        "settings.dictionary.replacements",
                        "settings.dictionary.replacementsHelp",
                    ))
                    .child(
                        SettingsGroup::new()
                            .children(rows)
                            .child(add_replacement_row(first_row.is_none(), cx)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(section_header(
                        "settings.dictionary.words",
                        "settings.dictionary.wordsHelp",
                    ))
                    .child(
                        div()
                            .flex()
                            .row_dir()
                            .flex_wrap()
                            .gap(px(6.))
                            .children(chips)
                            .child(add_word_chip(cx)),
                    ),
            )
    }

    fn render_try_it(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let lit = p.data;
        let shortcut = SettingsStore::get(cx)
            .bindings
            .get(TRANSCRIBE_BINDING)
            .map(|binding| {
                scrawl::host::shortcut_from_binding(&binding.current_binding)
                    .map_or_else(|| binding.current_binding.clone(), |c| c.label)
            })
            .unwrap_or_default();

        let hint = t("settings.dictionary.tryItHint");
        let mut hint_line = div()
            .flex()
            .row_dir()
            .flex_wrap()
            .items_center()
            .gap(px(4.))
            .text_px(12.)
            .line_height(px(20.))
            .text_color(p.text.a(0.6));
        match hint.split_once("{{shortcut}}") {
            Some((before, after)) if !shortcut.is_empty() => {
                for word in before.split_whitespace() {
                    hint_line = hint_line.child(SharedString::from(word.to_string()));
                }
                hint_line = hint_line.child(keycap(shortcut, 18., p.clay, p.text, p.is_dark));
                for word in after.split_whitespace() {
                    hint_line = hint_line.child(SharedString::from(word.to_string()));
                }
            }
            _ => hint_line = hint_line.child(hint.replace("{{shortcut}}", "")),
        }

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .text_px(11.)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.mid_gray)
                    .child(t("settings.dictionary.tryIt")),
            )
            .child(
                div()
                    .p(px(12.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(p.line)
                    .bg(p.card)
                    .child(hint_line),
            );

        let Some(take) = &self.take else {
            return body;
        };
        if take.text.trim().is_empty() {
            return body.child(
                div()
                    .text_px(13.)
                    .text_color(p.text.a(0.6))
                    .child(t("settings.dictionary.tryItEmpty")),
            );
        }

        let selection = self.selection.clone();
        let mut items: Vec<TakeItem> = Vec::new();
        let mut glue = false;
        let mut index = 0;
        let mut fixed = 0;
        for piece in pieces(take) {
            match piece {
                Piece::Fix { heard, written } => {
                    fixed += 1;
                    let written = written.split_whitespace().collect::<Vec<_>>().join(" ");
                    items.push(TakeItem {
                        joins_previous: glue,
                        element: div()
                            .flex()
                            .row_dir()
                            .items_baseline()
                            .gap(px(4.))
                            .px(px(4.))
                            .rounded(px(5.))
                            .bg(lit.a(0.2))
                            .when(!heard.is_empty(), |el| {
                                el.child(
                                    div()
                                        .text_px(10.)
                                        .line_through()
                                        .text_color(p.text.a(0.5))
                                        .child(heard),
                                )
                            })
                            .child(written)
                            .into_any_element(),
                    });
                    glue = true;
                }
                Piece::Plain(text) => {
                    let mut first = !text.starts_with(char::is_whitespace);
                    for word in text.split_whitespace() {
                        let i = index;
                        index += 1;
                        let picked = selection.as_ref().is_some_and(|range| range.contains(&i));
                        items.push(TakeItem {
                            joins_previous: glue && first,
                            element: div()
                                .id(("take-word", i))
                                .when(!(glue && first), |el| el.px(px(1.5)))
                                .rounded(px(4.))
                                .cursor_pointer()
                                .when(picked, |el| el.bg(p.warning.a(0.25)))
                                .when(!picked, |el| el.hover(|s| s.bg(p.text.a(0.08))))
                                .on_click(cx.listener(
                                    move |this, event: &gpui::ClickEvent, window, cx| {
                                        this.pick(i, event.modifiers().shift, window, cx)
                                    },
                                ))
                                .child(word.to_string())
                                .into_any_element(),
                        });
                        first = false;
                    }
                    glue = !text.ends_with(char::is_whitespace);
                }
            }
        }
        let words = unspaced_runs(items);

        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .flex()
                        .row_dir()
                        .flex_wrap()
                        .items_baseline()
                        .gap_x(px(1.5))
                        .gap_y(px(4.))
                        .text_px(14.)
                        .line_height(px(22.))
                        .children(words),
                )
                .child(div().text_px(11.).text_color(p.text.a(0.5)).child(t_count(
                    "settings.dictionary.fixedCount",
                    fixed,
                    &[],
                ))),
        );

        if let Some(heard) = self.heard() {
            let catches = self.spelling_catches(cx);
            let spelling_empty = normalize_word(&self.spelling.read(cx).value()).is_empty();
            let word_draft = self.teach_draft(false, cx);
            let replacement_draft = self.teach_draft(true, cx);
            let duplicate = match if catches {
                &word_draft
            } else {
                &replacement_draft
            } {
                Err(TeachError::Duplicate(entry)) => Some(entry.clone()),
                _ => None,
            };
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .p(px(10.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(p.warning.a(0.6))
                    .bg(p.warning.a(0.08))
                    .text_px(12.)
                    .child(
                        div()
                            .flex()
                            .row_dir()
                            .items_center()
                            .gap(px(6.))
                            .min_w_0()
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(p.text.a(0.6))
                                    .child(t("settings.dictionary.teachLabel")),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(MONO_FONT)
                                    .child(heard.clone()),
                            ),
                    )
                    .child(TextField::new(&self.spelling).w_full())
                    .when(!spelling_empty && !catches, |el| {
                        el.child(div().text_color(p.text.a(0.6)).child(t_with(
                            "settings.dictionary.wontMatch",
                            &[("heard", &heard)],
                        )))
                    })
                    .when_some(duplicate, |el, word| {
                        el.child(
                            div()
                                .text_color(tw::red_500())
                                .child(t_with("settings.dictionary.duplicate", &[("word", &word)])),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .row_dir()
                            .flex_wrap()
                            .gap(px(6.))
                            .child(
                                Button::new("teach-word")
                                    .size(ButtonSize::Sm)
                                    .when(!catches, |b| b.variant(ButtonVariant::Secondary))
                                    .disabled(word_draft.is_err())
                                    .child(t("settings.dictionary.teachWord"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.teach(false, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("teach-replacement")
                                    .size(ButtonSize::Sm)
                                    .when(catches, |b| b.variant(ButtonVariant::Secondary))
                                    .disabled(replacement_draft.is_err())
                                    .child(t("settings.dictionary.teachReplacement"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.teach(true, window, cx)
                                    })),
                            ),
                    ),
            );
        } else if self.just_added.is_some() {
            body = body.child(
                div()
                    .text_px(12.)
                    .text_color(lit)
                    .child(t("settings.dictionary.added")),
            );
        } else {
            body = body.child(
                div()
                    .text_px(12.)
                    .text_color(p.text.a(0.55))
                    .child(t("settings.dictionary.pickWord")),
            );
        }
        body
    }
}

impl super::Page for DictionaryPage {
    fn inspector(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        Some(crate::ui::shell::inspector(
            "dictionary-try-it",
            INSPECTOR_WIDTH,
            Vec::new(),
            div()
                .id("dictionary-try-it-scroll")
                .size_full()
                .overflow_y_scroll()
                .px_4()
                .pb(px(16.))
                .child(self.render_try_it(cx)),
        ))
    }
}

impl Render for DictionaryPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_dictionary(cx)
    }
}

fn remove_button(
    id: SharedString,
    group: SharedString,
    label: SharedString,
    disabled: bool,
    on_click: impl Fn(&mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let p = palette(cx);
    div()
        .id(id)
        .flex_shrink_0()
        .rounded_lg()
        .p(rems(0.375))
        .text_color(p.text.a(0.55))
        .opacity(0.)
        .group_hover(group, |s| s.opacity(1.))
        .hover(|s| s.bg(p.mid_gray.a(0.1)).text_color(tw::red_500()))
        .when(disabled, |el| el.cursor_not_allowed())
        .when(!disabled, |el| {
            el.cursor_pointer()
                .keyboard_focus_reveal(cx)
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    on_click(cx)
                })
        })
        .tooltip(move |window, cx| crate::ui::components::Tooltip::text(label.clone(), window, cx))
        .child(icon("trash-2").size(rems(1.)))
}

fn remove_badge(
    id: SharedString,
    group: SharedString,
    label: SharedString,
    disabled: bool,
    on_click: impl Fn(&mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let p = palette(cx);
    div()
        .id(id)
        .absolute()
        .top(px(-5.))
        .right(px(-5.))
        .size(px(16.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(p.background)
        .border_1()
        .border_color(p.line)
        .text_color(p.mid_gray)
        .opacity(0.)
        .group_hover(group, |s| s.opacity(1.))
        .hover(|s| s.text_color(tw::red_500()))
        .when(disabled, |el| el.cursor_not_allowed())
        .when(!disabled, |el| {
            el.cursor_pointer()
                .keyboard_focus_reveal(cx)
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    on_click(cx)
                })
        })
        .tooltip(move |window, cx| crate::ui::components::Tooltip::text(label.clone(), window, cx))
        .child(icon("x").size(px(10.)))
}

fn entry_id(at: EntryRef) -> String {
    match at {
        EntryRef::Word(index) => format!("word-{index}"),
        EntryRef::Snippet(index) => format!("snippet-{index}"),
    }
}

fn add_word_chip(cx: &App) -> impl IntoElement {
    let p = palette(cx);
    div()
        .id("add-word")
        .h(px(CHIP_HEIGHT))
        .flex()
        .items_center()
        .gap(px(4.))
        .ps(px(9.))
        .pe(px(12.))
        .rounded_full()
        .border_1()
        .border_dashed()
        .border_color(p.mid_gray.a(0.45))
        .text_px(13.)
        .text_color(p.mid_gray)
        .whitespace_nowrap()
        .cursor_pointer()
        .keyboard_focus(cx)
        .hover(|s| s.text_color(p.text).border_color(p.mid_gray))
        .on_click(|_, window, cx| open_entry_dialog(Target::NewWord, window, cx))
        .child(icon("plus").size(px(12.)))
        .child(t("settings.dictionary.addWord"))
}

fn add_replacement_row(is_first_row: bool, cx: &App) -> impl IntoElement {
    let p = palette(cx);
    div()
        .id("add-replacement")
        .rounded_b(group_inner_radius())
        .when(is_first_row, |el| el.rounded_t(group_inner_radius()))
        .flex()
        .row_dir()
        .items_center()
        .gap(px(6.))
        .h(px(38.))
        .px(px(14.))
        .text_px(13.)
        .text_color(p.mid_gray)
        .cursor_pointer()
        .keyboard_focus(cx)
        .hover(|s| s.bg(p.card).text_color(p.text))
        .on_click(|_, window, cx| open_entry_dialog(Target::NewSnippet, window, cx))
        .child(icon("plus").size(px(13.)))
        .child(t("settings.dictionary.addReplacement"))
}

#[derive(Clone, Copy)]
enum Target {
    NewWord,
    NewSnippet,
    Existing(EntryRef),
}

fn open_entry_dialog(target: Target, window: &mut Window, cx: &mut App) {
    open_dialog(window, cx, move |window, cx| -> AnyView {
        cx.new(|cx| EntryDialog::new(target, window, cx)).into()
    });
}

#[cfg(debug_assertions)]
pub(super) fn dev_open_entry_dialog(word: bool, window: &mut Window, cx: &mut App) {
    let target = if word {
        Target::NewWord
    } else {
        Target::NewSnippet
    };
    open_entry_dialog(target, window, cx);
}

pub struct EntryDialog {
    editing: Option<EntryRef>,
    snippet: bool,
    phrase: Entity<InputState>,
    text: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl EntryDialog {
    fn new(target: Target, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (editing, snippet) = match target {
            Target::NewWord => (None, false),
            Target::NewSnippet => (None, true),
            Target::Existing(at) => (Some(at), matches!(at, EntryRef::Snippet(_))),
        };
        let settings = SettingsStore::get(cx);
        let (phrase, text) = match editing {
            Some(EntryRef::Word(index)) => (
                settings
                    .custom_words
                    .get(index)
                    .cloned()
                    .unwrap_or_default(),
                String::new(),
            ),
            Some(EntryRef::Snippet(index)) => settings
                .snippets
                .get(index)
                .map(|s| (s.trigger.clone(), s.text.clone()))
                .unwrap_or_default(),
            None => Default::default(),
        };
        let phrase = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(if snippet {
                    t("settings.dictionary.phrasePlaceholder")
                } else {
                    t("settings.dictionary.wordPlaceholder")
                })
                .validate(|text, _| text.chars().count() <= MAX_PHRASE_LENGTH)
                .default_value(phrase)
        });
        let text = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder(t("settings.dictionary.writeInsteadPlaceholder"))
                .default_value(text)
        });
        phrase.update(cx, |input, cx| input.focus(window, cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &phrase,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.save(window, cx),
                    _ => {}
                },
            ),
            cx.subscribe(&text, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify()
                }
            }),
        ];
        Self {
            editing,
            snippet,
            phrase,
            text,
            _subscriptions: subscriptions,
        }
    }

    fn draft(&self, cx: &App) -> (Option<Draft>, bool) {
        let phrase = self.phrase.read(cx).value();
        let snippet = self.snippet;
        let duplicate = !phrase.trim().is_empty()
            && is_duplicate(&phrase, snippet, self.editing, SettingsStore::get(cx));
        let draft = if snippet {
            let trigger = phrase.split_whitespace().collect::<Vec<_>>().join(" ");
            // Checked here, not with `validate`: gpui-component allows that on
            // single-line inputs only and asserts it in debug builds.
            let text = self.text.read(cx).value();
            let fits = text.chars().count() <= MAX_TEXT_LENGTH;
            (!trigger_key(&trigger).is_empty() && !text.trim().is_empty() && fits).then(|| {
                Draft::Snippet(Snippet {
                    trigger,
                    text: text.to_string(),
                })
            })
        } else {
            let word = normalize_word(&phrase);
            (!word.is_empty() && word.chars().count() <= MAX_PHRASE_LENGTH)
                .then_some(Draft::Word(word))
        };
        (draft.filter(|_| !duplicate), duplicate)
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(draft), _) = self.draft(cx) else {
            return;
        };
        let stored = SettingsStore::get(cx);
        let mut words = stored.custom_words.clone();
        let mut snippets = stored.snippets.clone();
        place(draft, self.editing, &mut words, &mut snippets);
        let words_changed = words != stored.custom_words;
        let snippets_changed = snippets != stored.snippets;
        if words_changed {
            set_words(words, cx);
        }
        if snippets_changed {
            set_snippets(snippets, cx);
        }
        close_dialog(window, cx);
    }
}

impl Render for EntryDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let (draft, duplicate) = self.draft(cx);
        let phrase = self.phrase.read(cx).value().trim().to_string();
        let count = self.text.read(cx).value().chars().count();
        let focus = gpui::Focusable::focus_handle(self.phrase.read(cx), cx);
        let snippet = self.snippet;
        let title = match (self.editing.is_some(), snippet) {
            (true, true) => t("settings.dictionary.editReplacement"),
            (true, false) => t("settings.dictionary.editWord"),
            (false, true) => t("settings.dictionary.addReplacement"),
            (false, false) => t("settings.dictionary.addWord"),
        };
        let action = if self.editing.is_some() {
            t("settings.dictionary.save")
        } else {
            title.clone()
        };
        let label = |key: &str| {
            div()
                .mb_1()
                .text_px(13.)
                .text_color(p.text.a(0.6))
                .child(t(key))
        };
        Dialog::new(title, focus, close_dialog)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .when(snippet, |el| {
                                el.child(label("settings.dictionary.phraseLabel"))
                            })
                            .child(TextField::new(&self.phrase).w_full())
                            .when(duplicate, |el| {
                                el.child(div().mt_2().text_px(13.).text_color(tw::red_500()).child(
                                    t_with("settings.dictionary.duplicate", &[("word", &phrase)]),
                                ))
                            }),
                    )
                    .when(snippet, |el| {
                        el.child(
                            div()
                                .child(label("settings.dictionary.writeInsteadLabel"))
                                .child(TextArea::new(&self.text).min_height(px(96.)).w_full())
                                .when(count > 0, |el| {
                                    el.child(
                                        div()
                                            .mt_1()
                                            .flex()
                                            .row_dir()
                                            .justify_flex_end()
                                            .xs_text()
                                            .when(count <= MAX_TEXT_LENGTH, |el| {
                                                el.text_color(p.text.a(0.45))
                                            })
                                            .when(count > MAX_TEXT_LENGTH, |el| {
                                                el.text_color(tw::red_500())
                                            })
                                            .child(format!("{count}/{MAX_TEXT_LENGTH}")),
                                    )
                                }),
                        )
                    }),
            )
            .footer(
                Button::new("cancel-entry")
                    .variant(ButtonVariant::Secondary)
                    .child(t("settings.dictionary.cancel"))
                    .on_click(|_, window, cx| close_dialog(window, cx)),
            )
            .footer(
                Button::new("save-entry")
                    .disabled(draft.is_none())
                    .child(action)
                    .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_toolkit::Fix;

    #[test]
    fn normalizes_words() {
        assert_eq!(normalize_word("  <Bespoke>  app \"x\" "), "Bespoke app x");
    }

    #[test]
    fn trigger_keys_ignore_punctuation_and_case() {
        assert_eq!(trigger_key("My-Email"), "myemail");
        assert_eq!(trigger_key("my  email!"), "my email");
    }

    fn snippet(trigger: &str, text: &str) -> Snippet {
        Snippet {
            trigger: trigger.into(),
            text: text.into(),
        }
    }

    fn settings(words: &[&str], snippets: Vec<Snippet>) -> AppSettings {
        let mut settings = crate::settings::get_default_settings();
        settings.custom_words = words.iter().map(|w| w.to_string()).collect();
        settings.snippets = snippets;
        settings
    }

    #[test]
    fn entries_mix_words_and_snippets_by_phrase() {
        let settings = settings(
            &["Karat", "idempotency"],
            vec![snippet("my email", "andy@example.com")],
        );
        let phrases: Vec<_> = entries(&settings).into_iter().map(|e| e.phrase).collect();
        assert_eq!(phrases, ["idempotency", "Karat", "my email"]);
    }

    #[test]
    fn duplicates_span_both_lists_but_skip_the_entry_being_edited() {
        let settings = settings(&["Karat"], vec![snippet("my email", "andy@example.com")]);
        assert!(is_duplicate("karat", true, None, &settings));
        assert!(is_duplicate("My Email", false, None, &settings));
        assert!(is_duplicate("My email!", true, None, &settings));
        assert!(!is_duplicate(
            "Karat",
            false,
            Some(EntryRef::Word(0)),
            &settings
        ));
        assert!(!is_duplicate("IntraFi", false, None, &settings));
    }

    #[test]
    fn an_edit_keeps_its_slot_and_a_new_entry_goes_last() {
        let mut words = vec!["IntraFi".to_string(), "Karat".to_string()];
        let mut snippets = vec![snippet("my email", "a@example.com")];

        place(
            Draft::Word("Carat".into()),
            Some(EntryRef::Word(1)),
            &mut words,
            &mut snippets,
        );
        assert_eq!(words, ["IntraFi", "Carat"]);

        place(
            Draft::Snippet(snippet("intrafi", "IntraFi Network")),
            None,
            &mut words,
            &mut snippets,
        );
        assert_eq!(
            snippets,
            [
                snippet("my email", "a@example.com"),
                snippet("intrafi", "IntraFi Network")
            ]
        );

        place(
            Draft::Snippet(snippet("my email", "b@example.com")),
            Some(EntryRef::Snippet(0)),
            &mut words,
            &mut snippets,
        );
        assert_eq!(snippets[0], snippet("my email", "b@example.com"));
        assert_eq!(words, ["IntraFi", "Carat"]);
    }

    fn fix(heard: &str, written: &str, replacement: bool) -> Fix {
        Fix {
            heard: heard.into(),
            written: written.into(),
            replacement,
        }
    }

    #[test]
    fn pieces_split_a_take_around_its_fixes() {
        let take = Take {
            text: "Ping the karat team, mail andy@example.com.".into(),
            fixes: vec![
                fix("karet", "Karat", false),
                fix("missing", "Nowhere", false),
                fix("my email", "andy@example.com", true),
            ],
        };
        assert_eq!(
            pieces(&take),
            [
                Piece::Plain("Ping the ".into()),
                Piece::Fix {
                    heard: "karet".into(),
                    written: "karat".into(),
                },
                Piece::Plain(" team, mail ".into()),
                Piece::Fix {
                    heard: "my email".into(),
                    written: "andy@example.com".into(),
                },
                Piece::Plain(".".into()),
            ]
        );
    }

    #[test]
    fn fired_counts_fixes_and_words_the_model_spelled_itself() {
        let settings = settings(
            &["Karat", "IntraFi Network", "GPUI", "Hasura"],
            vec![
                snippet("my email", "a@example.com"),
                snippet("sign off", "Bye"),
            ],
        );
        let take = Take {
            text: "Karat uses the IntraFi network and a@example.com".into(),
            fixes: vec![
                fix("karet", "Karat", false),
                fix("My Email", "a@example.com", true),
            ],
        };
        let fired = used_entries(&take, &settings);
        assert_eq!(
            fired,
            HashSet::from([EntryRef::Word(0), EntryRef::Word(1), EntryRef::Snippet(0),])
        );
    }

    #[test]
    fn a_word_catches_only_what_the_dictionary_would_change() {
        assert!(word_catches("intra fi", "IntraFi", 0.18));
        assert!(word_catches("linear", "Linear", 0.18));
        assert!(!word_catches("lino", "Linear", 0.18));
    }
}
