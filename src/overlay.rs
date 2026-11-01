use crate::app_handle::AppHandle;
use crate::settings::{self, OverlayStyle};
use crate::ui::overlay::OverlayState;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static LAST_MIC_LEVEL_EMIT: AtomicU64 = AtomicU64::new(0);
const EMIT_THROTTLE_MS: u64 = 33;

pub const MIC_LEVEL_EVENT: &str = "mic-level";

fn show_overlay_state(app_handle: &AppHandle, state: OverlayState) {
    let settings = settings::get_settings(app_handle);
    if settings.overlay_style == OverlayStyle::None {
        return;
    }
    let position = settings.overlay_position;
    app_handle.with_ui(move |cx| crate::ui::overlay::show(state, position, cx));
}

pub fn emit_recording_ready(app_handle: &AppHandle) {
    if !OVERLAY_ENABLED.load(Ordering::Relaxed) {
        return;
    }

    app_handle.with_ui(crate::ui::overlay::recording_ready);
}

pub fn show_recording_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, OverlayState::Recording);
}

pub fn show_streaming_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, OverlayState::Streaming);
}

pub fn show_transcribing_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, OverlayState::Transcribing);
}

pub fn show_processing_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, OverlayState::Processing);
}

pub fn update_overlay_position(app_handle: &AppHandle) {
    let position = settings::get_settings(app_handle).overlay_position;
    app_handle.with_ui(move |cx| crate::ui::overlay::reposition(position, cx));
}

pub fn hide_recording_overlay(app_handle: &AppHandle) {
    app_handle.with_ui(crate::ui::overlay::hide);
}

static OVERLAY_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn update_overlay_enabled_cache(enabled: bool) {
    OVERLAY_ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn emit_levels(app_handle: &AppHandle, levels: &[f32]) {
    if !OVERLAY_ENABLED.load(Ordering::Relaxed) {
        return;
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let last = LAST_MIC_LEVEL_EMIT.load(Ordering::Relaxed);
    if now.saturating_sub(last) < EMIT_THROTTLE_MS {
        return;
    }
    LAST_MIC_LEVEL_EMIT.store(now, Ordering::Relaxed);

    let _ = app_handle.emit(MIC_LEVEL_EVENT, levels);
}
