//! `ui/Input.tsx` and `ui/Textarea.tsx`: gpui-component's text editing,
//! framed with Bespoke's border, hover and focus ring.

use gpui::{div, prelude::*, px, App, Entity, Focusable, FontWeight, Pixels, Window};
use gpui_component::input::{Input, InputState};

use super::RowDirection;
use super::Typography;
use crate::ui::theme::{palette, Alpha};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Variant {
    Default,
    Compact,
}

#[derive(IntoElement)]
pub struct TextField {
    state: Entity<InputState>,
    variant: Variant,
    disabled: bool,
    invalid: bool,
    medium_weight: bool,
    style: gpui::StyleRefinement,
    prefix: Option<gpui::AnyElement>,
}

impl TextField {
    pub fn new(state: &Entity<InputState>) -> Self {
        Self {
            state: state.clone(),
            variant: Variant::Default,
            disabled: false,
            invalid: false,
            medium_weight: true,
            style: Default::default(),
            prefix: None,
        }
    }

    /// `variant="compact"`: tighter padding.
    pub fn compact(mut self) -> Self {
        self.variant = Variant::Compact;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Red border, like `aria-invalid` + `border-error`.
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    /// Regular weight text (the Models search box is not `font-medium`).
    pub fn regular_weight(mut self) -> Self {
        self.medium_weight = false;
        self
    }

    /// An element before the text, like the search icon.
    pub fn prefix(mut self, prefix: impl IntoElement) -> Self {
        self.prefix = Some(prefix.into_any_element());
        self
    }
}

impl Styled for TextField {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        &mut self.style
    }
}

/// The border, background and focus ring shared by inputs and textareas.
fn frame(focused: bool, disabled: bool, invalid: bool, cx: &App) -> gpui::Div {
    let p = palette(cx);
    let border = if invalid {
        p.error
    } else if focused {
        p.accent.a(0.7)
    } else {
        p.line
    };
    let mut shadows = Vec::new();
    if focused {
        shadows.push(gpui::BoxShadow {
            color: p.accent.a(0.15),
            offset: gpui::point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(2.),
        });
    }
    div()
        .bg(p.background)
        .border_1()
        .border_color(border)
        .rounded(px(7.))
        .shadow(shadows)
        .text_color(p.text)
        .when(disabled, |el| el.opacity(0.6).cursor_not_allowed())
}

fn padding(variant: Variant) -> (Pixels, Pixels) {
    match variant {
        Variant::Default => (px(9.), px(5.)),
        Variant::Compact => (px(7.), px(3.)),
    }
}

impl RenderOnce for TextField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let focused = self.state.read(cx).focus_handle(cx).is_focused(window);
        let (px_, py_) = padding(self.variant);
        let mut root = frame(focused, self.disabled, self.invalid, cx)
            .id(("text-field", self.state.entity_id()))
            .flex()
            .row_dir()
            .items_center()
            .px(px_)
            .when(!focused && !self.disabled && !self.invalid, |el| {
                el.hover(|s| s.border_color(p.mid_gray.a(0.4)))
            });
        *root.style() = super::merge_styles(root.style().clone(), self.style);
        root.children(self.prefix).child(
            Input::new(&self.state)
                .appearance(false)
                .disabled(self.disabled)
                .flex_1()
                .px_0()
                .py(py_)
                .h(px(16.) + py_ + py_)
                .text_px(12.5)
                .line_height(px(16.))
                .when(self.medium_weight, |el| el.font_weight(FontWeight::MEDIUM)),
        )
    }
}

/// A multi-line field (`ui/Textarea.tsx`). Create its state with
/// `InputState::new(window, cx).multi_line(true)`.
#[derive(IntoElement)]
pub struct TextArea {
    state: Entity<InputState>,
    min_height: Pixels,
    style: gpui::StyleRefinement,
}

impl TextArea {
    pub fn new(state: &Entity<InputState>) -> Self {
        Self {
            state: state.clone(),
            min_height: px(100.),
            style: Default::default(),
        }
    }

    pub fn min_height(mut self, height: Pixels) -> Self {
        self.min_height = height;
        self
    }
}

impl Styled for TextArea {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for TextArea {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let focused = self.state.read(cx).focus_handle(cx).is_focused(window);
        let (px_, py_) = padding(Variant::Default);
        let mut root = frame(focused, false, false, cx)
            .id(("text-area", self.state.entity_id()))
            .when(!focused, |el| {
                el.hover(|s| s.border_color(p.mid_gray.a(0.4)))
            });
        *root.style() = super::merge_styles(root.style().clone(), self.style);
        root.child(
            Input::new(&self.state)
                .appearance(false)
                .h(self.min_height)
                .px(px_)
                .py(py_)
                .text_px(12.5)
                .line_height(px(18.)),
        )
    }
}
