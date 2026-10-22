//! One view per sidebar section. A page is rebuilt each time its section is
//! opened, like the React components remounting.

#[cfg(target_os = "macos")]
mod annotate;
pub mod captures;
mod debug;
mod dictate;
mod history;
mod keyword_field;
mod language_picker;
#[cfg(target_os = "macos")]
mod mcp;
mod models;
mod post_processing;
pub mod rows;
mod settings;
mod shortcut_input;
mod stats;

#[cfg(not(target_os = "macos"))]
use gpui::{div, IntoElement};
use gpui::{AnyElement, AnyView, App, AppContext, Context, Entity, Render, Window};

use crate::ui::shell::{Section, Toolbar};

/// How the shell lays out a page's body under the toolbar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageLayout {
    /// Scrolls, with padding, at most 48rem wide and centered.
    Padded,
    /// Scrolls, edge to edge, for lists with their own row dividers.
    Flush,
    /// Fills the space under the toolbar and scrolls its own parts.
    Fill,
}

/// A sidebar section's view.
pub trait Page: Render + Sized {
    /// The page's tabs, summary and actions for the toolbar.
    fn toolbar(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Toolbar {
        Toolbar::new()
    }

    fn layout(&self, _cx: &App) -> PageLayout {
        PageLayout::Padded
    }

    /// A panel along the window's trailing edge, built with
    /// `shell::inspector`. The shell puts it beside the toolbar and body.
    fn inspector(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<AnyElement> {
        None
    }
}

type ToolbarFn = Box<dyn Fn(&mut Window, &mut App) -> Toolbar>;
type InspectorFn = Box<dyn Fn(&mut Window, &mut App) -> Option<AnyElement>>;
type LayoutFn = Box<dyn Fn(&App) -> PageLayout>;

/// The open page, with its toolbar and layout reachable from the shell.
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

/// A section with nothing to show on this platform.
#[cfg(not(target_os = "macos"))]
struct EmptyPage;

#[cfg(not(target_os = "macos"))]
impl Render for EmptyPage {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[cfg(not(target_os = "macos"))]
impl Page for EmptyPage {}

pub fn build(section: Section, window: &mut Window, cx: &mut App) -> PageHandle {
    match section {
        Section::Dictate => PageHandle::new(cx.new(|cx| dictate::DictatePage::new(window, cx))),
        Section::Stats => PageHandle::new(cx.new(stats::StatsPage::new)),
        Section::Models => PageHandle::new(cx.new(|cx| models::ModelsPage::new(window, cx))),
        #[cfg(target_os = "macos")]
        Section::Annotate => PageHandle::new(cx.new(annotate::AnnotatePage::new)),
        #[cfg(target_os = "macos")]
        Section::Mcp => PageHandle::new(cx.new(mcp::McpPage::new)),
        #[cfg(not(target_os = "macos"))]
        Section::Annotate | Section::Mcp => PageHandle::new(cx.new(|_| EmptyPage)),
        Section::Debug => PageHandle::new(cx.new(debug::DebugPage::new)),
        Section::Settings => PageHandle::new(cx.new(|cx| settings::SettingsPage::new(window, cx))),
    }
}

/// Switches a page's tab by id (Settings, Dictate and Stats tabs), for dev
/// snapshots.
#[cfg(debug_assertions)]
pub fn select_tab(page: &AnyView, tab: &str, window: &mut Window, cx: &mut App) {
    if let Ok(settings) = page.clone().downcast::<settings::SettingsPage>() {
        settings.update(cx, |page, cx| page.select_tab(tab, cx));
    } else if let Ok(dictate) = page.clone().downcast::<dictate::DictatePage>() {
        dictate.update(cx, |page, cx| page.select_tab(tab, window, cx));
    } else if let Ok(stats) = page.clone().downcast::<stats::StatsPage>() {
        stats.update(cx, |page, cx| page.select_tab(tab, cx));
    }
}
