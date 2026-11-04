use gpui::{
    div, prelude::*, px, rems, AnyView, App, Context, Entity, FontWeight, SharedString,
    Subscription, Window,
};
use gpui_component::input::{InputEvent, InputState};

use crate::i18n::{t, t_with};
use crate::settings::{AppSettings, Snippet};
use crate::shortcut;
use crate::ui::components::{
    focus_handle_for, icon, Button, ButtonVariant, Dialog, FocusVisible, KeyboardFocus,
    RowDirection, SettingsGroup, TextArea, TextField, Typography,
};
use crate::ui::shell::{close_dialog, open_dialog};
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{palette, tw, Alpha};

use super::rows::set;

const MAX_PHRASE_LENGTH: usize = 50;
const MAX_TEXT_LENGTH: usize = 4000;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

pub struct DictionaryPage {
    store: Entity<SettingsStore>,
    _subscriptions: Vec<Subscription>,
}

impl DictionaryPage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let store = SettingsStore::global(cx);
        let subscriptions = vec![cx.observe(&store, |_, _, cx| cx.notify())];
        Self {
            store,
            _subscriptions: subscriptions,
        }
    }

    fn render_dictionary(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let store = self.store.read(cx);
        let updating = store.is_updating("custom_words") || store.is_updating("snippets");
        let all = entries(&store.settings);
        let (replacements, words): (Vec<Entry>, Vec<Entry>) =
            all.into_iter().partition(|entry| entry.text.is_some());
        let entry_focus = FocusVisible::styled(cx, |s| s.underline());
        let section_title = |key: &str| {
            div()
                .text_px(11.)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.mid_gray)
                .child(t(key))
        };

        let rows = replacements.into_iter().map(|entry| {
            let at = entry.at;
            let id = entry_id(at);
            let group = SharedString::from(format!("entry-{id}"));
            let handle = focus_handle_for(SharedString::from(format!("edit-{id}")).into(), cx);
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
                .hover(|s| s.bg(p.card))
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
                        .h(px(38.))
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
                                .truncate()
                                .text_color(p.text.a(0.7))
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
                        .rounded_full()
                        .bg(p.card)
                        .border_1()
                        .border_color(p.line)
                        .text_px(13.)
                        .whitespace_nowrap()
                        .cursor_pointer()
                        .hover(|s| s.bg(p.text.a(0.08)))
                        .on_click(move |_, window, cx| {
                            open_entry_dialog(Target::Existing(at), window, cx)
                        })
                        .child(entry.phrase.clone()),
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
                .flex_col()
                .gap(px(2.))
                .child(section_title(title))
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
                    .sm_text()
                    .text_color(p.text.a(0.65))
                    .child(t("settings.dictionary.intro")),
            )
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
                            .child(add_replacement_row(cx)),
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
}

impl super::Page for DictionaryPage {}

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

const PHRASE_COLUMN: f32 = 200.;
const ARROW_COLUMN: f32 = 28.;
const CHIP_HEIGHT: f32 = 28.;

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

fn add_replacement_row(cx: &App) -> impl IntoElement {
    let p = palette(cx);
    div()
        .id("add-replacement")
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
}
