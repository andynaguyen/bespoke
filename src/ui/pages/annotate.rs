//! The Annotate page. Its Captures tab lists the captures and opens one in place of Scrawl's
//! Inspect window (`captures`). Its Settings tab holds Scrawl's settings (its General,
//! Recording, Storage and Permissions panes). The shortcuts are ordinary Bespoke bindings.
//! Everything else reads and writes Scrawl's own `config.json` through `scrawl::host`. Agent
//! Access for `annotate::mcp` has its own page.

use std::collections::BTreeSet;
use std::time::Duration;

use gpui::{div, prelude::*, rems, AnyElement, Context, Entity, FontWeight, SharedString, Task};
use scrawl::host::{
    Grants, MicrophoneChoice, Microphones, Permission, PermissionStatus, Retention, SpeechState,
    SpeechStatus,
};

use crate::i18n::{t, t_count, t_with};
use crate::ui::components::{
    icon, Button, ButtonSize, ButtonVariant, Dropdown, DropdownOption, RowDirection,
    SegmentedControl, SettingContainer, SettingsGroup, Typography,
};
use crate::ui::shell::Toolbar;
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{palette, tw, Alpha};

use super::captures::Captures;
use super::shortcut_input::ShortcutInput;
use super::PageLayout;

/// How often the page re-reads the grants, the speech model and the microphones while it's open,
/// so a grant made in System Settings shows up by itself.
const POLL: Duration = Duration::from_millis(1500);

/// A small secondary button with an icon, for the Captures row and the MCP page.
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

/// The id a retention preset's strings are under (`annotate.retention.options.<id>`).
fn retention_id(preset: Retention) -> &'static str {
    match preset {
        Retention::Week => "week",
        Retention::Month => "month",
        Retention::Forever => "forever",
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Captures,
    Settings,
}

pub struct AnnotatePage {
    tab: Tab,
    captures: Entity<Captures>,
    record_shortcut: Entity<ShortcutInput>,
    paste_shortcut: Entity<ShortcutInput>,
    microphones: Option<Microphones>,
    grants: Option<Grants>,
    /// Permissions whose button already asked macOS; the next press opens System Settings.
    requested: BTreeSet<Permission>,
    /// `None` for a policy written into `config.json` by hand.
    retention: Option<Retention>,
    captures_size: Option<String>,
    speech: SpeechStatus,
    _poll: Task<()>,
}

