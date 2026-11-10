use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, BoxShadow, Context, Div, Entity, FocusHandle, FontWeight, IntoElement,
    KeyDownEvent, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Render,
    SharedString, TextRun, Window, WindowBackgroundAppearance, WindowBounds, WindowKind,
    WindowOptions, div, point, prelude::*, px, rgb, rgba,
};

mod macos;
use macos::{install_status_bar, reduce_motion};

use crate::annotate::audio::AudioRecorder;
use crate::annotate::capture::{
    LiveOverlayEvent, ScreenCaptureSession, SharedCaptureTelemetry, ShortcutConfig, annotate_log,
    current_mouse_screen, discover_target, list_selectable_windows, record_external_frontmost_app,
    region_target, selectable_window_at, shortcut_from_key_and_mods, start_global_input_tap,
};
use crate::annotate::laser_overlay::MetalLaserOverlay as LaserOverlay;
use crate::annotate::model::{CaptureMode, TargetInfo, format_clock};

const TEXT_COLOR: u32 = 0xf2f2f2;

fn tabular_nums() -> gpui::FontFeatures {
    gpui::FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

fn tabular_font(weight: FontWeight) -> gpui::Font {
    gpui::Font {
        family: ".SystemUIFont".into(),
        features: tabular_nums(),
        fallbacks: None,
        weight,
        style: gpui::FontStyle::Normal,
    }
}

fn measure_text(window: &Window, text: &str, size: f32, weight: FontWeight) -> f32 {
    let mut font = window.text_style().font();
    font.weight = weight;
    font.features = tabular_nums();
    let run = TextRun {
        len: text.len(),
        font,
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window.text_system().shape_line(
        SharedString::from(text.to_string()),
        px(size),
        &[run],
        None,
    );
    f32::from(line.width).ceil()
}

fn clock_template(clock: &str) -> String {
    clock
        .chars()
        .map(|c| if c.is_ascii_digit() { '0' } else { c })
        .collect()
}

/// Milliseconds since the UI thread first asked, the clock the capsule's chips reveal on.
fn reveal_clock_ms() -> u64 {
    thread_local! {
        static START: Instant = Instant::now();
    }
    START.with(|start| start.elapsed().as_millis() as u64)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MenuAction {
    ToggleCapture,
    ChooseTarget,
    OpenLatest,
}

static MENU: OnceLock<(flume::Sender<MenuAction>, flume::Receiver<MenuAction>)> = OnceLock::new();

fn menu_channel() -> &'static (flume::Sender<MenuAction>, flume::Receiver<MenuAction>) {
    MENU.get_or_init(flume::unbounded)
}

pub(crate) fn menu_receiver() -> flume::Receiver<MenuAction> {
    menu_channel().1.clone()
}

/// Queues `action` for the coordinator, which runs it on its next tick. Bespoke's tray, Annotate
/// page and Triage dialog send these.
pub fn menu(action: MenuAction) {
    let _ = menu_channel().0.try_send(action);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionMode {
    Area,
    Window,
    Screen,
}

impl SelectionMode {
    const ALL: [SelectionMode; 3] = [Self::Area, Self::Window, Self::Screen];

    fn key(self) -> &'static str {
        match self {
            Self::Area => "area",
            Self::Window => "window",
            Self::Screen => "screen",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.key() == key.trim())
    }

    fn last_used_path() -> Option<PathBuf> {
        crate::annotate::handoff::annotate_dir().map(|dir| dir.join("last-mode"))
    }

    fn load_last_used() -> Self {
        Self::last_used_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| Self::from_key(&text))
            .unwrap_or(Self::Area)
    }

    fn save_last_used(self) {
        if let Some(path) = Self::last_used_path() {
            let _ = std::fs::write(path, self.key());
        }
    }

    pub fn available() -> &'static [SelectionMode] {
        &Self::ALL
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Area => "Area",
            Self::Window => "Window",
            Self::Screen => "Screen",
        }
    }

    pub fn shows_record(self) -> bool {
        self == Self::Screen
    }

    fn from_digit(key: &str) -> Option<Self> {
        let index = key.parse::<usize>().ok()?.checked_sub(1)?;
        Self::available().get(index).copied()
    }

    fn cycled(self, forward: bool) -> Self {
        let modes = Self::available();
        let Some(index) = modes.iter().position(|m| *m == self) else {
            return if forward {
                modes[0]
            } else {
                modes[modes.len() - 1]
            };
        };
        let next = if forward {
            (index + 1) % modes.len()
        } else {
            (index + modes.len() - 1) % modes.len()
        };
        modes[next]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IslandPhase {
    Hidden,
    Selecting {
        mode: SelectionMode,
    },
    Recording,
    Processing,
    Completed {
        finished_at: Instant,
        frame_count: usize,
        duration_ms: u64,
        /// True when the capture reached the clipboard. False makes the capsule say it was saved
        /// but not copied.
        copied: bool,
    },
    /// The capsule says the capture could not be saved.
    Failed {
        finished_at: Instant,
    },
    Dismissing,
}

/// How long a finished or failed capsule stays on screen.
const FINISHED_CAPSULE_MS: u64 = 25_000;

/// The capsule's phase once a finish is over. `saved` is the keyframe count of a capture that
/// was written, and `copied` says whether it also reached the clipboard.
fn phase_after_finish(
    saved: Option<usize>,
    copied: bool,
    duration_ms: u64,
    now: Instant,
) -> IslandPhase {
    match saved {
        Some(frame_count) => IslandPhase::Completed {
            finished_at: now,
            frame_count,
            duration_ms,
            copied,
        },
        None => IslandPhase::Failed { finished_at: now },
    }
}

enum CaptureSource {
    Screen {
        session: ScreenCaptureSession,
        audio: Option<AudioRecorder>,
        windows: Option<crate::annotate::windows::WindowLogger>,
    },
}

struct ActiveRecording {
    started: Instant,
    target: TargetInfo,
    bundle_dir: PathBuf,
    source: CaptureSource,
    _metal_overlay: Option<LaserOverlay>,
}

impl ActiveRecording {
    fn elapsed_ms(&self) -> u64 {
        match &self.source {
            CaptureSource::Screen { session, .. } => session.elapsed_ms(),
        }
    }
}

pub struct Coordinator {
    telemetry: Arc<SharedCaptureTelemetry>,
    active_recording: Option<ActiveRecording>,
    picker_metal_overlay: Option<LaserOverlay>,
    native_pill: Option<crate::annotate::capsule_link::NativePill>,
    island_phase: IslandPhase,
    reveal: crate::annotate::hold::Reveal,
    reduced_motion: bool,
    last_elapsed_ms: u64,
    selectable_windows: Vec<TargetInfo>,
    selection_generation: u64,
    windows_loading: bool,
    selection_pointer: Option<[f32; 2]>,
    hovered_window: Option<TargetInfo>,
    animated_window_rect: Option<[f32; 4]>,
    selection_drag: Option<([f32; 2], [f32; 2])>,
    display_rect: [f32; 4],
    review_dir: Option<PathBuf>,
    completed_dir: Option<PathBuf>,
    last_mode: SelectionMode,
    grants: Option<crate::annotate::permissions::Grants>,
    gate: crate::annotate::permissions::Gate,
    switcher: crate::annotate::hold::Switcher,
    hold_mods: u64,
    hold_picker: bool,
    clock: Instant,
}

impl Coordinator {
    fn new(telemetry: Arc<SharedCaptureTelemetry>) -> Self {
        Self {
            telemetry,
            active_recording: None,
            picker_metal_overlay: None,
            native_pill: None,
            island_phase: IslandPhase::Hidden,
            reveal: crate::annotate::hold::Reveal::default(),
            reduced_motion: false,
            last_elapsed_ms: 0,
            selectable_windows: Vec::new(),
            selection_generation: 0,
            windows_loading: false,
            selection_pointer: None,
            hovered_window: None,
            animated_window_rect: None,
            selection_drag: None,
            display_rect: [0.0, 0.0, 1440.0, 900.0],
            review_dir: None,
            completed_dir: None,
            last_mode: SelectionMode::load_last_used(),
            grants: None,
            gate: crate::annotate::permissions::Gate::default(),
            switcher: crate::annotate::hold::Switcher::default(),
            hold_mods: 0,
            hold_picker: false,
            clock: Instant::now(),
        }
    }

    fn remember_mode(&mut self, mode: SelectionMode) {
        if self.last_mode != mode {
            self.last_mode = mode;
            mode.save_last_used();
        }
    }

    fn paste_target(&self) -> PathBuf {
        matches!(self.island_phase, IslandPhase::Completed { .. })
            .then(|| self.completed_dir.clone())
            .flatten()
            .unwrap_or_else(|| PathBuf::from("latest"))
    }

    pub fn paste_latest(&mut self, cx: &mut Context<Self>) {
        // Without Accessibility macOS drops the posted ⌘V (it can be turned off after it was
        // granted); bring the Annotate page instead.
        if !crate::ui::platform::accessibility_granted() {
            annotate_log("Paste shortcut without Accessibility; asking for it");
            self.gate.defer(vec![
                crate::annotate::permissions::Permission::Accessibility,
            ]);
            return;
        }
        let dir = self.paste_target();
        if matches!(self.island_phase, IslandPhase::Completed { .. }) {
            self.dismiss_island(cx);
        }
        std::thread::spawn(move || {
            if let Err(err) = crate::annotate::handoff::paste_bundle_image_and_text(&dir) {
                annotate_log(format!("paste_bundle_image_and_text error: {err}"));
            }
        });
    }

    pub fn is_recording(&self) -> bool {
        self.active_recording.is_some()
    }

    pub fn is_processing(&self) -> bool {
        matches!(self.island_phase, IslandPhase::Processing)
    }

    pub fn is_selecting(&self) -> bool {
        matches!(self.island_phase, IslandPhase::Selecting { .. })
    }

    pub fn open_settings(&mut self) {
        crate::annotate::open_settings(&[]);
    }

    fn note_grants(&mut self, grants: crate::annotate::permissions::Grants) {
        self.grants = Some(grants);
    }

    fn missing_to_record(&mut self) -> Vec<crate::annotate::permissions::Permission> {
        use crate::annotate::permissions::{Permission, missing_to_record};
        let screen = crate::annotate::capture::screen_capture_usable(false);
        let accessibility = crate::ui::platform::accessibility_granted();
        let missing = missing_to_record(self.grants.as_ref(), screen, accessibility);
        if missing.contains(&Permission::Microphone)
            && let Some(fresh) = crate::annotate::permissions::read_grants()
        {
            self.note_grants(fresh);
            return missing_to_record(self.grants.as_ref(), screen, accessibility);
        }
        missing
    }

    fn ready_to_record(&mut self) -> bool {
        let missing = self.missing_to_record();
        if missing.is_empty() {
            return true;
        }
        annotate_log(format!(
            "Not recording: {missing:?} off; opening the Annotate page"
        ));
        self.gate.defer(missing);
        false
    }

    pub fn tick_gate(&mut self) {
        if let Some(missing) = self
            .gate
            .tick(crate::annotate::capture::modifier_flags() != 0)
        {
            crate::annotate::open_settings(&missing);
        }
    }

    fn activity(&self) -> crate::annotate::hold::Activity {
        use crate::annotate::hold::Activity;
        if let Some(rec) = &self.active_recording {
            Activity::Recording {
                elapsed_ms: rec.started.elapsed().as_millis() as u64,
            }
        } else if self.is_processing() {
            Activity::Processing
        } else if self.is_selecting() && !self.switcher.is_switching() {
            Activity::Picker
        } else {
            Activity::Free
        }
    }

    fn clock_ms(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }

    pub fn press_shortcut(&mut self, cx: &mut Context<Self>) {
        let activity = self.activity();
        if self.gate.is_pending() {
            return;
        }
        if !self.switcher.is_held() && activity == crate::annotate::hold::Activity::Free {
            // No switcher, picker or overlay before the permissions are on: they sit above
            // macOS's permission prompts and take the clicks meant for them.
            let missing = self.missing_to_record();
            if !self.gate.press(missing) {
                annotate_log("Shortcut held back: a required permission is off");
                return;
            }
            self.hold_mods = crate::annotate::capture::modifier_flags();
            self.telemetry.take_escape();
        }
        if let Some(mode) = self.selection_mode() {
            self.switcher.follow(mode);
        }
        let action = self.switcher.press(self.clock_ms(), activity);
        self.apply_switcher(action, cx);
    }

    pub fn tick_switcher(&mut self, cx: &mut Context<Self>) {
        if !self.switcher.is_held() {
            return;
        }
        if self.switcher.is_switching() && !self.is_selecting() {
            self.switcher.reset();
            return;
        }
        let action = if self.telemetry.take_escape() {
            self.switcher.escape()
        } else {
            let mods = crate::annotate::capture::modifier_flags();
            let held = self.hold_mods != 0 && mods & self.hold_mods == self.hold_mods;
            self.switcher.tick(self.clock_ms(), held)
        };
        self.apply_switcher(action, cx);
    }

    fn apply_switcher(&mut self, action: crate::annotate::hold::Action, cx: &mut Context<Self>) {
        use crate::annotate::hold::Action;
        if action != Action::None {
            annotate_log(format!("switcher -> {action:?}"));
        }
        match action {
            Action::None => {}
            Action::RecordScreen => self.record_screen_now(cx),
            Action::Open(mode) => {
                self.open_picker(Some(SelectionMode::Screen), true, cx);
                if mode != SelectionMode::Screen {
                    self.show_switcher_mode(mode, cx);
                }
            }
            Action::Show(mode) => self.show_switcher_mode(mode, cx),
            Action::Commit(mode) => self.commit_switcher(mode, cx),
            Action::Cancel => {
                if self.is_selecting() {
                    self.cancel_recording(cx);
                }
            }
            Action::RecordSelected => self.record_selected_mode(cx),
            Action::Finish => self.finish_recording(cx),
        }
    }

    pub fn record_or_finish(&mut self, cx: &mut Context<Self>) {
        if self.is_recording() {
            self.finish_recording(cx);
        } else if self.is_selecting() {
            self.record_selected_mode(cx);
        } else if !self.is_processing() {
            self.record_screen_now(cx);
        }
    }

    fn show_switcher_mode(&mut self, mode: SelectionMode, cx: &mut Context<Self>) {
        self.set_selection_mode(mode, cx);
    }

    fn commit_switcher(&mut self, mode: SelectionMode, cx: &mut Context<Self>) {
        match self.selection_mode().unwrap_or(mode) {
            SelectionMode::Screen => self.start_recording_full_screen(cx),
            SelectionMode::Window => {
                if self.hovered_window.is_none() {
                    if self.windows_loading {
                        self.selectable_windows = list_selectable_windows();
                        self.windows_loading = false;
                    }
                    self.hovered_window = current_mouse_screen()
                        .and_then(|[x, y]| {
                            selectable_window_at(
                                &self.selectable_windows,
                                f64::from(x),
                                f64::from(y),
                            )
                        })
                        .cloned();
                }
                match self.hovered_window.clone() {
                    Some(target) => self.start_recording_with_target(target, cx),
                    None => self.start_recording_full_screen(cx),
                }
            }
            SelectionMode::Area => match self
                .selection_drag
                .and_then(|(a, b)| area_rect(a, b, self.display_rect))
            {
                Some([x, y, w, h]) => {
                    let target = region_target(x, y, w, h, &self.selectable_windows);
                    self.start_recording_with_target(target, cx);
                }
                None => self.cancel_recording(cx),
            },
        }
    }

    pub fn record_screen_now(&mut self, cx: &mut Context<Self>) {
        self.start_recording_with_target(discover_target(None), cx);
        if self.is_recording() {
            self.show_reveal(cx);
        }
    }

    fn reveal_activity(&self) -> usize {
        use std::sync::PoisonError;
        let t = &self.telemetry;
        t.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
            + t.strokes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .len()
            + usize::from(
                t.live_ink
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .is_some(),
            )
    }

    fn show_reveal(&mut self, cx: &mut Context<Self>) {
        self.reduced_motion = reduce_motion();
        let activity = self.reveal_activity();
        self.reveal.show(reveal_clock_ms(), activity);
        self.sync_native_pill();
        cx.notify();
    }

    fn fold_reveal(&mut self) {
        if self.reveal.is_open() {
            self.reveal.close();
            self.play_fold();
        }
    }

    fn play_fold(&mut self) {
        self.sync_native_pill();
    }

    pub fn switch_recording(&mut self, mode: SelectionMode, cx: &mut Context<Self>) {
        if !self.is_recording() {
            return;
        }
        if mode == SelectionMode::Screen {
            self.fold_reveal();
            cx.notify();
            return;
        }
        annotate_log(format!("switch_recording -> {mode:?}"));
        let Some((_, overlay)) = self.discard_recording() else {
            return;
        };
        self.picker_metal_overlay = overlay;
        self.open_picker(Some(mode), false, cx);
    }

    fn discard_recording(&mut self) -> Option<(TargetInfo, Option<LaserOverlay>)> {
        let rec = self.active_recording.take()?;
        let _ = self.telemetry.finish_session();
        let CaptureSource::Screen { session, audio, .. } = rec.source;
        drop(audio);
        // Stopping waits for the video to finish writing, which would hold up the UI, and the
        // bundle is deleted either way.
        let bundle_dir = rec.bundle_dir;
        std::thread::spawn(move || {
            let _ = session.stop();
            let _ = std::fs::remove_dir_all(bundle_dir);
        });
        Some((rec.target, rec._metal_overlay))
    }

    pub fn is_island_visible(&self) -> bool {
        !matches!(self.island_phase, IslandPhase::Hidden)
    }

    pub fn shortcut_label(&self) -> String {
        self.telemetry
            .shortcut
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .label
            .clone()
    }

    fn sync_picker_overlay(&self) {
        let rect = match self.island_phase {
            IslandPhase::Selecting {
                mode: SelectionMode::Area,
            } => self.selection_drag.and_then(|(a, b)| {
                let [a, b] = [a, b].map(|p| clamp_point(p, self.display_rect));
                let w = (a[0] - b[0]).abs();
                let h = (a[1] - b[1]).abs();
                (w >= 1.0 && h >= 1.0).then_some(([a[0].min(b[0]), a[1].min(b[1]), w, h], 1))
            }),
            IslandPhase::Selecting {
                mode: SelectionMode::Window,
            } => self.hovered_window.as_ref().map(|t| {
                (
                    [
                        t.frame.x as f32,
                        t.frame.y as f32,
                        t.frame.width as f32,
                        t.frame.height as f32,
                    ],
                    2,
                )
            }),
            IslandPhase::Selecting {
                mode: SelectionMode::Screen,
            } => Some((self.display_rect, 2)),
            _ => None,
        };
        self.telemetry.set_picker_rect(rect);
    }

    pub fn set_selection_mode(&mut self, mode: SelectionMode, cx: &mut Context<Self>) {
        if let IslandPhase::Selecting { mode: _prev_mode } = self.island_phase {
            self.island_phase = IslandPhase::Selecting { mode };
            self.selection_drag = None;
            if let Some(pos) = current_mouse_screen().or(self.selection_pointer) {
                self.selection_pointer = Some(pos);
                if mode == SelectionMode::Window {
                    self.hovered_window = selectable_window_at(
                        &self.selectable_windows,
                        f64::from(pos[0]),
                        f64::from(pos[1]),
                    )
                    .cloned();
                    self.animated_window_rect = self.hovered_window.as_ref().map(|w| {
                        [
                            w.frame.x as f32,
                            w.frame.y as f32,
                            w.frame.width as f32,
                            w.frame.height as f32,
                        ]
                    });
                } else {
                    self.animated_window_rect = None;
                }
            }
            self.sync_picker_overlay();
            cx.notify();
        }
    }

    pub fn toggle_selection_mode(&mut self, cx: &mut Context<Self>) {
        if let IslandPhase::Selecting { mode } = self.island_phase {
            let next = match mode {
                SelectionMode::Area => SelectionMode::Window,
                SelectionMode::Window | SelectionMode::Screen => SelectionMode::Area,
            };
            self.set_selection_mode(next, cx);
        }
    }

    fn selection_mode(&self) -> Option<SelectionMode> {
        match self.island_phase {
            IslandPhase::Selecting { mode } => Some(mode),
            _ => None,
        }
    }

    pub fn choose_selection_mode(&mut self, mode: SelectionMode, cx: &mut Context<Self>) {
        if self.selection_mode().is_none() {
            return;
        }
        self.set_selection_mode(mode, cx);
    }

    pub fn click_selection_mode(&mut self, mode: SelectionMode, cx: &mut Context<Self>) {
        if self.selection_mode() == Some(mode) && mode.shows_record() {
            self.record_selected_mode(cx);
        } else {
            self.choose_selection_mode(mode, cx);
        }
    }

    pub fn record_selected_mode(&mut self, cx: &mut Context<Self>) {
        if self.selection_mode().is_some() {
            self.start_recording_full_screen(cx);
        }
    }

    /// ⌘Q and ⌘, in Scrawl's own windows: a menu bar (accessory) app has no main menu to
    /// carry them.
    pub fn handle_app_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let mods = keystroke.modifiers;
        if !mods.secondary() || (mods.control && mods.platform) || mods.alt || mods.shift {
            return false;
        }
        match keystroke.key.as_str() {
            "q" => cx.quit(),
            "," => {
                if self.is_selecting() {
                    self.switcher.reset();
                    self.cancel_recording(cx);
                }
                self.open_settings();
            }
            _ => return false,
        }
        true
    }

    pub fn handle_selection_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let Some(mode) = self.selection_mode() else {
            return false;
        };
        let mods = keystroke.modifiers;
        if keystroke.key == "escape" {
            self.switcher.reset();
            self.cancel_recording(cx);
            return true;
        }
        if self.handle_app_key(keystroke, cx) {
            return true;
        }
        if let Some(cfg) = shortcut_from_key_and_mods(
            &keystroke.key,
            mods.platform,
            mods.control,
            mods.alt,
            mods.shift,
        ) && self.telemetry.matches_shortcut(
            cfg.key_code as u16,
            mods.platform,
            mods.control,
            mods.alt,
            mods.shift,
        ) {
            self.telemetry.emit_capture_shortcut();
            return true;
        }
        if mods.platform || mods.control || mods.alt {
            return false;
        }
        let label = self.shortcut_label();
        let record_key = crate::annotate::capture::shortcut_parts(&label).1;
        match keystroke.key.as_str() {
            "space" | " " => self.toggle_selection_mode(cx),
            "escape" => self.cancel_recording(cx),
            "enter" => self.record_selected_mode(cx),
            key if key.eq_ignore_ascii_case(record_key) => self.record_selected_mode(cx),
            "tab" => self.choose_selection_mode(mode.cycled(!mods.shift), cx),
            "right" => self.choose_selection_mode(mode.cycled(true), cx),
            "left" => self.choose_selection_mode(mode.cycled(false), cx),
            key => match SelectionMode::from_digit(key) {
                Some(next) => self.choose_selection_mode(next, cx),
                None => return false,
            },
        }
        true
    }

    pub fn undo_stroke(&mut self, cx: &mut Context<Self>) {
        let mut strokes = self
            .telemetry
            .strokes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(idx) = strokes.iter().rposition(|s| !s.laser) {
            strokes.remove(idx);
        }
        drop(strokes);
        cx.notify();
    }

    pub fn request_immediate_frame(&self) {
        if let Some(ActiveRecording {
            source: CaptureSource::Screen { session, .. },
            ..
        }) = &self.active_recording
        {
            session.request_immediate_frame();
        }
    }

    fn set_island_phase(&mut self, phase: IslandPhase) {
        let changed = std::mem::discriminant(&self.island_phase) != std::mem::discriminant(&phase);
        if changed {
            self.reveal.close();
        }
        let appearing = matches!(
            self.island_phase,
            IslandPhase::Hidden | IslandPhase::Dismissing
        );
        self.island_phase = phase;
        let place = changed
            && (appearing
                || matches!(
                    phase,
                    IslandPhase::Selecting { .. } | IslandPhase::Recording
                ));
        if !matches!(phase, IslandPhase::Hidden) {
            self.ensure_native_pill();
        }
        if place {
            self.place_capsule();
        }
        self.sync_native_pill();
    }

    fn place_capsule(&self) {
        let centre = |[x, y, w, h]: [f64; 4]| [x + w / 2.0, y + h / 2.0];
        let anchor = match self.island_phase {
            IslandPhase::Recording => self.active_recording.as_ref().map(|rec| {
                let f = rec.target.frame;
                centre([f.x, f.y, f.width, f.height])
            }),
            IslandPhase::Selecting { .. } => Some(centre(self.display_rect.map(f64::from))),
            _ => None,
        };
        let Some(point) = anchor.or_else(crate::annotate::screens::pointer) else {
            return;
        };
        if let Some(ref pill) = self.native_pill {
            pill.place_on(point);
        }
    }

    fn follow_pointer_display(&mut self, pointer: [f32; 2], cx: &mut Context<Self>) {
        if self.selection_drag.is_some() {
            return;
        }
        let p = pointer.map(f64::from);
        let current = self.display_rect.map(f64::from);
        if crate::annotate::screens::contains(current, p) {
            return;
        }
        let screens = crate::annotate::screens::current();
        let Some(i) = crate::annotate::screens::index_at(&screens, p) else {
            return;
        };
        let frame = screens[i].frame;
        if (0..4).all(|k| (frame[k] - current[k]).abs() < 1.0) {
            return;
        }
        annotate_log(format!("picker follows the pointer to display {frame:?}"));
        self.display_rect = frame.map(|v| v as f32);
        if let Some(ref mut overlay) = self.picker_metal_overlay {
            overlay.move_to(frame);
        }
        macos::move_picker_window(frame, cx);
        self.place_capsule();
        self.sync_picker_overlay();
        cx.notify();
    }

    fn picker_mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        let [sx, sy] = current_mouse_screen()
            .unwrap_or([f32::from(event.position.x), f32::from(event.position.y)]);
        if self.hold_picker {
            if self.selection_mode() == Some(SelectionMode::Area)
                && let Some((start, _)) = self.selection_drag
            {
                self.selection_drag = Some((start, [sx, sy]));
            } else {
                self.selection_drag = None;
            }
            self.sync_picker_overlay();
            cx.notify();
            return;
        }
        if let Some((start, _)) = self.selection_drag.take() {
            if let Some([x, y, w, h]) = area_rect(start, [sx, sy], self.display_rect) {
                let target = region_target(x, y, w, h, &self.selectable_windows);
                self.start_recording_with_target(target, cx);
            } else {
                self.sync_picker_overlay();
                cx.notify();
            }
        }
    }

    pub(crate) fn ensure_native_pill(&mut self) {
        if self.native_pill.is_some() {
            return;
        }
        match crate::annotate::capsule_link::NativePill::open(Arc::clone(
            &self.telemetry.excluded_window_ids,
        )) {
            Ok(pill) => {
                self.native_pill = Some(pill);
            }
            Err(err) => {
                annotate_log(format!("NativePill::open error: {err}"));
            }
        }
    }

    fn pill_snapshot(&self) -> crate::annotate::capsule_link::PillSnapshot {
        use crate::annotate::capsule_link::{PillSnapshot, Pres};
        let recording_ms = self
            .active_recording
            .as_ref()
            .map(ActiveRecording::elapsed_ms);
        let elapsed_ms = recording_ms.unwrap_or(self.last_elapsed_ms);
        let clock = format_clock(elapsed_ms);
        let stroke_count = self
            .telemetry
            .strokes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|s| !s.laser)
            .count();

        let pres = match self.island_phase {
            IslandPhase::Hidden | IslandPhase::Dismissing => Pres::Hidden,
            IslandPhase::Selecting { mode } => Pres::Selecting {
                mode,
                hold: self.hold_picker,
            },
            IslandPhase::Recording => Pres::Recording {
                chips: self.reveal.is_open(),
                stroke_count,
            },
            IslandPhase::Processing => Pres::Processing,
            IslandPhase::Completed { copied, .. } => Pres::Completed { copied },
            IslandPhase::Failed { .. } => Pres::Failed,
        };
        PillSnapshot {
            pres,
            clock,
            reduced_motion: self.reduced_motion,
            shortcut: self.shortcut_label(),
        }
    }

    pub(crate) fn sync_native_pill(&mut self) {
        if self.native_pill.is_some() {
            let snap = self.pill_snapshot();
            if let Some(ref pill) = self.native_pill {
                pill.sync(snap);
            }
        }
    }

    fn apply_pill_action(
        &mut self,
        action: crate::annotate::capsule_link::PillAction,
        cx: &mut Context<Self>,
    ) {
        use crate::annotate::capsule_link::PillAction;
        match action {
            PillAction::SelectMode(mode) => self.click_selection_mode(mode, cx),
            PillAction::SwitchRecording(mode) => self.switch_recording(mode, cx),
            PillAction::RecordSelected => self.record_selected_mode(cx),
            PillAction::Undo => self.undo_stroke(cx),
            PillAction::Cancel => self.cancel_recording(cx),
            PillAction::Finish => self.finish_recording(cx),
            PillAction::Paste(target) => self.paste_completed_bundle(target, cx),
            PillAction::Inspect => self.inspect_completed_bundle(cx),
            PillAction::DismissCompleted => self.dismiss_completed_bundle(cx),
        }
    }

    pub fn dismiss_island(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.island_phase,
            IslandPhase::Hidden | IslandPhase::Dismissing
        ) {
            return;
        }
        self.telemetry
            .selecting
            .store(false, std::sync::atomic::Ordering::Release);
        self.telemetry.set_picker_rect(None);
        self.picker_metal_overlay = None;
        self.selection_drag = None;
        self.hovered_window = None;
        self.animated_window_rect = None;
        self.set_island_phase(IslandPhase::Dismissing);
        cx.defer(close_picker_window);
        cx.notify();
    }

    pub fn dismiss_completed_bundle(&mut self, cx: &mut Context<Self>) {
        self.dismiss_island(cx);
    }

    pub fn inspect_completed_bundle(&mut self, cx: &mut Context<Self>) {
        self.set_island_phase(IslandPhase::Hidden);
        let entity = cx.entity().clone();
        cx.defer(move |cx| {
            close_picker_window(cx);
            inspect_review_dir(&entity, cx);
        });
        cx.notify();
    }

    pub fn paste_completed_bundle(
        &mut self,
        click_target: Option<[f32; 2]>,
        cx: &mut Context<Self>,
    ) {
        let dir = self
            .completed_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("latest"));
        self.dismiss_island(cx);
        if !crate::ui::platform::accessibility_granted() {
            annotate_log("Paste chip without Accessibility; asking for it");
            self.gate.defer(vec![
                crate::annotate::permissions::Permission::Accessibility,
            ]);
            return;
        }
        std::thread::spawn(move || {
            if let Err(err) = crate::annotate::handoff::paste_bundle_at_target(&dir, click_target) {
                annotate_log(format!("paste_bundle_at_target error: {err}"));
            }
        });
    }

    pub fn island_tick(&mut self, cx: &mut Context<Self>) {
        if let Some(actions) = self.native_pill.as_ref().map(|p| p.drain_actions()) {
            for action in actions {
                self.apply_pill_action(action, cx);
            }
        }
        if let IslandPhase::Completed { finished_at, .. } | IslandPhase::Failed { finished_at } =
            self.island_phase
            && finished_at.elapsed() >= Duration::from_millis(FINISHED_CAPSULE_MS)
        {
            self.set_island_phase(IslandPhase::Dismissing);
        }

        if let IslandPhase::Selecting { mode } = self.island_phase {
            if let Some(pos) = current_mouse_screen() {
                self.follow_pointer_display(pos, cx);
                self.selection_pointer = Some(pos);
                if mode == SelectionMode::Area
                    && let Some((start, _)) = self.selection_drag
                {
                    self.selection_drag = Some((start, pos));
                }
                if mode == SelectionMode::Window && !self.windows_loading {
                    self.hovered_window = selectable_window_at(
                        &self.selectable_windows,
                        f64::from(pos[0]),
                        f64::from(pos[1]),
                    )
                    .cloned();
                }
            }
            if mode == SelectionMode::Window
                && let Some(ref win) = self.hovered_window
            {
                let target_rect = [
                    win.frame.x as f32,
                    win.frame.y as f32,
                    win.frame.width as f32,
                    win.frame.height as f32,
                ];
                self.animated_window_rect = Some(target_rect);
            } else {
                self.animated_window_rect = None;
            }
            self.sync_picker_overlay();
        }

        if self.reveal.is_open() {
            let activity = self.reveal_activity();
            let on_pill = self
                .native_pill
                .as_ref()
                .is_some_and(|p| p.pointer_on_pill());
            if self.reveal.tick(reveal_clock_ms(), on_pill, activity) {
                self.play_fold();
            }
        }
        self.sync_native_pill();

        let dismissed = self
            .native_pill
            .as_ref()
            .is_none_or(|pill| pill.is_fully_hidden());
        if matches!(self.island_phase, IslandPhase::Dismissing) && dismissed {
            self.set_island_phase(IslandPhase::Hidden);
            cx.defer(close_picker_window);
        }
    }

    pub fn begin_target_selection(&mut self, cx: &mut Context<Self>) {
        if self.ready_to_record() {
            self.open_picker(None, false, cx);
        }
    }

    fn open_picker(&mut self, mode: Option<SelectionMode>, hold: bool, cx: &mut Context<Self>) {
        if self.is_recording() || self.is_processing() {
            return;
        }
        annotate_log(format!(
            "open_picker invoked (mode: {mode:?}, hold: {hold})"
        ));
        self.hold_picker = hold;
        self.selectable_windows.clear();
        self.hovered_window = None;
        self.animated_window_rect = None;
        self.selection_pointer = current_mouse_screen();
        let screens = crate::annotate::screens::current();
        if let Some(i) = crate::annotate::screens::pointer()
            .and_then(|p| crate::annotate::screens::index_at(&screens, p))
        {
            self.display_rect = screens[i].frame.map(|v| v as f32);
        }
        if let Some(ref mut overlay) = self.picker_metal_overlay {
            overlay.move_to(self.display_rect.map(f64::from));
        }
        self.selection_generation += 1;
        self.windows_loading = true;
        let generation = self.selection_generation;
        cx.spawn(async move |entity, cx| {
            let windows = cx
                .background_executor()
                .spawn(async { list_selectable_windows() })
                .await;
            let _ = entity.update(cx, |coord, cx| {
                if coord.selection_generation != generation || !coord.is_selecting() {
                    return;
                }
                coord.windows_loading = false;
                let ptr = current_mouse_screen().or(coord.selection_pointer);
                coord.selection_pointer = ptr;
                coord.hovered_window = ptr
                    .and_then(|[x, y]| selectable_window_at(&windows, f64::from(x), f64::from(y)))
                    .cloned();
                if matches!(
                    coord.island_phase,
                    IslandPhase::Selecting {
                        mode: SelectionMode::Window
                    }
                ) {
                    coord.animated_window_rect = coord.hovered_window.as_ref().map(|w| {
                        [
                            w.frame.x as f32,
                            w.frame.y as f32,
                            w.frame.width as f32,
                            w.frame.height as f32,
                        ]
                    });
                }
                coord.selectable_windows = windows;
                coord.sync_picker_overlay();
                cx.notify();
            });
        })
        .detach();
        self.selection_drag = None;
        if matches!(self.island_phase, IslandPhase::Dismissing) {
            self.island_phase = IslandPhase::Hidden;
        }
        let opening = mode.unwrap_or(self.last_mode);
        self.set_island_phase(IslandPhase::Selecting { mode: opening });
        self.telemetry.begin_picker_session();
        self.sync_picker_overlay();
        if self.picker_metal_overlay.is_none() {
            let [x, y, w, h] = self.display_rect.map(f64::from);
            let overlay = macos::open_overlay(Arc::clone(&self.telemetry), Some([x, y, w, h]), cx)
                .map_err(|error| annotate_log(format!("Metal overlay failed: {error}")))
                .ok();
            if let Some(ref ov) = overlay {
                self.telemetry
                    .excluded_window_ids
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(ov.window_number());
                self.telemetry
                    .passthrough_window_ids
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(ov.window_number());
            }
            self.picker_metal_overlay = overlay;
        }

        let entity = cx.entity().clone();
        cx.defer(move |cx| open_picker_windows(&entity, cx));
        cx.notify();
    }

    pub fn start_recording_full_screen(&mut self, cx: &mut Context<Self>) {
        let [x, y, w, h] = self.display_rect;
        let centre = [f64::from(x + w / 2.0), f64::from(y + h / 2.0)];
        let target = crate::annotate::capture::discover_target_at(None, Some(centre));
        self.start_recording_with_target(target, cx);
    }

    pub fn start_recording_with_target(&mut self, target: TargetInfo, cx: &mut Context<Self>) {
        if self.is_recording() || self.is_processing() {
            return;
        }
        if !self.ready_to_record() {
            self.cancel_recording(cx);
            return;
        }
        let Some(captures) = crate::annotate::handoff::default_captures_dir() else {
            annotate_log("No captures folder");
            self.cancel_recording(cx);
            return;
        };
        annotate_log(format!(
            "start_recording_with_target invoked: mode={:?} frame={:?}",
            target.mode, target.frame
        ));
        let quick = !self.is_selecting();
        self.telemetry.start_glow.store(
            quick && target.mode == CaptureMode::Display,
            std::sync::atomic::Ordering::Release,
        );
        if let IslandPhase::Selecting { mode } = self.island_phase
            && !self.hold_picker
        {
            self.remember_mode(mode);
        }
        self.selection_drag = None;
        self.hovered_window = None;
        self.animated_window_rect = None;
        let id = crate::annotate::bundle::make_bundle_id();
        let bundle_dir = captures.join(format!("{id}.scrawl"));
        let _ = std::fs::create_dir_all(&bundle_dir);
        let video_path = bundle_dir.join("recording.mp4");

        self.telemetry.set_capture_mode(target.mode);
        self.telemetry.reset_for_session(target.frame);
        let f = target.frame;
        let target_rect = [f.x, f.y, f.width, f.height];
        if let Some(ref pill) = self.native_pill {
            pill.place_on([f.x + f.width / 2.0, f.y + f.height / 2.0]);
        }
        let metal_overlay = self.picker_metal_overlay.take().map(|mut overlay| {
            overlay.move_to(target_rect);
            overlay
        });
        let metal_overlay = metal_overlay.or_else(|| {
            let ov = macos::open_overlay(Arc::clone(&self.telemetry), Some(target_rect), cx)
                .map_err(|error| annotate_log(format!("Metal overlay failed: {error}")))
                .ok();
            if let Some(ref o) = ov {
                self.telemetry
                    .excluded_window_ids
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(o.window_number());
                self.telemetry
                    .passthrough_window_ids
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(o.window_number());
            }
            ov
        });
        let started_at = self.telemetry.session_start();
        let audio_recorder = AudioRecorder::start().ok();
        let screen_session = ScreenCaptureSession::start(
            target.clone(),
            Some(video_path),
            Arc::clone(&self.telemetry.excluded_window_ids),
            started_at,
        );
        let window_log = (target.mode == CaptureMode::Display).then(|| {
            crate::annotate::windows::WindowLogger::start(
                target.frame,
                started_at,
                Arc::clone(&self.telemetry.window_log_request),
            )
        });
        self.active_recording = Some(ActiveRecording {
            started: Instant::now(),
            target,
            bundle_dir,
            source: CaptureSource::Screen {
                session: screen_session,
                audio: audio_recorder,
                windows: window_log,
            },
            _metal_overlay: metal_overlay,
        });
        self.present_recording_island(cx);
    }

    fn present_recording_island(&mut self, cx: &mut Context<Self>) {
        self.set_island_phase(IslandPhase::Recording);
        let entity = cx.entity().clone();
        cx.defer(move |cx| {
            close_picker_window(cx);
            ensure_pill_open(&entity, cx);
        });
        cx.notify();
    }

    pub fn cancel_recording(&mut self, cx: &mut Context<Self>) {
        annotate_log("cancel_recording invoked");
        crate::annotate::capture_cancelled();
        self.discard_recording();
        self.dismiss_island(cx);
    }

    pub fn finish_recording(&mut self, cx: &mut Context<Self>) {
        annotate_log("finish_recording invoked");
        let Some(rec) = self.active_recording.take() else {
            return;
        };
        let (events, strokes) = self.telemetry.finish_session();
        let bundle_dir = rec.bundle_dir;
        let target = rec.target;
        let source = rec.source;
        drop(rec._metal_overlay);
        self.set_island_phase(IslandPhase::Processing);
        cx.notify();

        cx.spawn(async move |entity, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let CaptureSource::Screen {
                        session: screen_session,
                        audio: audio_recorder,
                        windows: window_log,
                    } = source;
                    let stop_started = Instant::now();
                    let started_at = screen_session.started_at();
                    let (recording, recorded_audio, mut activity) = std::thread::scope(|s| {
                        let screen_handle = s.spawn(|| screen_session.stop());
                        let win_handle = s.spawn(|| {
                            window_log
                                .map(crate::annotate::windows::WindowLogger::stop)
                                .unwrap_or_default()
                        });
                        let audio = audio_recorder.and_then(|r| {
                            r.stop(started_at)
                                .inspect_err(|error| {
                                    annotate_log(format!("No narration: {error:#}"));
                                })
                                .ok()
                        });
                        (
                            screen_handle.join().unwrap(),
                            audio,
                            win_handle.join().unwrap_or_default(),
                        )
                    });
                    activity.stop_started = Some(stop_started);
                    activity.stop_ms = Some(stop_started.elapsed().as_millis() as u64);
                    let duration_ms = recording.duration_ms;
                    let finalized = crate::annotate::bundle::finalize_capture_bundle(
                        &bundle_dir,
                        target,
                        recording,
                        strokes,
                        events,
                        recorded_audio,
                        None,
                        activity,
                    );
                    match &finalized {
                        Ok((dir, bundle, _)) => {
                            crate::annotate::thumbnail::write_finished(dir);
                            crate::annotate::capture_finished(
                                crate::annotate::handoff::Finished::of(dir, bundle),
                            );
                        }
                        Err(error) => {
                            annotate_log(format!("Capture could not be saved: {error:#}"));
                            crate::annotate::capture_cancelled();
                        }
                    }
                    (finalized, duration_ms)
                })
                .await;

            let _ = entity.update(cx, |coord, cx| {
                let (finalized, duration_ms) = result;
                let mut saved = None;
                let mut copied = false;
                coord.completed_dir = None;
                if let Ok((dir, bundle, _)) = finalized {
                    saved = Some(bundle.keyframes.len());
                    match crate::annotate::handoff::copy_bundle_to_clipboard(&dir) {
                        Ok(()) => copied = true,
                        Err(error) => {
                            annotate_log(format!("Capture saved but not copied: {error:#}"));
                        }
                    }
                    coord.completed_dir = Some(dir.clone());
                    coord.review_dir = Some(dir);
                }
                coord.set_island_phase(phase_after_finish(
                    saved,
                    copied,
                    duration_ms,
                    Instant::now(),
                ));
                cx.notify();
            });
        })
        .detach();
    }
}

