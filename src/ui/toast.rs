//! Toast notifications, styled like the React app's Sonner toaster:
//! bottom-right, newest at the bottom, gone after four seconds.

use std::time::Duration;

use gpui::{
    div, prelude::*, px, svg, App, Context, Entity, Global, IntoElement, SharedString, Window,
};

use crate::ui::components::{KeyboardFocus, RowDirection};
use crate::ui::theme::{palette, shadow_lg, Alpha};

const DURATION: Duration = Duration::from_millis(4000);
const MAX_VISIBLE: usize = 3;

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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let start = self.toasts.len().saturating_sub(MAX_VISIBLE);
        div()
            .absolute()
            .bottom(px(24.))
            .right(px(24.))
            .w(px(356.))
            .flex()
            .flex_col()
            .gap(px(14.))
            .children(self.toasts[start..].iter().map(|toast| {
                let icon = match toast.kind {
                    ToastKind::Error => "icons/circle-alert.svg",
                    ToastKind::Info => "icons/info.svg",
                    ToastKind::Warning => "icons/triangle-alert.svg",
                };
                div()
                    .id(("toast", toast.id))
                    .occlude()
                    .bg(p.background)
                    .border_1()
                    .border_color(p.mid_gray.a(0.2))
                    .rounded_xl()
                    .shadow(shadow_lg())
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
                        el.child(
                            div()
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
                                .keyboard_focus(cx)
                                .on_click(move |_, window, cx| on_click(window, cx)),
                        )
                    })
            }))
    }
}
