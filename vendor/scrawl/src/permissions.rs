//! What a capture needs from macOS. Screen Recording, Microphone and Accessibility are required;
//! until they're on, the shortcut starts nothing and Bespoke's Annotate page comes forward once
//! the keys are up (`Gate`). A Screen Recording grant may only reach a running app after it
//! relaunches, so it counts once a fresh helper says it's granted and this process can capture.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    ScreenRecording,
    Microphone,
    Accessibility,
}

impl Permission {
    /// In the order the Annotate page lists them. Accessibility drives the key event tap (⌘Z,
    /// Esc, the switcher's presses) and the paste shortcut's ⌘V.
    pub const ALL: [Permission; 3] = [
        Permission::ScreenRecording,
        Permission::Microphone,
        Permission::Accessibility,
    ];

    /// Privacy lists the app can be dragged into: granting docks the drag card beside System
    /// Settings (`scrawl-helper permission-panel`) instead of raising Apple's prompt.
    pub fn draggable(self) -> bool {
        matches!(
            self,
            Permission::ScreenRecording | Permission::Accessibility
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    #[default]
    NotDetermined,
    Denied,
    Granted,
}

/// What macOS has granted, from a fresh `scrawl-helper permissions`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grants {
    pub statuses: BTreeMap<Permission, Status>,
    /// Screen recording is granted, but this running process can't use it until it relaunches.
    pub screen_needs_relaunch: bool,
}

impl Grants {
    pub fn status(&self, permission: Permission) -> Status {
        self.statuses.get(&permission).copied().unwrap_or_default()
    }

    pub fn granted(&self, permission: Permission) -> bool {
        self.status(permission) == Status::Granted
    }
}

/// Reads what's granted now from a fresh helper, plus whether this process can use a Screen
/// Recording grant yet. Only once the helper says it's granted does this process try a real
/// ScreenCaptureKit capture (`capture::screen_capture_usable`), so no prompt can come of it.
pub fn read_grants() -> Option<Grants> {
    let statuses = crate::helper::permissions()
        .inspect_err(|error| crate::capture::scrawl_log(format!("permissions: {error:#}")))
        .ok()?;
    let screen = statuses.get(&Permission::ScreenRecording) == Some(&Status::Granted);
    Some(Grants {
        statuses,
        screen_needs_relaunch: screen && !crate::capture::screen_capture_usable(true),
    })
}

/// Required permissions that keep the shortcut from recording now. Screen Recording and
/// Accessibility are read in this process (a Screen Recording grant it can't use yet still
/// blocks; Accessibility is live); Microphone comes from the last fresh read.
pub fn missing_to_record(
    grants: Option<&Grants>,
    screen_usable: bool,
    accessibility: bool,
) -> Vec<Permission> {
    Permission::ALL
        .into_iter()
        .filter(|p| match p {
            Permission::ScreenRecording => !screen_usable,
            Permission::Accessibility => !accessibility,
            p => grants.is_some_and(|g| !g.granted(*p)),
        })
        .collect()
}

/// Holds a request to show the Annotate page until the shortcut's modifiers are up, so nothing
/// appears while ⌥⇧ is still down and every button is clickable when it does.
#[derive(Debug, Default)]
pub struct Gate {
    pending: Option<Vec<Permission>>,
}

impl Gate {
    /// The shortcut went down with nothing recording. Returns whether it may go on; with
    /// `missing` permissions it starts nothing and the page waits for the keys to come up.
    pub fn press(&mut self, missing: Vec<Permission>) -> bool {
        if self.pending.is_some() {
            return false;
        }
        if missing.is_empty() {
            return true;
        }
        self.pending = Some(missing);
        false
    }

    /// Show the page for `asks` once the modifiers are up.
    pub fn defer(&mut self, asks: Vec<Permission>) {
        self.pending = Some(asks);
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Polled while pending: the permissions to show, once no modifier is held.
    pub fn tick(&mut self, modifiers_down: bool) -> Option<Vec<Permission>> {
        if modifiers_down {
            None
        } else {
            self.pending.take()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use Permission::{Accessibility, Microphone, ScreenRecording};

    fn grants(granted: &[Permission]) -> Grants {
        Grants {
            statuses: granted.iter().map(|p| (*p, Status::Granted)).collect(),
            screen_needs_relaunch: false,
        }
    }

    #[test]
    fn the_shortcut_with_a_permission_off_starts_nothing_and_opens_the_page_on_release() {
        // Mic and Accessibility on, Screen Recording not usable in this process.
        let some = grants(&[Microphone, Accessibility]);
        let missing = missing_to_record(Some(&some), false, true);
        assert_eq!(missing, [ScreenRecording]);
        // Accessibility is read live in this process, not from the last helper read.
        assert_eq!(
            missing_to_record(Some(&grants(&Permission::ALL)), true, false),
            [Accessibility]
        );
        let mut gate = Gate::default();
        assert!(!gate.press(missing.clone()), "nothing starts");
        // Pressing again while it waits starts nothing either.
        assert!(!gate.press(Vec::new()));
        // Nothing shows while ⌥⇧ is held.
        assert_eq!(gate.tick(true), None);
        let asks = gate.tick(false).expect("the page, once the keys are up");
        assert_eq!(asks, missing);
        assert_eq!(gate.tick(false), None);
        assert!(!gate.is_pending());
        // With everything on, the shortcut goes through.
        assert!(missing_to_record(Some(&grants(&Permission::ALL)), true, true).is_empty());
        assert!(gate.press(Vec::new()));
        // Unknown grants (the launch read failed): only what's read here blocks.
        assert!(missing_to_record(None, true, true).is_empty());
    }

    #[test]
    fn helper_statuses_parse_into_grants() {
        let statuses: BTreeMap<Permission, Status> =
            serde_json::from_str(r#"{"screen-recording":"granted","microphone":"denied"}"#)
                .unwrap();
        let g = Grants {
            statuses,
            screen_needs_relaunch: false,
        };
        assert!(g.granted(ScreenRecording));
        assert_eq!(g.status(Microphone), Status::Denied);
        assert_eq!(g.status(Accessibility), Status::NotDetermined);
    }
}
