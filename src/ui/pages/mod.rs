pub mod annotate;
pub mod captures;
mod debug;
mod dictate;
mod dictation_settings;
mod dictionary;
mod history;
mod keyword_field;
mod language_picker;
mod mcp;
mod microphone_priority;
mod models;
mod post_processing;
pub mod rows;
pub mod settings;
mod shortcut_input;
mod stats;
mod style_board;
pub mod triage;

use gpui::{AnyElement, AnyView, App, AppContext, Context, Entity, Render, Window};

use crate::ui::shell::{Section, Toolbar};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageLayout {
    Padded,
    Fill,
}

pub trait Page: Render + Sized {
    fn toolbar(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Toolbar {
        Toolbar::new()
    }

    fn layout(&self, _cx: &App) -> PageLayout {
        PageLayout::Padded
    }

    fn inspector(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<AnyElement> {
        None
    }
}

type ToolbarFn = Box<dyn Fn(&mut Window, &mut App) -> Toolbar>;
type InspectorFn = Box<dyn Fn(&mut Window, &mut App) -> Option<AnyElement>>;
type LayoutFn = Box<dyn Fn(&App) -> PageLayout>;

pub struct PageHandle {
    pub view: AnyView,
    toolbar: ToolbarFn,
    layout: LayoutFn,
    inspector: InspectorFn,
}

impl PageHandle {
    fn new<T: Page>(entity: Entity<T>) -> Self {
        let toolbar_entity = entity.clone();
        let layout_entity = entity.clone();
        let inspector_entity = entity.clone();
        Self {
            view: entity.into(),
            toolbar: Box::new(move |window, cx| {
                toolbar_entity.update(cx, |page, cx| page.toolbar(window, cx))
            }),
            layout: Box::new(move |cx| layout_entity.read(cx).layout(cx)),
            inspector: Box::new(move |window, cx| {
                inspector_entity.update(cx, |page, cx| page.inspector(window, cx))
            }),
        }
    }

    pub fn toolbar(&self, window: &mut Window, cx: &mut App) -> Toolbar {
        (self.toolbar)(window, cx)
    }

    pub fn layout(&self, cx: &App) -> PageLayout {
        (self.layout)(cx)
    }

    pub fn inspector(&self, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        (self.inspector)(window, cx)
    }
}

pub fn build(section: Section, window: &mut Window, cx: &mut App) -> PageHandle {
    match section {
        Section::Dictate => PageHandle::new(cx.new(|cx| dictate::DictatePage::new(window, cx))),
        Section::Dictionary => {
            PageHandle::new(cx.new(|cx| dictionary::DictionaryPage::new(window, cx)))
        }
        Section::Stats => PageHandle::new(cx.new(stats::StatsPage::new)),
        Section::Models => PageHandle::new(cx.new(|cx| models::ModelsPage::new(window, cx))),
        Section::Annotate => PageHandle::new(cx.new(annotate::AnnotatePage::new)),
        Section::Mcp => PageHandle::new(cx.new(mcp::McpPage::new)),
        Section::Debug => PageHandle::new(cx.new(debug::DebugPage::new)),
    }
}

#[cfg(debug_assertions)]
pub use dictionary::dev_open_entry_dialog;

pub fn select_tab(page: &AnyView, tab: &str, window: &mut Window, cx: &mut App) {
    if let Ok(dictate) = page.clone().downcast::<dictate::DictatePage>() {
        dictate.update(cx, |page, cx| page.select_tab(tab, window, cx));
    } else if let Ok(stats) = page.clone().downcast::<stats::StatsPage>() {
        stats.update(cx, |page, cx| page.select_tab(tab, cx));
    }
}
