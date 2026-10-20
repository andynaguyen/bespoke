//! Which microphone Mac captures record from: `"microphone": {"id", "name"}` in
//! `~/.config/scrawl/config.json` (absent = the macOS default, followed as it changes). The id is
//! the CoreAudio device UID, which is also `AVCaptureDevice.uniqueID`; the name is kept only to
//! say which device is missing. A chosen device that isn't connected falls back to the default.

use serde::{Deserialize, Serialize};

/// The `microphone` key: a device the user picked in Settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    pub id: String,
    pub name: String,
}

/// The input devices connected now, in CoreAudio's order, and the default's id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    pub devices: Vec<Input>,
    pub default: Option<String>,
}

impl Inputs {
    pub fn find(&self, id: &str) -> Option<&Input> {
        self.devices.iter().find(|d| d.id == id)
    }

    pub fn default_input(&self) -> Option<&Input> {
        self.default.as_deref().and_then(|id| self.find(id))
    }
}

/// What a recording uses, given the choice and what's connected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick<'a> {
    /// No choice: the macOS default (none when there's no microphone at all).
    Default(Option<&'a Input>),
    Chosen(&'a Input),
    /// The chosen device isn't connected; the default stands in.
    Missing {
        wanted: &'a Choice,
        using: Option<&'a Input>,
    },
}

impl<'a> Pick<'a> {
    pub fn new(choice: Option<&'a Choice>, inputs: &'a Inputs) -> Self {
        match choice {
            None => Pick::Default(inputs.default_input()),
            Some(wanted) => match inputs.find(&wanted.id) {
                Some(input) => Pick::Chosen(input),
                None => Pick::Missing {
                    wanted,
                    using: inputs.default_input(),
                },
            },
        }
    }

    /// "AirPods Pro isn't connected. Using MacBook Pro Microphone." for a missing device.
    pub fn note(&self) -> Option<String> {
        let Pick::Missing { wanted, using } = self else {
            return None;
        };
        Some(match using {
            Some(using) => format!("{} isn't connected. Using {}.", wanted.name, using.name),
            None => format!(
                "{} isn't connected, and there's no other microphone.",
                wanted.name
            ),
        })
    }
}

/// The configured choice, if any.
pub fn choice() -> Option<Choice> {
    choice_in(&crate::handoff::default_config_path())
}

pub fn choice_in(path: &std::path::Path) -> Option<Choice> {
    crate::handoff::load_config_from(path)
        .microphone
        .filter(|c| !c.id.trim().is_empty())
}

/// Saves a pick from Settings: `"default"` clears the key; an id that isn't connected (so it
/// can't be named) changes nothing.
pub fn save_in(path: &std::path::Path, id: &str, inputs: &Inputs) -> anyhow::Result<()> {
    let value = if id == "default" {
        serde_json::Value::Null
    } else if let Some(input) = inputs.find(id) {
        serde_json::to_value(Choice {
            id: input.id.clone(),
            name: input.name.clone(),
        })?
    } else {
        return Ok(());
    };
    crate::handoff::set_config_value_in(path, "microphone", value)
}

/// Enumerates input devices (CoreAudio, a few ms; never on the UI thread).
pub fn inputs() -> Inputs {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let uids = coreaudio::uids_by_name();
    let devices = host
        .input_devices()
        .map(|devices| {
            devices
                .filter_map(|device| {
                    let name = device.name().ok()?;
                    let id = uids.get(&name)?.clone();
                    Some(Input { id, name })
                })
                .collect()
        })
        .unwrap_or_default();
    let default = host
        .default_input_device()
        .and_then(|device| device.name().ok())
        .and_then(|name| uids.get(&name).cloned());
    Inputs { devices, default }
}

/// The cpal device for `id`, if connected.
pub fn device(id: &str) -> Option<cpal::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let uids = coreaudio::uids_by_name();
    cpal::default_host().input_devices().ok()?.find(|device| {
        device
            .name()
            .is_ok_and(|name| uids.get(&name).is_some_and(|uid| uid == id))
    })
}

/// `device`'s CoreAudio UID, its id here.
pub fn device_uid(device: &cpal::Device) -> Option<String> {
    use cpal::traits::DeviceTrait;
    coreaudio::uids_by_name().get(&device.name().ok()?).cloned()
}

