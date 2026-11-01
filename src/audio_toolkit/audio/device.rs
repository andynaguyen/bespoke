use cpal::traits::{DeviceTrait, HostTrait};

pub struct CpalDeviceInfo {
    pub index: String,
    pub name: String,
    pub device: cpal::Device,
}

pub fn list_input_devices() -> Result<Vec<CpalDeviceInfo>, Box<dyn std::error::Error>> {
    Ok(describe(
        crate::audio_toolkit::get_cpal_host().input_devices()?,
    ))
}

pub fn list_output_devices() -> Result<Vec<CpalDeviceInfo>, Box<dyn std::error::Error>> {
    Ok(describe(
        crate::audio_toolkit::get_cpal_host().output_devices()?,
    ))
}

fn describe(devices: impl Iterator<Item = cpal::Device>) -> Vec<CpalDeviceInfo> {
    devices
        .enumerate()
        .map(|(index, device)| CpalDeviceInfo {
            index: index.to_string(),
            name: device.name().unwrap_or_else(|_| "Unknown".into()),
            device,
        })
        .collect()
}
