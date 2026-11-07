use std::collections::BTreeSet;
use std::time::Duration;

use crate::annotate::host::{Grants, Permission, PermissionStatus, Retention};
use gpui::{
    AnyElement, App, Context, Entity, FontWeight, SharedString, Task, div, prelude::*, rems,
};

use crate::i18n::{t, t_with};
use crate::settings::AppSettings;
use crate::ui::components::{
    Button, ButtonSize, ButtonVariant, Dropdown, DropdownOption, RowDirection, SettingContainer,
    SettingsGroup, Typography, icon,
};
use crate::ui::keyboard::format_key_combination;
use crate::ui::shell::Toolbar;
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{Alpha, CONTROL_HEIGHT, palette, tw};

use super::PageLayout;
use super::captures::Captures;
use super::settings::SettingsSection;
use super::shortcut_input::ShortcutInput;

const POLL: Duration = Duration::from_millis(1500);

pub(super) fn icon_button(
    id: &'static str,
    icon_name: &'static str,
    label: SharedString,
) -> Button {
    Button::new(id)
        .variant(ButtonVariant::Secondary)
        .size(ButtonSize::Sm)
        .gap(rems(0.375))
        .child(icon(icon_name).size(rems(0.875)))
        .child(label)
}

fn permission_key(permission: Permission) -> &'static str {
    match permission {
        Permission::ScreenRecording => "screenRecording",
        Permission::Microphone => "microphone",
        Permission::Accessibility => "accessibility",
    }
}

/// Opens Settings at Annotate, where the permission rows are, and names the permissions that
/// held back a recording. Record and the shortcut land here, and the captures alone don't say
/// why nothing started.
pub fn show_missing_permissions(missing: &[Permission], cx: &mut App) {
    crate::ui::show_settings(SettingsSection::System, cx);
    let names = missing
        .iter()
        .map(|p| {
            t(&format!(
                "annotate.permissions.{}.title",
                permission_key(*p)
            ))
            .to_string()
        })
        .collect::<Vec<_>>()
        .join(", ");
    crate::ui::toast::warning(
        t_with("annotate.permissions.blocked", &[("permissions", &names)]),
        cx,
    );
}

fn retention_id(preset: Retention) -> &'static str {
    match preset {
        Retention::Week => "week",
        Retention::Month => "month",
        Retention::Forever => "forever",
    }
}

/// The Captures library, Annotate's sidebar page.
pub struct AnnotatePage {
    captures: Entity<Captures>,
}

impl AnnotatePage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let captures = cx.new(Captures::new);
        cx.observe(&captures, |_, _, cx| cx.notify()).detach();
        Self { captures }
    }
}

/// How the empty captures page names one of Annotate's shortcuts: Scrawl's short label (⌥⇧S)
/// for a key combination, or Bespoke's label (Left Option + Mouse 8) for anything else, so it
/// matches the binding the shortcut row shows.
pub(super) fn shortcut_label(
    settings: &AppSettings,
    id: &str,
    kind: crate::annotate::capture::ShortcutKind,
) -> String {
    let Some(binding) = settings.bindings.get(id) else {
        return kind.default_config().label;
    };
    crate::annotate::host::shortcut_from_binding(&binding.current_binding).map_or_else(
        || format_key_combination(&binding.current_binding),
        |config| config.label,
    )
}

/// Annotate's pages of the Settings dialog: its shortcuts, its capture history, and the
/// permission rows the System page shows.
pub struct AnnotateSettings {
    record_shortcut: Entity<ShortcutInput>,
    paste_shortcut: Entity<ShortcutInput>,
    grants: Option<Grants>,
    requested: BTreeSet<Permission>,
    retention: Option<Retention>,
    captures_size: Option<String>,
    _poll: Task<()>,
}