fn camera_cursor_badge(x: f32, y: f32) -> Div {
    div()
        .absolute()
        .left(px(x - 20.0))
        .top(px(y - 20.0))
        .size(px(40.0))
        .rounded_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .bg(rgba(0x161619eb))
        .border_1()
        .border_color(rgba(0xffffff38))
        .shadow(vec![BoxShadow {
            color: gpui::hsla(0.0, 0.0, 0.0, 0.45),
            offset: point(px(0.0), px(3.0)),
            blur_radius: px(10.0),
            spread_radius: px(0.0),
        }])
        .child(
            div()
                .w(px(9.0))
                .h(px(2.5))
                .rounded_t(px(1.5))
                .bg(rgb(0xf5f5f7)),
        )
        .child(
            div()
                .w(px(21.0))
                .h(px(14.5))
                .rounded(px(3.5))
                .bg(rgb(0xf5f5f7))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .size(px(9.0))
                        .rounded_full()
                        .bg(rgb(0x161619))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(div().size(px(4.5)).rounded_full().bg(rgb(0xf5f5f7))),
                ),
        )
}

pub struct TargetPickerWindow {
    coordinator: Entity<Coordinator>,
    focus: FocusHandle,
}

impl Render for TargetPickerWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let coord = self.coordinator.read(cx);
        let mode = match coord.island_phase {
            IslandPhase::Selecting { mode } => mode,
            _ => SelectionMode::Area,
        };
        let drag = coord.selection_drag;
        let hovered = coord.hovered_window.clone();
        let anim_rect = coord.animated_window_rect;
        let pointer = coord.selection_pointer;

        let c_down = self.coordinator.clone();
        let c_move = self.coordinator.clone();
        let c_up = self.coordinator.clone();
        let c_key = self.coordinator.clone();

        let win_bounds = window.bounds();
        let origin = [coord.display_rect[0], coord.display_rect[1]];
        let k = macos::points_per_pixel(window);
        let screen = [
            f32::from(win_bounds.size.width),
            f32::from(win_bounds.size.height),
        ];

        let hold = coord.hold_picker;
        let shortcut = coord.shortcut_label();
        let (hold_mods, hold_key) = crate::annotate::capture::shortcut_parts(&shortcut);

        let highlight: Option<([f32; 4], String)> = match mode {
            SelectionMode::Area => drag.map(|(a, b)| {
                let [a, b] = [a, b].map(|p| clamp_point(p, coord.display_rect));
                let w = (a[0] - b[0]).abs().max(1.0);
                let h = (a[1] - b[1]).abs().max(1.0);
                (
                    [(a[0].min(b[0]) - origin[0]) * k, (a[1].min(b[1]) - origin[1]) * k, w * k, h * k],
                    format!("{} × {}", w.round() as i32, h.round() as i32),
                )
            }),
            SelectionMode::Window => hovered.map(|t| {
                let title = match (&t.app, &t.window_title) {
                    (Some(app), Some(win)) if !win.is_empty() && win != app => {
                        format!("{app} — {win}")
                    }
                    (Some(app), _) => app.clone(),
                    _ => format!(
                        "{} × {}",
                        t.frame.width.round() as i32,
                        t.frame.height.round() as i32
                    ),
                };
                let [rx, ry, rw, rh] = anim_rect.unwrap_or([
                    t.frame.x as f32,
                    t.frame.y as f32,
                    t.frame.width as f32,
                    t.frame.height as f32,
                ]);
                ([(rx - origin[0]) * k, (ry - origin[1]) * k, rw * k, rh * k], title)
            }),
            SelectionMode::Screen if hold => Some((
                [0.0, 0.0, screen[0], screen[1]],
                format!(
                    "Full screen · {hold_key} to switch · drag for an area · release {hold_mods} to record"
                ),
            )),
            SelectionMode::Screen => Some((
                [0.0, 0.0, screen[0], screen[1]],
                "Full screen · Click to record".to_string(),
            )),
        }
        .map(|(rect, label)| (clip_rect(rect, screen), label));

        let badge = highlight.as_ref().map(|(rect, label)| {
            let max_text = (screen[0] - 2.0 * BADGE_MARGIN - 2.0 * BADGE_PX - 2.0).min(480.0);
            let measured_label = if mode == SelectionMode::Area {
                clock_template(label)
            } else {
                label.clone()
            };
            let text_w =
                measure_text(window, &measured_label, BADGE_TEXT, FontWeight::MEDIUM).min(max_text);
            let size = [text_w + 2.0 * BADGE_PX + 2.0, BADGE_H];
            let anchor = match mode {
                SelectionMode::Area => BadgeAnchor::BelowTrailing,
                SelectionMode::Window | SelectionMode::Screen => BadgeAnchor::Center,
            };
            (
                place_badge(*rect, size, screen, anchor),
                text_w,
                label.clone(),
            )
        });

        let mut root = div()
            .id("annotate-target-picker")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .when(mode == SelectionMode::Area, |d| d.cursor_crosshair())
            .when(mode == SelectionMode::Screen, |d| d.cursor_pointer())
            .when(mode == SelectionMode::Window, |d| {
                d.cursor(gpui::CursorStyle::None)
            });

        if mode == SelectionMode::Window
            && let Some([px_screen, py_screen]) = pointer
        {
            let cx_local = (px_screen - origin[0]) * k;
            let cy_local = (py_screen - origin[1]) * k;
            root = root.child(camera_cursor_badge(cx_local, cy_local));
        }

        root.when_some(badge, |d, ([bx, by], text_w, label)| {
            d.child(
                div()
                    .absolute()
                    .left(px(bx))
                    .top(px(by))
                    .h(px(BADGE_H))
                    .px(px(BADGE_PX))
                    .rounded(px(BADGE_H / 2.0))
                    .flex()
                    .items_center()
                    .bg(rgba(0x16161ac7))
                    .border_1()
                    .border_color(rgba(0xffffff38))
                    .shadow(vec![BoxShadow {
                        color: gpui::hsla(0.0, 0.0, 0.0, 0.3),
                        offset: point(px(0.0), px(2.0)),
                        blur_radius: px(8.0),
                        spread_radius: px(0.0),
                    }])
                    .text_size(px(BADGE_TEXT))
                    .font(tabular_font(FontWeight::MEDIUM))
                    .text_color(rgb(TEXT_COLOR))
                    .child(div().w(px(text_w)).truncate().child(label)),
            )
        })
        .on_key_down(move |event: &KeyDownEvent, _, cx| {
            // Unhandled, gpui hands the key back to AppKit, which offers it again (as a key
            // equivalent, keyDown, or input method command) and this runs up to three times:
            // Space and the arrows then toggle or cycle back to where they started.
            if c_key.update(cx, |coord, cx| {
                coord.handle_selection_key(&event.keystroke, cx)
            }) {
                cx.stop_propagation();
            }
        })
        .on_mouse_down(
            MouseButton::Left,
            move |event: &MouseDownEvent, _window, cx| {
                let [sx, sy] = current_mouse_screen()
                    .unwrap_or([f32::from(event.position.x), f32::from(event.position.y)]);
                c_down.update(cx, |coord, cx| {
                    let cur_mode = match coord.island_phase {
                        IslandPhase::Selecting { mode } => mode,
                        _ => return,
                    };
                    if coord.hold_picker {
                        coord.selection_drag = Some(([sx, sy], [sx, sy]));
                        if cur_mode == SelectionMode::Area {
                            coord.sync_picker_overlay();
                            cx.notify();
                        }
                        return;
                    }
                    match cur_mode {
                        SelectionMode::Area => {
                            coord.selection_drag = Some(([sx, sy], [sx, sy]));
                            coord.sync_picker_overlay();
                            cx.notify();
                        }
                        SelectionMode::Window => {
                            if coord.windows_loading {
                                return;
                            }
                            if let Some(target) = selectable_window_at(
                                &coord.selectable_windows,
                                f64::from(sx),
                                f64::from(sy),
                            )
                            .cloned()
                            {
                                coord.start_recording_with_target(target, cx);
                            } else {
                                coord.start_recording_full_screen(cx);
                            }
                        }
                        SelectionMode::Screen => coord.start_recording_full_screen(cx),
                    }
                });
            },
        )
        .on_mouse_move(move |event: &MouseMoveEvent, _window, cx| {
            let [sx, sy] = current_mouse_screen()
                .unwrap_or([f32::from(event.position.x), f32::from(event.position.y)]);
            let dragging = event.pressed_button == Some(MouseButton::Left);
            c_move.update(cx, |coord, cx| {
                coord.selection_pointer = Some([sx, sy]);
                let cur_mode = match coord.island_phase {
                    IslandPhase::Selecting { mode } => mode,
                    _ => return,
                };
                if coord.hold_picker
                    && dragging
                    && let Some((start, _)) = coord.selection_drag
                {
                    let moved = (sx - start[0]).hypot(sy - start[1]) >= 6.0;
                    if cur_mode != SelectionMode::Area && moved {
                        let action = coord.switcher.drag();
                        coord.apply_switcher(action, cx);
                    }
                    if coord.selection_mode() == Some(SelectionMode::Area) {
                        coord.selection_drag = Some((start, [sx, sy]));
                        coord.sync_picker_overlay();
                        cx.notify();
                    }
                    return;
                }
                match cur_mode {
                    SelectionMode::Area => {
                        if let Some((start, _)) = coord.selection_drag {
                            coord.selection_drag = Some((start, [sx, sy]));
                            coord.sync_picker_overlay();
                            cx.notify();
                        }
                    }
                    SelectionMode::Screen => {}
                    SelectionMode::Window => {
                        let found = coord
                            .selectable_windows
                            .iter()
                            .find(|w| {
                                let f = w.frame;
                                f64::from(sx) >= f.x
                                    && f64::from(sx) <= f.x + f.width
                                    && f64::from(sy) >= f.y
                                    && f64::from(sy) <= f.y + f.height
                            })
                            .cloned();
                        if coord.hovered_window.as_ref().and_then(|w| w.window_id)
                            != found.as_ref().and_then(|w| w.window_id)
                        {
                            coord.hovered_window = found;
                            coord.sync_picker_overlay();
                            cx.notify();
                        }
                    }
                }
            });
        })
        .on_mouse_up(MouseButton::Left, {
            let c_up = c_up.clone();
            move |event: &MouseUpEvent, _window, cx| {
                c_up.update(cx, |coord, cx| coord.picker_mouse_up(event, cx));
            }
        })
        .on_mouse_up_out(
            MouseButton::Left,
            move |event: &MouseUpEvent, _window, cx| {
                c_up.update(cx, |coord, cx| coord.picker_mouse_up(event, cx));
            },
        )
    }
}

