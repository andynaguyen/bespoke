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
    pub const ALL: [Permission; 3] = [
        Permission::ScreenRecording,
        Permission::Microphone,
        Permission::Accessibility,
    ];

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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grants {
    pub statuses: BTreeMap<Permission, Status>,
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

#[derive(Debug, Default)]
pub struct Gate {
    pending: Option<Vec<Permission>>,
}

impl Gate {
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

    pub fn defer(&mut self, asks: Vec<Permission>) {
        self.pending = Some(asks);
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

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
        let some = grants(&[Microphone, Accessibility]);
        let missing = missing_to_record(Some(&some), false, true);
        assert_eq!(missing, [ScreenRecording]);
        assert_eq!(
            missing_to_record(Some(&grants(&Permission::ALL)), true, false),
            [Accessibility]
        );
        let mut gate = Gate::default();
        assert!(!gate.press(missing.clone()), "nothing starts");
        assert!(!gate.press(Vec::new()));
        assert_eq!(gate.tick(true), None);
        let asks = gate.tick(false).expect("the page, once the keys are up");
        assert_eq!(asks, missing);
        assert_eq!(gate.tick(false), None);
        assert!(!gate.is_pending());
        assert!(missing_to_record(Some(&grants(&Permission::ALL)), true, true).is_empty());
        assert!(gate.press(Vec::new()));
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
