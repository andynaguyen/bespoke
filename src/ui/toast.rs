//! Toast notifications, styled like the React app's Sonner toaster:
//! bottom-right, newest at the bottom, gone after four seconds. In the main
//! window each toast sits on its own glass, in a popup window (`ui::popup`).

use std::time::Duration;

use gpui::{
    div, prelude::*, px, svg, App, Context, Entity, Global, IntoElement, SharedString, Window,
};

use crate::ui::components::{KeyboardFocus, RowDirection};
use crate::ui::popup::{self, Placement, Spec};
use crate::ui::theme::{palette, radius, shadow_lg, Alpha};

const DURATION: Duration = Duration::from_millis(4000);
const MAX_VISIBLE: usize = 3;
const WIDTH: f32 = 356.;
/// The gap between the stack and the window's bottom-right corner.
const MARGIN: f32 = 24.;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Error,
    Info,
    Warning,
}

/// A button shown on the toast, e.g. "How to fix".
#[derive(Clone)]
pub struct ToastAction {
    pub label: SharedString,
    pub on_click: crate::ui::WindowCallback,
}

#[derive(Clone)]
struct Toast {
    id: usize,
    kind: ToastKind,
    title: SharedString,
    description: Option<SharedString>,
    action: Option<ToastAction>,
}

pub struct Toasts {
    toasts: Vec<Toast>,
    next_id: usize,
}

struct GlobalToasts(Entity<Toasts>);

impl Global for GlobalToasts {}

pub fn init(cx: &mut App) {
    let toasts = cx.new(|_| Toasts {
        toasts: Vec::new(),
        next_id: 0,
    });
    cx.set_global(GlobalToasts(toasts));
}

pub fn toasts(cx: &App) -> Entity<Toasts> {
    cx.global::<GlobalToasts>().0.clone()
}

pub fn show(
    kind: ToastKind,
    title: impl Into<SharedString>,
    description: Option<SharedString>,
    action: Option<ToastAction>,
    cx: &mut App,
) {
    let title = title.into();
    toasts(cx).update(cx, |toasts, cx| {
        let id = toasts.next_id;
        toasts.next_id += 1;
        toasts.toasts.push(Toast {
            id,
            kind,
            title,
            description,
            action,
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DURATION).await;
            let _ = this.update(cx, |toasts, cx| {
                toasts.toasts.retain(|t| t.id != id);
                cx.notify();
            });
        })
        .detach();
    });
}

pub fn error(title: impl Into<SharedString>, cx: &mut App) {
    show(ToastKind::Error, title, None, None, cx);
}

pub fn error_with(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    cx: &mut App,
) {
    show(ToastKind::Error, title, Some(description.into()), None, cx);
}

pub fn info(title: impl Into<SharedString>, cx: &mut App) {
    show(ToastKind::Info, title, None, None, cx);
}

pub fn warning(title: impl Into<SharedString>, cx: &mut App) {
    show(ToastKind::Warning, title, None, None, cx);
}

impl Render for Toasts {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let start = self.toasts.len().saturating_sub(MAX_VISIBLE);
        let visible = self.toasts[start..].to_vec();
        if visible.is_empty() {
            return div().into_any_element();
        }
        if popup::hosts(window, cx) {
            let spec = Spec {
                placement: Placement::BottomRight { margin: px(MARGIN) },
                width: Some(px(WIDTH)),
                // Each toast has its own glass and corners.
                frame_radius: None,
                interactive: true,
            };
            return div()
                .absolute()
                .inset_0()
                .child(popup::anchor("toasts", spec, move |_, cx| {
                    stack(&visible, true, cx).into_any_element()
                }))
                .into_any_element();
        }
        div()
            .absolute()
            .bottom(px(MARGIN))
            .right(px(MARGIN))
            .w(px(WIDTH))
            .child(stack(&visible, false, cx))
            .into_any_element()
    }
}

/// The toasts, newest last. `on_glass` draws each on glass in a popup
/// window, whose clicks run against the main window; otherwise each gets an
/// opaque card.
fn stack(toasts: &[Toast], on_glass: bool, cx: &App) -> impl IntoElement {
    let p = palette(cx);
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(14.))
        .children(toasts.iter().map(|toast| {
            let icon = match toast.kind {
                ToastKind::Error => "icons/circle-alert.svg",
                ToastKind::Info => "icons/info.svg",
                ToastKind::Warning => "icons/triangle-alert.svg",
            };
            let card = if on_glass {
                popup::surface(
                    SharedString::from(format!("toast-{}", toast.id)),
                    radius::CARD,
                )
            } else {
                div()
                    .bg(p.background)
                    .border_1()
                    .border_color(p.mid_gray.a(0.2))
                    .rounded(radius::CARD)
                    .shadow(shadow_lg())
            };
            card.id(("toast", toast.id))
                .occlude()
                .px_4()
                .py_3()
                .flex()
                .row_dir()
                .items_center()
                .gap_3()
                .text_sm()
                .text_color(p.text)
                .child(
                    svg()
                        .path(icon)
                        .size(px(16.))
                        .flex_none()
                        .text_color(p.text),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(toast.title.clone()),
                        )
                        .when_some(toast.description.clone(), |el, description| {
                            el.child(div().text_color(p.mid_gray).child(description))
                        }),
                )
                .when_some(toast.action.clone(), |el, action| {
                    let on_click = action.on_click.clone();
                    let button = div()
                        .id(("toast-action", toast.id))
                        .px_2()
                        .py_1()
                        .text_xs()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .rounded_lg()
                        .border_1()
                        .bg(p.mid_gray.a(0.1))
                        .border_color(p.mid_gray.a(0.2))
                        .hover(|s| s.bg(p.mid_gray.a(0.2)))
                        .cursor_pointer()
                        .whitespace_nowrap()
                        .child(action.label.clone())
                        .on_click(move |_, window, cx| {
                            let on_click = on_click.clone();
                            popup::in_host(window, cx, move |window, cx| on_click(window, cx));
                        });
                    // A popup never takes the keyboard, so only the inline
                    // toast is a tab stop.
                    el.child(if on_glass {
                        button
                    } else {
                        button.keyboard_focus(cx)
                    })
                })
        }))
}
