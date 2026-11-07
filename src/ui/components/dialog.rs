use std::rc::Rc;

use gpui::{
    AnyElement, App, FocusHandle, FontWeight, KeyDownEvent, MouseButton, Pixels, SharedString,
    Window, div, prelude::*, px, rems,
};

use super::RowDirection;
use super::{KeyboardFocus, Typography, icon};
use crate::ui::glass::{GlassSpec, GlassStyle, glass_with};
use crate::ui::theme::{Alpha, palette, radius, shadow_xl};

type CloseHandler = Rc<dyn Fn(&mut Window, &mut App)>;

pub const DIALOG_CONTEXT: &str = "BespokeDialog";

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

/// What a dialog's panel is made of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DialogMaterial {
    /// Regular glass, like the dropdowns and toasts. Sized to its content.
    Popup,
    /// The main window's own material, window glass tinted to the content surface. Fills the
    /// window's height, for a dialog that stands in for a page.
    Window,
}

/// The parts every dialog shares: the backdrop that closes it on a click, the focus trap, Escape,
/// and the panel with its material. The children lay out in a column inside the panel.
#[derive(IntoElement)]
pub struct DialogFrame {
    children: Vec<AnyElement>,
    width: Pixels,
    material: DialogMaterial,
    focus: FocusHandle,
    on_close: CloseHandler,
}

impl DialogFrame {
    pub fn new(focus: FocusHandle, on_close: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        Self::with_close(focus, Rc::new(on_close))
    }

    fn with_close(focus: FocusHandle, on_close: CloseHandler) -> Self {
        Self {
            children: Vec::new(),
            width: px(480.),
            material: DialogMaterial::Popup,
            focus,
            on_close,
        }
    }

    /// The panel's width, narrowed to the window when the window is smaller.
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    pub fn material(mut self, material: DialogMaterial) -> Self {
        self.material = material;
        self
    }
}

impl ParentElement for DialogFrame {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for DialogFrame {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let viewport = window.viewport_size();
        let wide = viewport.width >= px(640.);
        let margin = if wide { rems(1.5) } else { rems(1.) };
        let max_height = viewport.height - margin.to_pixels(window.rem_size()) * 2.;
        // On macOS a dialog is its own window over the main one, so glass can show through.
        let on_glass = cfg!(target_os = "macos") && !crate::ui::popup::hosts(window, cx);
        let fills = self.material == DialogMaterial::Window;

        let close_backdrop = self.on_close.clone();
        let close_key = self.on_close.clone();
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
            .when(!on_glass, |el| el.bg(gpui::black().a(0.5)))
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
                    .w(self.width)
                    .max_w_full()
                    .max_h(max_height)
                    .when(fills, |el| el.h(max_height))
                    .relative()
                    .overflow_hidden()
                    .rounded(radius::PANEL)
                    .when(on_glass, |el| {
                        el.child(match self.material {
                            DialogMaterial::Popup => glass_with(
                                "dialog",
                                GlassSpec {
                                    style: GlassStyle::Regular,
                                    radius: radius::PANEL,
                                    tint: None,
                                },
                            ),
                            DialogMaterial::Window => glass_with(
                                "dialog",
                                GlassSpec {
                                    style: GlassStyle::Window,
                                    radius: radius::PANEL,
                                    tint: Some(p.surface),
                                },
                            ),
                        })
                    })
                    .when(!on_glass, |el| {
                        el.border_1()
                            .border_color(p.line)
                            .bg(p.background)
                            .shadow(shadow_xl())
                    })
                    .text_color(p.text)
                    .children(self.children),
            )
    }
}

/// A dialog with a title row and close button, a scrolling body, and an optional footer.
#[derive(IntoElement)]
pub struct Dialog {
    title: SharedString,
    body: Vec<AnyElement>,
    footer: Vec<AnyElement>,
    width: Option<Pixels>,
    focus: FocusHandle,
    on_close: CloseHandler,
}

impl Dialog {
    pub fn new(
        title: impl Into<SharedString>,
        focus: FocusHandle,
        on_close: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            title: title.into(),
            body: Vec::new(),
            footer: Vec::new(),
            width: None,
            focus,
            on_close: Rc::new(on_close),
        }
    }

    /// The panel's width in place of the frame's default, narrowed to the window when the window
    /// is smaller.
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
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
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = palette(cx);
        let close_button = self.on_close.clone();
        let header = div()
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
            );
        let body = div()
            .id("dialog-body")
            .min_h_0()
            .flex_1()
            .overflow_y_scroll()
            .px_5()
            .pb_4()
            .pt_2()
            .children(self.body);
        let footer = (!self.footer.is_empty()).then(|| {
            div()
                .flex()
                .row_dir()
                .flex_shrink_0()
                .justify_flex_end()
                .gap_2()
                .px_5()
                .pb_4()
                .pt_2()
                .children(self.footer)
        });
        let mut frame = DialogFrame::with_close(self.focus, self.on_close);
        if let Some(width) = self.width {
            frame = frame.width(width);
        }
        frame.child(header).child(body).children(footer)
    }
}