impl AnnotateSettings {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.observe(&SettingsStore::global(cx), |_, _, cx| cx.notify())
            .detach();
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let grants = crate::runtime::spawn_blocking(crate::annotate::host::read_grants)
                    .await
                    .unwrap_or(None);
                let updated = this.update(cx, |this, cx| {
                    if grants.is_some() {
                        this.grants = grants;
                    }
                    cx.notify();
                });
                if updated.is_err() {
                    break;
                }
                cx.background_executor().timer(POLL).await;
            }
        });
        let mut page = Self {
            record_shortcut: cx.new(|cx| ShortcutInput::new(crate::annotate::RECORD_BINDING, cx)),
            paste_shortcut: cx.new(|cx| ShortcutInput::new(crate::annotate::PASTE_BINDING, cx)),
            grants: None,
            requested: BTreeSet::new(),
            retention: crate::annotate::host::retention(),
            captures_size: None,
            _poll: poll,
        };
        page.measure(cx);
        page
    }

    fn measure(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let size = crate::runtime::spawn_blocking(crate::annotate::host::captures_size)
                .await
                .ok();
            let _ = this.update(cx, |this, cx| {
                this.captures_size = size;
                cx.notify();
            });
        })
        .detach();
    }

    fn status(&self, permission: Permission) -> PermissionStatus {
        self.grants
            .as_ref()
            .map_or(PermissionStatus::NotDetermined, |g| g.status(permission))
    }

    fn grant(&mut self, permission: Permission) {
        let status = self.status(permission);
        if status == PermissionStatus::Granted {
            return;
        }
        let first = self.requested.insert(permission) && status != PermissionStatus::Denied;
        match permission {
            Permission::Microphone if first => crate::ui::platform::request_microphone(),
            // Bespoke isn't in the Screen Recording list until it asks, so the user would have to
            // drag it in. Asking adds it, and the user only switches it on. The panel still
            // opens, because macOS prompts only once per app and a later launch gets no prompt.
            Permission::ScreenRecording if first => {
                std::thread::spawn(|| {
                    crate::annotate::host::request_screen_capture();
                    crate::annotate::host::open_permission_settings(Permission::ScreenRecording);
                });
            }
            _ => crate::annotate::host::open_permission_settings(permission),
        }
    }

    /// Whether a permission row shows something other than Allowed, once the first read is in.
    pub fn permission_missing(&self) -> bool {
        self.grants.as_ref().is_some_and(|g| {
            g.screen_needs_relaunch || Permission::ALL.iter().any(|p| !g.granted(*p))
        })
    }

    fn retention_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let options = Retention::ALL
            .into_iter()
            .map(|preset| {
                let id = retention_id(preset);
                DropdownOption::new(id, t(&format!("annotate.retention.options.{id}")))
            })
            .collect();
        let description = match self.retention {
            Some(preset) => t(&format!(
                "annotate.retention.notes.{}",
                retention_id(preset)
            )),
            None => t("annotate.retention.notes.custom"),
        };
        let entity = cx.entity().downgrade();
        SettingContainer::new(t("annotate.retention.title"))
            .description(description)
            .child(
                Dropdown::new("annotate-retention", options)
                    .selected(
                        self.retention
                            .map(|preset| SharedString::new_static(retention_id(preset))),
                    )
                    .placeholder(t("annotate.retention.custom"))
                    .on_select(move |id, _, cx| {
                        let Some(preset) = Retention::ALL
                            .into_iter()
                            .find(|preset| retention_id(*preset) == id.as_ref())
                        else {
                            return;
                        };
                        let _ = entity.update(cx, |this, cx| {
                            crate::annotate::host::set_retention(preset);
                            this.retention = Some(preset);
                            this.measure(cx);
                            cx.notify();
                        });
                    }),
            )
            .into_any_element()
    }

    fn captures_row(&self, cx: &Context<Self>) -> AnyElement {
        let p = palette(cx);
        let description = match &self.captures_size {
            Some(size) => t_with("annotate.captures.size", &[("size", size)]),
            None => t("annotate.captures.measuring"),
        };
        SettingContainer::new(t("annotate.captures.title"))
            .description(description)
            .child(
                div()
                    .flex()
                    .row_dir()
                    .gap_2()
                    .text_color(p.text)
                    .child(
                        icon_button(
                            "annotate-open-latest",
                            "external-link",
                            t("annotate.captures.openLatest"),
                        )
                        .on_click(|_, _, _| {
                            crate::annotate::host::menu(
                                crate::annotate::coordinator::MenuAction::OpenLatest,
                            );
                        }),
                    )
                    .child(
                        icon_button(
                            "annotate-show-captures",
                            "folder-open",
                            t("annotate.captures.showInFinder"),
                        )
                        .on_click(|_, _, _| crate::annotate::host::show_captures()),
                    ),
            )
            .into_any_element()
    }

    fn permission_row(&self, permission: Permission, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let key = permission_key(permission);
        let status = self.status(permission);
        let relaunch = permission == Permission::ScreenRecording
            && self
                .grants
                .as_ref()
                .is_some_and(|g| g.granted(Permission::ScreenRecording) && g.screen_needs_relaunch);
        let control = if status == PermissionStatus::Granted && !relaunch {
            div()
                .flex()
                .row_dir()
                .items_center()
                .gap_1()
                .sm_text()
                .font_weight(FontWeight::MEDIUM)
                .text_color(tw::emerald_500())
                .child(icon("circle-check").size(rems(1.)))
                .child(t("annotate.permissions.granted"))
                .into_any_element()
        } else if relaunch {
            div()
                .max_w(rems(14.))
                .xs_text()
                .text_color(p.text.a(0.6))
                .child(t("annotate.permissions.relaunch"))
                .into_any_element()
        } else {
            let asks_first = !permission.draggable()
                && status == PermissionStatus::NotDetermined
                && !self.requested.contains(&permission);
            let label = if asks_first {
                t("annotate.permissions.allow")
            } else {
                t("annotate.permissions.openSettings")
            };
            Button::new(SharedString::from(format!("annotate-grant-{key}")))
                .size(ButtonSize::Sm)
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.grant(permission);
                    cx.notify();
                }))
                .into_any_element()
        };
        SettingContainer::new(t(&format!("annotate.permissions.{key}.title")))
            .description(t(&format!("annotate.permissions.{key}.description")))
            .child(control)
            .into_any_element()
    }
}