impl AnnotatePage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.observe(&SettingsStore::global(cx), |_, _, cx| cx.notify())
            .detach();
        let poll = cx.spawn(async move |this, cx| loop {
            let (grants, microphones) = crate::runtime::spawn_blocking(|| {
                (scrawl::host::read_grants(), scrawl::host::microphones())
            })
            .await
            .unwrap_or((None, Microphones::default()));
            let updated = this.update(cx, |this, cx| {
                if grants.is_some() {
                    this.grants = grants;
                }
                this.microphones = Some(microphones);
                this.speech = scrawl::host::speech_status();
                cx.notify();
            });
            if updated.is_err() {
                break;
            }
            cx.background_executor().timer(POLL).await;
        });
        let captures = cx.new(Captures::new);
        cx.observe(&captures, |_, _, cx| cx.notify()).detach();
        let mut page = Self {
            tab: Tab::Captures,
            captures,
            record_shortcut: cx.new(|cx| ShortcutInput::new(crate::annotate::RECORD_BINDING, cx)),
            paste_shortcut: cx.new(|cx| ShortcutInput::new(crate::annotate::PASTE_BINDING, cx)),
            microphones: None,
            grants: None,
            requested: BTreeSet::new(),
            retention: scrawl::host::retention(),
            captures_size: None,
            speech: scrawl::host::speech_status(),
            _poll: poll,
        };
        page.measure(cx);
        page
    }

    /// Re-reads how much space the captures take.
    fn measure(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let size = crate::runtime::spawn_blocking(scrawl::host::captures_size)
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

    /// Like Scrawl's checklist: Screen Recording and Accessibility open System Settings with
    /// the drag card; Microphone asks macOS first, then opens System Settings.
    fn grant(&mut self, permission: Permission) {
        let status = self.status(permission);
        if status == PermissionStatus::Granted {
            return;
        }
        let first = self.requested.insert(permission) && status != PermissionStatus::Denied;
        if permission == Permission::Microphone && first {
            crate::ui::platform::request_microphone();
        } else {
            scrawl::host::open_permission_settings(permission);
        }
    }

    fn how_it_works(&self, cx: &Context<Self>) -> AnyElement {
        let p = palette(cx);
        let settings = crate::ui::stores::SettingsStore::get(cx);
        let shortcut = |id: &str| {
            settings
                .bindings
                .get(id)
                .and_then(|b| scrawl::host::shortcut_from_binding(&b.current_binding))
        };
        let record_shortcut = shortcut(crate::annotate::RECORD_BINDING);
        let record = record_shortcut
            .as_ref()
            .map_or_else(|| "⌥⇧S".to_owned(), |c| c.label.clone());
        // Pressing the record key again during a hold switches modes, as in the capsule's badge.
        let record_key = record_shortcut
            .as_ref()
            .map_or_else(|| "S".to_owned(), |c| c.key_label().to_owned());
        let paste =
            shortcut(crate::annotate::PASTE_BINDING).map_or_else(|| "⌥⇧V".to_owned(), |c| c.label);
        let line = |key: &str| {
            div().sm_text().text_color(p.text.a(0.75)).child(t_with(
                key,
                &[
                    ("record", &record),
                    ("recordKey", &record_key),
                    ("paste", &paste),
                ],
            ))
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .px_5()
            .py_4()
            .child(line("annotate.howItWorks.record"))
            .child(line("annotate.howItWorks.choose"))
            .child(line("annotate.howItWorks.draw"))
            .child(line("annotate.howItWorks.finish"))
            .into_any_element()
    }

    fn microphone_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let default_name = self
            .microphones
            .as_ref()
            .and_then(|m| m.default_input())
            .map(|input| input.name.clone());
        let mut options = vec![DropdownOption::new(
            "default",
            match &default_name {
                Some(name) => t_with("annotate.microphone.defaultNamed", &[("name", name)]),
                None => t("annotate.microphone.default"),
            },
        )];
        if let Some(inputs) = &self.microphones {
            options.extend(
                inputs
                    .devices
                    .iter()
                    .map(|input| DropdownOption::new(input.id.clone(), input.name.clone())),
            );
        }
        let (selected, note) = match scrawl::host::microphone_choice() {
            MicrophoneChoice::Default => (SharedString::from("default"), None),
            MicrophoneChoice::Device { id, name } => {
                // Until the first list arrives, a choice is shown as saved, not as missing.
                let missing = self
                    .microphones
                    .as_ref()
                    .is_some_and(|inputs| inputs.find(&id).is_none());
                if missing {
                    // A chosen device that isn't connected stays listed by its saved name.
                    options.push(
                        DropdownOption::new(
                            id.clone(),
                            t_with("annotate.microphone.notConnected", &[("name", &name)]),
                        )
                        .disabled(true),
                    );
                }
                let note =
                    missing.then(|| t_with("annotate.microphone.fallback", &[("name", &name)]));
                (SharedString::from(id), note)
            }
        };
        let entity = cx.entity().downgrade();
        SettingContainer::new(t("annotate.microphone.title"))
            .description(note.unwrap_or_else(|| t("annotate.microphone.description")))
            .child(
                Dropdown::new("annotate-microphone", options)
                    .selected(Some(selected))
                    .disabled(self.microphones.is_none())
                    .on_select(move |id, _, cx| {
                        let _ = entity.update(cx, |this, cx| {
                            if let Some(inputs) = &this.microphones {
                                scrawl::host::set_microphone(id, inputs);
                            }
                            cx.notify();
                        });
                    }),
            )
            .into_any_element()
    }

    fn transcription_row(&self, cx: &Context<Self>) -> AnyElement {
        let p = palette(cx);
        let state = match self.speech.state {
            SpeechState::Unknown | SpeechState::Checking => t("annotate.transcription.checking"),
            SpeechState::Downloading => t_with(
                "annotate.transcription.downloading",
                &[(
                    "percent",
                    &format!("{}", (self.speech.progress * 100.0).round() as u32),
                )],
            ),
            SpeechState::Ready => t("annotate.transcription.ready"),
            SpeechState::Unavailable => t("annotate.transcription.unavailable"),
            SpeechState::Failed => t("annotate.transcription.failed"),
        };
        SettingContainer::new(t("annotate.transcription.title"))
            .description(t("annotate.transcription.description"))
            .child(div().sm_text().text_color(p.text.a(0.7)).child(state))
            .into_any_element()
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
                            scrawl::host::set_retention(preset);
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
                            scrawl::host::menu(scrawl::MenuAction::OpenLatest);
                        }),
                    )
                    .child(
                        icon_button(
                            "annotate-show-captures",
                            "folder-open",
                            t("annotate.captures.showInFinder"),
                        )
                        .on_click(|_, _, _| scrawl::host::show_captures()),
                    ),
            )
            .into_any_element()
    }

    fn permission_row(&self, permission: Permission, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let key = match permission {
            Permission::ScreenRecording => "screenRecording",
            Permission::Microphone => "microphone",
            Permission::Accessibility => "accessibility",
        };
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
        if self.tab == Tab::Captures {
            if let Some(toolbar) = self
                .captures
                .update(cx, |captures, cx| captures.toolbar(cx))
            {
                return toolbar;
            }
        }
        let entity = cx.entity().downgrade();
        let mut toolbar = Toolbar::new().tabs(SegmentedControl::new(
            "annotate-tabs",
            vec![
                ("captures".into(), t("annotate.library.captures")),
                ("settings".into(), t("annotate.library.settings")),
            ],
            match self.tab {
                Tab::Captures => "captures",
                Tab::Settings => "settings",
            },
            move |id, _, cx| {
                let tab = if id.as_ref() == "settings" {
                    Tab::Settings
                } else {
                    Tab::Captures
                };
                let _ = entity.update(cx, |this, cx| {
                    this.tab = tab;
                    if tab == Tab::Captures {
                        this.captures.update(cx, |captures, cx| captures.reload(cx));
                    }
                    cx.notify();
                });
            },
        ));
        if self.tab == Tab::Captures {
            if let Some(count) = self.captures.read(cx).count() {
                toolbar = toolbar.meta(t_count("annotate.library.count", count as i64, &[]));
            }
            toolbar = toolbar
                .action(
                    icon_button(
                        "captures-folder",
                        "folder-open",
                        t("annotate.library.folder"),
                    )
                    .on_click(|_, _, _| scrawl::host::show_captures()),
                )
                .action(
                    Button::new("captures-record")
                        .variant(ButtonVariant::Live)
                        .gap(rems(0.375))
                        .on_click(|_, _, _| scrawl::host::menu(scrawl::MenuAction::ChooseTarget))
                        .child(div().size(rems(0.5)).rounded_full().bg(gpui::white()))
                        .child(t("annotate.library.record")),
                );
        }
        toolbar
    }

    fn layout(&self, cx: &gpui::App) -> PageLayout {
        if self.tab == Tab::Captures && self.captures.read(cx).is_open() {
            PageLayout::Fill
        } else {
            PageLayout::Padded
        }
    }
}

