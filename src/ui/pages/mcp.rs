//! The MCP page: where `annotate::mcp`, which serves Annotate's captures to coding agents,
//! listens (off, this Mac, or this Mac and the tailnet), then, while it's on, the setup to copy
//! into Claude Code and Codex.

use std::net::Ipv4Addr;
use std::time::Duration;

use gpui::{div, prelude::*, AnyElement, App, Context, SharedString, Task};

use crate::annotate::mcp;
use crate::app_handle::AppHandle;
use crate::i18n::{t, t_with};
use crate::settings::McpReach;
use crate::ui::components::{ButtonVariant, SegmentedControl, SettingContainer, SettingsGroup};
use crate::ui::stores::SettingsStore;
use crate::ui::theme::palette;
use crate::ui::toast;

use super::annotate::icon_button;
use super::rows;

/// The setting behind the Agent Access control.
const SETTING: &str = "annotate_mcp_reach";

/// How often the page re-reads the server's status and this Mac's Tailscale address.
/// `mcp::sync` starts the server in the background, so a port that's already taken shows up
/// after the choice has saved.
const POLL: Duration = Duration::from_millis(500);

const REACHES: [McpReach; 3] = [McpReach::Off, McpReach::Local, McpReach::Tailnet];

/// The id a reach's label is under (`mcp.access.options.<id>`).
fn reach_id(reach: McpReach) -> &'static str {
    match reach {
        McpReach::Off => "off",
        McpReach::Local => "local",
        McpReach::Tailnet => "tailnet",
    }
}

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
    tailscale: Option<Ipv4Addr>,
    _poll: Task<()>,
}

impl McpPage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.observe(&SettingsStore::global(cx), |_, _, cx| cx.notify())
            .detach();
        let poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL).await;
            let status = mcp::status();
            let tailscale = mcp::tailscale_address();
            let updated = this.update(cx, |this, cx| {
                if this.status != status || this.tailscale != tailscale {
                    this.status = status;
                    this.tailscale = tailscale;
                    cx.notify();
                }
            });
            if updated.is_err() {
                break;
            }
        });
        Self {
            status: mcp::status(),
            tailscale: mcp::tailscale_address(),
            _poll: poll,
        }
    }

    fn access_description(&self, reach: McpReach) -> SharedString {
        let port = &mcp::PORT;
        if let (mcp::Status::Failed(error), McpReach::Local | McpReach::Tailnet) =
            (&self.status, reach)
        {
            return t_with("mcp.access.failed", &[("error", error)]);
        }
        match (reach, self.tailscale) {
            (McpReach::Off, _) => t_with("mcp.access.off", &[("port", port)]),
            (McpReach::Local, _) => t_with("mcp.access.local", &[("port", port)]),
            (McpReach::Tailnet, Some(address)) => t_with(
                "mcp.access.tailnet",
                &[("address", &address), ("port", port)],
            ),
            (McpReach::Tailnet, None) => t_with("mcp.access.tailnetWaiting", &[("port", port)]),
        }
    }

    fn rows(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let p = palette(cx);
        let reach = SettingsStore::get(cx).annotate_mcp_reach;
        let access = SettingContainer::new(t("mcp.access.title"))
            .description(self.access_description(reach))
            .child(SegmentedControl::new(
                "mcp-reach",
                REACHES
                    .iter()
                    .map(|&reach| {
                        let id = reach_id(reach);
                        (
                            SharedString::from(id),
                            t(&format!("mcp.access.options.{id}")),
                        )
                    })
                    .collect(),
                reach_id(reach),
                |id, _, cx| {
                    let Some(&reach) = REACHES
                        .iter()
                        .find(|&&reach| reach_id(reach) == id.as_ref())
                    else {
                        return;
                    };
                    rows::set(
                        cx,
                        SETTING,
                        move |s| s.annotate_mcp_reach = reach,
                        move |app| mcp::set_reach(app, reach),
                    );
                },
            ))
            .into_any_element();
        if reach == McpReach::Off {
            return vec![access];
        }
        // Tailnet setups carry this Mac's Tailscale address, so there's nothing to copy until
        // Tailscale connects.
        let waiting = reach == McpReach::Tailnet && self.tailscale.is_none();
        let agents = SettingContainer::new(t("mcp.agents.title"))
            .description(match reach {
                McpReach::Tailnet => t("mcp.agents.tailnet"),
                _ => t("mcp.agents.local"),
            })
            .child(
                div()
                    .flex()
                    .gap_2()
                    .text_color(p.text)
                    .child(
                        icon_button("mcp-copy-claude", "copy", t("mcp.agents.claudeCode"))
                            .disabled(waiting)
                            .on_click(|_, _, cx| {
                                copy(
                                    mcp::claude_command(AppHandle::global())
                                        .map_err(|error| format!("{error:#}")),
                                    cx,
                                );
                            }),
                    )
                    .child(
                        icon_button("mcp-copy-codex", "copy", t("mcp.agents.codex"))
                            .disabled(waiting)
                            .on_click(|_, _, cx| {
                                copy(
                                    mcp::codex_config(AppHandle::global())
                                        .map_err(|error| format!("{error:#}")),
                                    cx,
                                );
                            }),
                    )
                    // The tunnel ends at 127.0.0.1, which only the local setup points at.
                    .when(reach == McpReach::Local, |row| {
                        row.child(
                            icon_button("mcp-copy-ssh", "copy", t("mcp.agents.ssh"))
                                .variant(ButtonVariant::Ghost)
                                .on_click(|_, _, cx| copy(Ok(mcp::ssh_forward()), cx)),
                        )
                    }),
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
        vec![access, agents, token]
    }
}

impl super::Page for McpPage {}

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
