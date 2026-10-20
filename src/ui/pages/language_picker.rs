//! A searchable language list in a popover. Settings uses it for the
//! transcription language (`LanguageSelector.tsx`); the Models page uses it
//! as the language filter. Enter picks the first match, Escape or a click
//! outside closes and clears the search.

use std::rc::Rc;

use gpui::{
    anchored, canvas, deferred, div, point, prelude::*, px, rems, App, Bounds, Context, Corner,
    Entity, FontWeight, Pixels, SharedString, Subscription, Window,
};
use gpui_component::input::{InputEvent, InputState};

use crate::i18n::t;
use crate::ui::components::{icon, KeyboardFocus, RowDirection, TextField, Typography};
use crate::ui::theme::{palette, shadow_lg, Alpha};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PickerStyle {
    /// The settings row button: label and chevron, `min-w-[200px]`.
    Setting,
    /// The Models filter chip: globe icon, label and chevron.
    Filter,
}

type SelectHandler = Rc<dyn Fn(SharedString, &mut Window, &mut App)>;

pub struct LanguagePicker {
    style: PickerStyle,
    /// `(code, label)` in display order.
    options: Vec<(SharedString, SharedString)>,
    /// An entry listed first that Enter never picks ("All Languages").
    all_option: Option<(SharedString, SharedString)>,
    selected: Option<SharedString>,
    disabled: bool,
    open: bool,
    query: Entity<InputState>,
    trigger: Bounds<Pixels>,
    on_select: SelectHandler,
    _subscription: Subscription,
}

