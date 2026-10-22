//! `ui/Button.tsx`.

use gpui::{
    div, prelude::*, px, AnyElement, App, ClickEvent, ElementId, FontWeight, Hsla, SharedString,
    Window,
};

use super::KeyboardFocus;
use super::RowDirection;
use super::Typography;
use crate::ui::theme::{palette, tw, Alpha};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum ButtonVariant {
    #[default]
    Primary,
    Secondary,
    DangerGhost,
    Ghost,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum ButtonSize {
    Sm,
    #[default]
    Md,
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    variant: ButtonVariant,
    size: ButtonSize,
    disabled: bool,
    children: Vec<AnyElement>,
    on_click: Option<ClickHandler>,
    style: gpui::StyleRefinement,
    hover: Option<Box<dyn Fn(gpui::StyleRefinement) -> gpui::StyleRefinement>>,
    title: Option<SharedString>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            variant: ButtonVariant::Primary,
            size: ButtonSize::Md,
            disabled: false,
            children: Vec::new(),
            on_click: None,
            style: Default::default(),
            hover: None,
            title: None,
        }
    }

    /// Extra hover styling on top of the variant's (`hover:text-text`).
    pub fn hover(
        mut self,
        f: impl Fn(gpui::StyleRefinement) -> gpui::StyleRefinement + 'static,
    ) -> Self {
        self.hover = Some(Box::new(f));
        self
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Native-style tooltip text, like the HTML `title` attribute.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl ParentElement for Button {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Styled for Button {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        &mut self.style
    }
}

struct Colors {
    text: Hsla,
    bg: Hsla,
    border: Hsla,
    hover_text: Option<Hsla>,
    hover_bg: Hsla,
    hover_border: Option<Hsla>,
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let transparent = gpui::transparent_black();
        let c = match self.variant {
            ButtonVariant::Primary => Colors {
                text: p.accent_contrast,
                bg: p.accent,
                border: p.accent,
                hover_text: None,
                hover_bg: p.accent.a(0.85),
                hover_border: Some(p.accent.a(0.85)),
            },
            ButtonVariant::Secondary => Colors {
                text: p.text,
                bg: p.card,
                border: p.line,
                hover_text: None,
                hover_bg: p.raised,
                hover_border: None,
            },
            ButtonVariant::DangerGhost => Colors {
                text: tw::red_400(),
                bg: transparent,
                border: transparent,
                hover_text: Some(tw::red_300()),
                hover_bg: tw::red_500().a(0.1),
                hover_border: None,
            },
            ButtonVariant::Ghost => Colors {
                text: p.text,
                bg: transparent,
                border: transparent,
                hover_text: None,
                hover_bg: p.mid_gray.a(0.1),
                hover_border: None,
            },
        };

        let mut button = div()
            .id(self.id)
            .flex()
            .row_dir()
            .items_center()
            .justify_center()
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .rounded(px(7.))
            .border_1()
            .text_color(c.text)
            .bg(c.bg)
            .border_color(c.border);
        button = match self.size {
            ButtonSize::Sm => button.h(px(24.)).px(px(8.)).text_px(12.),
            ButtonSize::Md => button.h(px(28.)).px(px(12.)).text_px(12.5),
        };
        *button.style() = super::merge_styles(button.style().clone(), self.style);

        if self.disabled {
            button = button.opacity(0.5).cursor_not_allowed();
        } else {
            let extra = self.hover;
            button = button.keyboard_focus(cx).cursor_pointer().hover(move |s| {
                let s = s.bg(c.hover_bg);
                let s = match c.hover_text {
                    Some(color) => s.text_color(color),
                    None => s,
                };
                let s = match c.hover_border {
                    Some(color) => s.border_color(color),
                    None => s,
                };
                match &extra {
                    Some(extra) => extra(s),
                    None => s,
                }
            });
            if let Some(handler) = self.on_click {
                button = button.on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    handler(event, window, cx)
                });
            }
        }
        if let Some(title) = self.title {
            button = button
                .tooltip(move |window, cx| super::misc::Tooltip::text(title.clone(), window, cx));
        }
        button.children(self.children)
    }
}