const BADGE_H: f32 = 28.0;
const BADGE_PX: f32 = 12.0;
const BADGE_TEXT: f32 = 12.5;
const BADGE_GAP: f32 = 8.0;
const BADGE_MARGIN: f32 = 8.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BadgeAnchor {
    BelowTrailing,
    Center,
}

const MIN_AREA_SIDE: f32 = 36.0;

fn clamp_point([x, y]: [f32; 2], [dx, dy, dw, dh]: [f32; 4]) -> [f32; 2] {
    [x.clamp(dx, dx + dw), y.clamp(dy, dy + dh)]
}

fn area_rect(a: [f32; 2], b: [f32; 2], display: [f32; 4]) -> Option<[f64; 4]> {
    let [a, b] = [a, b].map(|p| clamp_point(p, display));
    let w = (a[0] - b[0]).abs();
    let h = (a[1] - b[1]).abs();
    (w >= MIN_AREA_SIDE && h >= MIN_AREA_SIDE)
        .then(|| [a[0].min(b[0]), a[1].min(b[1]), w, h].map(f64::from))
}

fn clip_rect([x, y, w, h]: [f32; 4], [sw, sh]: [f32; 2]) -> [f32; 4] {
    let x0 = x.clamp(0.0, sw);
    let y0 = y.clamp(0.0, sh);
    let x1 = (x + w).clamp(0.0, sw);
    let y1 = (y + h).clamp(0.0, sh);
    [x0, y0, x1 - x0, y1 - y0]
}

