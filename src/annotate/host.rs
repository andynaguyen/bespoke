use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::annotate::capture::{SharedCaptureTelemetry, ShortcutConfig, ShortcutKind};
use crate::annotate::coordinator::MenuAction;
use crate::annotate::model::CaptureBundle;

static OPEN_SETTINGS: OnceLock<Box<dyn Fn(&[Permission]) + Send + Sync>> = OnceLock::new();
static ON_CAPTURE: OnceLock<Box<dyn Fn(Finished) + Send + Sync>> = OnceLock::new();
static TELEMETRY: OnceLock<Arc<SharedCaptureTelemetry>> = OnceLock::new();
static MENU: OnceLock<(flume::Sender<MenuAction>, flume::Receiver<MenuAction>)> = OnceLock::new();
static PENDING_SHORTCUTS: Mutex<Option<(ShortcutConfig, ShortcutConfig)>> = Mutex::new(None);

/// `open_settings` opens the host's Annotate settings. It gets the permissions that held back a
/// recording, or none when the user asked for the settings.
pub fn configure(open_settings: impl Fn(&[Permission]) + Send + Sync + 'static) {
    let _ = OPEN_SETTINGS.set(Box::new(open_settings));
}

pub(crate) fn open_settings(missing: &[Permission]) {
    if let Some(open) = OPEN_SETTINGS.get() {
        open(missing);
    }
}

pub(crate) fn set_telemetry(telemetry: Arc<SharedCaptureTelemetry>) {
    let pending = PENDING_SHORTCUTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some((record, paste)) = pending {
        crate::annotate::capture::set_shortcuts(&telemetry, record, paste);
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
        crate::annotate::capture::pause_shortcuts(telemetry, paused);
    }
}

pub fn set_shortcuts(record: ShortcutConfig, paste: ShortcutConfig) {
    if let Some(telemetry) = TELEMETRY.get() {
        crate::annotate::capture::set_shortcuts(telemetry, record, paste);
    } else {
        *PENDING_SHORTCUTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((record, paste));
    }
}

