use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::annotate::model::{
    CaptureMode, InteractionEvent, RgbaFrame, SCALE, Stroke, TargetFrame, TargetInfo,
};

mod macos;
pub use macos::*;

pub const FRAME_SAMPLE_INTERVAL: Duration = Duration::from_millis(220);
pub const CANDIDATE_EDGE: f64 = 480.0;
pub(crate) const CANDIDATE_EDGE_NO_VIDEO: f64 = 1280.0;
pub const CANDIDATE_BUDGET_BYTES: usize = 384 << 20;

pub struct CandidateBuffer {
    frames: Vec<(RgbaFrame, bool)>,
    cap: usize,
    budget: usize,
    interval: Duration,
}

impl CandidateBuffer {
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            frames: Vec::new(),
            cap: 0,
            budget: budget_bytes,
            interval: FRAME_SAMPLE_INTERVAL,
        }
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn push(&mut self, frame: RgbaFrame, urgent: bool) {
        if self.cap == 0 {
            self.cap = (self.budget / frame.rgba.len().max(1)).clamp(60, 1_200);
        }
        self.frames.push((frame, urgent));
        if self.frames.len() >= self.cap {
            self.thin();
        }
    }

    fn thin(&mut self) {
        let mut regular = 0_usize;
        let mut kept: Vec<(RgbaFrame, bool)> = std::mem::take(&mut self.frames)
            .into_iter()
            .enumerate()
            .filter(|(i, (_, urgent))| {
                let keep = *i == 0 || *urgent || regular.is_multiple_of(2);
                if !urgent {
                    regular += 1;
                }
                keep
            })
            .map(|(_, f)| f)
            .collect();
        if kept.len() > self.cap * 3 / 4 {
            kept = kept
                .into_iter()
                .enumerate()
                .filter(|(i, _)| i.is_multiple_of(2))
                .map(|(_, f)| f)
                .collect();
        }
        self.frames = kept;
        self.interval *= 2;
    }

    pub fn drop_since(&mut self, t_ms: u64) {
        let mut index = 0;
        self.frames.retain(|(frame, _)| {
            index += 1;
            index == 1 || frame.t_ms <= t_ms
        });
    }

    pub fn into_frames(self) -> Vec<RgbaFrame> {
        self.frames.into_iter().map(|(f, _)| f).collect()
    }
}

pub struct ScreenRecording {
    pub frames: Vec<RgbaFrame>,
    pub video: Option<PathBuf>,
    pub duration_ms: u64,
    pub video_offset_ms: u64,
}

#[derive(Clone, Debug)]
pub enum LiveOverlayEvent {
    CaptureShortcut,
    PasteLatestShortcut,
    UndoStroke,
    RequestSnapshot,
    Grants(crate::annotate::permissions::Grants),
}

pub const CARBON_CMD_KEY: u32 = 1 << 8;
pub const CARBON_SHIFT_KEY: u32 = 1 << 9;
pub const CARBON_OPTION_KEY: u32 = 1 << 11;
pub const CARBON_CONTROL_KEY: u32 = 1 << 12;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShortcutConfig {
    pub key_code: u32,
    pub carbon_modifiers: u32,
    pub label: String,
}

