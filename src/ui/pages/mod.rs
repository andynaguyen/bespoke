//! One view per sidebar section. A page is rebuilt each time its section is
//! opened, like the React components remounting.

#[cfg(target_os = "macos")]
mod annotate;
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

use gpui::{AnyView, App, AppContext, Window};

use crate::ui::shell::Section;

pub fn build(section: Section, window: &mut Window, cx: &mut App) -> AnyView {
    match section {
        Section::Dictate => cx.new(|cx| dictate::DictatePage::new(window, cx)).into(),
        Section::Stats => cx.new(stats::StatsPage::new).into(),
        Section::Models => cx.new(|cx| models::ModelsPage::new(window, cx)).into(),
        #[cfg(target_os = "macos")]
        Section::Annotate => cx.new(annotate::AnnotatePage::new).into(),
        #[cfg(not(target_os = "macos"))]
        Section::Annotate => cx.new(|_| gpui::EmptyView).into(),
        #[cfg(target_os = "macos")]
        Section::Mcp => cx.new(mcp::McpPage::new).into(),
        #[cfg(not(target_os = "macos"))]
        Section::Mcp => cx.new(|_| gpui::EmptyView).into(),
        Section::Debug => cx.new(debug::DebugPage::new).into(),
        Section::Settings => cx.new(|cx| settings::SettingsPage::new(window, cx)).into(),
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
