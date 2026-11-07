use gpui::{AnyElement, div, prelude::*, px};

use super::toolbar::{TOOLBAR_HEIGHT, drag_region};
use crate::ui::components::RowDirection;
use crate::ui::glass::{GlassStyle, glass};

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
