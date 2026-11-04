use gpui::{div, prelude::*, App, Context, Entity, Window};

use crate::i18n::t;
use crate::ui::components::SegmentedControl;
use crate::ui::shell::Toolbar;

use super::dictation_settings::DictationSettings;
use super::history::HistoryPage;
use super::style_board::StyleBoard;
use super::PageLayout;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Style,
    History,
    Settings,
}

pub struct DictatePage {
    tab: Tab,
    style: Option<Entity<StyleBoard>>,
    history: Option<Entity<HistoryPage>>,
    settings: Option<Entity<DictationSettings>>,
}

impl DictatePage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            tab: Tab::Style,
            style: Some(cx.new(|cx| StyleBoard::new(window, cx))),
            history: None,
            settings: None,
        }
    }

    pub fn select_tab(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = match id {
            "history" => Tab::History,
            "settings" => Tab::Settings,
            _ => Tab::Style,
        };
        if self.tab != Tab::Style {
            self.style = None;
        } else if self.style.is_none() {
            self.style = Some(cx.new(|cx| StyleBoard::new(window, cx)));
        }
        if self.tab != Tab::History {
            self.history = None;
        } else if self.history.is_none() {
            self.history = Some(cx.new(|cx| HistoryPage::new(window, cx)));
        }
        if self.tab != Tab::Settings {
            self.settings = None;
        } else if self.settings.is_none() {
            self.settings = Some(cx.new(|cx| DictationSettings::new(window, cx)));
        }
        cx.notify();
    }
}

impl super::Page for DictatePage {
    fn toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Toolbar {
        let entity = cx.entity().downgrade();
        let tab_id = match self.tab {
            Tab::Style => "style",
            Tab::History => "history",
            Tab::Settings => "settings",
        };
        let toolbar = Toolbar::new().tabs(SegmentedControl::new(
            "dictate-tabs",
            vec![
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
            Tab::Style | Tab::Settings => PageLayout::Padded,
            Tab::History => PageLayout::Fill,
        }
    }
}

impl Render for DictatePage {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let view = match self.tab {
            Tab::Style => self.style.clone().map(|view| view.into_any_element()),
            Tab::History => self.history.clone().map(|view| view.into_any_element()),
            Tab::Settings => self.settings.clone().map(|view| view.into_any_element()),
        };
        view.unwrap_or_else(|| div().into_any_element())
    }
}