fn place_badge(
    [x, y, w, h]: [f32; 4],
    [bw, bh]: [f32; 2],
    [sw, sh]: [f32; 2],
    anchor: BadgeAnchor,
) -> [f32; 2] {
    let (bx, by) = match anchor {
        BadgeAnchor::BelowTrailing => {
            let below = y + h + BADGE_GAP;
            let above = y - BADGE_GAP - bh;
            let by = if below + bh <= sh - BADGE_MARGIN {
                below
            } else if above >= BADGE_MARGIN {
                above
            } else {
                y + h - BADGE_GAP - bh
            };
            (x + w - bw, by)
        }
        BadgeAnchor::Center => (x + (w - bw) / 2.0, y + (h - bh) / 2.0),
    };
    let max_x = (sw - BADGE_MARGIN - bw).max(BADGE_MARGIN);
    let max_y = (sh - BADGE_MARGIN - bh).max(BADGE_MARGIN);
    [
        bx.clamp(BADGE_MARGIN, max_x).round(),
        by.clamp(BADGE_MARGIN, max_y).round(),
    ]
}

fn open_picker_windows(coord_entity: &Entity<Coordinator>, cx: &mut App) {
    close_picker_window(cx);
    let screen = coord_entity.read(cx).display_rect;
    let display = macos::display_at(screen, cx);
    let picker_bounds = Bounds {
        origin: macos::window_origin(display.as_deref()),
        size: macos::layout_size(screen, display.as_deref()),
    };
    let display_id = display.as_ref().map(|d| d.id());

    let picker_coord = coord_entity.clone();
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(picker_bounds)),
            display_id,
            titlebar: None,
            focus: true,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        },
        move |window, cx| {
            let frame = picker_coord.read(cx).display_rect.map(f64::from);
            if let Some(num) = macos::configure_picker_window(window, frame, cx) {
                picker_coord
                    .read(cx)
                    .telemetry
                    .excluded_window_ids
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(num);
            }
            let focus = cx.focus_handle();
            window.focus(&focus);
            cx.new(|cx| {
                cx.observe(&picker_coord, |_, _, cx| cx.notify()).detach();
                TargetPickerWindow {
                    coordinator: picker_coord,
                    focus,
                }
            })
        },
    );
    // A menu bar (accessory) app isn't active when its menu or capsule is used, and keys only
    // reach the picker while it is. The held switcher takes its keys from the key tap instead,
    // so it leaves the app in front alone.
    if !coord_entity.read(cx).switcher.is_held() {
        macos::take_keys(cx);
        macos::focus_picker_soon(cx);
    }

    ensure_pill_open(coord_entity, cx);
}

