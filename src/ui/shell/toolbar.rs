//! The toolbar above each page, after Annotate's Inspect window: the page
//! name, the page's tabs as a segmented control, a muted summary, and the
//! page's actions at the end. Empty space in it drags the window.

use gpui::{div, prelude::*, px, AnyElement, App, FontWeight, MouseButton, SharedString, Window};

use crate::ui::components::{RowDirection, Typography};
use crate::ui::theme::palette;

pub const TOOLBAR_HEIGHT: f32 = 48.;
/// The width the macOS traffic lights cover at the window's top left.
const TRAFFIC_LIGHTS_WIDTH: f32 = 84.;

/// What a page puts in the toolbar. Built on every render.
#[derive(Default)]
pub struct Toolbar {
    tabs: Option<AnyElement>,
    meta: Option<SharedString>,
    actions: Vec<AnyElement>,
}

impl Toolbar {
    pub fn new() -> Self {
        Self::default()
    }

    /// The page's tabs, usually a `SegmentedControl`, after the page name.
    pub fn tabs(mut self, tabs: impl IntoElement) -> Self {
        self.tabs = Some(tabs.into_any_element());
        self
    }

    /// Muted text after the tabs, like "8 entries".
    pub fn meta(mut self, meta: impl Into<SharedString>) -> Self {
        self.meta = Some(meta.into());
        self
    }

    /// A control at the end of the toolbar. Actions keep their order.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }
}

/// Draws `toolbar` with `title` as the page name.
pub fn render(title: SharedString, toolbar: Toolbar, _window: &mut Window, cx: &App) -> AnyElement {
    let p = palette(cx);
    // Controls block the mouse so a click on one never reaches the drag
    // layer underneath.
    let control = |element: AnyElement| div().flex_none().occlude().child(element);
    div()
        .relative()
        .flex_none()
        .h(px(TOOLBAR_HEIGHT))
        .px_4()
        .flex()
        .row_dir()
        .items_center()
        .justify_between()
        .gap_4()
        .bg(p.shell)
        .border_b_1()
        .border_color(p.line)
        // In RTL the sidebar moves right, but macOS keeps the traffic lights
        // at the top left, over the toolbar's end.
        .when(cfg!(target_os = "macos") && crate::i18n::is_rtl(), |bar| {
            bar.pl(px(TRAFFIC_LIGHTS_WIDTH))
        })
        .child(
            div()
                .id("toolbar-drag")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_mouse_down(MouseButton::Left, |event, window, _| {
                    if event.click_count == 2 {
                        window.titlebar_double_click();
                    } else {
                        window.start_window_move();
                    }
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .row_dir()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .flex_none()
                        .text_px(13.)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .children(toolbar.tabs.map(control))
                .children(toolbar.meta.map(|meta| {
                    div()
                        .min_w_0()
                        .truncate()
                        .text_px(12.)
                        .text_color(p.mid_gray)
                        .child(meta)
                })),
        )
        .when(!toolbar.actions.is_empty(), |bar| {
            bar.child(
                div()
                    .flex_none()
                    .flex()
                    .row_dir()
                    .items_center()
                    .gap_2()
                    .children(toolbar.actions.into_iter().map(control)),
            )
        })
        .into_any_element()
}
