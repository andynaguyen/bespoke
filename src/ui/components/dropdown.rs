//! `ui/Dropdown.tsx`: a button showing the selected label that opens a menu
//! under it. The menu matches the trigger's width unless a width is set, and
//! closes on any click outside it.

use std::rc::Rc;

use gpui::{
    anchored, canvas, deferred, div, point, prelude::*, px, rems, AnyElement, App, Bounds,
    ClickEvent, Corner, ElementId, FocusHandle, FontWeight, KeyDownEvent, Pixels, Rems,
    SharedString, StyleRefinement, Window,
};

use super::RowDirection;
use super::{icon, FocusVisible, Typography};
use crate::i18n::t;
use crate::ui::theme::{palette, shadow_lg, Alpha};

#[derive(Clone)]
pub struct DropdownOption {
    pub value: SharedString,
    pub label: SharedString,
    pub description: Option<SharedString>,
    pub disabled: bool,
}

impl DropdownOption {
    pub fn new(value: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: None,
            disabled: false,
        }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

#[derive(Default)]
struct DropdownState {
    open: bool,
    trigger: Bounds<Pixels>,
    /// Explicit handles so the keyboard can move focus into the menu (which
    /// paints last, after every other tab stop) and back to the trigger.
    trigger_focus: Option<FocusHandle>,
    option_focus: Vec<FocusHandle>,
}

type SelectHandler = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;
type OpenHandler = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Dropdown {
    id: ElementId,
    options: Vec<DropdownOption>,
    selected: Option<SharedString>,
    placeholder: Option<SharedString>,
    disabled: bool,
    on_select: Option<SelectHandler>,
    on_open: Option<OpenHandler>,
    menu_width: Option<Rems>,
    menu_align_end: bool,
    style: StyleRefinement,
}

impl Dropdown {
    pub fn new(id: impl Into<ElementId>, options: Vec<DropdownOption>) -> Self {
        Self {
            id: id.into(),
            options,
            selected: None,
            placeholder: None,
            disabled: false,
            on_select: None,
            on_open: None,
            menu_width: None,
            menu_align_end: false,
            style: StyleRefinement::default(),
        }
    }