fn ensure_pill_open(coord_entity: &Entity<Coordinator>, cx: &mut App) {
    coord_entity.update(cx, |coord, _| {
        coord.ensure_native_pill();
        coord.sync_native_pill();
    });
}

fn close_picker_window(cx: &mut App) {
    for handle in cx.windows() {
        if handle.downcast::<TargetPickerWindow>().is_some() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }
}

/// Opens the capture shown last, the one just finished or the one Open Latest found, on
/// Bespoke's Annotate page. Does nothing before there is one.
fn inspect_review_dir(coord_entity: &Entity<Coordinator>, cx: &mut App) {
    if let Some(dir) = coord_entity.read(cx).review_dir.clone() {
        crate::ui::pages::captures::inspect(dir, cx);
    }
}

/// Starts Annotate in Bespoke's app, with the record and paste combos the key tap matches first.
pub fn start(shortcuts: (ShortcutConfig, ShortcutConfig), cx: &mut App) {
    annotate_log(format!("start pid={}", std::process::id()));
    cx.on_window_closed(|cx| cx.defer(macos::hand_back_keys))
        .detach();
    let (ui_sender, ui_receiver) = flume::unbounded();
    let telemetry = SharedCaptureTelemetry::new(ui_sender);
    let (record, paste) = shortcuts;
    telemetry.set_shortcuts(record, paste);
    crate::annotate::capture::set_telemetry(Arc::clone(&telemetry));
    start_global_input_tap(Arc::clone(&telemetry));
    crate::annotate::retention::start();
    crate::annotate::speech_model::ensure();
    let menu_rx = install_status_bar();

    {
        let sender = telemetry.ui_sender.clone();
        std::thread::spawn(move || {
            if let Some(grants) = crate::annotate::permissions::read_grants() {
                let _ = sender.send(LiveOverlayEvent::Grants(grants));
            }
        });
    }
    let coordinator = cx.new(|_| Coordinator::new(telemetry));

    let loop_coord = coordinator.clone();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(8))
                .await;
            let _ = cx.update(|cx| {
                if let Some(ref rx) = menu_rx {
                    while let Ok(action) = rx.try_recv() {
                        match action {
                            MenuAction::ToggleCapture => {
                                loop_coord.update(cx, |coord, cx| coord.record_or_finish(cx));
                            }
                            MenuAction::ChooseTarget => {
                                loop_coord.update(cx, |coord, cx| {
                                    if !coord.is_recording()
                                        && !coord.is_processing()
                                        && !coord.is_selecting()
                                    {
                                        coord.begin_target_selection(cx);
                                    }
                                });
                            }
                            MenuAction::OpenLatest => {
                                if let Ok((dir, _)) = crate::annotate::handoff::load_bundle(
                                    std::path::Path::new("latest"),
                                ) {
                                    loop_coord.update(cx, |coord, cx| {
                                        coord.review_dir = Some(dir);
                                        cx.notify();
                                    });
                                    inspect_review_dir(&loop_coord, cx);
                                }
                            }
                        }
                    }
                }

                while let Ok(ev) = ui_receiver.try_recv() {
                    loop_coord.update(cx, |coord, cx| match ev {
                        LiveOverlayEvent::CaptureShortcut => coord.press_shortcut(cx),
                        LiveOverlayEvent::PasteLatestShortcut => {
                            if !coord.is_recording() && !coord.is_selecting() {
                                coord.paste_latest(cx);
                            }
                        }
                        LiveOverlayEvent::UndoStroke => {
                            coord.undo_stroke(cx);
                        }
                        LiveOverlayEvent::RequestSnapshot => {
                            coord.request_immediate_frame();
                            cx.notify();
                        }
                        LiveOverlayEvent::Grants(grants) => coord.note_grants(grants),
                    });
                }
                if loop_coord.read(cx).switcher.is_held() {
                    loop_coord.update(cx, |coord, cx| coord.tick_switcher(cx));
                } else if loop_coord.read(cx).telemetry.take_escape()
                    && loop_coord.read(cx).is_selecting()
                {
                    loop_coord.update(cx, |coord, cx| coord.cancel_recording(cx));
                }
                if loop_coord.read(cx).gate.is_pending() {
                    loop_coord.update(cx, |coord, _| coord.tick_gate());
                }
                if loop_coord.read(cx).is_island_visible() {
                    record_external_frontmost_app();
                    loop_coord.update(cx, |coord, cx| coord.island_tick(cx));
                }
            });
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_drag_needs_room_for_an_area() {
        let laptop = [0.0, 0.0, 1728.0, 1117.0];
        assert_eq!(
            area_rect([100.0, 80.0], [40.0, 20.0], laptop),
            Some([40.0, 20.0, 60.0, 60.0])
        );
        assert_eq!(area_rect([0.0, 0.0], [30.0, 200.0], laptop), None);
    }

    #[test]
    fn an_area_drag_stays_on_the_display_it_started_on() {
        let left = [-1920.0, 37.0, 1920.0, 1080.0];
        assert_eq!(
            area_rect([-150.0, 200.0], [160.0, 400.0], left),
            Some([-150.0, 200.0, 150.0, 200.0])
        );
        assert_eq!(
            area_rect([-1800.0, 1000.0], [-2100.0, 1300.0], left),
            Some([-1920.0, 1000.0, 120.0, 117.0])
        );
        assert_eq!(area_rect([-20.0, 200.0], [400.0, 400.0], left), None);
        let screens = [
            crate::annotate::screens::Screen {
                id: 1,
                frame: [0.0, 0.0, 1728.0, 1117.0],
                visible: [0.0, 0.0, 1728.0, 1117.0],
                scale: 2.0,
                safe_top: 0.0,
            },
            crate::annotate::screens::Screen {
                id: 2,
                frame: left.map(f64::from),
                visible: left.map(f64::from),
                scale: 1.0,
                safe_top: 0.0,
            },
        ];
        let area = area_rect([-150.0, 200.0], [160.0, 400.0], left).unwrap();
        let (i, rect) = crate::annotate::screens::area_on_display(&screens, area).unwrap();
        assert_eq!((screens[i].id, rect), (2, area));
    }

    #[test]
    fn selection_modes_cycle_in_hud_order() {
        use SelectionMode::{Area, Screen, Window};
        let walk = |forward: bool| {
            let mut mode = Area;
            let mut seen = Vec::new();
            for _ in 0..5 {
                mode = mode.cycled(forward);
                seen.push(mode);
            }
            seen
        };
        assert_eq!(walk(true), [Window, Screen, Area, Window, Screen]);
        assert_eq!(walk(false), [Screen, Window, Area, Screen, Window]);
    }

    #[test]
    fn digits_pick_modes_by_hud_position() {
        use SelectionMode::{Area, Screen, Window};
        assert_eq!(SelectionMode::from_digit("1"), Some(Area));
        assert_eq!(SelectionMode::from_digit("2"), Some(Window));
        assert_eq!(SelectionMode::from_digit("3"), Some(Screen));
        assert_eq!(SelectionMode::from_digit("4"), None);
        assert_eq!(SelectionMode::from_digit("0"), None);
        assert_eq!(SelectionMode::from_digit("a"), None);
    }

    #[test]
    fn area_badge_sits_below_and_flips_near_edges() {
        let screen = [1440.0, 900.0];
        let badge = [90.0, BADGE_H];
        let [bx, by] = place_badge(
            [100.0, 100.0, 300.0, 200.0],
            badge,
            screen,
            BadgeAnchor::BelowTrailing,
        );
        assert_eq!([bx, by], [310.0, 308.0]);

        let [_, by] = place_badge(
            [100.0, 600.0, 300.0, 290.0],
            badge,
            screen,
            BadgeAnchor::BelowTrailing,
        );
        assert_eq!(by, 600.0 - BADGE_GAP - BADGE_H);

        let [bx, by] = place_badge(
            [0.0, 0.0, 40.0, 900.0],
            badge,
            screen,
            BadgeAnchor::BelowTrailing,
        );
        assert_eq!(bx, BADGE_MARGIN);
        assert!(by + BADGE_H <= 900.0 - BADGE_MARGIN);
    }

    #[test]
    fn window_badge_centers_and_clamps_on_screen() {
        let screen = [1440.0, 900.0];
        let [bx, by] = place_badge(
            [200.0, 100.0, 600.0, 400.0],
            [200.0, BADGE_H],
            screen,
            BadgeAnchor::Center,
        );
        assert_eq!([bx, by], [400.0, 300.0 - BADGE_H / 2.0]);

        let clipped = clip_rect([-300.0, -50.0, 400.0, 200.0], screen);
        assert_eq!(clipped, [0.0, 0.0, 100.0, 150.0]);
    }

    #[test]
    fn last_used_mode_keys_round_trip() {
        for mode in SelectionMode::ALL {
            assert_eq!(SelectionMode::from_key(mode.key()), Some(mode));
        }
        assert_eq!(SelectionMode::from_key("fullscreen"), None);
    }

    #[test]
    fn clock_template_is_width_stable() {
        assert_eq!(clock_template("12:34"), "00:00");
    }

    #[test]
    fn a_capture_that_was_not_saved_shows_the_failure() {
        let now = Instant::now();
        assert_eq!(
            phase_after_finish(None, false, 4_000, now),
            IslandPhase::Failed { finished_at: now }
        );
    }

    #[test]
    fn a_saved_capture_says_whether_it_was_copied() {
        let now = Instant::now();
        for copied in [true, false] {
            assert_eq!(
                phase_after_finish(Some(3), copied, 4_000, now),
                IslandPhase::Completed {
                    finished_at: now,
                    frame_count: 3,
                    duration_ms: 4_000,
                    copied,
                }
            );
        }
    }
}
