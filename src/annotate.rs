//! Annotate: Scrawl's screen, voice and ink capture (`vendor/scrawl`), running inside Bespoke.
//! Bespoke registers its two shortcuts like any other binding and forwards their presses, keeps
//! Scrawl's captures and config under its own data folder, and shows the Annotate page wherever
//! Scrawl would open its own Settings or welcome window.

use gpui::App;

use crate::app_handle::AppHandle;
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
    sync_shortcuts(&get_settings(app));
    scrawl::start(cx);
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
