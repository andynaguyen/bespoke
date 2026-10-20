//! Recording overlay control for the backend.
//!
//! Recording, transcription and cancellation code calls these functions from
//! any thread. They forward to `ui::overlay`, which owns the overlay window,
//! its placement and its animations on the GPUI main thread.

use crate::app_handle::AppHandle;
use crate::settings::{self, OverlayStyle};
use crate::ui::overlay::OverlayState;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static LAST_MIC_LEVEL_EMIT: AtomicU64 = AtomicU64::new(0);
const EMIT_THROTTLE_MS: u64 = 33; // ~30 FPS

/// Event carrying the 16 spectrum buckets from the recorder's visualizer.
pub const MIC_LEVEL_EVENT: &str = "mic-level";

fn show_overlay_state(app_handle: &AppHandle, state: OverlayState) {
    // Whether the overlay shows at all is governed by overlay_style; position
    // only chooses Top vs Bottom placement. Checked here (off the main thread)
    // so the common overlay-disabled case never pays for a main-thread hop.
    let settings = settings::get_settings(app_handle);
    if settings.overlay_style == OverlayStyle::None {
        return;
    }
    let position = settings.overlay_position;
    app_handle.with_ui(move |cx| crate::ui::overlay::show(state, position, cx));
}

/// Notify the visible recording overlay that the input stream has delivered its
/// first sample chunk. Audio feedback uses the same backend readiness signal,
/// but this is skipped when overlays are disabled.
pub fn emit_recording_ready(app_handle: &AppHandle) {
    if !OVERLAY_ENABLED.load(Ordering::Relaxed) {
        return;
    }

    // Queued on the main thread behind any pending show, so a very fast
    // always-on stream cannot overtake the show and then get reset back to
    // the arming state.
    app_handle.with_ui(crate::ui::overlay::recording_ready);
}

/// Shows the compact recording overlay
pub fn show_recording_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, OverlayState::Recording);
}

/// Shows the larger streaming overlay that displays live transcription text
pub fn show_streaming_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, OverlayState::Streaming);
}

/// Shows the transcribing overlay
pub fn show_transcribing_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, OverlayState::Transcribing);
}

/// Shows the processing overlay
pub fn show_processing_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, OverlayState::Processing);
}

/// Moves the overlay to match the current position setting
pub fn update_overlay_position(app_handle: &AppHandle) {
    let position = settings::get_settings(app_handle).overlay_position;
    app_handle.with_ui(move |cx| crate::ui::overlay::reposition(position, cx));
}

/// Hides the recording overlay. Always runs regardless of settings: if the
/// setting changed while recording, the overlay still has to go away.
pub fn hide_recording_overlay(app_handle: &AppHandle) {
    app_handle.with_ui(crate::ui::overlay::hide);
}

/// The overlay window is created on first show, so there is nothing to do at
/// startup. Kept so startup code reads the same as before.
pub fn create_recording_overlay(_app_handle: &AppHandle) {}

// Cached "overlay is enabled" flag, kept in sync with overlay_style. Avoids
// reading the settings store on every audio callback (~24 Hz during
// recording). Defaults to false so the audio path doesn't emit until startup
// populates the cache from initial settings.
static OVERLAY_ENABLED: AtomicBool = AtomicBool::new(false);

/// Update the cached overlay-enabled flag. Called at startup after settings
/// load, and from `change_overlay_style_setting` whenever the user changes
/// whether the overlay is shown.
pub fn update_overlay_enabled_cache(enabled: bool) {
    OVERLAY_ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn emit_levels(app_handle: &AppHandle, levels: &[f32]) {
    if !OVERLAY_ENABLED.load(Ordering::Relaxed) {
        return;
    }

    // Throttle to ~30 FPS. The raw audio callback fires far faster than the
    // overlay can show.
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
