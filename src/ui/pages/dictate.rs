//! The Dictate page (`customize/`): the dictionary of custom words and text
//! snippets, per-app writing style, the History tab, and dictation's settings.

use gpui::{
    div, prelude::*, px, rems, AnyView, App, Context, Entity, FontWeight, Pixels, SharedString,
    Subscription, Window,
};
use gpui_component::input::{InputEvent, InputState};

use crate::i18n::{t, t_with};
use crate::settings::{AppSettings, AppStyleRule, Snippet, WritingStyle};
use crate::shortcut;
use crate::ui::components::{
    focus_handle_for, icon, width_tracker, Button, ButtonVariant, Dialog, Dropdown, DropdownOption,
    FocusVisible, KeyboardFocus, RowDirection, SegmentedControl, TextArea, TextField, Typography,
};
use crate::ui::shell::{close_dialog, open_dialog, Toolbar};
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{palette, tw, Alpha};

use super::dictate_settings::DictateSettings;
use super::history::HistoryPage;
use super::rows::set;
use super::PageLayout;

/// The longest word or snippet trigger.
const MAX_PHRASE_LENGTH: usize = 50;
const MAX_TEXT_LENGTH: usize = 4000;
/// Tailwind's `@2xl` container breakpoint, 42rem at the 15px root.
const STYLE_CARDS_WIDE: f32 = 42. * 15.;

/// Strips `< > " '`, collapses whitespace and trims, like the React form.
fn normalize_word(word: &str) -> String {
    word.chars()
        .filter(|c| !matches!(c, '<' | '>' | '"' | '\''))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The backend's snippet matching key: whole words, case-insensitive,
/// letters and digits only.
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

fn set_app_styles(rules: Vec<AppStyleRule>, cx: &mut App) {
    let stored = rules.clone();
    set(
        cx,
        "app_styles",
        move |s| s.app_styles = stored,
        move |app| shortcut::update_app_styles(app, rules),
    );
}

/// Where a dictionary row is stored. Words and snippets share the Dictionary
/// tab but stay in separate settings: words steer transcription, and snippets
/// expand after post-processing, just before the paste.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryRef {
    Word(usize),
    Snippet(usize),
}

/// A dictionary row: a word, or a snippet that writes `text` in place of
/// `phrase`.
struct Entry {
    at: EntryRef,
    phrase: String,
    text: Option<String>,
}

/// Every word and snippet, sorted by phrase. The two lists share no
/// timestamps, so newest first isn't possible across both.
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

/// Whether `phrase` is in the dictionary as anything other than `editing`.
/// Phrases compare by spelling, case aside. A new snippet also compares
/// against other triggers by the key they match on, so "my email" and
/// "My email!" can't both expand.
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

/// What the dictionary dialog saves.
#[derive(Debug, PartialEq)]
enum Draft {
    Word(String),
    Snippet(Snippet),
}