impl LanguagePicker {
    pub fn new(
        style: PickerStyle,
        on_select: impl Fn(SharedString, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("settings.general.language.searchPlaceholder"))
        });
        let subscription = cx.subscribe_in(
            &query,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => {
                    if let Some((code, _)) = this.filtered(cx).into_iter().next() {
                        this.pick(code, window, cx);
                    }
                }
                _ => {}
            },
        );
        Self {
            style,
            options: Vec::new(),
            all_option: None,
            selected: None,
            disabled: false,
            open: false,
            query,
            trigger: Bounds::default(),
            on_select: Rc::new(on_select),
            _subscription: subscription,
        }
    }

    pub fn set_options(
        &mut self,
        options: Vec<(SharedString, SharedString)>,
        all_option: Option<(SharedString, SharedString)>,
        selected: Option<SharedString>,
        disabled: bool,
    ) {
        self.options = options;
        self.all_option = all_option;
        self.selected = selected;
        self.disabled = disabled;
        if disabled {
            self.open = false;
        }
    }

    fn filtered(&self, cx: &App) -> Vec<(SharedString, SharedString)> {
        let query = self.query.read(cx).value().to_lowercase();
        self.options
            .iter()
            .filter(|(_, label)| label.to_lowercase().contains(&query))
            .cloned()
            .collect()
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        self.query.update(cx, |q, cx| q.set_value("", window, cx));
        cx.notify();
    }

    fn pick(&mut self, code: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        (self.on_select)(code, window, cx);
        self.close(window, cx);
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        if self.open {
            self.close(window, cx);
        } else {
            self.open = true;
            self.query.update(cx, |q, cx| q.focus(window, cx));
            cx.notify();
        }
    }

    fn selected_label(&self) -> Option<SharedString> {
        let selected = self.selected.as_ref()?;
        self.all_option
            .iter()
            .chain(self.options.iter())
            .find(|(code, _)| code == selected)
            .map(|(_, label)| label.clone())
    }

    fn render_trigger(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let label = self.selected_label();
        let chevron = icon("chevron-down-thin")
            .flex_none()
            .when(self.open, |svg| svg.rotate(gpui::percentage(0.5)));
        let base = div()
            .id("language-picker-trigger")
            .keyboard_focus(cx)
            .on_click(cx.listener(|this, _, window, cx| this.toggle(window, cx)));
        match self.style {
            PickerStyle::Setting => base
                .px_2()
                .py_1()
                .sm_text()
                .font_weight(FontWeight::SEMIBOLD)
                .bg(p.background)
                .border_1()
                .border_color(p.mid_gray.a(0.25))
                .rounded_sm()
                .min_w(px(200.))
                .flex()
                .items_center()
                .justify_between()
                .row_dir()
                .when(self.disabled, |el| el.opacity(0.5).cursor_not_allowed())
                .when(!self.disabled, |el| {
                    el.cursor_pointer()
                        .hover(|s| s.bg(p.mid_gray.a(0.1)).border_color(p.mid_gray.a(0.5)))
                })
                .child(
                    div()
                        .truncate()
                        .child(label.unwrap_or_else(|| t("settings.general.language.auto"))),
                )
                .child(chevron.size(rems(1.)).ms(rems(0.5))),
            PickerStyle::Filter => {
                let active = self.selected.as_ref().is_some_and(|s| s.as_ref() != "all");
                base.flex()
                    .items_center()
                    .gap(rems(0.375))
                    .h_8()
                    .px_3()
                    .sm_text()
                    .font_weight(FontWeight::MEDIUM)
                    .rounded_lg()
                    .cursor_pointer()
                    .when(active, |el| el.bg(p.mid_gray.a(0.15)).text_color(p.text))
                    .when(!active, |el| {
                        el.bg(p.mid_gray.a(0.1))
                            .text_color(p.text.a(0.6))
                            .hover(|s| s.bg(p.mid_gray.a(0.2)))
                    })
                    .child(icon("globe").size(rems(0.875)).flex_none())
                    .child(
                        div().max_w(px(120.)).truncate().child(
                            label.unwrap_or_else(|| t("settings.models.filters.allLanguages")),
                        ),
                    )
                    .child(chevron.size(rems(0.875)))
            }
        }
    }

    fn render_menu(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let trigger = self.trigger;
        let filtered = self.filtered(cx);
        let selected = self.selected.clone();
        let setting = self.style == PickerStyle::Setting;

        let item = |code: SharedString, label: SharedString, cx: &mut Context<Self>| {
            let is_selected = selected.as_ref() == Some(&code);
            div()
                .id(SharedString::from(format!("language-{code}")))
                .w_full()
                .sm_text()
                .when(setting, |el| el.px_2().py_1())
                .when(!setting, |el| el.px_3().py(rems(0.375)))
                .cursor_pointer()
                .hover(|s| s.bg(p.mid_gray.a(0.1)))
                .when(is_selected, |el| {
                    el.bg(p.mid_gray.a(0.15)).font_weight(FontWeight::SEMIBOLD)
                })
                .keyboard_focus(cx)
                .on_click(
                    cx.listener(move |this, _, window, cx| this.pick(code.clone(), window, cx)),
                )
                .child(div().truncate().child(label))
        };

        let mut list = div()
            .id("language-list")
            .max_h(rems(12.))
            .overflow_y_scroll();
        if let Some((code, label)) = self.all_option.clone() {
            list = list.child(item(code, label, cx));
        }
        if filtered.is_empty() {
            list = list.child(
                div()
                    .px_2()
                    .py_2()
                    .sm_text()
                    .text_color(if setting { p.mid_gray } else { p.text.a(0.5) })
                    .flex()
                    .row_dir()
                    .justify_center()
                    .child(t("settings.general.language.noResults")),
            );
        } else {
            for (code, label) in filtered {
                list = list.child(item(code, label, cx));
            }
        }

        let width = if setting {
            trigger.size.width
        } else {
            px(210.)
        };
        let panel = div()
            .id("language-menu")
            .occlude()
            .w(width)
            .bg(p.background)
            .border_1()
            .border_color(p.mid_gray.a(0.25))
            .when(setting, |el| el.rounded_sm())
            .when(!setting, |el| el.rounded_lg())
            .shadow(shadow_lg())
            .overflow_hidden()
            .text_color(p.text)
            .on_mouse_down_out(cx.listener(
                move |this, event: &gpui::MouseDownEvent, window, cx| {
                    if !trigger.contains(&event.position) {
                        this.close(window, cx);
                    }
                },
            ))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.close(window, cx);
                }
            }))
            .child(
                div()
                    .p_2()
                    .border_b_1()
                    .border_color(p.mid_gray.a(if setting { 0.25 } else { 0.4 }))
                    .child(
                        TextField::new(&self.query)
                            .compact()
                            .regular_weight()
                            .bg(p.mid_gray.a(0.1)),
                    ),
            )
            .child(list);

        let (corner, x) = if setting {
            (Corner::TopLeft, trigger.left())
        } else {
            (Corner::TopRight, trigger.right())
        };
        let _ = window;
        deferred(
            anchored()
                .anchor(corner)
                .position(point(x, trigger.bottom() + px(3.75)))
                .snap_to_window_with_margin(px(8.))
                .child(panel),
        )
        .with_priority(1)
    }
}

impl Render for LanguagePicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity().downgrade();
        div()
            .relative()
            .child(self.render_trigger(cx))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let _ = entity.update(cx, |this, _| this.trigger = bounds);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .when(self.open && !self.disabled, |el| {
                el.child(self.render_menu(window, cx))
            })
    }
}