impl Default for ShortcutConfig {
    fn default() -> Self {
        Self::default_record()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShortcutKind {
    Record,
    Paste,
}

impl ShortcutKind {
    pub fn default_config(self) -> ShortcutConfig {
        match self {
            Self::Record => ShortcutConfig::default_record(),
            Self::Paste => ShortcutConfig::default_paste(),
        }
    }

    fn default_char(self) -> char {
        match self {
            Self::Record => 's',
            Self::Paste => 'v',
        }
    }

    pub(crate) fn ansi_code(self) -> u16 {
        match self {
            Self::Record => KEY_S,
            Self::Paste => KEY_V,
        }
    }
}

impl ShortcutConfig {
    pub fn default_record() -> Self {
        Self {
            key_code: u32::from(layout_s_keycode()),
            carbon_modifiers: CARBON_OPTION_KEY | CARBON_SHIFT_KEY,
            label: shortcut_label(CARBON_OPTION_KEY | CARBON_SHIFT_KEY, "S"),
        }
    }

    pub fn default_paste() -> Self {
        Self {
            key_code: u32::from(layout_v_keycode()),
            carbon_modifiers: CARBON_OPTION_KEY | CARBON_SHIFT_KEY,
            label: shortcut_label(CARBON_OPTION_KEY | CARBON_SHIFT_KEY, "V"),
        }
    }

    pub fn is_default_for(&self, kind: ShortcutKind) -> bool {
        self.carbon_modifiers == CARBON_OPTION_KEY | CARBON_SHIFT_KEY
            && shortcut_parts(&self.label).1 == kind.default_char().to_ascii_uppercase().to_string()
    }
}

pub fn shortcut_label(carbon_modifiers: u32, key: &str) -> String {
    let mut label = String::new();
    for (bit, glyph) in [
        (CARBON_CONTROL_KEY, '⌃'),
        (CARBON_OPTION_KEY, '⌥'),
        (CARBON_SHIFT_KEY, '⇧'),
        (CARBON_CMD_KEY, '⌘'),
    ] {
        if carbon_modifiers & bit != 0 {
            label.push(glyph);
        }
    }
    label.push_str(key);
    label
}

pub fn annotate_log(msg: impl AsRef<str>) {
    use std::io::Write;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let line = format!("[{ts}] {}\n", msg.as_ref());
    eprint!("{line}");
    // Without a running Bespoke, as in tests, the line goes to stderr only.
    let Some(dir) = crate::app_handle::AppHandle::try_global()
        .and_then(|app| crate::portable::app_log_dir(app).ok())
    else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("annotate.log"))
    {
        let _ = f.write_all(line.as_bytes());
    }
}

static TELEMETRY: OnceLock<Arc<SharedCaptureTelemetry>> = OnceLock::new();

/// Keeps the telemetry `coordinator::start` made, for `press`, `pause_shortcuts` and
/// `set_shortcuts`.
pub(crate) fn set_telemetry(telemetry: Arc<SharedCaptureTelemetry>) {
    let _ = TELEMETRY.set(telemetry);
}

/// Reports a press of one of Bespoke's Annotate shortcuts. Does nothing before Annotate starts.
pub fn press(kind: ShortcutKind) {
    let Some(telemetry) = TELEMETRY.get() else {
        return;
    };
    match kind {
        ShortcutKind::Record => telemetry.emit_capture_shortcut(),
        ShortcutKind::Paste => telemetry.emit_paste_shortcut(),
    }
}

/// Ignores shortcut presses while `paused`, as while Bespoke records a new binding.
pub fn pause_shortcuts(paused: bool) {
    if let Some(telemetry) = TELEMETRY.get() {
        telemetry.shortcuts_paused.store(paused, Ordering::Release);
    }
}

/// Sets the record and paste combos the key tap matches. Does nothing before Annotate starts,
/// which takes the first ones from `coordinator::start`.
pub fn set_shortcuts(record: ShortcutConfig, paste: ShortcutConfig) {
    if let Some(telemetry) = TELEMETRY.get() {
        telemetry.set_shortcuts(record, paste);
    }
}

/// Parses a Bespoke binding such as `option+shift+s` into a combo, or `None` when it isn't one.
pub fn shortcut_from_binding(binding: &str) -> Option<ShortcutConfig> {
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
        let code = key_code_for_char(ch)?;
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

pub fn shortcut_parts(label: &str) -> (&str, &str) {
    let before_key = label.char_indices().last().map_or("", |(i, _)| &label[..i]);
    if let Some(plus) = before_key.rfind('+') {
        return (&label[..plus], &label[plus + 1..]);
    }
    let split = label
        .char_indices()
        .find(|(_, c)| !matches!(c, '⌃' | '⌥' | '⇧' | '⌘'))
        .map_or(label.len(), |(i, _)| i);
    label.split_at(split)
}

pub fn is_capture_shortcut(
    cfg: &ShortcutConfig,
    s_code: u16,
    keycode: u16,
    cmd: bool,
    ctrl: bool,
    alt: bool,
    shift: bool,
) -> bool {
    shortcut_matches(
        ShortcutKind::Record,
        cfg,
        s_code,
        keycode,
        cmd,
        ctrl,
        alt,
        shift,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn shortcut_matches(
    kind: ShortcutKind,
    cfg: &ShortcutConfig,
    layout_code: u16,
    keycode: u16,
    cmd: bool,
    ctrl: bool,
    alt: bool,
    shift: bool,
) -> bool {
    let want_cmd = (cfg.carbon_modifiers & CARBON_CMD_KEY) != 0;
    let want_ctrl = (cfg.carbon_modifiers & CARBON_CONTROL_KEY) != 0;
    let want_alt = (cfg.carbon_modifiers & CARBON_OPTION_KEY) != 0;
    let want_shift = (cfg.carbon_modifiers & CARBON_SHIFT_KEY) != 0;
    let exact = u32::from(keycode) == cfg.key_code
        && cmd == want_cmd
        && ctrl == want_ctrl
        && alt == want_alt
        && shift == want_shift;
    if exact {
        return true;
    }
    let is_key = keycode == layout_code || keycode == kind.ansi_code();
    cfg.is_default_for(kind) && is_key && alt && (shift || (ctrl && cmd))
}

fn clean_label(input: &str) -> String {
    let squashed = input.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = squashed.trim();
    if trimmed.chars().count() > 44 {
        let prefix: String = trimmed.chars().take(43).collect();
        format!("{}…", prefix.trim_end())
    } else {
        trimmed.to_string()
    }
}

pub fn pick_display(
    displays: &[[f64; 4]],
    point: Option<[f64; 2]>,
    focused: Option<[f64; 4]>,
) -> usize {
    let inside = |p: [f64; 2]| {
        displays
            .iter()
            .position(|&[x, y, w, h]| p[0] >= x && p[0] < x + w && p[1] >= y && p[1] < y + h)
    };
    point
        .and_then(inside)
        .or_else(|| focused.and_then(|[x, y, w, h]| inside([x + w / 2.0, y + h / 2.0])))
        .unwrap_or(0)
}

pub fn selectable_window_at(windows: &[TargetInfo], x: f64, y: f64) -> Option<&TargetInfo> {
    windows.iter().find(|window| {
        let f = window.frame;
        x >= f.x && x <= f.x + f.width && y >= f.y && y <= f.y + f.height
    })
}

pub fn region_target(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    windows: &[TargetInfo],
) -> TargetInfo {
    let mid_x = x + width * 0.5;
    let mid_y = y + height * 0.5;
    let hit = selectable_window_at(windows, mid_x, mid_y);
    let frontmost = frontmost_app_info();
    let screens = crate::annotate::screens::displays();
    let (display_id, scale, [x, y, width, height]) =
        crate::annotate::screens::area_on_display(&screens, [x, y, width, height]).map_or_else(
            || {
                let min = crate::annotate::screens::MIN_AREA;
                let rect = [x, y, width.max(min), height.max(min)];
                (main_display_id(), MAX_CAPTURE_SCALE, rect)
            },
            |(i, rect)| (screens[i].id, screens[i].scale, rect),
        );
    TargetInfo {
        mode: CaptureMode::Region,
        app: hit
            .and_then(|window| window.app.clone())
            .or_else(|| frontmost.as_ref().map(|(_, name, _)| name.clone())),
        bundle_id: hit
            .and_then(|window| window.bundle_id.clone())
            .or_else(|| frontmost.and_then(|(_, _, bid)| bid)),
        window_title: hit.and_then(|window| window.window_title.clone()),
        window_id: None,
        display_id,
        frame: TargetFrame {
            x,
            y,
            width,
            height,
            scale,
        },
    }
}

pub fn capture_size(target: &TargetInfo, long_edge: f64) -> (u32, u32) {
    let max_dim = target.frame.width.max(target.frame.height).max(1.0);
    let native = target.frame.scale.clamp(1.0, MAX_CAPTURE_SCALE);
    let scale = (long_edge / max_dim).min(native);
    (
        ((target.frame.width * scale).round() as u32).max(48) & !1,
        ((target.frame.height * scale).round() as u32).max(48) & !1,
    )
}

pub(crate) struct Sampler {
    pub target: TargetInfo,
    pub video_output_path: Option<PathBuf>,
    pub excluded_window_ids: Arc<Mutex<Vec<u32>>>,
    pub started_at: Instant,
    pub stopping: Arc<AtomicBool>,
    pub urgent: Arc<AtomicBool>,
    pub frames: Arc<Mutex<CandidateBuffer>>,
    pub video_offset_ms: Arc<AtomicU64>,
}

pub struct ScreenCaptureSession {
    started_at: Instant,
    stopping: Arc<AtomicBool>,
    urgent_snapshot: Arc<AtomicBool>,
    frames: Arc<Mutex<CandidateBuffer>>,
    video_offset_ms: Arc<AtomicU64>,
    sampler_thread: Option<JoinHandle<Option<PathBuf>>>,
    _recording: crate::annotate::retention::Recording,
}

impl ScreenCaptureSession {
    /// Starts recording `target`. Frame and video times count from `started_at`, the clock
    /// the capture's clicks, keys and ink use.
    pub fn start(
        target: TargetInfo,
        video_output_path: Option<PathBuf>,
        excluded_window_ids: Arc<Mutex<Vec<u32>>>,
        started_at: Instant,
    ) -> Self {
        let stopping = Arc::new(AtomicBool::new(false));
        let urgent_snapshot = Arc::new(AtomicBool::new(true));
        let frames = Arc::new(Mutex::new(CandidateBuffer::new(CANDIDATE_BUDGET_BYTES)));
        let video_offset_ms = Arc::new(AtomicU64::new(0));

        let thread_stopping = Arc::clone(&stopping);
        let thread_urgent = Arc::clone(&urgent_snapshot);
        let thread_frames = Arc::clone(&frames);
        let thread_offset = Arc::clone(&video_offset_ms);
        let sampler_target = target;
        let sampler_thread = thread::Builder::new()
            .name("annotate-frame-sampler".into())
            .spawn(move || {
                macos::run_sampler(Sampler {
                    target: sampler_target,
                    video_output_path,
                    excluded_window_ids,
                    started_at,
                    stopping: thread_stopping,
                    urgent: thread_urgent,
                    frames: thread_frames,
                    video_offset_ms: thread_offset,
                })
            })
            .ok();

        Self {
            started_at,
            stopping,
            urgent_snapshot,
            frames,
            video_offset_ms,
            sampler_thread,
            _recording: crate::annotate::retention::Recording::start(),
        }
    }

    pub fn request_immediate_frame(&self) {
        self.urgent_snapshot.store(true, Ordering::Release);
        if let Some(ref handle) = self.sampler_thread {
            handle.thread().unpark();
        }
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }

    pub fn started_at(&self) -> Instant {
        self.started_at
    }

    pub fn stop(mut self) -> ScreenRecording {
        let duration_ms = self.elapsed_ms();
        self.stopping.store(true, Ordering::Release);
        let video_path = self
            .sampler_thread
            .take()
            .and_then(|handle| {
                handle.thread().unpark();
                handle.join().ok()
            })
            .flatten();
        let buffer = std::mem::replace(
            &mut *self
                .frames
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            CandidateBuffer::new(0),
        );
        let mut collected = buffer.into_frames();
        if collected.is_empty() {
            collected.push(crate::annotate::render::render_synthetic_demo_frame(
                1280, 800, 0, 0,
            ));
        }
        let valid_video =
            video_path.filter(|path| path.exists() && path.metadata().is_ok_and(|m| m.len() > 0));
        ScreenRecording {
            frames: collected,
            video: valid_video,
            duration_ms: duration_ms.max(100),
            video_offset_ms: self.video_offset_ms.load(Ordering::Acquire),
        }
    }
}

impl Drop for ScreenCaptureSession {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Some(handle) = self.sampler_thread.take() {
            handle.thread().unpark();
            let _ = handle.join();
        }
    }
}

/// Where a drag began: time, x and y as fractions of the target, the label of what was under
/// the pointer, and the target then.
type DragStart = (u64, f32, f32, Option<String>, TargetFrame);
/// A ⇧-press waiting for its release: time, x and y in screen points, and the target then.
type ShiftPress = (u64, f64, f64, TargetFrame);

pub struct SharedCaptureTelemetry {
    pub active: AtomicBool,
    pub selecting: AtomicBool,
    pub shift_held: AtomicBool,
    pub capture_mode: AtomicU64,
    pub shortcut: Mutex<ShortcutConfig>,
    pub paste_shortcut: Mutex<ShortcutConfig>,
    pub shortcuts_paused: AtomicBool,
    pub started_at: Mutex<Instant>,
    pub picker_started_at: Mutex<Instant>,
    pub picker_rect: Mutex<Option<([f32; 4], u8)>>,
    pub target_frame: Mutex<TargetFrame>,
    pub pointer_screen: Mutex<[f32; 2]>,
    pub events: Mutex<Vec<InteractionEvent>>,
    pub strokes: Mutex<Vec<Stroke>>,
    pub active_laser: Mutex<Option<Stroke>>,
    pub live_ink: Mutex<Option<Stroke>>,
    pub excluded_window_ids: Arc<Mutex<Vec<u32>>>,
    pub window_log_request: Arc<AtomicBool>,
    pub ui_sender: flume::Sender<LiveOverlayEvent>,
    pub passthrough_window_ids: Mutex<Vec<u32>>,
    tap_port: AtomicU64,
    tap_run_loop: AtomicU64,
    tap_starting: AtomicBool,
    tap_tried_ms: AtomicU64,
    tap_live: AtomicBool,
    escape_pressed: AtomicBool,
    pub start_glow: AtomicBool,
    mouse_live: AtomicBool,
    last_shortcut_ms: AtomicU64,
    drag_start: Mutex<Option<DragStart>>,
    shift_press: Mutex<Option<ShiftPress>>,
}

impl SharedCaptureTelemetry {
    pub fn new(ui_sender: flume::Sender<LiveOverlayEvent>) -> Arc<Self> {
        Arc::new(Self {
            active: AtomicBool::new(false),
            selecting: AtomicBool::new(false),
            shift_held: AtomicBool::new(false),
            capture_mode: AtomicU64::new(0),
            shortcut: Mutex::new(ShortcutConfig::default_record()),
            paste_shortcut: Mutex::new(ShortcutConfig::default_paste()),
            shortcuts_paused: AtomicBool::new(false),
            started_at: Mutex::new(Instant::now()),
            picker_started_at: Mutex::new(Instant::now()),
            picker_rect: Mutex::new(None),
            target_frame: Mutex::new(TargetFrame {
                x: 0.0,
                y: 0.0,
                width: 1440.0,
                height: 900.0,
                scale: 2.0,
            }),
            pointer_screen: Mutex::new([0.0, 0.0]),
            events: Mutex::new(Vec::new()),
            strokes: Mutex::new(Vec::new()),
            active_laser: Mutex::new(None),
            live_ink: Mutex::new(None),
            excluded_window_ids: Arc::new(Mutex::new(Vec::new())),
            window_log_request: Arc::new(AtomicBool::new(false)),
            ui_sender,
            passthrough_window_ids: Mutex::new(Vec::new()),
            tap_port: AtomicU64::new(0),
            tap_run_loop: AtomicU64::new(0),
            tap_starting: AtomicBool::new(false),
            tap_tried_ms: AtomicU64::new(0),
            tap_live: AtomicBool::new(false),
            escape_pressed: AtomicBool::new(false),
            start_glow: AtomicBool::new(false),
            mouse_live: AtomicBool::new(false),
            last_shortcut_ms: AtomicU64::new(0),
            drag_start: Mutex::new(None),
            shift_press: Mutex::new(None),
        })
    }

    /// Sets the record and paste combos the key tap matches.
    pub fn set_shortcuts(&self, record: ShortcutConfig, paste: ShortcutConfig) {
        *self
            .shortcut
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = record;
        *self
            .paste_shortcut
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = paste;
    }

    pub fn emit_capture_shortcut(&self) {
        if self.shortcuts_paused.load(Ordering::Acquire) {
            return;
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let prev = self.last_shortcut_ms.load(Ordering::Acquire);
        if now_ms.saturating_sub(prev) >= 90
            && self
                .last_shortcut_ms
                .compare_exchange(prev, now_ms, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            annotate_log("emit_capture_shortcut");
            let _ = self.ui_sender.try_send(LiveOverlayEvent::CaptureShortcut);
        }
    }

    pub fn emit_paste_shortcut(&self) {
        if self.shortcuts_paused.load(Ordering::Acquire) {
            return;
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let prev = self.last_shortcut_ms.load(Ordering::Acquire);
        if now_ms.saturating_sub(prev) >= 400
            && self
                .last_shortcut_ms
                .compare_exchange(prev, now_ms, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            annotate_log("emit_paste_shortcut -> sending PasteLatestShortcut");
            let _ = self
                .ui_sender
                .try_send(LiveOverlayEvent::PasteLatestShortcut);
        }
    }

    pub fn matches_shortcut(
        &self,
        keycode: u16,
        cmd: bool,
        ctrl: bool,
        alt: bool,
        shift: bool,
    ) -> bool {
        let cfg = self
            .shortcut
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        is_capture_shortcut(&cfg, layout_s_keycode(), keycode, cmd, ctrl, alt, shift)
    }

    pub fn set_capture_mode(&self, mode: CaptureMode) {
        let code = match mode {
            CaptureMode::Display => 0,
            CaptureMode::Region => 1,
            CaptureMode::Window => 2,
        };
        self.capture_mode.store(code, Ordering::Release);
    }

    pub fn begin_picker_session(&self) {
        *self
            .picker_started_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
        *self
            .picker_rect
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.active.store(false, Ordering::Release);
        self.selecting.store(true, Ordering::Release);
    }

    pub fn set_picker_rect(&self, rect: Option<([f32; 4], u8)>) {
        *self
            .picker_rect
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = rect;
    }

    pub fn reset_for_session(&self, frame: TargetFrame) {
        self.selecting.store(false, Ordering::Release);
        *self
            .picker_rect
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        *self
            .started_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
        *self
            .target_frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = frame;
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.strokes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        *self
            .active_laser
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        *self
            .live_ink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        *self
            .drag_start
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        *self
            .shift_press
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.excluded_window_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.shift_held.store(false, Ordering::Release);
        self.window_log_request.store(false, Ordering::Release);
        self.active.store(true, Ordering::Release);
        let clicks = if self.mouse_live.load(Ordering::Acquire) {
            macos::MOUSE_SOURCE
        } else {
            "polled mouse buttons"
        };
        let keys = if self.tap_live.load(Ordering::Acquire) {
            macos::KEY_SOURCE
        } else {
            macos::NO_KEY_SOURCE
        };
        annotate_log(format!("session clicks from {clicks}, keys from {keys}"));
    }

    pub fn take_escape(&self) -> bool {
        self.escape_pressed.swap(false, Ordering::AcqRel)
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.session_start().elapsed().as_millis() as u64
    }

    /// When the current session's clock started: every time in a capture counts from here.
    pub fn session_start(&self) -> Instant {
        *self
            .started_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn contains_screen_point(&self, screen_x: f64, screen_y: f64) -> bool {
        let frame = *self
            .target_frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        frame.width >= 16.0
            && frame.height >= 16.0
            && frame.x > -10_000.0
            && screen_x >= frame.x
            && screen_x <= frame.x + frame.width
            && screen_y >= frame.y
            && screen_y <= frame.y + frame.height
    }

    fn pointer_down(&self, t_ms: u64, screen_x: f64, screen_y: f64) {
        if !self.contains_screen_point(screen_x, screen_y) {
            return;
        }
        let raw = objc2::rc::autoreleasepool(|_| crate::annotate::windows::raw_windows());
        if crate::annotate::windows::system_prompt_at(&raw, screen_x, screen_y) {
            return;
        }
        let (nx, ny, _, _) = self.normalize_point(screen_x, screen_y);
        self.window_log_request.store(true, Ordering::Release);
        let start_frame = *self
            .target_frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *self
            .drag_start
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((t_ms, nx, ny, None, start_frame));
    }

    fn pointer_up(&self, t_ms: u64, screen_x: f64, screen_y: f64, right: bool) {
        let Some((t0, x0, y0, start_label, start_frame)) = self
            .drag_start
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        else {
            return;
        };
        let end_frame = *self
            .target_frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !same_target_frame(start_frame, end_frame) {
            return;
        }
        let (nx, ny, fw, fh) = self.normalize_point(screen_x, screen_y);
        let dist_px = ((nx - x0) * fw as f32).hypot((ny - y0) * fh as f32);
        let _ = self.ui_sender.try_send(LiveOverlayEvent::RequestSnapshot);
        let event = if dist_px < CLICK_SLOP_PT {
            InteractionEvent::Click {
                t: t0,
                x: x0,
                y: y0,
                button: Some(if right { "right" } else { "left" }.into()),
                label: start_label,
                role: None,
            }
        } else {
            InteractionEvent::Drag {
                t: t0,
                t1: t_ms,
                x0,
                y0,
                x1: nx,
                y1: ny,
                label: start_label,
                button: None,
                path: Vec::new(),
            }
        };
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }

    fn shift_down(&self, t_ms: u64, screen_x: f64, screen_y: f64) {
        if !self.contains_screen_point(screen_x, screen_y) {
            return;
        }
        let frame = *self
            .target_frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *self
            .shift_press
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((t_ms, screen_x, screen_y, frame));
    }

    /// The button of a ⇧-press came up (with or without ⇧ still held). A ⇧-click drops the next
    /// numbered pin where it went down, unless ink was already drawn in this ⇧ hold: a click in
    /// the middle of a drawing belongs to the drawing, so it neither pins nor counts as a click.
    /// A ⇧-drag is left to the ink the poller is drawing. Returns whether a ⇧-press was waiting,
    /// so the release isn't also taken as a click's.
    fn shift_up(&self, t_ms: u64, screen_x: f64, screen_y: f64) -> bool {
        let Some((t0, x0, y0, start_frame)) = self
            .shift_press
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        else {
            return false;
        };
        let frame = *self
            .target_frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !same_target_frame(start_frame, frame)
            || !is_pin_click((t0, x0, y0), (t_ms, screen_x, screen_y))
        {
            return true;
        }
        {
            let mut live = self
                .live_ink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if live.as_ref().is_some_and(|ink| drew_before(ink, t0)) {
                return true;
            }
            // ⇧ draws as it's held, so the live ink is only the click's jitter; further ink
            // starts a new stroke.
            *live = None;
        }
        let (nx, ny, fw, fh) = self.normalize_point(x0, y0);
        let mut strokes = self
            .strokes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let number = strokes.iter().filter(|s| s.pin.is_some()).count() as u32 + 1;
        strokes.push(Stroke {
            t0,
            t1: t_ms,
            fw,
            fh,
            on: None,
            pin: Some(number),
            laser: false,
            pts: vec![
                (nx * SCALE as f32).round() as i32,
                (ny * SCALE as f32).round() as i32,
                0,
            ],
        });
        drop(strokes);
        self.window_log_request.store(true, Ordering::Release);
        let _ = self.ui_sender.try_send(LiveOverlayEvent::RequestSnapshot);
        true
    }

    fn on_own_window(&self, x: f64, y: f64) -> bool {
        let passthrough = self
            .passthrough_window_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let windows = macos::raw_windows_now();
        crate::annotate::windows::owned_window_at(
            &windows,
            x,
            y,
            std::process::id() as i32,
            &passthrough,
        )
    }

    pub fn normalize_point(&self, screen_x: f64, screen_y: f64) -> (f32, f32, u32, u32) {
        let frame = *self
            .target_frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let nx = ((screen_x - frame.x) / frame.width.max(1.0)).clamp(0.0, 1.0) as f32;
        let ny = ((screen_y - frame.y) / frame.height.max(1.0)).clamp(0.0, 1.0) as f32;
        (
            nx,
            ny,
            frame.width.round().max(1.0) as u32,
            frame.height.round().max(1.0) as u32,
        )
    }

    pub fn finish_active_laser(&self, _ax_point: Option<(f64, f64)>) {
        if let Some(mut laser) = self
            .active_laser
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            && !laser.pts.is_empty()
        {
            if let Some(last) = laser.pts.as_chunks::<3>().0.last() {
                let lx = last[0] as f32 / SCALE as f32;
                let ly = last[1] as f32 / SCALE as f32;
                laser.t1 = laser.t0.saturating_add(last[2].max(0) as u64);
                self.events
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(InteractionEvent::Point {
                        t: laser.t1,
                        x: lx,
                        y: ly,
                        label: None,
                    });
            }
            self.strokes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(laser);
        }
    }

    pub fn finish_session(&self) -> (Vec<InteractionEvent>, Vec<Stroke>) {
        self.active.store(false, Ordering::Release);
        self.selecting.store(false, Ordering::Release);
        *self
            .drag_start
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        *self
            .shift_press
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.finish_active_laser(None);
        if let Some(ink) = self
            .live_ink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            && ink.pts.len() >= 3
        {
            self.strokes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(ink);
        }
        let events = std::mem::take(
            &mut *self
                .events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        let strokes = std::mem::take(
            &mut *self
                .strokes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        (events, strokes)
    }
}

fn poll_clicks(
    telemetry: &SharedCaptureTelemetry,
    prev: &mut [bool; 2],
    now: [bool; 2],
    (x, y): (f64, f64),
    pure_shift: bool,
    on_scrawl: impl Fn(f64, f64) -> bool,
) {
    let was = std::mem::replace(prev, now);
    if telemetry.mouse_live.load(Ordering::Acquire) {
        return;
    }
    for (button, (&down, &before)) in now.iter().zip(&was).enumerate() {
        let left = button == 0;
        if down && !before && !on_scrawl(x, y) {
            if !pure_shift {
                telemetry.pointer_down(telemetry.elapsed_ms(), x, y);
            } else if left {
                telemetry.shift_down(telemetry.elapsed_ms(), x, y);
            }
        } else if !down && before {
            let shift_press = left && telemetry.shift_up(telemetry.elapsed_ms(), x, y);
            if !shift_press && !pure_shift {
                telemetry.pointer_up(telemetry.elapsed_ms(), x, y, !left);
            }
        }
    }
}

const CLICK_SLOP_PT: f32 = 12.0;
const PIN_MAX_PRESS_MS: u64 = 1_000;
const MIN_INK_SPAN_PT: f32 = 6.0;

fn is_pin_click(down: (u64, f64, f64), up: (u64, f64, f64)) -> bool {
    up.0.saturating_sub(down.0) <= PIN_MAX_PRESS_MS
        && ((up.1 - down.1).hypot(up.2 - down.2) as f32) < CLICK_SLOP_PT
}

fn ink_span_pt(stroke: &Stroke) -> f32 {
    let Some(first) = stroke.pts.get(0..2) else {
        return 0.0;
    };
    stroke
        .pts
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| {
            let dx = (p[0] - first[0]) as f32 / SCALE as f32 * stroke.fw as f32;
            let dy = (p[1] - first[1]) as f32 / SCALE as f32 * stroke.fh as f32;
            dx.hypot(dy)
        })
        .fold(0.0_f32, f32::max)
}

fn is_real_ink(stroke: &Stroke) -> bool {
    stroke.pts.len() >= 6 && ink_span_pt(stroke) >= MIN_INK_SPAN_PT
}

/// Whether live ink holds a real stroke drawn before `t_ms` (a ⇧-press at `t_ms` then isn't a pin).
fn drew_before(ink: &Stroke, t_ms: u64) -> bool {
    is_real_ink(&ink_before(ink.clone(), t_ms))
}

/// The part of live ink drawn before `t_ms`.
fn ink_before(mut stroke: Stroke, t_ms: u64) -> Stroke {
    let t0 = stroke.t0;
    let at = |p: &[i32; 3]| t0.saturating_add(p[2].max(0) as u64);
    let keep = stroke
        .pts
        .as_chunks::<3>()
        .0
        .iter()
        .take_while(|p| at(p) < t_ms)
        .count();
    stroke.pts.truncate(keep * 3);
    if let Some(last) = stroke.pts.as_chunks::<3>().0.last() {
        stroke.t1 = at(last);
    }
    stroke
}

fn same_target_frame(a: TargetFrame, b: TargetFrame) -> bool {
    (a.x - b.x).abs() <= 0.5
        && (a.y - b.y).abs() <= 0.5
        && (a.width - b.width).abs() <= 0.5
        && (a.height - b.height).abs() <= 0.5
}

fn track_shift_ink(
    telemetry: &SharedCaptureTelemetry,
    (x, y): (f64, f64),
    pure_shift: bool,
    prev_shift_ink: bool,
) {
    let inside_target = telemetry.contains_screen_point(x, y);
    if pure_shift && inside_target {
        telemetry.shift_held.store(true, Ordering::Release);
        let t_ms = telemetry.elapsed_ms();
        let (nx, ny, fw, fh) = telemetry.normalize_point(x, y);
        let sx = (nx * SCALE as f32).round() as i32;
        let sy = (ny * SCALE as f32).round() as i32;
        let mut guard = telemetry
            .live_ink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.is_none() {
            telemetry.window_log_request.store(true, Ordering::Release);
        }
        let stroke = guard.get_or_insert_with(|| Stroke {
            t0: t_ms,
            t1: t_ms,
            fw,
            fh,
            on: None,
            pin: None,
            laser: false,
            pts: Vec::new(),
        });
        let dt = t_ms.saturating_sub(stroke.t0) as i32;
        let should_push = stroke.pts.as_chunks::<3>().0.last().is_none_or(|prev| {
            let dx = (sx - prev[0]) as f32 / SCALE as f32 * fw as f32;
            let dy = (sy - prev[1]) as f32 / SCALE as f32 * fh as f32;
            dx.hypot(dy) >= 1.5
        });
        if should_push && stroke.pts.len() / 3 < 2_400 {
            stroke.pts.extend_from_slice(&[sx, sy, dt]);
            stroke.t1 = t_ms;
        }
    } else if !pure_shift && prev_shift_ink {
        telemetry.shift_held.store(false, Ordering::Release);
        if let Some(stroke) = telemetry
            .live_ink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            && is_real_ink(&stroke)
        {
            telemetry
                .strokes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(stroke);
            let _ = telemetry
                .ui_sender
                .try_send(LiveOverlayEvent::RequestSnapshot);
        }
    }
}

pub fn is_frontmost_terminal() -> bool {
    let Some((_pid, app, bundle_id)) = frontmost_app_info() else {
        return false;
    };
    let a = app.to_ascii_lowercase();
    let b = bundle_id.unwrap_or_default().to_ascii_lowercase();
    [
        "ghostty",
        "iterm",
        "terminal",
        "kitty",
        "wezterm",
        "alacritty",
        "warp",
        "windowsterminal",
        "conhost",
        "powershell",
        "pwsh",
        "cmd.exe",
        "mintty",
    ]
    .iter()
    .any(|needle| a.contains(needle) || b.contains(needle))
}

pub fn write_bundle_artifacts(
    out_dir: &Path,
    bundle: &crate::annotate::model::CaptureBundle,
    annotated_frames: &[(crate::annotate::model::Keyframe, RgbaFrame)],
    contact_sheet: &RgbaFrame,
    summary_md: &str,
) -> Result<()> {
    std::fs::create_dir_all(out_dir.join("frames"))
        .with_context(|| format!("could not create {}", out_dir.display()))?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(out_dir, std::fs::Permissions::from_mode(0o700));
    }

    thread::scope(|scope| -> Result<()> {
        let workers: Vec<_> = annotated_frames
            .iter()
            .map(|(kf, frame)| {
                scope.spawn(move || -> Result<()> {
                    crate::annotate::render::save_png(frame, &out_dir.join(&kf.file))
                })
            })
            .collect();
        crate::annotate::render::save_png(contact_sheet, &out_dir.join(&bundle.contact_sheet))?;
        for worker in workers {
            worker
                .join()
                .map_err(|_| anyhow::anyhow!("PNG encoding worker panicked"))??;
        }
        Ok(())
    })?;

    let manifest_bytes = serde_json::to_vec_pretty(bundle)?;
    std::fs::write(out_dir.join("scrawl.json"), manifest_bytes)?;
    std::fs::write(out_dir.join("summary.md"), summary_md.as_bytes())?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_stop_joins_worker_before_collecting_its_final_media() {
        let stopping = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(Mutex::new(CandidateBuffer::new(1 << 20)));
        let worker_stopping = Arc::clone(&stopping);
        let worker_frames = Arc::clone(&frames);
        let path = std::env::temp_dir().join(crate::annotate::bundle::make_bundle_id());
        let worker_path = path.clone();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !worker_stopping.load(Ordering::Acquire) && Instant::now() < deadline {
                thread::yield_now();
            }
            assert!(worker_stopping.load(Ordering::Acquire));
            worker_frames
                .lock()
                .unwrap()
                .push(RgbaFrame::new(42, 1, 1, vec![1, 2, 3, 255]), false);
            std::fs::write(&worker_path, b"finalized media").unwrap();
            Some(worker_path)
        });
        let session = ScreenCaptureSession {
            started_at: Instant::now(),
            stopping,
            urgent_snapshot: Arc::new(AtomicBool::new(false)),
            frames,
            video_offset_ms: Arc::new(AtomicU64::new(120)),
            sampler_thread: Some(worker),
            _recording: crate::annotate::retention::Recording::start(),
        };
        let recording = session.stop();
        let frames = recording.frames;
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].t_ms, 42);
        assert_eq!(frames[0].rgba, vec![1, 2, 3, 255]);
        assert_eq!(recording.video.as_ref(), Some(&path));
        assert_eq!(recording.video_offset_ms, 120);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn clicks_are_recorded_from_polled_buttons_while_the_mouse_monitor_is_missing() {
        let (sender, _receiver) = flume::unbounded();
        let telemetry = SharedCaptureTelemetry::new(sender);
        telemetry.reset_for_session(TargetFrame {
            x: 0.0,
            y: 0.0,
            width: 1728.0,
            height: 1117.0,
            scale: 2.0,
        });
        assert!(!telemetry.mouse_live.load(Ordering::Acquire));
        let mut prev = [false; 2];
        let not_ours = |_: f64, _: f64| false;
        let composer = (778.0, 1050.0);
        poll_clicks(
            &telemetry,
            &mut prev,
            [true, false],
            composer,
            true,
            not_ours,
        );
        poll_clicks(
            &telemetry,
            &mut prev,
            [false, false],
            composer,
            true,
            not_ours,
        );
        poll_clicks(
            &telemetry,
            &mut prev,
            [true, false],
            composer,
            false,
            not_ours,
        );
        poll_clicks(
            &telemetry,
            &mut prev,
            [true, false],
            composer,
            false,
            not_ours,
        );
        poll_clicks(
            &telemetry,
            &mut prev,
            [false, false],
            composer,
            false,
            not_ours,
        );
        let stop = (860.0, 20.0);
        poll_clicks(&telemetry, &mut prev, [true, false], stop, false, |_, _| {
            true
        });
        poll_clicks(
            &telemetry,
            &mut prev,
            [false, false],
            stop,
            false,
            |_, _| true,
        );
        telemetry.mouse_live.store(true, Ordering::Release);
        poll_clicks(
            &telemetry,
            &mut prev,
            [true, false],
            composer,
            false,
            not_ours,
        );
        poll_clicks(
            &telemetry,
            &mut prev,
            [false, false],
            composer,
            false,
            not_ours,
        );

        let (events, _) = telemetry.finish_session();
        assert_eq!(events.len(), 1, "{events:?}");
        let InteractionEvent::Click { x, y, button, .. } = &events[0] else {
            panic!("{events:?}");
        };
        assert!(
            (x - 0.45).abs() < 0.01 && (y - 0.94).abs() < 0.01,
            "{x}, {y}"
        );
        assert_eq!(button.as_deref(), Some("left"));
    }

    #[test]
    fn the_capture_shortcut_matches_its_aliases_and_a_custom_one() {
        let s = 1_u16;
        let default = ShortcutConfig {
            key_code: u32::from(s),
            carbon_modifiers: CARBON_OPTION_KEY | CARBON_SHIFT_KEY,
            label: "⌥⇧S".into(),
        };
        let hit = |cfg: &ShortcutConfig, key, (cmd, ctrl, alt, shift)| {
            is_capture_shortcut(cfg, s, key, cmd, ctrl, alt, shift)
        };
        assert!(hit(&default, s, (false, false, true, true)), "⌥⇧S");
        assert!(hit(&default, s, (true, false, true, true)), "⌥⇧⌘S");
        assert!(hit(&default, s, (false, true, true, true)), "⌃⌥⇧S");
        assert!(hit(&default, s, (true, true, true, false)), "⌃⌥⌘S");
        assert!(hit(&default, s, (true, true, true, true)), "hyper S");
        assert!(hit(&default, KEY_S, (false, false, true, true)), "ANSI S");
        assert!(!hit(&default, s, (false, false, false, true)), "⇧S");
        assert!(!hit(&default, s, (true, false, false, false)), "⌘S");
        assert!(!hit(&default, 2, (false, false, true, true)), "⌥⇧D");

        let custom = ShortcutConfig {
            key_code: 15,
            carbon_modifiers: CARBON_CONTROL_KEY | CARBON_OPTION_KEY,
            label: "⌃⌥R".into(),
        };
        assert!(hit(&custom, 15, (false, true, true, false)));
        assert!(!hit(&custom, 15, (true, true, true, false)));
        assert!(!hit(&custom, 15, (false, false, true, false)));
        assert!(!hit(&custom, s, (false, false, true, true)), "⌥⇧S");
        assert!(!hit(&custom, KEY_S, (true, true, true, true)), "hyper S");

        let v = 9_u16;
        let paste = ShortcutConfig {
            key_code: u32::from(v),
            carbon_modifiers: CARBON_OPTION_KEY | CARBON_SHIFT_KEY,
            label: "⌥⇧V".into(),
        };
        let paste_hit = |cfg: &ShortcutConfig, key, (cmd, ctrl, alt, shift)| {
            shortcut_matches(ShortcutKind::Paste, cfg, v, key, cmd, ctrl, alt, shift)
        };
        assert!(paste_hit(&paste, v, (false, false, true, true)), "⌥⇧V");
        assert!(paste_hit(&paste, v, (true, true, true, false)), "⌃⌥⌘V");
        assert!(
            !paste_hit(&paste, s, (false, false, true, true)),
            "⌥⇧S isn't paste"
        );
        let paste_custom = ShortcutConfig {
            key_code: 35,
            carbon_modifiers: CARBON_CONTROL_KEY | CARBON_OPTION_KEY,
            label: "⌃⌥P".into(),
        };
        assert!(paste_hit(&paste_custom, 35, (false, true, true, false)));
        assert!(!paste_hit(&paste_custom, v, (false, false, true, true)));
        assert!(
            paste.is_default_for(ShortcutKind::Paste)
                && !paste.is_default_for(ShortcutKind::Record)
        );

        assert_eq!(shortcut_parts("⌥⇧S"), ("⌥⇧", "S"));
        assert_eq!(shortcut_parts("⌃⌥⌘R"), ("⌃⌥⌘", "R"));
        assert_eq!(shortcut_parts("⌥⇧É"), ("⌥⇧", "É"));
        assert_eq!(shortcut_parts("Alt+Shift+S"), ("Alt+Shift", "S"));
        assert_eq!(shortcut_parts("Ctrl+Alt+Ä"), ("Ctrl+Alt", "Ä"));
        assert_eq!(shortcut_parts("Ctrl++"), ("Ctrl", "+"));
        assert_eq!(shortcut_parts(""), ("", ""));
    }

    #[test]
    fn recordings_use_the_display_under_the_cursor() {
        let displays = [[0.0, 0.0, 1728.0, 1117.0], [1728.0, -200.0, 2560.0, 1440.0]];
        assert_eq!(pick_display(&displays, Some([2000.0, 300.0]), None), 1);
        assert_eq!(pick_display(&displays, Some([100.0, 100.0]), None), 0);
        let focused = Some([1900.0, 0.0, 800.0, 600.0]);
        assert_eq!(pick_display(&displays, None, focused), 1);
        assert_eq!(pick_display(&displays, Some([-50.0, 0.0]), focused), 1);
        assert_eq!(pick_display(&displays, None, None), 0);
        let left = [[0.0, 0.0, 1728.0, 1117.0], [-1920.0, 37.0, 1920.0, 1080.0]];
        assert_eq!(pick_display(&left, Some([-5.0, 600.0]), None), 1);
        let above = [[0.0, 0.0, 1728.0, 1117.0], [0.0, -1440.0, 2560.0, 1440.0]];
        assert_eq!(pick_display(&above, Some([10.0, -1.0]), None), 1);
        assert_eq!(pick_display(&above, Some([10.0, 0.0]), None), 0);
    }

    #[test]
    fn input_on_an_external_display_normalizes_into_it() {
        let (sender, _receiver) = flume::unbounded();
        let telemetry = SharedCaptureTelemetry::new(sender);
        let left = [-1920.0, 37.0, 1920.0, 1080.0];
        let right = [1728.0, 0.0, 1920.0, 1080.0];
        let above = [0.0, -1440.0, 2560.0, 1440.0];
        let small_left = [-1280.0, -100.0, 1280.0, 800.0];
        let below = [0.0, 1117.0, 1920.0, 1080.0];
        let cases = [
            (left, 1.0, [-960.0, 577.0], (0.5, 0.5), (1920, 1080)),
            (right, 1.0, [2208.0, 270.0], (0.25, 0.25), (1920, 1080)),
            (below, 1.0, [480.0, 1387.0], (0.25, 0.25), (1920, 1080)),
            (above, 2.0, [640.0, -360.0], (0.25, 0.75), (2560, 1440)),
            (small_left, 2.0, [-320.0, 500.0], (0.75, 0.75), (2560, 1600)),
        ];
        for (rect, scale, point, expected, size) in cases {
            let frame = crate::annotate::screens::with_scale(rect, scale);
            let target = TargetInfo {
                mode: CaptureMode::Display,
                app: None,
                bundle_id: None,
                window_title: None,
                window_id: None,
                display_id: 2,
                frame,
            };
            assert_eq!(capture_size(&target, 2560.0), size, "{rect:?}");
            telemetry.reset_for_session(frame);
            assert!(telemetry.contains_screen_point(point[0], point[1]));
            assert!(!telemetry.contains_screen_point(864.0, 558.0));
            let (nx, ny, fw, fh) = telemetry.normalize_point(point[0], point[1]);
            assert_eq!((nx, ny), expected, "{rect:?}");
            assert_eq!((fw, fh), (rect[2] as u32, rect[3] as u32));
            telemetry.pointer_down(10, point[0], point[1]);
            telemetry.pointer_up(20, point[0], point[1], false);
            let (events, _) = telemetry.finish_session();
            assert!(
                matches!(events[..], [InteractionEvent::Click { x, y, .. }] if (x, y) == (nx, ny)),
                "{events:?}"
            );
        }
        let frame = crate::annotate::screens::with_scale([-1920.0, 37.0, 1920.0, 1080.0], 1.0);
        telemetry.reset_for_session(frame);
        let (nx, ny, _, _) = telemetry.normalize_point(-960.0, 577.0);
        assert_eq!((nx * 1920.0, ny * 1080.0), (960.0, 540.0));
        let frame = crate::annotate::screens::with_scale([0.0, -1440.0, 2560.0, 1440.0], 2.0);
        telemetry.reset_for_session(frame);
        let (nx, ny, _, _) = telemetry.normalize_point(640.0, -360.0);
        assert_eq!((nx * 2560.0, ny * 1440.0), (640.0, 1080.0));
        telemetry.finish_session();
    }

    fn session(width: f64, height: f64) -> Arc<SharedCaptureTelemetry> {
        let (sender, _receiver) = flume::unbounded();
        let telemetry = SharedCaptureTelemetry::new(sender);
        telemetry.reset_for_session(TargetFrame {
            x: 0.0,
            y: 0.0,
            width,
            height,
            scale: 2.0,
        });
        telemetry
    }

    #[test]
    fn a_long_still_shift_press_drops_no_pin() {
        let down = (1_000, 10.0, 10.0);
        let slop = f64::from(CLICK_SLOP_PT);
        assert!(is_pin_click(down, (1_150, 14.0, 13.0)));
        assert!(is_pin_click(down, (1_000 + PIN_MAX_PRESS_MS, 10.0, 10.0)));
        assert!(!is_pin_click(down, (2_500, 10.0, 10.0)));
        assert!(!is_pin_click(down, (1_100, 10.0, 10.0 + slop)));
    }

    /// The poller draws ink while ⇧ is held. A ⇧-click after ink was drawn in the same hold is
    /// part of the drawing: no pin, and the stroke carries on whole. A ⇧-click with nothing drawn
    /// first (only its own jitter) drops a pin and leaves no stroke.
    #[test]
    fn a_shift_click_mid_drawing_drops_no_pin() {
        let telemetry = session(1000.0, 1000.0);
        let ink = |pts: &[(i32, i32, i32)]| Stroke {
            t0: 0,
            t1: 0,
            fw: 1000,
            fh: 1000,
            on: None,
            pin: None,
            laser: false,
            pts: pts.iter().flat_map(|&(x, y, t)| [x, y, t]).collect(),
        };
        let press = telemetry.elapsed_ms() + 50;
        let p = press as i32;
        let s = SCALE;
        // A line drawn while ⇧ was held, then a click at its end.
        let line = ink(&[
            (s / 10, s / 2, 0),
            (s / 5, s / 2, p - 20),
            (s / 5 + s / 500, s / 2, p + 5),
        ]);
        *telemetry.live_ink.lock().unwrap() = Some(line.clone());
        telemetry.shift_down(press, 200.0, 500.0);
        assert!(telemetry.shift_up(press + 80, 201.0, 500.0));
        assert_eq!(telemetry.live_ink.lock().unwrap().as_ref(), Some(&line));
        // Letting go of ⇧ commits it as one stroke.
        let at = (200.0, 500.0);
        track_shift_ink(&telemetry, at, false, true);

        // Drawn long before the press still counts: the hold is a drawing.
        *telemetry.live_ink.lock().unwrap() = Some(ink(&[(s / 10, s / 10, 0), (s / 5, s / 10, 1)]));
        let t = telemetry.elapsed_ms() + 5_000;
        telemetry.shift_down(t, 200.0, 100.0);
        assert!(telemetry.shift_up(t + 60, 200.0, 100.0));
        *telemetry.live_ink.lock().unwrap() = None;

        // A ⇧-click with only its own few points of jitter is a pin.
        let jitter = ink(&[(s / 2, s / 2, 0), (s / 2 + 2, s / 2, 30)]);
        *telemetry.live_ink.lock().unwrap() = Some(jitter);
        let t = telemetry.elapsed_ms();
        telemetry.shift_down(t, 500.0, 500.0);
        assert!(telemetry.shift_up(t + 60, 500.0, 500.0));
        assert!(telemetry.live_ink.lock().unwrap().is_none());

        let (events, strokes) = telemetry.finish_session();
        assert!(events.is_empty(), "{events:?}");
        let kinds: Vec<_> = strokes.iter().map(|s| (s.pin, s.pts.len() / 3)).collect();
        assert_eq!(kinds, vec![(None, 3), (Some(1), 1)]);
    }

    #[test]
    fn shift_clicks_drop_pins_from_polled_buttons_too() {
        let telemetry = session(1000.0, 1000.0);
        let mut prev = [false; 2];
        for (at, on_scrawl) in [((250.0, 750.0), false), ((400.0, 400.0), true)] {
            for buttons in [[true, false], [false, false]] {
                poll_clicks(&telemetry, &mut prev, buttons, at, true, |_, _| on_scrawl);
            }
        }
        let (events, strokes) = telemetry.finish_session();
        assert!(events.is_empty(), "{events:?}");
        assert_eq!(strokes.len(), 1);
        assert_eq!(strokes[0].pin, Some(1));
        assert_eq!(strokes[0].normalized_points(), vec![[0.25, 0.75]]);
    }

    #[test]
    fn candidate_buffer_stays_bounded_and_keeps_the_whole_timeline() {
        let mut buffer = CandidateBuffer::new(1 << 20);
        let frame = |t| RgbaFrame::new(t, 32, 32, vec![0; 32 * 32 * 4]);
        let mut t = 0;
        let mut peak = 0;
        while t < 3 * 3_600_000 {
            buffer.push(frame(t), t % 10_000 == 0);
            peak = peak.max(buffer.len());
            t += buffer.interval().as_millis() as u64;
        }
        assert!(peak <= 256, "peak {peak}");
        assert!(buffer.interval() >= FRAME_SAMPLE_INTERVAL * 64);
        let frames = buffer.into_frames();
        assert_eq!(frames[0].t_ms, 0, "the start is kept");
        assert!(frames.windows(2).all(|w| w[0].t_ms < w[1].t_ms));
        let first_half = frames.iter().filter(|f| f.t_ms < 3 * 1_800_000).count();
        assert!(
            first_half * 3 > frames.len(),
            "{first_half} of {}",
            frames.len()
        );
    }

    #[test]
    fn candidate_buffer_keeps_urgent_frames_when_thinning() {
        let mut buffer = CandidateBuffer::new(40 * 4096);
        let frame = |t| RgbaFrame::new(t, 32, 32, vec![0; 32 * 32 * 4]);
        for t in 0..60 {
            buffer.push(frame(t * 220), t == 33);
        }
        assert!(buffer.len() < 60);
        assert_eq!(buffer.interval(), FRAME_SAMPLE_INTERVAL * 2);
        assert!(buffer.into_frames().iter().any(|f| f.t_ms == 33 * 220));
    }

    #[test]
    fn frames_that_may_show_a_system_prompt_are_dropped_but_never_the_first() {
        let mut buffer = CandidateBuffer::new(1 << 20);
        for t in [0, 220, 440, 660, 880] {
            buffer.push(RgbaFrame::new(t, 4, 4, vec![0; 64]), false);
        }
        buffer.drop_since(440);
        let times: Vec<u64> = buffer.into_frames().iter().map(|f| f.t_ms).collect();
        assert_eq!(times, [0, 220, 440]);
        let mut buffer = CandidateBuffer::new(1 << 20);
        buffer.push(RgbaFrame::new(500, 4, 4, vec![0; 64]), false);
        buffer.drop_since(0);
        assert_eq!(buffer.len(), 1);
    }

    #[test]
    fn candidates_are_small_and_recordings_keep_full_size() {
        let target = |mode, width, height| TargetInfo {
            mode,
            app: None,
            bundle_id: None,
            window_title: None,
            window_id: None,
            display_id: 1,
            frame: TargetFrame {
                x: 0.0,
                y: 0.0,
                width,
                height,
                scale: 2.0,
            },
        };
        let display = target(CaptureMode::Display, 1728.0, 1117.0);
        assert_eq!(capture_size(&display, CANDIDATE_EDGE), (480, 310));
        let region = target(CaptureMode::Region, 480.0, 320.0);
        assert_eq!(capture_size(&region, CANDIDATE_EDGE), (480, 320));
        assert_eq!(capture_size(&display, 2560.0), (2560, 1654));
        assert_eq!(capture_size(&region, 1280.0), (960, 640));

        let external = |mode, width, height| TargetInfo {
            frame: crate::annotate::screens::with_scale([-1920.0, 37.0, width, height], 1.0),
            ..target(mode, width, height)
        };
        let screen = external(CaptureMode::Display, 1920.0, 1080.0);
        assert_eq!(capture_size(&screen, 2560.0), (1920, 1080));
        assert_eq!(capture_size(&screen, CANDIDATE_EDGE), (480, 270));
        let region = external(CaptureMode::Region, 480.0, 320.0);
        assert_eq!(capture_size(&region, 1280.0), (480, 320));
        let above = TargetInfo {
            frame: crate::annotate::screens::with_scale([0.0, -1440.0, 2560.0, 1440.0], 2.0),
            ..target(CaptureMode::Display, 2560.0, 1440.0)
        };
        assert_eq!(capture_size(&above, 2560.0), (2560, 1440));
    }

    #[test]
    fn cached_window_hit_preserves_front_to_back_order_and_screen_coordinates() {
        let window = |id, x, y| TargetInfo {
            mode: CaptureMode::Window,
            app: Some(format!("App {id}")),
            bundle_id: None,
            window_title: Some(format!("Window {id}")),
            window_id: Some(id),
            display_id: 1,
            frame: TargetFrame {
                x,
                y,
                width: 200.0,
                height: 100.0,
                scale: 2.0,
            },
        };
        let windows = [window(1, -100.0, 20.0), window(2, 0.0, 0.0)];
        assert_eq!(
            selectable_window_at(&windows, 50.0, 50.0)
                .unwrap()
                .window_id,
            Some(1)
        );
        assert_eq!(
            selectable_window_at(&windows, 150.0, 50.0)
                .unwrap()
                .window_id,
            Some(2)
        );
        assert_eq!(
            selectable_window_at(&windows, -100.0, 20.0)
                .unwrap()
                .window_id,
            Some(1)
        );
        assert!(selectable_window_at(&windows, -101.0, 50.0).is_none());
        assert!(selectable_window_at(&[], 0.0, 0.0).is_none());
    }
}
