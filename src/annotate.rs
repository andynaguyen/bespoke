pub mod capsule;
pub mod mcp;
mod transcribe;

use std::sync::Arc;

use gpui::App;

use crate::app_handle::AppHandle;
use crate::managers::history::{CaptureUsage, HistoryManager};
use crate::settings::{get_settings, AppSettings};
use crate::ui::pages::settings::SettingsSection;

pub const RECORD_BINDING: &str = "annotate";
pub const PASTE_BINDING: &str = "annotate_paste";

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
    scrawl::host::configure(scrawl::host::Paths { data_dir, log_dir }, move |missing| {
        let missing = missing.to_vec();
        ui.with_ui(move |cx| {
            if missing.is_empty() {
                crate::ui::show_settings(SettingsSection::AnnotateShortcut, cx);
            } else {
                crate::ui::pages::annotate::show_missing_permissions(&missing, cx);
            }
        });
    });
    count_captures(app);
    transcribe::init(app);
    sync_shortcuts(&get_settings(app));
    capsule::init(app);
    let settings = app.clone();
    scrawl::host::on_microphone(move || {
        crate::managers::audio::preferred_microphone(&get_settings(&settings))
    });
    let ui = app.clone();
    scrawl::host::on_inspect(move |dir| {
        ui.with_ui(move |cx| crate::ui::pages::captures::inspect(dir, cx));
    });
    scrawl::start(cx);
    mcp::sync(app);
}

fn count_captures(app: &AppHandle) {
    let history = Arc::clone(&app.state::<Arc<HistoryManager>>());
    let seed = Arc::clone(&history);
    scrawl::host::on_capture(move |capture| {
        if let Err(e) = history.record_capture(&capture_usage(capture)) {
            log::error!("Failed to record Annotate usage: {e}");
        }
    });
    crate::runtime::spawn_blocking(move || {
        let load = || {
            scrawl::host::finished_captures()
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

fn capture_usage(capture: scrawl::host::Finished) -> CaptureUsage {
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
        scrawl::ShortcutKind::Paste
    } else {
        scrawl::ShortcutKind::Record
    };
    scrawl::host::press(kind);
}

pub fn sync_shortcuts(settings: &AppSettings) {
    let config = |id: &str, kind: scrawl::ShortcutKind| {
        let Some(binding) = settings.bindings.get(id) else {
            return kind.default_config();
        };
        scrawl::host::shortcut_from_binding(&binding.current_binding).unwrap_or_else(|| {
            scrawl::ShortcutConfig {
                key_code: u32::MAX,
                carbon_modifiers: 0,
                label: binding.current_binding.clone(),
            }
        })
    };
    scrawl::host::set_shortcuts(
        config(RECORD_BINDING, scrawl::ShortcutKind::Record),
        config(PASTE_BINDING, scrawl::ShortcutKind::Paste),
    );
}
