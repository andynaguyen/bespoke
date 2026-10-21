//! A segmented control: a few options side by side in a track, the selected
//! one raised. Reads as a filter, where `Tabs` reads as navigation.

use std::rc::Rc;

use gpui::{div, prelude::*, px, rems, App, ElementId, FontWeight, SharedString, Window};

use super::RowDirection;
use super::{KeyboardFocus, Typography};
use crate::ui::theme::{palette, shadow_sm, Alpha};

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
        // The raised segment is lighter than the track in both themes.
        let raised = if p.is_dark {
            p.text.a(0.12)
        } else {
            p.background
        };
        div()
            .flex()
            .row_dir()
            .p(px(3.))
            .rounded(px(9.))
            .bg(p.card)
            .sm_text()
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
                            .px(rems(0.625))
                            .py(px(3.))
                            .rounded(px(7.))
                            .whitespace_nowrap()
                            .cursor_pointer()
                            .keyboard_focus(cx)
                            .when(selected, |el| {
                                el.bg(raised).text_color(p.text).shadow(shadow_sm())
                            })
                            .when(!selected, |el| {
                                el.text_color(p.text.a(0.55))
                                    .hover(|s| s.text_color(p.text))
                            })
                            .on_click(move |_, window, cx| on_select(&id, window, cx))
                            .child(label)
                    }),
            )
    }
}
