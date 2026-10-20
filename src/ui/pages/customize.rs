//! The Customize page (`customize/`): the custom-word dictionary, text
//! snippets, and per-app writing style.

use gpui::{
    div, prelude::*, px, rems, AnyView, App, Context, Entity, FocusHandle, FontWeight, Pixels,
    SharedString, Subscription, Window,
};
use gpui_component::input::{InputEvent, InputState};

use crate::i18n::{t, t_with};
use crate::settings::{AppStyleRule, Snippet, WritingStyle};
use crate::shortcut;
use crate::ui::components::{
    focus_handle_for, icon, width_tracker, Button, ButtonVariant, Dialog, Dropdown, DropdownOption,
    FocusVisible, KeyboardFocus, RowDirection, Tabs, TextArea, TextField, Typography,
};
use crate::ui::shell::{close_dialog, open_dialog};
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{palette, tw, Alpha};

use super::rows::set;

const MAX_WORD_LENGTH: usize = 50;
const MAX_TRIGGER_LENGTH: usize = 50;
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Words,
    Snippets,
    Style,
}

pub struct CustomizePage {
    store: Entity<SettingsStore>,
    tab: Tab,
    search: Entity<InputState>,
    running_apps: Vec<crate::utils::RunningApp>,
    style_cards_width: Pixels,
    _subscriptions: Vec<Subscription>,
}

