use serde::{Deserialize, Serialize};

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick<'a> {
    Default(Option<&'a Input>),
    Chosen(&'a Input),
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

pub fn choice() -> Option<Choice> {
    choice_in(&crate::annotate::handoff::default_config_path())
}

pub fn choice_in(path: &std::path::Path) -> Option<Choice> {
    crate::annotate::handoff::load_config_from(path)
        .microphone
        .filter(|c| !c.id.trim().is_empty())
}

pub fn device(id: &str) -> Option<cpal::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let uids = coreaudio::uids_by_name();
    cpal::default_host().input_devices().ok()?.find(|device| {
        device
            .name()
            .is_ok_and(|name| uids.get(&name).is_some_and(|uid| uid == id))
    })
}

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

pub fn note_pick(pick: &Pick) {
    static WARNED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let mut warned = WARNED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match (pick, pick.note()) {
        (Pick::Missing { wanted, .. }, Some(note)) => {
            if warned.as_deref() != Some(wanted.id.as_str()) {
                crate::annotate::capture::annotate_log(format!("Microphone: {note}"));
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
}
