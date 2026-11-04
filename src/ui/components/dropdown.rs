use std::rc::Rc;

use gpui::{
    anchored, canvas, deferred, div, point, prelude::*, px, rems, AnyElement, App, Bounds,
    ClickEvent, Corner, ElementId, FocusHandle, FontWeight, KeyDownEvent, Pixels, Rems,
    SharedString, StyleRefinement, Window,
};

use super::RowDirection;
use super::{icon, FocusVisible, Typography};
use crate::i18n::t;
use crate::ui::theme::{palette, shadow_lg, Alpha, CONTROL_HEIGHT};

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

struct DropdownState {
    open: bool,
    trigger: Bounds<Pixels>,
    trigger_focus: FocusHandle,
    highlight: Option<usize>,
}

type SelectHandler = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;
type OpenHandler = Rc<dyn Fn(&mut Window, &mut App)>;
type TriggerBuilder = Box<dyn FnOnce(bool, &App) -> AnyElement>;

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
    trigger: Option<TriggerBuilder>,
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
            trigger: None,
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

    pub fn on_open(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_open = Some(Rc::new(handler));
        self
    }

    pub fn menu_end_aligned(mut self, width: Rems) -> Self {
        self.menu_width = Some(width);
        self.menu_align_end = true;
        self
    }

    /// Sets the menu's width instead of matching the trigger's.
    pub fn menu_width(mut self, width: Rems) -> Self {
        self.menu_width = Some(width);
        self
    }

    /// Draws the trigger with `build`, which gets whether the menu is open, in place of the
    /// default field and chevron. The trigger keeps its click, keyboard and focus behavior.
    pub fn trigger(mut self, build: impl FnOnce(bool, &App) -> AnyElement + 'static) -> Self {
        self.trigger = Some(Box::new(build));
        self
    }
}