pub fn shortcut_from_binding(binding: &str) -> Option<ShortcutConfig> {
    use crate::annotate::capture::{
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
        let code = crate::annotate::capture::key_code_for_char(ch)?;
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

pub use crate::annotate::handoff::{Capture, KeyframeRow};

pub fn recent_captures(limit: usize) -> Vec<Capture> {
    captures_dir().map_or_else(Vec::new, |dir| Capture::recent(&dir, limit))
}

pub fn copy_capture(dir: &Path) -> Result<(), String> {
    crate::annotate::handoff::copy_bundle_to_clipboard(dir).map_err(|e| e.to_string())
}

pub fn reveal_capture(dir: &Path) {
    let _ = crate::annotate::handoff::open_path(dir);
}

/// `None` without a running Bespoke, as in tests.
pub fn captures_dir() -> Option<PathBuf> {
    crate::annotate::handoff::default_captures_dir()
}

#[derive(Clone, Debug)]
pub struct Finished {
    pub dir: PathBuf,
    pub created_at_ms: u64,
    pub duration_ms: u64,
    pub marks: usize,
    pub app: Option<String>,
}

impl Finished {
    pub(crate) fn of(dir: &Path, bundle: &CaptureBundle) -> Self {
        Self {
            dir: dir.to_path_buf(),
            created_at_ms: bundle.created_at_ms,
            duration_ms: bundle.duration_ms,
            marks: bundle.marks.len(),
            app: bundle.target.app.clone(),
        }
    }

    pub fn duration(&self) -> String {
        crate::annotate::model::format_clock(self.duration_ms)
    }
}

static ON_CANCEL: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Calls `cancelled` each time a recording or the picker ends without a capture: cancelled,
/// held back by a missing permission, or failed while writing its bundle.
pub fn on_cancel(cancelled: impl Fn() + Send + Sync + 'static) {
    let _ = ON_CANCEL.set(Box::new(cancelled));
}

pub(crate) fn cancelled() {
    if let Some(cancelled) = ON_CANCEL.get() {
        cancelled();
    }
}

pub fn on_capture(record: impl Fn(Finished) + Send + Sync + 'static) {
    let _ = ON_CAPTURE.set(Box::new(record));
}

pub(crate) fn captured(dir: &Path, bundle: &CaptureBundle) {
    if let Some(record) = ON_CAPTURE.get() {
        record(Finished::of(dir, bundle));
    }
}

/// The newest capture on disk, found without reading every capture's manifest.
pub fn newest_capture() -> Option<Finished> {
    Capture::newest(&captures_dir()?).map(|capture| capture.finished())
}

pub fn finished_captures() -> Vec<Finished> {
    captures_dir().map_or_else(Vec::new, |dir| {
        Capture::recent(&dir, usize::MAX)
            .iter()
            .map(Capture::finished)
            .collect()
    })
}

pub use crate::annotate::capsule_link::{
    CapsuleCommand, CapsuleLink, CapsuleState, PillAction, PillSnapshot, Pres,
};
pub use crate::annotate::coordinator::SelectionMode;

pub fn preview_capsule(snapshot: PillSnapshot, anchor: [f64; 2]) {
    static PREVIEW: Mutex<Option<crate::annotate::capsule_link::NativePill>> = Mutex::new(None);
    let mut preview = PREVIEW
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if preview.is_none() {
        *preview =
            crate::annotate::capsule_link::NativePill::open(Arc::new(Mutex::new(Vec::new()))).ok();
    }
    if let Some(pill) = preview.as_ref() {
        pill.place_on(anchor);
        pill.sync(snapshot);
    }
}

pub type CapsuleActions = flume::Sender<PillAction>;

pub fn capsule_origin(point: [f64; 2], size: [f64; 2], at_top: bool, gap: f64) -> Option<[f64; 2]> {
    let screens = crate::annotate::screens::current();
    let index = crate::annotate::screens::index_at(&screens, point).or(if screens.is_empty() {
        None
    } else {
        Some(0)
    })?;
    let screen = &screens[index];
    Some(if at_top {
        crate::annotate::screens::pill_origin(screen, size[0], gap)
    } else {
        crate::annotate::screens::pill_origin_bottom(screen, size, gap)
    })
}

pub fn on_capsule(open: impl Fn(CapsuleLink) + Send + Sync + 'static) {
    crate::annotate::capsule_link::set_host(open);
}

pub use crate::annotate::permissions::{Grants, Permission, Status as PermissionStatus};
pub use crate::annotate::retention::Preset as Retention;
pub use crate::annotate::speech_model::{State as SpeechState, Status as SpeechStatus};

pub fn retention() -> Option<Retention> {
    crate::annotate::retention::Policy::load().preset()
}

pub fn set_retention(preset: Retention) {
    let saved = crate::annotate::handoff::default_config_path()
        .ok_or_else(|| anyhow::anyhow!("No config.json"))
        .and_then(|path| preset.policy().save_to(&path));
    if let Err(error) = saved {
        crate::annotate::capture::annotate_log(format!("Saving retention failed: {error:#}"));
    }
    crate::annotate::retention::nudge();
}

pub fn captures_size() -> String {
    let size = captures_dir().map_or(0, |dir| crate::annotate::retention::captures_size(&dir));
    crate::annotate::retention::format_bytes(size)
}

pub fn show_captures() {
    let Some(dir) = captures_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let _ = crate::annotate::handoff::open_path(&dir);
}

pub fn read_grants() -> Option<Grants> {
    crate::annotate::permissions::read_grants()
}

/// Whether this process may record the screen, by `CGPreflightScreenCaptureAccess`. It keeps its
/// first answer until the app relaunches, unlike `read_grants`, which asks a fresh helper.
pub fn screen_capture_granted() -> bool {
    crate::annotate::capture::screen_capture_granted()
}

pub fn speech_status() -> SpeechStatus {
    crate::annotate::speech_model::current()
}

static ON_SPEECH_STATUS: OnceLock<Box<dyn Fn(&SpeechStatus) + Send + Sync>> = OnceLock::new();

/// Runs `changed` each time Apple's speech model changes state or download progress.
pub fn on_speech_status(changed: impl Fn(&SpeechStatus) + Send + Sync + 'static) {
    let _ = ON_SPEECH_STATUS.set(Box::new(changed));
}

pub(crate) fn speech_status_changed(status: &SpeechStatus) {
    if let Some(changed) = ON_SPEECH_STATUS.get() {
        changed(status);
    }
}

/// Starts installing Apple's speech model, or starts the next try now after a failure. Returns
/// the `SpeechStatus::attempts` number of the try that answers this call.
pub fn ensure_speech_model() -> u32 {
    crate::annotate::speech_model::ensure()
}

pub use crate::annotate::model::{TranscriptData, TranscriptSegment, WordTiming};

type Transcriber = Box<dyn Fn(&Path) -> anyhow::Result<TranscriptData> + Send + Sync>;

static TRANSCRIBER: OnceLock<Transcriber> = OnceLock::new();

/// Replaces Apple Speech as the engine that transcribes a capture's `audio.wav`, a 16 kHz mono
/// WAV. The transcript needs word or segment times, because each keyframe gets the words said
/// while it was on screen. An error leaves the capture without a transcript.
pub fn on_transcribe(
    transcribe: impl Fn(&Path) -> anyhow::Result<TranscriptData> + Send + Sync + 'static,
) {
    let _ = TRANSCRIBER.set(Box::new(transcribe));
}

pub(crate) fn transcriber() -> Option<&'static Transcriber> {
    TRANSCRIBER.get()
}

/// Transcribes a 16 kHz mono WAV with Apple Speech, with word times. `contextual_strings` bias
/// recognition toward those words. When the model is still downloading, the transcript is empty
/// and `missing` says so.
pub fn apple_speech(wav: &Path, contextual_strings: &[String]) -> anyhow::Result<TranscriptData> {
    crate::annotate::helper::transcribe(wav, contextual_strings)
}

/// Asks macOS for Screen Recording. The prompt is attributed to this process, so the host app
/// gets the grant. macOS prompts once per app and adds it to the Screen Recording list, switched
/// off. Later calls return without a prompt.
pub fn request_screen_capture() {
    let granted = crate::annotate::capture::request_screen_capture();
    crate::annotate::capture::annotate_log(format!(
        "screen recording: asked macOS, CGRequestScreenCaptureAccess -> {granted}"
    ));
}

pub fn open_permission_settings(permission: Permission) {
    crate::annotate::capture::annotate_log(format!(
        "permission settings: opening for {permission:?}"
    ));
    if permission.draggable() {
        let name = match permission {
            Permission::ScreenRecording => "screen-recording",
            _ => "accessibility",
        };
        let spawned = crate::annotate::helper::path().and_then(|helper| {
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
                crate::annotate::capture::annotate_log(format!("permission panel: {error:#}"));
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
    let _ = crate::annotate::handoff::open_path(Path::new(&url));
}