impl CustomizePage {
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
            tab: Tab::Words,
            search,
            running_apps: Vec::new(),
            style_cards_width: px(0.),
            _subscriptions: subscriptions,
        }
    }

    pub fn select_tab(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = match id {
            "snippets" => Tab::Snippets,
            "style" => Tab::Style,
            _ => Tab::Words,
        };
        // Each tab starts with an empty search.
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    fn query(&self, cx: &App) -> String {
        self.search.read(cx).value().trim().to_lowercase()
    }

    fn open_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.tab {
            Tab::Words => open_dialog(window, cx, |window, cx| {
                cx.new(|cx| AddWordDialog::new(window, cx)).into()
            }),
            Tab::Snippets => open_snippet_dialog(None, window, cx),
            Tab::Style => {}
        }
    }

    fn empty_card(text: SharedString, cx: &App) -> impl IntoElement {
        let p = palette(cx);
        div()
            .bg(p.card)
            .rounded_2xl()
            .px_5()
            .py_10()
            .flex()
            .row_dir()
            .justify_center()
            .sm_text()
            .text_color(p.text.a(0.55))
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

    fn render_words(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let store = self.store.read(cx);
        let words = store.settings.custom_words.clone();
        let updating = store.is_updating("custom_words");
        let query = self.query(cx);
        if words.is_empty() {
            return Self::empty_card(t("settings.dictionary.empty"), cx).into_any_element();
        }
        let shown: Vec<String> = words
            .iter()
            .rev()
            .filter(|w| w.to_lowercase().contains(&query))
            .cloned()
            .collect();
        if shown.is_empty() {
            let raw = self.search.read(cx).value().trim().to_string();
            return Self::empty_card(
                t_with("settings.dictionary.noMatches", &[("query", &raw)]),
                cx,
            )
            .into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .bg(p.card)
            .rounded_2xl()
            .children(shown.into_iter().enumerate().map(|(index, word)| {
                let group = SharedString::from(format!("word-{index}"));
                let all = words.clone();
                let target = word.clone();
                div()
                    .id(group.clone())
                    .group(group.clone())
                    .flex()
                    .items_center()
                    .justify_between()
                    .row_dir()
                    .gap_3()
                    .px_5()
                    .py(rems(0.875))
                    .when(index > 0, |el| {
                        el.border_t_1().border_color(p.mid_gray.a(0.15))
                    })
                    .child(div().min_w_0().text_px(15.).truncate().child(word.clone()))
                    .child(Self::remove_button(
                        SharedString::from(format!("remove-word-{index}")),
                        group,
                        t_with("settings.dictionary.remove", &[("word", &word)]),
                        updating,
                        move |cx| {
                            set_words(all.iter().filter(|w| **w != target).cloned().collect(), cx)
                        },
                        cx,
                    ))
            }))
            .into_any_element()
    }

    fn render_snippets(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let store = self.store.read(cx);
        let snippets = store.settings.snippets.clone();
        let updating = store.is_updating("snippets");
        let query = self.query(cx);
        if snippets.is_empty() {
            return Self::empty_card(t("settings.dictionary.snippets.empty"), cx)
                .into_any_element();
        }
        // Newest first, each with its stored index for edit and remove.
        let shown: Vec<(usize, Snippet)> = snippets
            .iter()
            .cloned()
            .enumerate()
            .rev()
            .filter(|(_, s)| {
                s.trigger.to_lowercase().contains(&query) || s.text.to_lowercase().contains(&query)
            })
            .collect();
        if shown.is_empty() {
            let raw = self.search.read(cx).value().trim().to_string();
            return Self::empty_card(
                t_with("settings.dictionary.snippets.noMatches", &[("query", &raw)]),
                cx,
            )
            .into_any_element();
        }
        // `focus-visible:underline`, no ring.
        let snippet_focus = FocusVisible::styled(cx, |s| s.underline());
        let snippet_handles: Vec<_> = shown
            .iter()
            .map(|(index, _)| {
                focus_handle_for(
                    SharedString::from(format!("edit-snippet-{index}")).into(),
                    cx,
                )
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .bg(p.card)
            .rounded_2xl()
            .children(
                shown
                    .into_iter()
                    .enumerate()
                    .map(|(row, (index, snippet))| {
                        let snippet_focus = snippet_focus.clone();
                        let snippet_handle = snippet_handles[row].clone();
                        let group = SharedString::from(format!("snippet-{index}"));
                        let all = snippets.clone();
                        div()
                            .group(group.clone())
                            .flex()
                            .row_dir()
                            .items_center()
                            .gap_3()
                            .pe(rems(1.25))
                            .when(row > 0, |el| {
                                el.border_t_1().border_color(p.mid_gray.a(0.15))
                            })
                            .child(
                                snippet_focus
                                    .apply_with(
                                        div().id(SharedString::from(format!(
                                            "edit-snippet-{index}"
                                        ))),
                                        snippet_handle,
                                    )
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .row_dir()
                                    .items_center()
                                    .gap_2()
                                    .ps(rems(1.25))
                                    .py(rems(0.875))
                                    .text_px(15.)
                                    .cursor_pointer()
                                    .on_click(move |_, window, cx| {
                                        open_snippet_dialog(Some(index), window, cx)
                                    })
                                    .child(div().flex_shrink_0().child(snippet.trigger.clone()))
                                    .child(div().text_color(p.text.a(0.4)).child("→"))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_color(p.text.a(0.7))
                                            .child(snippet.text.clone()),
                                    ),
                            )
                            .child(Self::remove_button(
                                SharedString::from(format!("remove-snippet-{index}")),
                                group,
                                t_with(
                                    "settings.dictionary.snippets.remove",
                                    &[("trigger", &snippet.trigger)],
                                ),
                                updating,
                                move |cx| {
                                    let mut next = all.clone();
                                    next.remove(index);
                                    set_snippets(next, cx);
                                },
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
                .rounded_2xl()
                .px_4()
                .py(rems(0.875))
                .child(
                    div()
                        .flex()
                        .row_dir()
                        .gap_1()
                        .text_px(15.)
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
                    .rounded_2xl()
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
                .rounded_2xl()
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
                                .text_px(15.)
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
                            .text_px(15.)
                            .font_weight(FontWeight::MEDIUM)
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

impl Render for CustomizePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let entity = cx.entity().downgrade();
        let tab_id = match self.tab {
            Tab::Words => "words",
            Tab::Snippets => "snippets",
            Tab::Style => "style",
        };
        let tabs = Tabs::new(
            "customize-tabs",
            vec![
                ("words".into(), t("settings.dictionary.tabs.dictionary")),
                ("snippets".into(), t("settings.dictionary.tabs.snippets")),
                ("style".into(), t("settings.dictionary.tabs.style")),
            ],
            tab_id,
            move |id, window, cx| {
                let id = id.to_string();
                let _ = entity.update(cx, |this, cx| this.select_tab(&id, window, cx));
            },
        );

        let body =
            match self.tab {
                Tab::Style => self.render_style(cx).into_any_element(),
                Tab::Words | Tab::Snippets => {
                    let list = if self.tab == Tab::Words {
                        self.render_words(cx).into_any_element()
                    } else {
                        self.render_snippets(cx).into_any_element()
                    };
                    div()
                        .flex()
                        .flex_col()
                        .gap_5()
                        .child(
                            div()
                                .flex()
                                .row_dir()
                                .items_center()
                                .gap_3()
                                .child(
                                    TextField::new(&self.search).flex_1().prefix(
                                        icon("search")
                                            .size(rems(1.))
                                            .text_color(p.text.a(0.4))
                                            .me(rems(0.5)),
                                    ),
                                )
                                .child(
                                    Button::new("add-new")
                                        .gap(rems(0.375))
                                        .py_2()
                                        .child(icon("plus").size(rems(1.)))
                                        .child(t("settings.dictionary.addNew"))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_add(window, cx)
                                        })),
                                ),
                        )
                        .child(list)
                        .into_any_element()
                }
            };

        div()
            .max_w(rems(48.))
            .w_full()
            .flex()
            .flex_col()
            .gap_5()
            .child(tabs)
            .child(body)
    }
}

/// "Add to dictionary" (`AddWordDialog`). Enter adds.
pub struct AddWordDialog {
    input: Entity<InputState>,
    _subscription: Subscription,
}

impl AddWordDialog {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("settings.dictionary.placeholder"))
                .validate(|text, _| text.chars().count() <= MAX_WORD_LENGTH)
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.add(window, cx),
                _ => {}
            },
        );
        Self {
            input,
            _subscription: subscription,
        }
    }

    fn state(&self, cx: &App) -> (String, bool, bool) {
        let word = normalize_word(&self.input.read(cx).value());
        let words = &SettingsStore::get(cx).custom_words;
        let duplicate = words.contains(&word);
        let can_add = !word.is_empty() && word.chars().count() <= MAX_WORD_LENGTH && !duplicate;
        (word, duplicate, can_add)
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (word, _, can_add) = self.state(cx);
        if !can_add {
            return;
        }
        let mut words = SettingsStore::get(cx).custom_words.clone();
        words.push(word);
        set_words(words, cx);
        close_dialog(window, cx);
    }

    fn focus(&self, cx: &App) -> FocusHandle {
        gpui::Focusable::focus_handle(self.input.read(cx), cx)
    }
}

impl Render for AddWordDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (word, duplicate, can_add) = self.state(cx);
        Dialog::new(
            t("settings.dictionary.dialogTitle"),
            self.focus(cx),
            close_dialog,
        )
        .max_width(px(420.))
        .child(TextField::new(&self.input).w_full())
        .when(duplicate, |el| {
            el.child(
                div()
                    .mt_2()
                    .text_px(13.)
                    .text_color(tw::red_500())
                    .child(t_with("settings.dictionary.duplicate", &[("word", &word)])),
            )
        })
        .footer(
            Button::new("cancel-word")
                .variant(ButtonVariant::Secondary)
                .child(t("settings.dictionary.cancel"))
                .on_click(|_, window, cx| close_dialog(window, cx)),
        )
        .footer(
            Button::new("add-word")
                .disabled(!can_add)
                .child(t("settings.dictionary.add"))
                .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
        )
    }
}

