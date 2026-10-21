//! The MCP page: the Agent Access switch for `annotate::mcp`, which serves Annotate's captures to
//! coding agents, then, while it's on, how to add the server to Claude Code here and on remote
//! machines.

use std::time::Duration;

use gpui::{div, prelude::*, AnyElement, App, Context, Task};

use crate::annotate::mcp;
use crate::app_handle::AppHandle;
use crate::i18n::{t, t_with};
use crate::ui::components::{SettingContainer, SettingsGroup, ToggleSwitchRow};
use crate::ui::stores::SettingsStore;
use crate::ui::theme::palette;
use crate::ui::toast;

use super::annotate::icon_button;
use super::rows;

/// The setting behind the Agent Access switch.
const SETTING: &str = "annotate_mcp_enabled";

/// How often the page re-reads the server's status. `mcp::sync` starts the server in the
/// background, so a port that's already taken shows up after the switch has saved.
const POLL: Duration = Duration::from_millis(500);

/// Copies `text` and confirms with a toast.
fn copy(text: Result<String, String>, cx: &mut App) {
    match text.and_then(|text| AppHandle::global().clipboard().write_text(text)) {
        Ok(()) => toast::info(t("mcp.copied"), cx),
        Err(error) => {
            log::error!("Copying for Agent Access failed: {error}");
            toast::error(t("mcp.copyFailed"), cx);
        }
    }
}

pub struct McpPage {
    status: mcp::Status,
    _poll: Task<()>,
}

impl McpPage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.observe(&SettingsStore::global(cx), |_, _, cx| cx.notify())
            .detach();
        let poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL).await;
            let status = mcp::status();
            let updated = this.update(cx, |this, cx| {
                if this.status != status {
                    this.status = status;
                    cx.notify();
                }
            });
            if updated.is_err() {
                break;
            }
        });
        Self {
            status: mcp::status(),
            _poll: poll,
        }
    }

    fn rows(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let p = palette(cx);
        let enabled = SettingsStore::get(cx).annotate_mcp_enabled;
        let description = match &self.status {
            mcp::Status::Failed(error) if enabled => {
                t_with("mcp.access.failed", &[("error", error)])
            }
            _ => t_with("mcp.access.description", &[("port", &mcp::PORT)]),
        };
        let access = ToggleSwitchRow::new(SETTING, t("mcp.access.title"), description, enabled)
            .updating(SettingsStore::global(cx).read(cx).is_updating(SETTING))
            .on_change(|value, _, cx| {
                rows::set(
                    cx,
                    SETTING,
                    move |s| s.annotate_mcp_enabled = value,
                    move |app| mcp::set_enabled(app, value),
                );
            })
            .into_any_element();
        if !enabled {
            return vec![access];
        }
        let claude_code = SettingContainer::new(t("mcp.claudeCode.title"))
            .description(t("mcp.claudeCode.description"))
            .child(div().text_color(p.text).child(
                icon_button("mcp-copy-command", "copy", t("mcp.claudeCode.copy")).on_click(
                    |_, _, cx| {
                        copy(
                            mcp::claude_command(AppHandle::global())
                                .map_err(|error| format!("{error:#}")),
                            cx,
                        );
                    },
                ),
            ))
            .into_any_element();
        let remote = SettingContainer::new(t("mcp.remote.title"))
            .description(t_with(
                "mcp.remote.description",
                &[("line", &mcp::ssh_forward())],
            ))
            .child(
                div().text_color(p.text).child(
                    icon_button("mcp-copy-forward", "copy", t("mcp.remote.copy"))
                        .on_click(|_, _, cx| copy(Ok(mcp::ssh_forward()), cx)),
                ),
            )
            .into_any_element();
        let token = SettingContainer::new(t("mcp.token.title"))
            .description(t("mcp.token.description"))
            .child(div().text_color(p.text).child(
                icon_button("mcp-renew-token", "refresh-cw", t("mcp.token.renew")).on_click(
                    |_, _, cx| match mcp::renew_token(AppHandle::global()) {
                        Ok(()) => toast::info(t("mcp.token.renewed"), cx),
                        Err(error) => {
                            log::error!("Renewing the Agent Access token failed: {error:#}");
                            toast::error(t("mcp.token.failed"), cx);
                        }
                    },
                ),
            ))
            .into_any_element();
        vec![access, claude_code, remote, token]
    }
}

impl Render for McpPage {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_5()
            .child(SettingsGroup::new().children(self.rows(cx)))
    }
}
