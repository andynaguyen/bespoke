//! Notices at the top of every page: the macOS Accessibility permission
//! request (`AccessibilityPermissions.tsx`) and the Secure Input warning
//! (`SecureInputWarning.tsx`).

use std::collections::BTreeSet;

use gpui::{div, prelude::*, rems, Context, FontWeight, SharedString, Window};

use crate::app_handle::{AppEvent, AppHandle};
use crate::i18n::{t, t_count};
use crate::secure_input::SecureInputStatus;
use crate::ui::components::{icon, KeyboardFocus, RowDirection, Typography};
use crate::ui::event_bus::event_bus;
use crate::ui::theme::{palette, Alpha};

#[derive(Clone, Copy, PartialEq, Eq)]
enum PermissionState {
    Request,
    Verify,
    Granted,
}

/// Asks for Accessibility access until it is granted. macOS only.
pub struct AccessibilityBanner {
    state: PermissionState,
}

impl AccessibilityBanner {
    pub fn new() -> Self {
        #[cfg(target_os = "macos")]
        let granted = crate::ui::platform::accessibility_granted();
        #[cfg(not(target_os = "macos"))]
        let granted = true;
        Self {
            state: if granted {
                PermissionState::Granted
            } else {
                PermissionState::Request
            },
        }
    }

    fn on_click(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        match self.state {
            PermissionState::Request => {
                crate::ui::platform::request_accessibility();
                self.state = PermissionState::Verify;
            }
            PermissionState::Verify => {
                if crate::ui::platform::accessibility_granted() {
                    self.state = PermissionState::Granted;
                }
            }
            PermissionState::Granted => {}
        }
        cx.notify();
    }
}

impl Render for AccessibilityBanner {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.state == PermissionState::Granted {
            return div().into_any_element();
        }
        let p = palette(cx);
        div()
            .w_full()
            .px_5()
            .py_4()
            .rounded_2xl()
            .bg(p.card)
            .child(
                div()
                    .flex()
                    .justify_between()
                    .row_dir()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .sm_text()
                            .font_weight(FontWeight::MEDIUM)
                            .child(t("accessibility.permissionsDescription")),
                    )
                    .child(
                        div()
                            .id("accessibility-open-settings")
                            .min_h(rems(2.5))
                            .px_3()
                            .py(rems(0.375))
                            .sm_text()
                            .font_weight(FontWeight::MEDIUM)
                            .bg(p.mid_gray.a(0.15))
                            .hover(|s| s.bg(p.mid_gray.a(0.25)))
                            .rounded_lg()
                            .cursor_pointer()
                            .whitespace_nowrap()
                            .flex()
                            .row_dir()
                            .items_center()
                            .justify_center()
                            .keyboard_focus(cx)
                            .on_click(cx.listener(|this, _, _, cx| this.on_click(cx)))
                            .child(t("accessibility.openSettings")),
                    ),
            )
            .into_any_element()
    }
}

/// Warns that macOS Secure Input is blocking shortcuts. Dismissing hides it
/// until the episode ends; the next episode warns again.
pub struct SecureInputWarning {
    status: Option<SecureInputStatus>,
    dismissed: bool,
}

impl SecureInputWarning {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.subscribe(&event_bus(cx), |this, _, event: &AppEvent, cx| {
            if event.name == "secure-input-changed" {
                if let Some(status) = event.parse::<SecureInputStatus>() {
                    this.set_status(status, cx);
                }
            }
        })
        .detach();
        let status = crate::secure_input::get_secure_input_status(AppHandle::global().clone());
        let mut this = Self {
            status: None,
            dismissed: false,
        };
        this.status = Some(status);
        this
    }

    fn impacted(&self) -> bool {
        self.status.as_ref().is_some_and(|status| {
            (status.sustained
                && (!status.degraded_bindings.is_empty() || !status.uncovered_bindings.is_empty()))
                || status.recorder_blocked
        })
    }

    fn set_status(&mut self, status: SecureInputStatus, cx: &mut Context<Self>) {
        self.status = Some(status);
        // A new episode warns again.
        if !self.impacted() {
            self.dismissed = false;
        }
        cx.notify();
    }

    fn message(&self) -> SharedString {
        let Some(status) = &self.status else {
            return SharedString::default();
        };
        let affected: BTreeSet<&String> = status
            .uncovered_bindings
            .iter()
            .chain(status.degraded_bindings.iter())
            .collect();
        let count = affected.len() as i64;
        match (&status.culprit_name, count > 0) {
            (Some(name), true) => {
                t_count("secureInput.blockedWithCulprit", count, &[("name", name)])
            }
            (None, true) => t_count("secureInput.blockedNoCulprit", count, &[]),
            (Some(name), false) => {
                crate::i18n::t_with("secureInput.recorderBlockedWithCulprit", &[("name", name)])
            }
            (None, false) => t("secureInput.recorderBlockedNoCulprit"),
        }
    }
}

impl Render for SecureInputWarning {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.impacted() || self.dismissed {
            return div().into_any_element();
        }
        let p = palette(cx);
        div()
            .w_full()
            .rounded_2xl()
            .border_1()
            .border_color(p.warning.a(0.3))
            .bg(p.warning.a(0.1))
            .px_4()
            .py_3()
            .child(
                div()
                    .flex()
                    .row_dir()
                    .items_center()
                    .gap_3()
                    .child(
                        icon("triangle-alert")
                            .size(rems(1.25))
                            .flex_none()
                            .text_color(p.warning),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .sm_text()
                            .font_weight(FontWeight::MEDIUM)
                            .line_height(rems(1.25))
                            .child(self.message()),
                    )
                    .child(
                        div()
                            .id("secure-input-dismiss")
                            .cursor_pointer()
                            .rounded_sm()
                            .p(rems(0.375))
                            .text_color(p.mid_gray)
                            .hover(|s| s.bg(p.warning.a(0.15)).text_color(p.warning))
                            .keyboard_focus(cx)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.dismissed = true;
                                cx.notify();
                            }))
                            .child(icon("x").size(rems(1.))),
                    ),
            )
            .into_any_element()
    }
}