/// Puts `draft` in place of the entry at `editing`: in the same slot when it
/// keeps its kind, otherwise at the end of the other list.
fn place(
    draft: Draft,
    editing: Option<EntryRef>,
    words: &mut Vec<String>,
    snippets: &mut Vec<Snippet>,
) {
    match editing {
        Some(EntryRef::Word(index)) if index < words.len() => {
            words.remove(index);
        }
        Some(EntryRef::Snippet(index)) if index < snippets.len() => {
            snippets.remove(index);
        }
        _ => {}
    }
    match (draft, editing) {
        (Draft::Word(word), Some(EntryRef::Word(index))) => {
            words.insert(index.min(words.len()), word)
        }
        (Draft::Word(word), _) => words.push(word),
        (Draft::Snippet(snippet), Some(EntryRef::Snippet(index))) => {
            snippets.insert(index.min(snippets.len()), snippet)
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Dictionary,
    Style,
    History,
    Settings,
}

pub struct DictatePage {
    store: Entity<SettingsStore>,
    tab: Tab,
    search: Entity<InputState>,
    /// Built when the History tab opens and dropped when it closes, so the
    /// list reloads each time like a remounted page.
    history: Option<Entity<HistoryPage>>,
    /// Built when the Settings tab opens and dropped when it closes, like
    /// `history`.
    settings: Option<Entity<DictateSettings>>,
    running_apps: Vec<crate::utils::RunningApp>,
    style_cards_width: Pixels,
    _subscriptions: Vec<Subscription>,
}

impl DictatePage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = SettingsStore::global(cx);
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("settings.dictionary.searchPlaceholder"))
        });
        let subscriptions = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        Self {
            store,
            tab: Tab::Dictionary,
            search,
            history: None,
            settings: None,
            running_apps: Vec::new(),
            style_cards_width: px(0.),
            _subscriptions: subscriptions,
        }
    }

    pub fn select_tab(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = match id {
            "style" => Tab::Style,
            "history" => Tab::History,
            "settings" => Tab::Settings,
            _ => Tab::Dictionary,
        };
        if self.tab != Tab::History {
            self.history = None;
        } else if self.history.is_none() {
            self.history = Some(cx.new(|cx| HistoryPage::new(window, cx)));
        }
        if self.tab != Tab::Settings {
            self.settings = None;
        } else if self.settings.is_none() {
            self.settings = Some(cx.new(|cx| DictateSettings::new(window, cx)));
        }
        // Each tab starts with an empty search.
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    fn query(&self, cx: &App) -> String {
        self.search.read(cx).value().trim().to_lowercase()
    }

    fn empty_card(text: SharedString, cx: &App) -> impl IntoElement {
        let p = palette(cx);
        div()
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .rounded(px(8.))
            .px_5()
            .py_10()
            .flex()
            .row_dir()
            .justify_center()
            .sm_text()
            .text_color(p.text.a(0.55))
            .child(text)
    }

    /// The message in place of an empty edge-to-edge list.
    fn empty_list(text: SharedString, cx: &App) -> impl IntoElement {
        let p = palette(cx);
        div()
            .px_8()
            .py_16()
            .flex()
            .row_dir()
            .justify_center()
            .text_center()
            .text_px(13.)
            .text_color(p.mid_gray)
            .child(text)
    }

    /// The trash button that appears when its row is hovered.
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
            .tooltip(move |window, cx| {
                crate::ui::components::Tooltip::text(label.clone(), window, cx)
            })
            .child(icon("trash-2").size(rems(1.)))
    }

    fn render_dictionary(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let store = self.store.read(cx);
        let updating = store.is_updating("custom_words") || store.is_updating("snippets");
        let all = entries(&store.settings);
        if all.is_empty() {
            return Self::empty_list(t("settings.dictionary.empty"), cx).into_any_element();
        }
        let query = self.query(cx);
        let shown: Vec<Entry> = all
            .into_iter()
            .filter(|entry| {
                entry.phrase.to_lowercase().contains(&query)
                    || entry
                        .text
                        .as_ref()
                        .is_some_and(|text| text.to_lowercase().contains(&query))
            })
            .collect();
        if shown.is_empty() {
            let raw = self.search.read(cx).value().trim().to_string();
            return Self::empty_list(
                t_with("settings.dictionary.noMatches", &[("query", &raw)]),
                cx,
            )
            .into_any_element();
        }
        let ids: Vec<String> = shown
            .iter()
            .map(|entry| match entry.at {
                EntryRef::Word(index) => format!("word-{index}"),
                EntryRef::Snippet(index) => format!("snippet-{index}"),
            })
            .collect();
        // `focus-visible:underline`, no ring.
        let entry_focus = FocusVisible::styled(cx, |s| s.underline());
        let entry_handles: Vec<_> = ids
            .iter()
            .map(|id| focus_handle_for(SharedString::from(format!("edit-{id}")).into(), cx))
            .collect();
        div()
            .flex()
            .flex_col()
            .children(
                shown
                    .into_iter()
                    .zip(ids)
                    .zip(entry_handles)
                    .map(|((entry, id), handle)| {
                        let at = entry.at;
                        let group = SharedString::from(format!("entry-{id}"));
                        div()
                            .group(group.clone())
                            .flex()
                            .row_dir()
                            .items_center()
                            .gap_3()
                            .pe(px(10.))
                            .border_b_1()
                            .border_color(p.line)
                            .hover(|s| s.bg(p.card))
                            .child(
                                entry_focus
                                    .clone()
                                    .apply_with(
                                        div().id(SharedString::from(format!("edit-{id}"))),
                                        handle,
                                    )
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .row_dir()
                                    .items_center()
                                    .gap_2()
                                    .ps(px(16.))
                                    .h(px(36.))
                                    .text_px(13.)
                                    .cursor_pointer()
                                    .on_click(move |_, window, cx| {
                                        open_entry_dialog(Some(at), window, cx)
                                    })
                                    .child(div().flex_shrink_0().child(entry.phrase.clone()))
                                    .when_some(entry.text, |el, text| {
                                        // One line, so a multi-line snippet keeps the row short.
                                        let preview =
                                            text.split_whitespace().collect::<Vec<_>>().join(" ");
                                        el.child(div().text_color(p.mid_gray).child("→")).child(
                                            div()
                                                .truncate()
                                                .text_color(p.text.a(0.7))
                                                .child(preview),
                                        )
                                    }),
                            )
                            .child(Self::remove_button(
                                SharedString::from(format!("remove-{id}")),
                                group,
                                t_with("settings.dictionary.remove", &[("word", &entry.phrase)]),
                                updating,
                                move |cx| remove_entry(at, cx),
                                cx,
                            ))
                    }),
            )
            .into_any_element()
    }

    fn render_style(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let wide = self.style_cards_width >= px(STYLE_CARDS_WIDE);
        let card = |key: &str, default: bool| {
            div()
                .when(wide, |el| el.flex_1().min_w_0())
                .bg(p.card)
                .border_1()
                .border_color(p.line)
                .rounded(px(8.))
                .px_4()
                .py(rems(0.875))
                .child(
                    div()
                        .flex()
                        .row_dir()
                        .gap_1()
                        .text_px(13.)
                        .font_weight(FontWeight::MEDIUM)
                        .child(t(&format!("settings.style.styles.{key}.name")))
                        .when(default, |el| {
                            el.child(
                                div()
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(p.text.a(0.55))
                                    .child(t("settings.style.default")),
                            )
                        }),
                )
                .child(
                    div()
                        .text_px(13.)
                        .text_color(p.text.a(0.55))
                        .child(t(&format!("settings.style.styles.{key}.description"))),
                )
                .child(
                    div()
                        .mt_2()
                        .text_px(13.)
                        .text_color(p.text.a(0.8))
                        .child(t(&format!("settings.style.styles.{key}.example"))),
                )
        };
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .sm_text()
                    .text_color(p.text.a(0.65))
                    .child(t("settings.style.intro")),
            )
            .child(
                div()
                    .relative()
                    .flex()
                    .row_dir()
                    .when(!wide, |el| el.flex_col())
                    .gap_3()
                    .child(card("formal", true))
                    .child(card("casual", false))
                    .child(card("veryCasual", false))
                    .child(width_tracker(cx.entity().downgrade(), |this: &mut Self| {
                        &mut this.style_cards_width
                    })),
            )
            .child(if cfg!(target_os = "macos") {
                self.render_app_rules(cx).into_any_element()
            } else {
                div()
                    .bg(p.card)
                    .border_1()
                    .border_color(p.line)
                    .rounded(px(8.))
                    .px_5()
                    .py_4()
                    .sm_text()
                    .text_color(p.text.a(0.55))
                    .child(t("settings.style.macOSOnly"))
                    .into_any_element()
            })
    }

    fn render_app_rules(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let store = self.store.read(cx);
        let rules = store.settings.app_styles.clone();
        let updating = store.is_updating("app_styles");
        let addable: Vec<DropdownOption> = self
            .running_apps
            .iter()
            .filter(|app| !rules.iter().any(|r| r.bundle_id == app.bundle_id))
            .map(|app| DropdownOption::new(app.bundle_id.clone(), app.name.clone()))
            .collect();
        let style_options = || {
            vec![
                DropdownOption::new("formal", t("settings.style.styles.formal.name")),
                DropdownOption::new("casual", t("settings.style.styles.casual.name")),
                DropdownOption::new("very_casual", t("settings.style.styles.veryCasual.name")),
            ]
        };
        let entity = cx.entity().downgrade();
        let running = self.running_apps.clone();
        let add_rules = rules.clone();

        let list = if rules.is_empty() {
            Self::empty_card(t("settings.style.apps.empty"), cx).into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .bg(p.card)
                .border_1()
                .border_color(p.line)
                .rounded(px(8.))
                .children(rules.iter().enumerate().map(|(index, rule)| {
                    let group = SharedString::from(format!("app-rule-{index}"));
                    let style = match rule.style {
                        WritingStyle::Formal => "formal",
                        WritingStyle::Casual => "casual",
                        WritingStyle::VeryCasual => "very_casual",
                    };
                    let change_rules = rules.clone();
                    let remove_rules = rules.clone();
                    let bundle_id = rule.bundle_id.clone();
                    div()
                        .group(group.clone())
                        .flex()
                        .items_center()
                        .justify_between()
                        .row_dir()
                        .gap_3()
                        .px_5()
                        .py(rems(0.625))
                        .when(index > 0, |el| {
                            el.border_t_1().border_color(p.mid_gray.a(0.15))
                        })
                        .child(
                            div()
                                .id(SharedString::from(format!("app-name-{index}")))
                                .min_w_0()
                                .text_px(13.)
                                .truncate()
                                .tooltip({
                                    let bundle_id = bundle_id.clone();
                                    move |window, cx| {
                                        crate::ui::components::Tooltip::text(
                                            bundle_id.clone(),
                                            window,
                                            cx,
                                        )
                                    }
                                })
                                .child(rule.app_name.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .row_dir()
                                .flex_shrink_0()
                                .items_center()
                                .gap_2()
                                .child(
                                    Dropdown::new(
                                        SharedString::from(format!("app-style-{index}")),
                                        style_options(),
                                    )
                                    .selected(Some(style))
                                    .disabled(updating)
                                    .on_select(
                                        move |value, _, cx| {
                                            let style = match value.as_ref() {
                                                "formal" => WritingStyle::Formal,
                                                "very_casual" => WritingStyle::VeryCasual,
                                                _ => WritingStyle::Casual,
                                            };
                                            let mut next = change_rules.clone();
                                            next[index].style = style;
                                            set_app_styles(next, cx);
                                        },
                                    ),
                                )
                                .child(Self::remove_button(
                                    SharedString::from(format!("remove-app-{index}")),
                                    group,
                                    t_with(
                                        "settings.style.apps.remove",
                                        &[("app", &rule.app_name)],
                                    ),
                                    updating,
                                    move |cx| {
                                        let next = remove_rules
                                            .iter()
                                            .filter(|r| r.bundle_id != bundle_id)
                                            .cloned()
                                            .collect();
                                        set_app_styles(next, cx);
                                    },
                                    cx,
                                )),
                        )
                }))
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .row_dir()
                    .gap_3()
                    .child(
                        div()
                            .text_px(11.)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.mid_gray)
                            .child(t("settings.style.apps.title")),
                    )
                    .child(
                        Dropdown::new("add-app-style", addable)
                            .placeholder(t("settings.style.apps.add"))
                            .disabled(updating)
                            .w(rems(14.))
                            .on_open(move |_, cx| {
                                let apps = crate::commands::get_running_apps();
                                let _ = entity.update(cx, |this, cx| {
                                    this.running_apps = apps;
                                    cx.notify();
                                });
                            })
                            .on_select(move |bundle_id, _, cx| {
                                let Some(app) = running
                                    .iter()
                                    .find(|a| a.bundle_id.as_str() == bundle_id.as_ref())
                                else {
                                    return;
                                };
                                let mut next = add_rules.clone();
                                // New rules start Casual.
                                next.push(AppStyleRule {
                                    bundle_id: app.bundle_id.clone(),
                                    app_name: app.name.clone(),
                                    style: WritingStyle::Casual,
                                });
                                set_app_styles(next, cx);
                            }),
                    ),
            )
            .child(list)
    }
}

