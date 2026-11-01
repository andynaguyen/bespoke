//! The inspector: a page's glass panel along the window's trailing edge. It
//! runs the window's full height, beside the toolbar rather than under it,
//! and meets the window's edge with no inset, like an inspector in Icon
//! Composer. Its top row, level with the toolbar, holds the actions for what
//! it shows.

use gpui::{div, prelude::*, px, AnyElement};

use super::toolbar::{drag_region, TOOLBAR_HEIGHT};
use crate::ui::components::RowDirection;
use crate::ui::glass::{glass, GlassStyle};

/// A `width`-wide inspector with `actions` in its top row over `content`,
/// which scrolls itself. `id` names its glass and must be unique within the
/// window.
pub fn inspector(
    id: &'static str,
    width: f32,
    actions: Vec<AnyElement>,
    content: impl IntoElement,
) -> AnyElement {
    div()
        .relative()
        .flex_none()
        .w(px(width))
        .h_full()
        .flex()
        .flex_col()
        // The window's corners clip the glass, so it needs none of its own.
        .child(glass(id, GlassStyle::Regular, px(0.)))
        .child(
            div()
                .relative()
                .flex_none()
                .h(px(TOOLBAR_HEIGHT))
                .px_4()
                .flex()
                .row_dir()
                .items_center()
                .justify_end()
                .gap_2()
                // Like the toolbar, empty space in the row drags the window.
                .child(drag_region(id))
                .children(
                    actions
                        .into_iter()
                        .map(|action| div().flex_none().occlude().child(action)),
                ),
        )
        .child(div().flex_1().min_h_0().child(content))
        .into_any_element()
}