fn open_snippet_dialog(editing: Option<usize>, window: &mut Window, cx: &mut App) {
    open_dialog(window, cx, move |window, cx| -> AnyView {
        cx.new(|cx| SnippetDialog::new(editing, window, cx)).into()
    });
}

/// Add or edit a snippet (`SnippetDialog`). Enter does not submit.
pub struct SnippetDialog {
    editing: Option<usize>,
    trigger: Entity<InputState>,
    text: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl SnippetDialog {
    fn new(editing: Option<usize>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let initial = editing
            .and_then(|i| SettingsStore::get(cx).snippets.get(i).cloned())
            .unwrap_or(Snippet {
                trigger: String::new(),
                text: String::new(),
            });
        let trigger = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("settings.dictionary.snippets.triggerPlaceholder"))
                .validate(|text, _| text.chars().count() <= MAX_TRIGGER_LENGTH)
                .default_value(initial.trigger.clone())
        });
        let text = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder(t("settings.dictionary.snippets.textPlaceholder"))
                .validate(|text, _| text.chars().count() <= MAX_TEXT_LENGTH)
                .default_value(initial.text.clone())
        });
        trigger.update(cx, |input, cx| input.focus(window, cx));
        let subscriptions = vec![
            cx.subscribe(&trigger, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify()
                }
            }),
            cx.subscribe(&text, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify()
                }
            }),
        ];
        Self {
            editing,
            trigger,
            text,
            _subscriptions: subscriptions,
        }
    }

    fn state(&self, cx: &App) -> (bool, bool) {
        let key = trigger_key(&self.trigger.read(cx).value());
        let text = self.text.read(cx).value();
        let duplicate = SettingsStore::get(cx)
            .snippets
            .iter()
            .enumerate()
            .any(|(index, s)| Some(index) != self.editing && trigger_key(&s.trigger) == key);
        let can_save = !key.is_empty() && !text.trim().is_empty() && !duplicate;
        (duplicate, can_save)
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (_, can_save) = self.state(cx);
        if !can_save {
            return;
        }
        let snippet = Snippet {
            trigger: self
                .trigger
                .read(cx)
                .value()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
            text: self.text.read(cx).value().to_string(),
        };
        let mut snippets = SettingsStore::get(cx).snippets.clone();
        match self.editing {
            Some(index) if index < snippets.len() => snippets[index] = snippet,
            _ => snippets.push(snippet),
        }
        set_snippets(snippets, cx);
        close_dialog(window, cx);
    }
}