impl super::Page for DictatePage {
    fn toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Toolbar {
        let p = palette(cx);
        let entity = cx.entity().downgrade();
        let tab_id = match self.tab {
            Tab::Dictionary => "dictionary",
            Tab::Style => "style",
            Tab::History => "history",
            Tab::Settings => "settings",
        };
        let toolbar = Toolbar::new().tabs(SegmentedControl::new(
            "dictate-tabs",
            vec![
                (
                    "dictionary".into(),
                    t("settings.dictionary.tabs.dictionary"),
                ),
                ("style".into(), t("settings.dictionary.tabs.style")),
                ("history".into(), t("settings.dictionary.tabs.history")),
                ("settings".into(), t("settings.dictionary.tabs.settings")),
            ],
            tab_id,
            move |id, window, cx| {
                let id = id.to_string();
                let _ = entity.update(cx, |this, cx| this.select_tab(&id, window, cx));
            },
        ));
        match self.tab {
            Tab::Dictionary => toolbar
                .action(
                    TextField::new(&self.search)
                        .w(px(200.))
                        .rounded_full()
                        .prefix(
                            icon("search")
                                .size(px(13.))
                                .text_color(p.mid_gray)
                                .me(px(6.)),
                        ),
                )
                .action(
                    Button::new("add-new")
                        .gap(px(4.))
                        // The plus draws inside a wider box, which leaves
                        // space before it, so the left takes less padding.
                        .ps(px(13.5))
                        .child(icon("plus").size(px(13.)))
                        .child(t("settings.dictionary.addNew"))
                        .on_click(|_, window, cx| open_entry_dialog(None, window, cx)),
                ),
            Tab::Style | Tab::Settings => toolbar,
            Tab::History => match &self.history {
                Some(history) => history.update(cx, |history, cx| {
                    history.toolbar_actions(toolbar, window, cx)
                }),
                None => toolbar,
            },
        }
    }

