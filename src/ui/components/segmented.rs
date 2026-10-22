//! A segmented control: a few options side by side in a track, the selected
//! one raised. Reads as a filter, where `Tabs` reads as navigation.

use std::rc::Rc;

use gpui::{div, prelude::*, px, App, ElementId, FontWeight, SharedString, Window};

use super::RowDirection;
use super::{KeyboardFocus, Typography};
use crate::ui::theme::{palette, shadow, Alpha};

type SelectHandler = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct SegmentedControl {
    id: ElementId,
    options: Vec<(SharedString, SharedString)>,
    selected: SharedString,
    on_select: SelectHandler,
}

impl SegmentedControl {
    /// `options` is `(id, label)` pairs in display order.
    pub fn new(
        id: impl Into<ElementId>,
        options: Vec<(SharedString, SharedString)>,
        selected: impl Into<SharedString>,
        on_select: impl Fn(&SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            options,
            selected: selected.into(),
            on_select: Rc::new(on_select),
        }
    }
}

impl RenderOnce for SegmentedControl {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        // A capsule track with a capsule thumb, like the toolbar's glass
        // controls in the design system.
        div()
            .flex()
            .row_dir()
            .items_center()
            .h(px(34.))
            .p(px(3.))
            .gap(px(2.))
            .rounded_full()
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .text_px(13.)
            .font_weight(FontWeight::MEDIUM)
            .children(
                self.options
                    .into_iter()
                    .enumerate()
                    .map(|(index, (id, label))| {
                        let selected = id == self.selected;
                        let on_select = self.on_select.clone();
                        div()
                            .id((self.id.clone(), SharedString::from(index.to_string())))
                            .h(px(26.))
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .rounded_full()
                            .whitespace_nowrap()
                            .cursor_pointer()
                            .keyboard_focus(cx)
                            .when(selected, |el| {
                                el.bg(p.raised).text_color(p.text).shadow(vec![shadow(
                                    0.,
                                    1.,
                                    2.,
                                    0.,
                                    if p.is_dark { 0.25 } else { 0.08 },
                                )])
                            })
                            .when(!selected, |el| {
                                el.text_color(p.mid_gray)
                                    .hover(|s| s.text_color(p.text.a(0.85)))
                            })
                            .on_click(move |_, window, cx| on_select(&id, window, cx))
                            .child(label)
                    }),
            )
    }
}