impl Render for AnnotatePage {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        if cx.default_global::<super::captures::PendingInspectFlag>().0 {
            cx.global_mut::<super::captures::PendingInspectFlag>().0 = false;
            self.tab = Tab::Captures;
        }
        if self.tab == Tab::Captures {
            return div()
                .w_full()
                .h_full()
                .flex()
                .flex_col()
                .child(self.captures.clone())
                .into_any_element();
        }
        let shortcuts = SettingsGroup::new()
            .title(t("annotate.groups.shortcuts"))
            .child(self.record_shortcut.clone())
            .child(self.paste_shortcut.clone())
            .child(self.how_it_works(cx));
        let recording = SettingsGroup::new()
            .title(t("annotate.groups.recording"))
            .child(self.microphone_row(cx))
            .child(self.transcription_row(cx));
        let storage = SettingsGroup::new()
            .title(t("annotate.groups.storage"))
            .child(self.retention_row(cx))
            .child(self.captures_row(cx));
        let permissions = SettingsGroup::new()
            .title(t("annotate.groups.permissions"))
            .children(Permission::ALL.map(|permission| self.permission_row(permission, cx)));
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_5()
            .child(shortcuts)
            .child(recording)
            .child(storage)
            .child(permissions)
            .into_any_element()
    }
}