    fn inspector(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        match (&self.tab, &self.history) {
            (Tab::History, Some(history)) => {
                history.update(cx, |history, cx| history.inspector(window, cx))
            }
            _ => None,
        }
    }

    fn layout(&self, _cx: &App) -> PageLayout {
        match self.tab {
            Tab::Dictionary => PageLayout::Flush,
            Tab::Style | Tab::Settings => PageLayout::Padded,
            Tab::History => PageLayout::Fill,
        }
    }
}

impl Render for DictatePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.tab {
            Tab::Style => div()
                .w_full()
                .flex()
                .flex_col()
                .child(self.render_style(cx))
                .into_any_element(),
            Tab::History => self
                .history
                .clone()
                .map(|history| history.into_any_element())
                .unwrap_or_else(|| div().into_any_element()),
            Tab::Settings => self
                .settings
                .clone()
                .map(|settings| settings.into_any_element())
                .unwrap_or_else(|| div().into_any_element()),
            Tab::Dictionary => self.render_dictionary(cx).into_any_element(),
        }
    }
}

fn open_entry_dialog(editing: Option<EntryRef>, window: &mut Window, cx: &mut App) {
    open_dialog(window, cx, move |window, cx| -> AnyView {
        cx.new(|cx| EntryDialog::new(editing, window, cx)).into()
    });
}

