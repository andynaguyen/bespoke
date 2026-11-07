use gpui::{App, Context, Entity, Window, div, prelude::*};

use crate::i18n::t;
use crate::ui::components::SegmentedControl;
use crate::ui::shell::Toolbar;

use super::PageLayout;
use super::history::HistoryPage;
use super::style_board::StyleBoard;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Style,
    History,
}

pub struct DictatePage {
    tab: Tab,
    style: Option<Entity<StyleBoard>>,
    history: Option<Entity<HistoryPage>>,
}

impl DictatePage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            tab: Tab::Style,
            style: Some(cx.new(|cx| StyleBoard::new(window, cx))),
            history: None,
        }
    }

    pub fn select_tab(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = match id {
            "history" => Tab::History,
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
        cx.notify();
    }
}

impl super::Page for DictatePage {
    fn toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Toolbar {
        let entity = cx.entity().downgrade();
        let tab_id = match self.tab {
            Tab::Style => "style",
            Tab::History => "history",
        };
        let toolbar = Toolbar::new().tabs(SegmentedControl::new(
            "dictate-tabs",
            vec![
                ("style".into(), t("settings.dictionary.tabs.style")),
                ("history".into(), t("settings.dictionary.tabs.history")),
            ],
            tab_id,
            move |id, window, cx| {
                let id = id.to_string();
                let _ = entity.update(cx, |this, cx| this.select_tab(&id, window, cx));
            },
        ));
        match self.tab {
            Tab::Style => toolbar,
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
            Tab::Style => PageLayout::Padded,
            Tab::History => PageLayout::Fill,
        }
    }
}

impl Render for DictatePage {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let view = match self.tab {
            Tab::Style => self.style.clone().map(|view| view.into_any_element()),
            Tab::History => self.history.clone().map(|view| view.into_any_element()),
        };
        view.unwrap_or_else(|| div().into_any_element())
    }
}
