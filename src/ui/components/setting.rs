//! `ui/SettingContainer.tsx` and `ui/SettingsGroup.tsx`.

use gpui::{div, prelude::*, px, AnyElement, App, FontWeight, SharedString, Window};

use super::RowDirection;
use super::Typography;
use crate::ui::theme::palette;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum SettingLayout {
    /// Title and description on the left, control on the right.
    #[default]
    Horizontal,
    /// Title and description above a full-width control.
    Stacked,
}

/// A titled setting row inside a [`SettingsGroup`], which draws the card and
/// the dividers. Every React call site passes `grouped`, so this port has no
/// standalone card form.
#[derive(IntoElement)]
pub struct SettingContainer {
    title: SharedString,
    description: Option<SharedString>,
    layout: SettingLayout,
    disabled: bool,
    children: Vec<AnyElement>,
}

impl SettingContainer {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            description: None,
            layout: SettingLayout::Horizontal,
            disabled: false,
            children: Vec::new(),
        }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn layout(mut self, layout: SettingLayout) -> Self {
        self.layout = layout;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl ParentElement for SettingContainer {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for SettingContainer {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let dim = |el: gpui::Div| if self.disabled { el.opacity(0.5) } else { el };

        let title = dim(div().text_px(13.).line_height(px(18.))).child(self.title.clone());
        let description = self.description.clone().map(|description| {
            dim(div()
                .text_px(12.)
                .line_height(px(16.))
                .text_color(p.mid_gray)
                .mt(px(2.)))
            .child(description)
        });

        let header = div().child(title).children(description);

        match self.layout {
            SettingLayout::Horizontal => div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .row_dir()
                .gap_4()
                .min_h(px(48.))
                .px_3()
                .py(px(10.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .max_w(gpui::relative(2. / 3.))
                        .child(header),
                )
                .child(div().relative().flex_shrink_0().children(self.children)),
            SettingLayout::Stacked => div()
                .w_full()
                .px_3()
                .py(px(10.))
                .child(div().mb_2().child(header))
                .child(div().w_full().children(self.children)),
        }
    }
}

/// A titled card of setting rows separated by dividers.
#[derive(IntoElement, Default)]
pub struct SettingsGroup {
    title: Option<SharedString>,
    children: Vec<AnyElement>,
}

impl SettingsGroup {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }
}

impl ParentElement for SettingsGroup {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for SettingsGroup {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(self.title.map(|title| {
                div()
                    .text_px(11.)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.mid_gray)
                    .child(title)
            }))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .bg(p.card)
                    .border_1()
                    .border_color(p.line)
                    .rounded(crate::ui::theme::radius::CARD)
                    .children(
                        self.children
                            .into_iter()
                            .enumerate()
                            .map(move |(index, child)| {
                                div()
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .when(index > 0, |el| el.border_t(px(1.)).border_color(p.line))
                                    .child(child)
                            }),
                    ),
            )
    }
}
