//! `ui/Tabs.tsx`: underlined text tabs.

use std::rc::Rc;

use gpui::{div, prelude::*, px, AnyElement, App, ElementId, FontWeight, SharedString, Window};

use super::RowDirection;
use super::{KeyboardFocus, Typography};
use crate::ui::theme::{palette, Alpha};

type SelectHandler = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;
#[derive(IntoElement)]
pub struct Tabs {
    id: ElementId,
    tabs: Vec<(SharedString, SharedString)>,
    selected: SharedString,
    on_select: SelectHandler,
    end: Option<AnyElement>,
}

impl Tabs {
    /// `tabs` is `(id, label)` pairs in display order.
    pub fn new(
        id: impl Into<ElementId>,
        tabs: Vec<(SharedString, SharedString)>,
        selected: impl Into<SharedString>,
        on_select: impl Fn(&SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            tabs,
            selected: selected.into(),
            on_select: Rc::new(on_select),
            end: None,
        }
    }

    /// An element at the end of the row, above the underline, like a filter
    /// for what the tabs show.
    pub fn end(mut self, element: impl IntoElement) -> Self {
        self.end = Some(element.into_any_element());
        self
    }
}

impl RenderOnce for Tabs {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        div()
            .flex()
            .row_dir()
            .items_end()
            .gap_6()
            .border_b_1()
            .border_color(p.mid_gray.a(0.2))
            .children(
                self.tabs
                    .into_iter()
                    .enumerate()
                    .map(|(index, (id, label))| {
                        let selected = id == self.selected;
                        let on_select = self.on_select.clone();
                        div()
                            .id((self.id.clone(), SharedString::from(index.to_string())))
                            .mb(px(-1.))
                            .pb_2()
                            .text_px(15.)
                            .font_weight(FontWeight::MEDIUM)
                            .border_b_2()
                            .cursor_pointer()
                            .keyboard_focus(cx)
                            .when(selected, |el| el.border_color(p.accent).text_color(p.text))
                            .when(!selected, |el| {
                                el.border_color(gpui::transparent_black())
                                    .text_color(p.text.a(0.5))
                                    .hover(|s| s.text_color(p.text))
                            })
                            .on_click(move |_, window, cx| on_select(&id, window, cx))
                            .child(label)
                    }),
            )
            .when_some(self.end, |row, end| {
                row.child(div().flex_1()).child(div().mb_2().child(end))
            })
    }
}
