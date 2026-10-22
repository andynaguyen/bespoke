//! `ui/Dialog.tsx`: a modal panel over a dimmed backdrop. Escape, the close
//! button and a click on the backdrop all dismiss it.

use std::rc::Rc;

use gpui::{
    div, prelude::*, px, rems, AnyElement, App, FocusHandle, FontWeight, KeyDownEvent, MouseButton,
    Pixels, SharedString, Window,
};

use super::RowDirection;
use super::{icon, KeyboardFocus, Typography};
use crate::ui::theme::{palette, shadow_xl, Alpha};

type CloseHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// Key context of an open dialog; Tab is rebound inside it.
pub const DIALOG_CONTEXT: &str = "BespokeDialog";

/// Moves focus to the next or previous tab stop inside `trap`, wrapping
/// around, the way `Dialog.tsx` keeps Tab inside the dialog.
fn cycle_focus(trap: &FocusHandle, forward: bool, window: &mut Window, cx: &mut App) {
    // GPUI walks tab stops across the whole window; keep stepping until one
    // lands inside the dialog. The bound covers every stop in the window.
    for _ in 0..512 {
        if forward {
            window.focus_next();
        } else {
            window.focus_prev();
        }
        if trap.contains_focused(window, cx) && !trap.is_focused(window) {
            return;
        }
    }
}

#[derive(IntoElement)]
pub struct Dialog {
    title: SharedString,
    body: Vec<AnyElement>,
    footer: Vec<AnyElement>,
    max_width: Pixels,
    focus: FocusHandle,
    on_close: CloseHandler,
}

impl Dialog {
    /// `focus` should be focused while the dialog is open, so Escape reaches
    /// it; pass the focus handle of the first input when there is one.
    pub fn new(
        title: impl Into<SharedString>,
        focus: FocusHandle,
        on_close: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            title: title.into(),
            body: Vec::new(),
            footer: Vec::new(),
            max_width: px(480.),
            focus,
            on_close: Rc::new(on_close),
        }
    }

    pub fn footer(mut self, child: impl IntoElement) -> Self {
        self.footer.push(child.into_any_element());
        self
    }
}

impl ParentElement for Dialog {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.body.extend(elements);
    }
}

impl RenderOnce for Dialog {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let viewport = window.viewport_size();
        let wide = viewport.width >= px(640.);
        let margin = if wide { rems(1.5) } else { rems(1.) };
        let max_height = viewport.height - margin.to_pixels(window.rem_size()) * 2.;

        let close_backdrop = self.on_close.clone();
        let close_key = self.on_close.clone();
        let close_button = self.on_close.clone();
        let trap = window
            .use_keyed_state("dialog-trap", cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let (next_trap, prev_trap) = (trap.clone(), trap.clone());

        div()
            .id("dialog-backdrop")
            .track_focus(&trap)
            .key_context(DIALOG_CONTEXT)
            .on_action(move |_: &crate::ui::DialogFocusNext, window, cx| {
                cycle_focus(&next_trap, true, window, cx)
            })
            .on_action(move |_: &crate::ui::DialogFocusPrev, window, cx| {
                cycle_focus(&prev_trap, false, window, cx)
            })
            .absolute()
            .inset_0()
            .flex()
            .row_dir()
            .items_center()
            .justify_center()
            .bg(gpui::black().a(0.5))
            .p(margin)
            .occlude()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                close_backdrop(window, cx)
            })
            .child(
                div()
                    .id("dialog-panel")
                    .track_focus(&self.focus)
                    .on_key_down(move |event: &KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" {
                            cx.stop_propagation();
                            close_key(window, cx);
                        }
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .flex()
                    .flex_col()
                    .w_full()
                    .max_w(self.max_width)
                    .max_h(max_height)
                    .overflow_hidden()
                    .rounded(crate::ui::theme::radius::PANEL)
                    .border_1()
                    .border_color(p.line)
                    .bg(p.background)
                    .shadow(shadow_xl())
                    .text_color(p.text)
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .items_start()
                            .justify_between()
                            .row_dir()
                            .gap_3()
                            .px_5()
                            .pt_4()
                            .pb_2()
                            .child(
                                div().min_w_0().child(
                                    div()
                                        .base_text()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(self.title),
                                ),
                            )
                            .child(
                                div()
                                    .id("dialog-close")
                                    .flex_shrink_0()
                                    .p_1()
                                    .rounded_lg()
                                    .cursor_pointer()
                                    .keyboard_focus_ring(0.5, cx)
                                    .text_color(p.mid_gray)
                                    .hover(|s| s.bg(p.mid_gray.a(0.1)).text_color(p.text))
                                    .on_click(move |_, window, cx| close_button(window, cx))
                                    .child(icon("x").size(rems(1.))),
                            ),
                    )
                    .child(
                        div()
                            .id("dialog-body")
                            .min_h_0()
                            .flex_1()
                            .overflow_y_scroll()
                            .px_5()
                            .pb_4()
                            .pt_2()
                            .children(self.body),
                    )
                    .when(!self.footer.is_empty(), |el| {
                        el.child(
                            div()
                                .flex()
                                .row_dir()
                                .flex_shrink_0()
                                .justify_flex_end()
                                .gap_2()
                                .px_5()
                                .pb_4()
                                .pt_2()
                                .children(self.footer),
                        )
                    }),
            )
    }
}
