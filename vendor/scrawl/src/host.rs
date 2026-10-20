//! The seam between Scrawl and the app it runs in (Bespoke, which calls it Annotate). The host
//! picks where captures, the config and the log go, registers the two shortcuts itself and
//! forwards their presses, sends its tray menu's Annotate items, and shows its own settings page
//! wherever Scrawl would open its Settings or welcome window.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::capture::{SharedCaptureTelemetry, ShortcutConfig, ShortcutKind};
use crate::ui::MenuAction;

pub struct Paths {
    /// Holds `captures/` and `config.json`.
    pub data_dir: PathBuf,
    /// Holds `annotate.log`.
    pub log_dir: PathBuf,
}

static PATHS: OnceLock<Paths> = OnceLock::new();
static OPEN_SETTINGS: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
static TELEMETRY: OnceLock<Arc<SharedCaptureTelemetry>> = OnceLock::new();
static MENU: OnceLock<(flume::Sender<MenuAction>, flume::Receiver<MenuAction>)> = OnceLock::new();
/// Bindings that arrived before `start` made the telemetry.
static PENDING_SHORTCUTS: Mutex<Option<(ShortcutConfig, ShortcutConfig)>> = Mutex::new(None);

/// Called once, before `start`. `open_settings` runs on any thread and shows the host's page
/// for Scrawl's settings and permissions.
pub fn configure(paths: Paths, open_settings: impl Fn() + Send + Sync + 'static) {
    let _ = PATHS.set(paths);
    let _ = OPEN_SETTINGS.set(Box::new(open_settings));
}

pub(crate) fn data_dir() -> Option<&'static Path> {
    PATHS.get().map(|paths| paths.data_dir.as_path())
}

pub(crate) fn log_dir() -> Option<&'static Path> {
    PATHS.get().map(|paths| paths.log_dir.as_path())
}

/// Brings the host's settings page forward (missing permissions, or Settings from the review
/// window).
pub(crate) fn open_settings() {
    if let Some(open) = OPEN_SETTINGS.get() {
        open();
    }
}

pub(crate) fn set_telemetry(telemetry: Arc<SharedCaptureTelemetry>) {
    let pending = PENDING_SHORTCUTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some((record, paste)) = pending {
        crate::capture::set_shortcuts(&telemetry, record, paste);
    }
    let _ = TELEMETRY.set(telemetry);
}

fn menu_channel() -> &'static (flume::Sender<MenuAction>, flume::Receiver<MenuAction>) {
    MENU.get_or_init(flume::unbounded)
}

pub(crate) fn menu_receiver() -> flume::Receiver<MenuAction> {
    menu_channel().1.clone()
}

/// One of the host's tray menu items was chosen.
pub fn menu(action: MenuAction) {
    let _ = menu_channel().0.try_send(action);
}

/// The host's binding for `kind` went down. Releases don't matter: Scrawl polls the modifiers
/// to tell a tap from a hold.
pub fn press(kind: ShortcutKind) {
    let Some(telemetry) = TELEMETRY.get() else {
        return;
    };
    match kind {
        ShortcutKind::Record => telemetry.emit_capture_shortcut(),
        ShortcutKind::Paste => telemetry.emit_paste_shortcut(),
    }
}

/// While the host records a new shortcut, Scrawl's key tap stops matching the record combo, so
/// pressing it reaches the recorder instead of starting a capture.
pub fn pause_shortcuts(paused: bool) {
    if let Some(telemetry) = TELEMETRY.get() {
        crate::capture::pause_shortcuts(telemetry, paused);
    }
}

/// Scrawl's copy of the host's two bindings: its key tap and windows match the record combo,
/// and the capsule and review window show both labels.
pub fn set_shortcuts(record: ShortcutConfig, paste: ShortcutConfig) {
    if let Some(telemetry) = TELEMETRY.get() {
        crate::capture::set_shortcuts(telemetry, record, paste);
    } else {
        *PENDING_SHORTCUTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((record, paste));
    }
}

/// A host binding written as `option+shift+s` (the `handy-keys` names), as Scrawl's shortcut:
/// the active layout's key code, Carbon modifier bits and a label like `⌥⇧S`. `None` for
/// combinations Scrawl can't match itself (mouse buttons, a lone modifier); the host's own
/// press still reaches `press`.
pub fn shortcut_from_binding(binding: &str) -> Option<ShortcutConfig> {
    use crate::capture::{
        CARBON_CMD_KEY, CARBON_CONTROL_KEY, CARBON_OPTION_KEY, CARBON_SHIFT_KEY, shortcut_label,
    };
    let mut modifiers = 0;
    let mut key = None;
    for token in binding.split('+').map(|t| t.trim().to_ascii_lowercase()) {
        match token.as_str() {
            "cmd" | "command" | "meta" | "super" => modifiers |= CARBON_CMD_KEY,
            "ctrl" | "control" => modifiers |= CARBON_CONTROL_KEY,
            "opt" | "option" | "alt" => modifiers |= CARBON_OPTION_KEY,
            "shift" => modifiers |= CARBON_SHIFT_KEY,
            "" => return None,
            _ if key.is_some() => return None,
            _ => key = Some(token),
        }
    }
    let key = key?;
    let (key_code, label) = named_key(&key).or_else(|| {
        let mut chars = key.chars();
        let ch = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        let code = crate::capture::key_code_for_char(ch)?;
        Some((u32::from(code), ch.to_uppercase().collect()))
    })?;
    Some(ShortcutConfig {
        key_code,
        carbon_modifiers: modifiers,
        label: shortcut_label(modifiers, &label),
    })
}