/// Adds or edits a dictionary entry. With "Write instead" empty it saves a
/// word, otherwise a snippet, and an edit that changes the kind moves the
/// entry to the other list. Enter in the phrase field saves.
pub struct EntryDialog {
    editing: Option<EntryRef>,
    phrase: Entity<InputState>,
    text: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl EntryDialog {
    fn new(editing: Option<EntryRef>, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
                .placeholder(t("settings.dictionary.phrasePlaceholder"))
                .validate(|text, _| text.chars().count() <= MAX_PHRASE_LENGTH)
                .default_value(phrase)
        });
        let text = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder(t("settings.dictionary.textPlaceholder"))
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
            phrase,
            text,
            _subscriptions: subscriptions,
        }
    }

    fn is_snippet(&self, cx: &App) -> bool {
        !self.text.read(cx).value().trim().is_empty()
    }

    /// What the fields would save, or `None` while they can't be saved, and
    /// whether the phrase is already taken.
    fn draft(&self, cx: &App) -> (Option<Draft>, bool) {
        let phrase = self.phrase.read(cx).value();
        let snippet = self.is_snippet(cx);
        let duplicate = !phrase.trim().is_empty()
            && is_duplicate(&phrase, snippet, self.editing, SettingsStore::get(cx));
        let draft = if snippet {
            let trigger = phrase.split_whitespace().collect::<Vec<_>>().join(" ");
            // Checked here, not with `validate`: gpui-component allows that on
            // single-line inputs only and asserts it in debug builds.
            let fits = self.text.read(cx).value().chars().count() <= MAX_TEXT_LENGTH;
            (!trigger_key(&trigger).is_empty() && fits).then(|| {
                Draft::Snippet(Snippet {
                    trigger,
                    text: self.text.read(cx).value().to_string(),
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
        let title = if self.editing.is_some() {
            t("settings.dictionary.editTitle")
        } else {
            t("settings.dictionary.dialogTitle")
        };
        // The button names the kind, so it's clear which one the fields make.
        let action = if self.editing.is_some() {
            t("settings.dictionary.save")
        } else if self.is_snippet(cx) {
            t("settings.dictionary.addSnippet")
        } else {
            t("settings.dictionary.addWord")
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
                            .child(label("settings.dictionary.phraseLabel"))
                            .child(TextField::new(&self.phrase).w_full())
                            .when(duplicate, |el| {
                                el.child(div().mt_2().text_px(13.).text_color(tw::red_500()).child(
                                    t_with("settings.dictionary.duplicate", &[("word", &phrase)]),
                                ))
                            }),
                    )
                    .child(
                        div()
                            .child(label("settings.dictionary.textLabel"))
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
                    ),
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
    fn an_edit_keeps_its_slot_or_moves_to_the_other_list() {
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
            Some(EntryRef::Word(0)),
            &mut words,
            &mut snippets,
        );
        assert_eq!(words, ["Carat"]);
        assert_eq!(
            snippets,
            [
                snippet("my email", "a@example.com"),
                snippet("intrafi", "IntraFi Network")
            ]
        );

        place(
            Draft::Word("my email".into()),
            Some(EntryRef::Snippet(0)),
            &mut words,
            &mut snippets,
        );
        assert_eq!(words, ["Carat", "my email"]);
        assert_eq!(snippets, [snippet("intrafi", "IntraFi Network")]);
    }
}
