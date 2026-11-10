use crate::TranscriptionCoordinator;
use crate::app_handle::AppHandle;
use crate::managers::audio::AudioRecordingManager;
use crate::managers::transcription::TranscriptionManager;
use crate::shortcut;
use log::info;
use std::sync::Arc;

pub use crate::clipboard::*;
pub use crate::overlay::*;
pub use crate::tray::*;

pub fn redact_text(text: &str) -> &str {
    if cfg!(debug_assertions) {
        text
    } else {
        "[REDACTED]"
    }
}

#[cfg(test)]
const IMAGE_FILE_MACHINE_ARM64: u16 = 0xaa64;

#[cfg(test)]
fn native_machine_is_arm64(native_machine: Option<u16>) -> bool {
    native_machine == Some(IMAGE_FILE_MACHINE_ARM64)
}

pub fn cancel_current_operation(app: &AppHandle) {
    info!("Initiating operation cancellation...");

    shortcut::unregister_cancel_shortcut(app);

    let audio_manager = app.state::<Arc<AudioRecordingManager>>();
    let recording_was_active = audio_manager.is_recording();
    audio_manager.cancel_recording();

    app.state::<Arc<crate::voice_submit::VoiceSubmit>>()
        .disarm();

    let tm = app.state::<Arc<TranscriptionManager>>();
    tm.cancel_stream();

    set_tray_state(app, crate::tray::TrayIconState::Idle);
    hide_recording_overlay(app);

    if let Some(coordinator) = app.try_state::<TranscriptionCoordinator>() {
        coordinator.notify_cancel(recording_was_active);
    }

    info!("Operation cancellation completed - returned to idle state");
}

pub fn frontmost_app_name() -> Option<String> {
    let app = objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication()?;
    app.localizedName().map(|name| name.to_string())
}

pub fn frontmost_app_bundle_id() -> Option<String> {
    let app = objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication()?;
    app.bundleIdentifier().map(|id| id.to_string())
}

#[derive(serde::Serialize, Clone)]
pub struct RunningApp {
    pub bundle_id: String,
    pub name: String,
}

pub fn running_apps() -> Vec<RunningApp> {
    use objc2_app_kit::{NSApplicationActivationPolicy, NSWorkspace};

    let own_pid = std::process::id() as i32;
    let mut apps: Vec<RunningApp> = NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .filter(|app| {
            app.activationPolicy() == NSApplicationActivationPolicy::Regular
                && app.processIdentifier() != own_pid
        })
        .filter_map(|app| {
            let bundle_id = app.bundleIdentifier()?.to_string();
            let name = app
                .localizedName()
                .map_or_else(|| bundle_id.clone(), |name| name.to_string());
            Some(RunningApp { bundle_id, name })
        })
        .collect();
    apps.sort_by_key(|app| app.name.to_lowercase());
    apps.dedup_by(|a, b| a.bundle_id == b.bundle_id);
    apps
}

pub fn env_flag_enabled(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no" | "off"
        ),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm64_native_machine_is_the_only_match() {
        assert!(native_machine_is_arm64(Some(IMAGE_FILE_MACHINE_ARM64)));
        assert!(!native_machine_is_arm64(Some(0x8664)));
        assert!(!native_machine_is_arm64(Some(0x014c)));
        assert!(!native_machine_is_arm64(None));
    }

    #[test]
    fn env_flag_enabled_true_for_truthy_values() {
        for value in ["1", "true", "TRUE", "yes", "on", " 1 "] {
            // FIXME: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::set_var("BESPOKE_TEST_FLAG_TRUTHY", value) };
            assert!(env_flag_enabled("BESPOKE_TEST_FLAG_TRUTHY"), "{value:?}");
        }
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("BESPOKE_TEST_FLAG_TRUTHY") };
    }

    #[test]
    fn env_flag_enabled_false_for_falsy_or_unset() {
        assert!(!env_flag_enabled("BESPOKE_TEST_FLAG_UNSET"));

        for value in ["0", "false", "FALSE", "no", "off", ""] {
            // FIXME: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::set_var("BESPOKE_TEST_FLAG_FALSY", value) };
            assert!(!env_flag_enabled("BESPOKE_TEST_FLAG_FALSY"), "{value:?}");
        }
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("BESPOKE_TEST_FLAG_FALSY") };
    }
}