impl Styled for Dropdown {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

type PickHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

const MENU_RADIUS: f32 = 14.;

fn step(options: &[DropdownOption], from: Option<usize>, forward: bool) -> Option<usize> {
    let mut enabled = options
        .iter()
        .enumerate()
        .filter(|(_, o)| !o.disabled)
        .map(|(index, _)| index);
    let next = match (from, forward) {
        (None, true) => enabled.next(),
        (None, false) => enabled.next_back(),
        (Some(from), true) => enabled.find(|&index| index > from),
        (Some(from), false) => enabled.rfind(|&index| index < from),
    };
    next.or(from)
}

fn menu_items(
    id: &ElementId,
    options: &[DropdownOption],
    selected: Option<&SharedString>,
    highlight: Option<usize>,
    pick: PickHandler,
    route: bool,
    cx: &App,
) -> Vec<AnyElement> {
    let p = palette(cx);
    if options.is_empty() {
        return vec![div()
            .px_2()
            .py_1()
            .sm_text()
            .text_color(p.mid_gray)
            .child(t("common.noOptionsFound"))
            .into_any_element()];
    }
    options
        .iter()
        .enumerate()
        .map(|(index, option)| {
            let is_selected = selected == Some(&option.value);
            let pick = pick.clone();
            div()
                .id((id.clone(), SharedString::from(index.to_string())))
                .w_full()
                .text_px(12.5)
                .rounded(px(8.))
                .when(option.description.is_some(), |el| el.px_2().py(px(6.)))
                .when(option.description.is_none(), |el| el.px_2().py(px(5.)))
                .when(is_selected, |el| el.bg(p.mid_gray.a(0.15)))
                .hover(|s| s.bg(p.mid_gray.a(0.1)))
                .when(highlight == Some(index), |el| el.bg(p.accent.a(0.15)))
                .when(option.disabled, |el| el.opacity(0.5).cursor_not_allowed())
                .when(!option.disabled, |el| {
                    el.cursor_pointer().on_click(move |_, window, cx| {
                        if route {
                            let pick = pick.clone();
                            crate::ui::popup::in_host(window, cx, move |window, cx| {
                                pick(index, window, cx)
                            });
                        } else {
                            pick(index, window, cx);
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
                }))
                .into_any_element()
        })
        .collect()
}

impl RenderOnce for Dropdown {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let mut custom_trigger = None;
        let p = palette(cx);
        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| DropdownState {
            open: false,
            trigger: Bounds::default(),
            trigger_focus: cx.focus_handle().tab_stop(true),
            highlight: None,
        });
        let open = state.read(cx).open && !self.disabled;
        let trigger_bounds = state.read(cx).trigger;
        let highlight = state.read(cx).highlight;
        let trigger_focus = state.read(cx).trigger_focus.clone();
        let focus = FocusVisible::ring(cx);
        let options = Rc::new(self.options);
        let first = options
            .iter()
            .position(|o| Some(&o.value) == self.selected.as_ref() && !o.disabled)
            .or_else(|| options.iter().position(|o| !o.disabled));

        let selected_label = options
            .iter()
            .find(|o| Some(&o.value) == self.selected.as_ref())
            .map(|o| o.label.clone())
            .or(self.placeholder.clone())
            .unwrap_or_else(|| "Select an option...".into());

        let pick: PickHandler = {
            let (options, on_select, state) =
                (options.clone(), self.on_select.clone(), state.clone());
            Rc::new(move |index, window, cx| {
                let Some(option) = options.get(index).filter(|o| !o.disabled) else {
                    return;
                };
                if let Some(on_select) = &on_select {
                    on_select(&option.value, window, cx);
                }
                state.update(cx, |state, cx| {
                    state.open = false;
                    state.highlight = None;
                    cx.notify();
                });
            })
        };
        let set_open = {
            let (on_open, state) = (self.on_open.clone(), state.clone());
            Rc::new(
                move |open: bool, highlight: Option<usize>, window: &mut Window, cx: &mut App| {
                    if open && !state.read(cx).open {
                        if let Some(on_open) = &on_open {
                            on_open(window, cx);
                        }
                    }
                    state.update(cx, |state, cx| {
                        state.open = open;
                        state.highlight = highlight;
                        cx.notify();
                    });
                },
            )
        };

        let disabled = self.disabled;
        let in_popup = crate::ui::popup::hosts(window, cx);
        let custom = self.trigger.take().is_some_and(|build| {
            custom_trigger = Some(build(open, cx));
            true
        });
        let trigger = div()
            .id((self.id.clone(), SharedString::from("trigger")))
            .w_full()
            .children(custom_trigger)
            .when(!custom, |el| {
                el.min_w(px(180.))
                    .h(CONTROL_HEIGHT)
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
            })
            .when(disabled, |el| el.opacity(0.5).cursor_not_allowed())
            .when(open && in_popup, |el| {
                let close = set_open.clone();
                el.on_mouse_down_out(move |_, window, cx| close(false, None, window, cx))
            })
            .when(!disabled, |el| {
                let (click_state, click_pick, click_open) =
                    (state.clone(), pick.clone(), set_open.clone());
                let (key_state, key_options, key_open) =
                    (state.clone(), options.clone(), set_open.clone());
                el.cursor_pointer()
                    .when(!custom, |el| {
                        el.hover(|s| s.bg(p.card).border_color(p.mid_gray.a(0.4)))
                    })
                    .on_click(move |event: &ClickEvent, window, cx| {
                        let (is_open, highlight) = {
                            let state = click_state.read(cx);
                            (state.open, state.highlight)
                        };
                        if is_open && event.is_keyboard() {
                            match highlight {
                                Some(index) => click_pick(index, window, cx),
                                None => click_open(false, None, window, cx),
                            }
                            return;
                        }
                        let opening = !is_open;
                        let highlight = (opening && event.is_keyboard()).then_some(first).flatten();
                        click_open(opening, highlight, window, cx);
                    })
                    .on_key_down(move |event: &KeyDownEvent, window, cx| {
                        let (is_open, highlight) = {
                            let state = key_state.read(cx);
                            (state.open, state.highlight)
                        };
                        match event.keystroke.key.as_str() {
                            key @ ("down" | "up") => {
                                cx.stop_propagation();
                                let highlight = if is_open {
                                    step(&key_options, highlight, key == "down")
                                } else {
                                    first
                                };
                                key_open(true, highlight, window, cx);
                            }
                            "escape" if is_open => {
                                cx.stop_propagation();
                                key_open(false, None, window, cx);
                            }
                            "tab" if is_open => key_open(false, None, window, cx),
                            _ => {}
                        }
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

        let gap = rems(0.25).to_pixels(window.rem_size());
        let width = match self.menu_width {
            Some(width) => width.to_pixels(window.rem_size()),
            None => trigger_bounds.size.width,
        };
        let id = self.id.clone();
        let selected = self.selected.clone();
        let menu = open.then(|| {
            if in_popup {
                let spec = crate::ui::popup::Spec {
                    placement: crate::ui::popup::Placement::Below {
                        gap,
                        end: self.menu_align_end,
                    },
                    width: Some(width),
                    frame_radius: Some(px(MENU_RADIUS)),
                    interactive: true,
                };
                crate::ui::popup::anchor("dropdown-menu", spec, move |_, cx| {
                    crate::ui::popup::surface("dropdown-menu", px(MENU_RADIUS))
                        .w_full()
                        .child(
                            div()
                                .id("dropdown-menu-rows")
                                .p(px(4.))
                                .max_h(rems(15.))
                                .overflow_y_scroll()
                                .children(menu_items(
                                    &id,
                                    &options,
                                    selected.as_ref(),
                                    highlight,
                                    pick.clone(),
                                    true,
                                    cx,
                                )),
                        )
                        .into_any_element()
                })
                .into_any_element()
            } else {
                let close = set_open.clone();
                let menu = div()
                    .id((self.id.clone(), SharedString::from("menu")))
                    .occlude()
                    .w(width)
                    .p(px(4.))
                    .bg(p.background)
                    .border_1()
                    .border_color(p.line)
                    .rounded(px(MENU_RADIUS))
                    .shadow(shadow_lg())
                    .max_h(rems(15.))
                    .overflow_y_scroll()
                    .text_color(p.text)
                    .on_mouse_down_out(move |event, window, cx| {
                        if !trigger_bounds.contains(&event.position) {
                            close(false, None, window, cx);
                        }
                    })
                    .children(menu_items(
                        &id,
                        &options,
                        selected.as_ref(),
                        highlight,
                        pick.clone(),
                        false,
                        cx,
                    ));
                let (corner, x) = if self.menu_align_end {
                    (Corner::TopRight, trigger_bounds.right())
                } else {
                    (Corner::TopLeft, trigger_bounds.left())
                };
                deferred(
                    anchored()
                        .anchor(corner)
                        .position(point(x, trigger_bounds.bottom() + gap))
                        .snap_to_window_with_margin(px(8.))
                        .child(menu),
                )
                .with_priority(1)
                .into_any_element()
            }
        });

        let mut root = div();
        *root.style() = self.style;
        root.relative().child(trigger).child(tracker).children(menu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrows_skip_disabled_options_and_stop_at_the_ends() {
        let options = vec![
            DropdownOption::new("a", "A"),
            DropdownOption::new("b", "B").disabled(true),
            DropdownOption::new("c", "C"),
        ];
        assert_eq!(step(&options, None, true), Some(0));
        assert_eq!(step(&options, None, false), Some(2));
        assert_eq!(step(&options, Some(0), true), Some(2));
        assert_eq!(step(&options, Some(2), false), Some(0));
        assert_eq!(step(&options, Some(2), true), Some(2));
        assert_eq!(step(&options, Some(0), false), Some(0));
    }
}