impl Render for SnippetDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let (duplicate, can_save) = self.state(cx);
        let trigger = self.trigger.read(cx).value().trim().to_string();
        let count = self.text.read(cx).value().chars().count();
        let focus = gpui::Focusable::focus_handle(self.trigger.read(cx), cx);
        let title = if self.editing.is_some() {
            t("settings.dictionary.snippets.editTitle")
        } else {
            t("settings.dictionary.snippets.addTitle")
        };
        Dialog::new(title, focus, close_dialog)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().child(TextField::new(&self.trigger).w_full()).when(
                        duplicate,
                        |el| {
                            el.child(div().mt_2().text_px(13.).text_color(tw::red_500()).child(
                                t_with(
                                    "settings.dictionary.snippets.duplicate",
                                    &[("trigger", &trigger)],
                                ),
                            ))
                        },
                    ))
                    .child(
                        div()
                            .child(TextArea::new(&self.text).min_height(px(150.)).w_full())
                            .child(
                                div()
                                    .mt_1()
                                    .flex()
                                    .row_dir()
                                    .justify_flex_end()
                                    .xs_text()
                                    .text_color(p.text.a(0.45))
                                    .child(format!("{count}/{MAX_TEXT_LENGTH}")),
                            ),
                    ),
            )
            .footer(
                Button::new("cancel-snippet")
                    .variant(ButtonVariant::Secondary)
                    .child(t("settings.dictionary.cancel"))
                    .on_click(|_, window, cx| close_dialog(window, cx)),
            )
            .footer(
                Button::new("save-snippet")
                    .disabled(!can_save)
                    .child(if self.editing.is_some() {
                        t("settings.dictionary.snippets.save")
                    } else {
                        t("settings.dictionary.snippets.add")
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_words() {
        assert_eq!(normalize_word("  <Handy>  app \"x\" "), "Handy app x");
    }

    #[test]
    fn trigger_keys_ignore_punctuation_and_case() {
        assert_eq!(trigger_key("My-Email"), "myemail");
        assert_eq!(trigger_key("my  email!"), "my email");
    }
}
