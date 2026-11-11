use crate::app_handle::AppHandle;
use log::warn;
use std::sync::Arc;

use crate::TranscriptionCoordinator;
use crate::managers::audio::AudioRecordingManager;
use crate::settings::get_settings;
use crate::transcription_coordinator::is_transcribe_binding;

pub fn handle_shortcut_event(app: &AppHandle, binding_id: &str, is_pressed: bool) {
    if crate::annotate::is_annotate_binding(binding_id) {
        crate::annotate::handle_shortcut(binding_id, is_pressed);
        return;
    }

    let settings = get_settings(app);

    if is_transcribe_binding(binding_id) {
        if let Some(coordinator) = app.try_state::<TranscriptionCoordinator>() {
            coordinator.send_input(
                binding_id,
                is_pressed,
                settings.shortcut_activation,
                std::time::Duration::from_millis(settings.hold_threshold_ms),
            );
        } else {
            warn!("TranscriptionCoordinator is not initialized");
        }
        return;
    }

    if binding_id == "cancel" {
        if is_pressed && app.state::<Arc<AudioRecordingManager>>().is_recording() {
            crate::utils::cancel_current_operation(app);
        }
        return;
    }

    warn!("No action defined for shortcut ID '{binding_id}'. Pressed: {is_pressed}");
}