/// Bespoke shares cpal 0.16, which has no device ids, so the UIDs come from CoreAudio and are
/// matched to cpal's devices by name (cpal names a device by `kAudioObjectPropertyName`).
mod coreaudio {
    use std::collections::HashMap;
    use std::ffi::c_void;

    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};

    #[repr(C)]
    struct PropertyAddress {
        selector: u32,
        scope: u32,
        element: u32,
    }

    #[link(name = "CoreAudio", kind = "framework")]
    unsafe extern "C" {
        fn AudioObjectGetPropertyDataSize(
            object: u32,
            address: *const PropertyAddress,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: *mut u32,
        ) -> i32;
        fn AudioObjectGetPropertyData(
            object: u32,
            address: *const PropertyAddress,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: *mut u32,
            data: *mut c_void,
        ) -> i32;
    }

    const SYSTEM_OBJECT: u32 = 1;
    const fn code(tag: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*tag)
    }
    const fn global(selector: u32) -> PropertyAddress {
        PropertyAddress {
            selector,
            scope: code(b"glob"),
            element: 0,
        }
    }

    fn string(object: u32, selector: u32) -> Option<String> {
        let address = global(selector);
        let mut value: CFStringRef = std::ptr::null();
        let mut size = std::mem::size_of::<CFStringRef>() as u32;
        // SAFETY: the property is a CFString, written into `value`; the caller owns it.
        let status = unsafe {
            AudioObjectGetPropertyData(
                object,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                (&raw mut value).cast(),
            )
        };
        if status != 0 || value.is_null() {
            return None;
        }
        // SAFETY: CoreAudio returns the string retained (the Create rule).
        Some(unsafe { CFString::wrap_under_create_rule(value) }.to_string())
    }

    /// Every audio device's name and UID.
    pub fn uids_by_name() -> HashMap<String, String> {
        let address = global(code(b"dev#"));
        let mut size = 0_u32;
        // SAFETY: asks for the byte size of the device list.
        let status = unsafe {
            AudioObjectGetPropertyDataSize(SYSTEM_OBJECT, &address, 0, std::ptr::null(), &mut size)
        };
        if status != 0 || size == 0 {
            return HashMap::new();
        }
        let mut ids = vec![0_u32; size as usize / std::mem::size_of::<u32>()];
        // SAFETY: `ids` holds `size` bytes of `AudioObjectID`s.
        let status = unsafe {
            AudioObjectGetPropertyData(
                SYSTEM_OBJECT,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                ids.as_mut_ptr().cast(),
            )
        };
        if status != 0 {
            return HashMap::new();
        }
        ids.truncate(size as usize / std::mem::size_of::<u32>());
        ids.into_iter()
            .filter_map(|id| Some((string(id, code(b"lnam"))?, string(id, code(b"uid "))?)))
            .collect()
    }
}

/// Logs a missing chosen device once, until it's back or another goes missing.
pub fn note_pick(pick: &Pick) {
    static WARNED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let mut warned = WARNED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match (pick, pick.note()) {
        (Pick::Missing { wanted, .. }, Some(note)) => {
            if warned.as_deref() != Some(wanted.id.as_str()) {
                crate::capture::scrawl_log(format!("Microphone: {note}"));
                *warned = Some(wanted.id.clone());
            }
        }
        _ => *warned = None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(id: &str, name: &str) -> Input {
        Input {
            id: id.into(),
            name: name.into(),
        }
    }

    fn inputs() -> Inputs {
        Inputs {
            devices: vec![
                input("BuiltInMicrophoneDevice", "MacBook Pro Microphone"),
                input("usb-1", "Shure MV7"),
            ],
            default: Some("BuiltInMicrophoneDevice".into()),
        }
    }

    fn airpods() -> Choice {
        Choice {
            id: "airpods".into(),
            name: "AirPods Pro".into(),
        }
    }

    #[test]
    fn a_missing_device_falls_back_to_the_default_and_says_so() {
        let inputs = inputs();
        let built_in = &inputs.devices[0];
        assert_eq!(Pick::new(None, &inputs), Pick::Default(Some(built_in)));
        assert_eq!(Pick::new(None, &inputs).note(), None);

        let shure = Choice {
            id: "usb-1".into(),
            name: "Old name".into(),
        };
        let pick = Pick::new(Some(&shure), &inputs);
        assert_eq!(pick, Pick::Chosen(&inputs.devices[1]));

        let wanted = airpods();
        let pick = Pick::new(Some(&wanted), &inputs);
        assert_eq!(
            pick,
            Pick::Missing {
                wanted: &wanted,
                using: Some(built_in)
            }
        );
        assert_eq!(
            pick.note().unwrap(),
            "AirPods Pro isn't connected. Using MacBook Pro Microphone."
        );

        let none = Inputs::default();
        assert_eq!(Pick::new(None, &none), Pick::Default(None));
        assert_eq!(
            Pick::new(Some(&wanted), &none).note().unwrap(),
            "AirPods Pro isn't connected, and there's no other microphone."
        );
    }

    #[test]
    fn the_choice_round_trips_through_the_config_keeping_other_keys() {
        let dir = std::env::temp_dir().join(format!(
            "scrawl-mic-{}-{}",
            std::process::id(),
            crate::now_epoch_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"pillStyle":"glass"}"#).unwrap();
        let inputs = inputs();
        assert_eq!(choice_in(&path), None);

        save_in(&path, "usb-1", &inputs).unwrap();
        assert_eq!(
            choice_in(&path),
            Some(Choice {
                id: "usb-1".into(),
                name: "Shure MV7".into()
            })
        );
        save_in(&path, "airpods", &inputs).unwrap();
        assert_eq!(
            choice_in(&path).unwrap().id,
            "usb-1",
            "can't name it, so no change"
        );
        save_in(&path, "default", &inputs).unwrap();
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(config.get("microphone"), None);
        assert_eq!(config["pillStyle"], "glass");
        assert_eq!(choice_in(&path), None);

        std::fs::write(&path, r#"{"microphone":{"id":"","name":"x"}}"#).unwrap();
        assert_eq!(choice_in(&path), None, "an empty id is the default");
        std::fs::write(&path, r#"{"microphone":42,"pillStyle":"kept"}"#).unwrap();
        assert_eq!(choice_in(&path), None);
        assert_eq!(
            crate::handoff::load_config_from(&path)
                .pill_style
                .as_deref(),
            Some("kept"),
            "a bad microphone value never drops the rest"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
