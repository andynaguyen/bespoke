mod audio;
mod autocrop;
mod bundle;
pub mod capsule;
mod capsule_link;
pub mod capture;
pub mod coordinator;
mod handoff;
mod helper;
mod hold;
pub mod host;
mod keyframes;
mod laser_overlay;
pub mod mcp;
mod mic;
mod model;
mod permissions;
mod render;
mod retention;
mod screens;
mod sketch;
mod speech_model;
mod transcribe;
mod windows;

use std::sync::{Arc, Mutex};

use gpui::App;

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

static NEWEST_CAPTURE: Mutex<Option<crate::annotate::host::Finished>> = Mutex::new(None);

/// The newest finished capture: the newest on disk at launch, then each one recorded since.
/// `CAPTURE_FINISHED` fires after this updates.
pub fn newest_capture() -> Option<crate::annotate::host::Finished> {
    NEWEST_CAPTURE.lock().ok()?.clone()
}

fn note_capture(capture: &crate::annotate::host::Finished) {
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
    let (Some(data_dir), Ok(log_dir)) = (data_dir(app), crate::portable::app_log_dir(app)) else {
        log::error!("Annotate is off: no data or log directory");
        return;
    };
    let ui = app.clone();
    crate::annotate::host::configure(
        crate::annotate::host::Paths { data_dir, log_dir },
        move |missing| {
            let missing = missing.to_vec();
            ui.with_ui(move |cx| {
                if missing.is_empty() {
                    crate::ui::show_settings(SettingsSection::AnnotateShortcut, cx);
                } else {
                    crate::ui::pages::annotate::show_missing_permissions(&missing, cx);
                }
            });
        },
    );
    follow_captures(app);
    transcribe::init(app);
    sync_shortcuts(&get_settings(app));
    capsule::init(app);
    let settings = app.clone();
    crate::annotate::host::on_microphone(move || {
        crate::managers::audio::preferred_microphone(&get_settings(&settings))
    });
    let ui = app.clone();
    crate::annotate::host::on_inspect(move |dir| {
        ui.with_ui(move |cx| crate::ui::pages::captures::inspect(dir, cx));
    });
    crate::annotate::coordinator::start(cx);
    mcp::sync(app);
}

fn follow_captures(app: &AppHandle) {
    let history = Arc::clone(&app.state::<Arc<HistoryManager>>());
    let seed = Arc::clone(&history);
    let events = app.clone();
    crate::annotate::host::on_capture(move |capture| {
        note_capture(&capture);
        if let Err(e) = history.record_capture(&capture_usage(capture)) {
            log::error!("Failed to record Annotate usage: {e}");
        }
        if let Err(e) = events.emit(CAPTURE_FINISHED, ()) {
            log::warn!("Failed to announce a finished capture: {e}");
        }
    });
    let events = app.clone();
    crate::annotate::host::on_cancel(move || {
        if let Err(e) = events.emit(CAPTURE_CANCELLED, ()) {
            log::warn!("Failed to announce a cancelled capture: {e}");
        }
    });
    crate::runtime::spawn_blocking(move || {
        if let Some(capture) = crate::annotate::host::newest_capture() {
            note_capture(&capture);
        }
        let load = || {
            crate::annotate::host::finished_captures()
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

fn capture_usage(capture: crate::annotate::host::Finished) -> CaptureUsage {
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
        crate::annotate::capture::ShortcutKind::Paste
    } else {
        crate::annotate::capture::ShortcutKind::Record
    };
    crate::annotate::host::press(kind);
}

pub fn sync_shortcuts(settings: &AppSettings) {
    let config = |id: &str, kind: crate::annotate::capture::ShortcutKind| {
        let Some(binding) = settings.bindings.get(id) else {
            return kind.default_config();
        };
        crate::annotate::host::shortcut_from_binding(&binding.current_binding).unwrap_or_else(
            || crate::annotate::capture::ShortcutConfig {
                key_code: u32::MAX,
                carbon_modifiers: 0,
                label: binding.current_binding.clone(),
            },
        )
    };
    crate::annotate::host::set_shortcuts(
        config(
            RECORD_BINDING,
            crate::annotate::capture::ShortcutKind::Record,
        ),
        config(PASTE_BINDING, crate::annotate::capture::ShortcutKind::Paste),
    );
}
