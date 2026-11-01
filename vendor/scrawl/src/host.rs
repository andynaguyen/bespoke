use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::capture::{SharedCaptureTelemetry, ShortcutConfig, ShortcutKind};
use crate::model::ScrawlBundle;
use crate::ui::MenuAction;

pub struct Paths {
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
}

static PATHS: OnceLock<Paths> = OnceLock::new();
static OPEN_SETTINGS: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
static ON_CAPTURE: OnceLock<Box<dyn Fn(Finished) + Send + Sync>> = OnceLock::new();
static TELEMETRY: OnceLock<Arc<SharedCaptureTelemetry>> = OnceLock::new();
static MENU: OnceLock<(flume::Sender<MenuAction>, flume::Receiver<MenuAction>)> = OnceLock::new();
static PENDING_SHORTCUTS: Mutex<Option<(ShortcutConfig, ShortcutConfig)>> = Mutex::new(None);

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

pub fn menu(action: MenuAction) {
    let _ = menu_channel().0.try_send(action);
}

pub fn press(kind: ShortcutKind) {
    let Some(telemetry) = TELEMETRY.get() else {
        return;
    };
    match kind {
        ShortcutKind::Record => telemetry.emit_capture_shortcut(),
        ShortcutKind::Paste => telemetry.emit_paste_shortcut(),
    }
}

pub fn pause_shortcuts(paused: bool) {
    if let Some(telemetry) = TELEMETRY.get() {
        crate::capture::pause_shortcuts(telemetry, paused);
    }
}

pub fn set_shortcuts(record: ShortcutConfig, paste: ShortcutConfig) {
    if let Some(telemetry) = TELEMETRY.get() {
        crate::capture::set_shortcuts(telemetry, record, paste);
    } else {
        *PENDING_SHORTCUTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((record, paste));
    }
}

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

pub use crate::handoff::{Capture, KeyframeRow};

pub fn recent_captures(limit: usize) -> Vec<Capture> {
    Capture::recent(&captures_dir(), limit)
}

pub fn copy_capture(dir: &Path) -> Result<(), String> {
    crate::handoff::copy_bundle_to_clipboard(dir).map_err(|e| e.to_string())
}

pub fn reveal_capture(dir: &Path) {
    let _ = crate::handoff::open_path(dir);
}

static ON_INSPECT: OnceLock<Box<dyn Fn(PathBuf) + Send + Sync>> = OnceLock::new();

pub fn on_inspect(inspect: impl Fn(PathBuf) + Send + Sync + 'static) {
    let _ = ON_INSPECT.set(Box::new(inspect));
}

pub(crate) fn inspect(dir: PathBuf) -> bool {
    match ON_INSPECT.get() {
        Some(inspect) => {
            inspect(dir);
            true
        }
        None => false,
    }
}

pub fn captures_dir() -> PathBuf {
    crate::handoff::default_captures_dir()
}

#[derive(Clone, Debug)]
pub struct Finished {
    pub created_at_ms: u64,
    pub duration_ms: u64,
    pub marks: usize,
    pub app: Option<String>,
}

impl Finished {
    pub(crate) fn of(bundle: &ScrawlBundle) -> Self {
        Self {
            created_at_ms: bundle.created_at_ms,
            duration_ms: bundle.duration_ms,
            marks: bundle.marks.len(),
            app: bundle.target.app.clone(),
        }
    }
}

pub fn on_capture(record: impl Fn(Finished) + Send + Sync + 'static) {
    let _ = ON_CAPTURE.set(Box::new(record));
}

pub(crate) fn captured(bundle: &ScrawlBundle) {
    if let Some(record) = ON_CAPTURE.get() {
        record(Finished::of(bundle));
    }
}

pub fn finished_captures() -> Vec<Finished> {
    Capture::recent(&captures_dir(), usize::MAX)
        .iter()
        .map(Capture::finished)
        .collect()
}

pub use crate::native::pill::{
    CapsuleCommand, CapsuleLink, CapsuleState, PillAction, PillSnapshot, Pres,
};
pub use crate::ui::SelectionMode;

pub fn preview_capsule(snapshot: PillSnapshot, anchor: [f64; 2]) {
    static PREVIEW: Mutex<Option<crate::native::pill::NativePill>> = Mutex::new(None);
    let mut preview = PREVIEW
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if preview.is_none() {
        *preview = crate::native::pill::NativePill::open(Arc::new(Mutex::new(Vec::new()))).ok();
    }
    if let Some(pill) = preview.as_ref() {
        pill.place_on(anchor);
        pill.sync(snapshot);
    }
}

pub type CapsuleActions = flume::Sender<PillAction>;

pub fn capsule_origin(point: [f64; 2], size: [f64; 2], at_top: bool, gap: f64) -> Option<[f64; 2]> {
    let screens = crate::screens::current();
    let index = crate::screens::index_at(&screens, point).or(if screens.is_empty() {
        None
    } else {
        Some(0)
    })?;
    let screen = &screens[index];
    Some(if at_top {
        crate::screens::pill_origin(screen, size[0], gap)
    } else {
        crate::screens::pill_origin_bottom(screen, size, gap)
    })
}

pub fn on_capsule(open: impl Fn(CapsuleLink) + Send + Sync + 'static) {
    crate::native::pill::set_host(open);
}

pub use crate::mic::{Input as Microphone, Inputs as Microphones};
pub use crate::permissions::{Grants, Permission, Status as PermissionStatus};
pub use crate::retention::Preset as Retention;
pub use crate::speech_model::{State as SpeechState, Status as SpeechStatus};

pub fn microphones() -> Microphones {
    crate::mic::inputs()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MicrophoneChoice {
    Default,
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

pub fn set_microphone(id: &str, inputs: &Microphones) {
    let path = crate::handoff::default_config_path();
    if let Err(error) = crate::mic::save_in(&path, id, inputs) {
        crate::capture::scrawl_log(format!("Saving microphone failed: {error:#}"));
    }
}

pub fn retention() -> Option<Retention> {
    crate::retention::Policy::load().preset()
}

pub fn set_retention(preset: Retention) {
    if let Err(error) = preset
        .policy()
        .save_to(&crate::handoff::default_config_path())
    {
        crate::capture::scrawl_log(format!("Saving retention failed: {error:#}"));
    }
    crate::retention::nudge();
}

pub fn captures_size() -> String {
    crate::retention::format_bytes(crate::retention::captures_size(
        &crate::handoff::default_captures_dir(),
    ))
}

pub fn show_captures() {
    let dir = crate::handoff::default_captures_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = crate::handoff::open_path(&dir);
}

pub fn read_grants() -> Option<Grants> {
    crate::permissions::read_grants()
}

/// Whether this process may record the screen, by `CGPreflightScreenCaptureAccess`. It keeps its
/// first answer until the app relaunches, unlike `read_grants`, which asks a fresh helper.
pub fn screen_capture_granted() -> bool {
    crate::capture::screen_capture_granted()
}

pub fn speech_status() -> SpeechStatus {
    crate::speech_model::current()
}

/// Asks macOS for Screen Recording. The prompt is attributed to this process, so the host app
/// gets the grant.
pub fn request_screen_capture() {
    crate::capture::request_screen_capture();
}

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