impl super::Page for AnnotatePage {
    fn toolbar(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> Toolbar {
        if let Some(toolbar) = self
            .captures
            .update(cx, |captures, cx| captures.toolbar(cx))
        {
            return toolbar;
        }
        Toolbar::new()
            // Round and icon-only, like the folder button in Dictate's History inspector.
            .action(
                Button::new("captures-folder")
                    .variant(ButtonVariant::Secondary)
                    .title(t("annotate.library.folder"))
                    .w(CONTROL_HEIGHT)
                    .px(gpui::px(0.))
                    .child(icon("folder-open").size(gpui::px(13.)))
                    .on_click(|_, _, _| crate::annotate::host::show_captures()),
            )
            .action(
                Button::new("captures-record")
                    .variant(ButtonVariant::Live)
                    .gap(rems(0.375))
                    .pe(gpui::px(13.5))
                    .on_click(|_, _, _| {
                        crate::annotate::host::menu(
                            crate::annotate::coordinator::MenuAction::ChooseTarget,
                        )
                    })
                    .child(div().size(rems(0.5)).rounded_full().bg(gpui::white()))
                    .child(t("annotate.library.record")),
            )
    }

    fn inspector(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        self.captures
            .update(cx, |captures, cx| captures.inspector(cx))
    }

    fn layout(&self, cx: &gpui::App) -> PageLayout {
        if self.captures.read(cx).is_open() {
            PageLayout::Fill
        } else {
            PageLayout::Padded
        }
    }
}

impl Render for AnnotatePage {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .child(self.captures.clone())
    }
}

/// A page of Annotate's settings in the Settings dialog.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AnnotateSettingsPage {
    Shortcut,
    History,
    Permissions,
}

impl AnnotateSettings {
    /// The settings groups on `page`, drawn by the Settings dialog.
    pub fn groups(
        &mut self,
        page: AnnotateSettingsPage,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        match page {
            // One group each, so the page title in the header names them.
            AnnotateSettingsPage::Shortcut => vec![
                SettingsGroup::new()
                    .child(self.record_shortcut.clone())
                    .child(self.paste_shortcut.clone())
                    .into_any_element(),
            ],
            AnnotateSettingsPage::History => vec![
                SettingsGroup::new()
                    .child(self.retention_row(cx))
                    .child(self.captures_row(cx))
                    .into_any_element(),
            ],
            // A group on the System page, among the app's other settings.
            AnnotateSettingsPage::Permissions => vec![
                SettingsGroup::new()
                    .title(t("annotate.groups.permissions"))
                    .children(Permission::ALL.map(|permission| self.permission_row(permission, cx)))
                    .into_any_element(),
            ],
        }
    }
}