/// macOS virtual key codes (`kVK_*`) for the named keys, with the labels Scrawl shows.
fn named_key(name: &str) -> Option<(u32, String)> {
    const FUNCTION_KEYS: [u32; 16] = [
        122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111, 105, 107, 113, 106,
    ];
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<usize>().ok())
        && (1..=FUNCTION_KEYS.len()).contains(&n)
    {
        return Some((FUNCTION_KEYS[n - 1], format!("F{n}")));
    }
    let (code, label) = match name {
        "space" => (49, "Space"),
        "return" | "enter" => (36, "↩"),
        "tab" => (48, "⇥"),
        "backspace" | "delete" => (51, "⌫"),
        "left" | "leftarrow" => (123, "←"),
        "right" | "rightarrow" => (124, "→"),
        "down" | "downarrow" => (125, "↓"),
        "up" | "uparrow" => (126, "↑"),
        _ => return None,
    };
    Some((code, label.to_owned()))
}

// MARK: The host's settings page

pub use crate::mic::{Input as Microphone, Inputs as Microphones};
pub use crate::permissions::{Grants, Permission, Status as PermissionStatus};
pub use crate::retention::Preset as Retention;
pub use crate::speech_model::{State as SpeechState, Status as SpeechStatus};

/// The connected microphones. Asks CoreAudio, so call it off the UI thread.
pub fn microphones() -> Microphones {
    crate::mic::inputs()
}

/// The chosen microphone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MicrophoneChoice {
    /// The macOS default, followed as it changes.
    Default,
    /// A device by its CoreAudio UID, with the name it was saved under.
    Device { id: String, name: String },
}

pub fn microphone_choice() -> MicrophoneChoice {
    match crate::mic::choice_in(&crate::handoff::default_config_path()) {
        Some(choice) => MicrophoneChoice::Device {
            id: choice.id,
            name: choice.name,
        },
        None => MicrophoneChoice::Default,
    }
}

/// Records from `id` ("default" follows macOS).
pub fn set_microphone(id: &str, inputs: &Microphones) {
    let path = crate::handoff::default_config_path();
    if let Err(error) = crate::mic::save_in(&path, id, inputs) {
        crate::capture::scrawl_log(format!("Saving microphone failed: {error:#}"));
    }
}

/// How long captures are kept, or `None` for a policy written into `config.json` by hand.
pub fn retention() -> Option<Retention> {
    crate::retention::Policy::load().preset()
}

/// Keeps captures for `preset` and runs a cleanup pass.
pub fn set_retention(preset: Retention) {
    if let Err(error) = preset
        .policy()
        .save_to(&crate::handoff::default_config_path())
    {
        crate::capture::scrawl_log(format!("Saving retention failed: {error:#}"));
    }
    crate::retention::nudge();
}

/// How much space the captures take, like "1.2 GB". Walks the folder, so call it off the UI
/// thread.
pub fn captures_size() -> String {
    crate::retention::format_bytes(crate::retention::captures_size(
        &crate::handoff::default_captures_dir(),
    ))
}

/// Opens the captures folder in Finder.
pub fn show_captures() {
    let dir = crate::handoff::default_captures_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = crate::handoff::open_path(&dir);
}

/// What macOS has granted, from a fresh helper process (a grant may not reach a running app).
/// Blocks on the helper, so call it off the UI thread.
pub fn read_grants() -> Option<Grants> {
    crate::permissions::read_grants()
}

/// Apple's speech model, which Scrawl installs in the background at start.
pub fn speech_status() -> SpeechStatus {
    crate::speech_model::current()
}

/// Asks macOS for Screen Recording. The prompt is attributed to this process, so the host app
/// gets the grant.
pub fn request_screen_capture() {
    crate::capture::request_screen_capture();
}

/// Opens `permission`'s System Settings pane. For the lists an app is dragged into (Screen
/// Recording, Accessibility), Scrawl's card docks beside System Settings with the host app to
/// drag in.
pub fn open_permission_settings(permission: Permission) {
    if permission.draggable() {
        let name = match permission {
            Permission::ScreenRecording => "screen-recording",
            _ => "accessibility",
        };
        let spawned = crate::helper::path().and_then(|helper| {
            std::process::Command::new(helper)
                .args(["permission-panel", "--permission", name])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(anyhow::Error::from)
        });
        match spawned {
            // The card exits by itself once the grant lands or System Settings closes. It reads
            // EOF on stdin as "the parent quit", and `wait` closes stdin first, so hold it open
            // until the card is gone.
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _stdin = child.stdin.take();
                    let _ = child.wait();
                });
                return;
            }
            Err(error) => {
                crate::capture::scrawl_log(format!("permission panel: {error:#}"));
            }
        }
    }
    let anchor = match permission {
        Permission::ScreenRecording => "Privacy_ScreenCapture",
        Permission::Accessibility => "Privacy_Accessibility",
        Permission::Microphone => "Privacy_Microphone",
    };
    let url =
        format!("x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?{anchor}");
    let _ = crate::handoff::open_path(Path::new(&url));
}
