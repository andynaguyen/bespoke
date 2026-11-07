mod audio;
mod autocrop;
mod bundle;
pub mod capsule;
mod capsule_link;
pub mod capture;
pub mod coordinator;
pub mod handoff;
pub mod helper;
mod hold;
mod keyframes;
mod laser_overlay;
pub mod mcp;
pub mod model;
pub mod permissions;
mod render;
pub mod retention;
mod screens;
mod sketch;
pub mod speech_model;
mod transcribe;
mod windows;

use std::sync::{Arc, Mutex};

use gpui::App;

use crate::annotate::capture::{ShortcutConfig, ShortcutKind};
use crate::annotate::handoff::Finished;
use crate::annotate::permissions::Permission;
use crate::app_handle::AppHandle;
use crate::managers::history::{CaptureUsage, HistoryManager};
use crate::settings::{AppSettings, get_settings};
use crate::ui::pages::settings::SettingsSection;

pub const RECORD_BINDING: &str = "annotate";
pub const PASTE_BINDING: &str = "annotate_paste";

/// Emitted when a capture's bundle is written, so open views can pick it up.
pub const CAPTURE_FINISHED: &str = "annotate-capture-finished";

/// Emitted when a recording or the picker ends without a capture.
pub const CAPTURE_CANCELLED: &str = "annotate-capture-cancelled";

static NEWEST_CAPTURE: Mutex<Option<Finished>> = Mutex::new(None);

/// The newest finished capture: the newest on disk at launch, then each one recorded since.
/// `CAPTURE_FINISHED` fires after this updates.
pub fn newest_capture() -> Option<Finished> {
    NEWEST_CAPTURE.lock().ok()?.clone()
}

fn note_capture(capture: &Finished) {
    let Ok(mut newest) = NEWEST_CAPTURE.lock() else {
        return;
    };
    if newest
        .as_ref()
        .is_none_or(|newest| capture.created_at_ms >= newest.created_at_ms)
    {
        *newest = Some(capture.clone());
    }
}

pub fn is_annotate_binding(id: &str) -> bool {
    id == RECORD_BINDING || id == PASTE_BINDING
}

pub fn data_dir(app: &AppHandle) -> Option<std::path::PathBuf> {
    crate::portable::app_data_dir(app)
        .ok()
        .map(|dir| dir.join("annotate"))
}

pub fn start(app: &AppHandle, cx: &mut App) {
    if data_dir(app).is_none() || crate::portable::app_log_dir(app).is_err() {
        log::error!("Annotate is off: no data or log directory");
        return;
    }
    follow_captures(app);
    crate::annotate::coordinator::start(shortcuts(&get_settings(app)), cx);
    mcp::sync(app);
}

/// Opens Bespoke's settings from one of Annotate's "Settings" buttons. With `missing`, the
/// permissions that held back a recording, it opens Settings at System with a toast naming them.
pub fn open_settings(missing: &[Permission]) {
    let Some(app) = AppHandle::try_global() else {
        return;
    };
    let missing = missing.to_vec();
    app.with_ui(move |cx| {
        if missing.is_empty() {
            crate::ui::show_settings(SettingsSection::AnnotateShortcut, cx);
        } else {
            crate::ui::pages::annotate::show_missing_permissions(&missing, cx);
        }
    });
}

/// Records a finished capture in history and stats, notes it as the newest, then emits
/// `CAPTURE_FINISHED`.
pub fn capture_finished(capture: Finished) {
    let Some(app) = AppHandle::try_global() else {
        return;
    };
    note_capture(&capture);
    if let Err(e) = app
        .state::<Arc<HistoryManager>>()
        .record_capture(&capture_usage(capture))
    {
        log::error!("Failed to record Annotate usage: {e}");
    }
    if let Err(e) = app.emit(CAPTURE_FINISHED, ()) {
        log::warn!("Failed to announce a finished capture: {e}");
    }
}

/// Emits `CAPTURE_CANCELLED` when a recording or the picker ends without a capture: cancelled,
/// held back by a missing permission, or failed while writing its bundle.
pub fn capture_cancelled() {
    let Some(app) = AppHandle::try_global() else {
        return;
    };
    if let Err(e) = app.emit(CAPTURE_CANCELLED, ()) {
        log::warn!("Failed to announce a cancelled capture: {e}");
    }
}

fn follow_captures(app: &AppHandle) {
    let seed = Arc::clone(&app.state::<Arc<HistoryManager>>());
    crate::runtime::spawn_blocking(move || {
        if let Some(capture) = crate::annotate::handoff::newest_capture() {
            note_capture(&capture);
        }
        let load = || {
            crate::annotate::handoff::finished_captures()
                .into_iter()
                .map(capture_usage)
                .collect()
        };
        match seed.seed_captures(load) {
            Ok(0) => {}
            Ok(count) => log::info!("Counted {count} Annotate captures already on disk"),
            Err(e) => log::error!("Failed to count Annotate captures on disk: {e}"),
        }
    });
}

fn capture_usage(capture: Finished) -> CaptureUsage {
    CaptureUsage {
        timestamp: (capture.created_at_ms / 1000) as i64,
        duration_ms: capture.duration_ms as i64,
        marks: capture.marks as i64,
        app_name: capture.app,
    }
}

/// A press of one of the Annotate bindings. Releases are ignored: Scrawl polls the modifiers to
/// tell a tap (record the screen) from a hold (the target switcher).
pub fn handle_shortcut(binding_id: &str, is_pressed: bool) {
    if !is_pressed {
        return;
    }
    let kind = if binding_id == PASTE_BINDING {
        ShortcutKind::Paste
    } else {
        ShortcutKind::Record
    };
    crate::annotate::capture::press(kind);
}

pub fn sync_shortcuts(settings: &AppSettings) {
    let (record, paste) = shortcuts(settings);
    crate::annotate::capture::set_shortcuts(record, paste);
}

/// The record and paste combos for the Annotate bindings in `settings`.
fn shortcuts(settings: &AppSettings) -> (ShortcutConfig, ShortcutConfig) {
    let config = |id: &str, kind: ShortcutKind| {
        let Some(binding) = settings.bindings.get(id) else {
            return kind.default_config();
        };
        crate::annotate::capture::shortcut_from_binding(&binding.current_binding).unwrap_or_else(
            || ShortcutConfig {
                key_code: u32::MAX,
                carbon_modifiers: 0,
                label: binding.current_binding.clone(),
            },
        )
    };
    (
        config(RECORD_BINDING, ShortcutKind::Record),
        config(PASTE_BINDING, ShortcutKind::Paste),
    )
}
