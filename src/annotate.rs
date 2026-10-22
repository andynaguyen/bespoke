//! Annotate: Scrawl's screen, voice and ink capture (`vendor/scrawl`), running inside Bespoke.
//! Bespoke registers its two shortcuts like any other binding and forwards their presses, keeps
//! Scrawl's captures and config under its own data folder, counts the captures for the Stats
//! page, draws the capsule (`capsule`) with the one dictation uses, and shows the Annotate page
//! wherever Scrawl would open its own Settings or welcome window. `mcp` serves the captures to
//! coding agents.

pub mod capsule;
pub mod mcp;

use std::sync::Arc;

use gpui::App;

use crate::app_handle::AppHandle;
use crate::managers::history::{CaptureUsage, HistoryManager};
use crate::settings::{get_settings, AppSettings};
use crate::ui::shell::Section;

/// Starts recording, switches the target, or finishes (`option+shift+s`).
pub const RECORD_BINDING: &str = "annotate";
/// Pastes the latest capture into the app in front (`option+shift+v`).
pub const PASTE_BINDING: &str = "annotate_paste";

pub fn is_annotate_binding(id: &str) -> bool {
    id == RECORD_BINDING || id == PASTE_BINDING
}

/// Captures and `config.json` live here; the Annotate page reads and writes the config.
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
    scrawl::host::configure(scrawl::host::Paths { data_dir, log_dir }, move || {
        ui.with_ui(|cx| crate::ui::show_section(Section::Annotate, cx));
    });
    count_captures(app);
    sync_shortcuts(&get_settings(app));
    capsule::init(app);
    scrawl::start(cx);
    mcp::sync(app);
}

/// Records each finished capture for the Stats page. Until one is recorded, it also counts the
/// captures already on disk, which Annotate made before it kept stats.
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

/// Gives Scrawl the current bindings, so its own windows and key tap match the record combo and
/// the capsule shows the right labels.
pub fn sync_shortcuts(settings: &AppSettings) {
    let config = |id: &str, kind: scrawl::ShortcutKind| {
        let Some(binding) = settings.bindings.get(id) else {
            return kind.default_config();
        };
        // A mouse button or lone modifier: Bespoke still forwards its presses, and Scrawl's
        // key tap matches nothing.
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