    pub fn selected(mut self, value: Option<impl Into<SharedString>>) -> Self {
        self.selected = value.map(Into::into);
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_select(
        mut self,
        handler: impl Fn(&SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    /// Runs when the menu opens (the React `onRefresh`), e.g. to reload
    /// device lists.
    pub fn on_open(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_open = Some(Rc::new(handler));
        self
    }

    /// A fixed menu width aligned to the trigger's end edge
    /// (`menuClassName="right-0 w-80"`).
    pub fn menu_end_aligned(mut self, width: Rems) -> Self {
        self.menu_width = Some(width);
        self.menu_align_end = true;
        self
    }
}

impl Styled for Dropdown {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Dropdown {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| DropdownState::default());
        let option_count = self.options.len();
        state.update(cx, |state, cx| {
            if state.trigger_focus.is_none() {
                state.trigger_focus = Some(cx.focus_handle().tab_stop(true));
            }
            while state.option_focus.len() < option_count {
                state.option_focus.push(cx.focus_handle().tab_stop(true));
            }
        });
        let open = state.read(cx).open && !self.disabled;
        let trigger_bounds = state.read(cx).trigger;
        let trigger_focus = state
            .read(cx)
            .trigger_focus
            .clone()
            .unwrap_or_else(|| cx.focus_handle());
        let option_focus = state.read(cx).option_focus.clone();
        let focus = FocusVisible::ring(cx);
        // The option keyboard focus lands on when the menu opens.
        let first_focus = self
            .options
            .iter()
            .position(|o| Some(&o.value) == self.selected.as_ref() && !o.disabled)
            .or_else(|| self.options.iter().position(|o| !o.disabled))
            .and_then(|index| option_focus.get(index).cloned());

        let selected_label = self
            .options
            .iter()
            .find(|o| Some(&o.value) == self.selected.as_ref())
            .map(|o| o.label.clone())
            .or(self.placeholder.clone())
            .unwrap_or_else(|| "Select an option...".into());

        let disabled = self.disabled;
        let on_open = self.on_open.clone();
        let toggle_state = state.clone();
        let trigger = div()
            .id((self.id.clone(), SharedString::from("trigger")))
            .w_full()
            .min_w(px(180.))
            .h(px(32.))
            .px(px(12.))
            .text_px(13.)
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .rounded(px(10.))
            .flex()
            .row_dir()
            .items_center()
            .gap_2()
            .child(div().flex_1().min_w_0().truncate().child(selected_label))
            .child(
                icon("chevron-down-thin")
                    .size(px(13.))
                    .flex_none()
                    .text_color(p.text)
                    .when(open, |svg| svg.rotate(gpui::percentage(0.5))),
            )
            .when(disabled, |el| el.opacity(0.5).cursor_not_allowed())
            .when(!disabled, |el| {
                el.cursor_pointer()
                    .hover(|s| s.bg(p.card).border_color(p.mid_gray.a(0.4)))
                    .on_click(move |event: &ClickEvent, window, cx| {
                        let opening = !toggle_state.read(cx).open;
                        if opening {
                            if let Some(on_open) = &on_open {
                                on_open(window, cx);
                            }
                            if event.is_keyboard() {
                                if let Some(first) = &first_focus {
                                    window.focus(first);
                                }
                            }
                        }
                        toggle_state.update(cx, |state, cx| {
                            state.open = opening;
                            cx.notify();
                        });
                    })
            });

        let trigger = if disabled {
            trigger
        } else {
            focus.apply_with(trigger, trigger_focus.clone())
        };

        let bounds_state = state.clone();
        let tracker = canvas(
            move |bounds, _, cx| {
                bounds_state.update(cx, |state, _| state.trigger = bounds);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

        let menu = open.then(|| {
            let close_state = state.clone();
            let escape_state = state.clone();
            let escape_focus = trigger_focus.clone();
            let mut menu = div()
                .id((self.id.clone(), SharedString::from("menu")))
                .on_key_down(move |event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        cx.stop_propagation();
                        escape_state.update(cx, |state, cx| {
                            state.open = false;
                            cx.notify();
                        });
                        window.focus(&escape_focus);
                    }
                })
                .occlude()
                .p_1()
                .bg(p.background)
                .border_1()
                .border_color(p.line)
                .rounded(px(14.))
                .p(px(4.))
                .shadow(shadow_lg())
                .max_h(rems(15.))
                .overflow_y_scroll()
                .text_color(p.text)
                .on_mouse_down_out(move |event, _, cx| {
                    if !trigger_bounds.contains(&event.position) {
                        close_state.update(cx, |state, cx| {
                            state.open = false;
                            cx.notify();
                        });
                    }
                });
            menu = match self.menu_width {
                Some(width) => menu.w(width),
                None => menu.w(trigger_bounds.size.width),
            };

            let selected = self.selected.clone();
            let children: Vec<AnyElement> = if self.options.is_empty() {
                vec![div()
                    .px_2()
                    .py_1()
                    .sm_text()
                    .text_color(p.mid_gray)
                    .child(t("common.noOptionsFound"))
                    .into_any_element()]
            } else {
                self.options
                    .iter()
                    .enumerate()
                    .map(|(index, option)| {
                        let is_selected = selected.as_ref() == Some(&option.value);
                        let on_select = self.on_select.clone();
                        let value = option.value.clone();
                        let select_state = state.clone();
                        let return_focus = trigger_focus.clone();
                        let item = div()
                            .id((self.id.clone(), SharedString::from(index.to_string())))
                            .w_full()
                            .text_px(12.5)
                            .rounded(px(8.))
                            .when(option.description.is_some(), |el| el.px_2().py(px(6.)))
                            .when(option.description.is_none(), |el| el.px_2().py(px(5.)))
                            .when(is_selected, |el| el.bg(p.mid_gray.a(0.15)))
                            .hover(|s| s.bg(p.mid_gray.a(0.1)))
                            .when(option.disabled, |el| el.opacity(0.5).cursor_not_allowed())
                            .when(!option.disabled, |el| {
                                el.cursor_pointer().on_click(move |event, window, cx| {
                                    if let Some(on_select) = &on_select {
                                        on_select(&value, window, cx);
                                    }
                                    select_state.update(cx, |state, cx| {
                                        state.open = false;
                                        cx.notify();
                                    });
                                    if event.is_keyboard() {
                                        window.focus(&return_focus);
                                    }
                                })
                            })
                            .child(
                                div()
                                    .when(option.description.is_some() || is_selected, |el| {
                                        el.font_weight(FontWeight::SEMIBOLD)
                                    })
                                    .child(option.label.clone()),
                            )
                            .children(option.description.clone().map(|description| {
                                div()
                                    .mt(rems(0.125))
                                    .xs_text()
                                    .leading_snug()
                                    .text_color(p.mid_gray)
                                    .child(description)
                            }));
                        match option_focus.get(index) {
                            Some(handle) if !option.disabled => {
                                focus.apply_with(item, handle.clone()).into_any_element()
                            }
                            _ => item.into_any_element(),
                        }
                    })
                    .collect()
            };
            menu = menu.children(children);

            let (corner, x) = if self.menu_align_end {
                (Corner::TopRight, trigger_bounds.right())
            } else {
                (Corner::TopLeft, trigger_bounds.left())
            };
            deferred(
                anchored()
                    .anchor(corner)
                    .position(point(
                        x,
                        trigger_bounds.bottom() + rems(0.25).to_pixels(window.rem_size()),
                    ))
                    .snap_to_window_with_margin(px(8.))
                    .child(menu),
            )
            .with_priority(1)
        });

        let mut root = div();
        *root.style() = self.style;
        root.relative().child(trigger).child(tracker).children(menu)
    }
}
