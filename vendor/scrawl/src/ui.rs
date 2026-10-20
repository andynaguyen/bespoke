use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, BoxShadow, Context, Div, Entity, FocusHandle, FontWeight, Image,
    ImageFormat, IntoElement, KeyDownEvent, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Render, SharedString, TextRun, Window, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions, div, img, point, prelude::*, px, relative, rgb, rgba, size,
};

// AppKit window levels, styles and placement for the picker and review windows.
#[path = "ui/macos.rs"]
mod platform;
use platform::{install_status_bar, reduce_motion};

use crate::audio::AudioRecorder;
use crate::capture::{
    LiveOverlayEvent, ScreenCaptureSession, SharedCaptureTelemetry, current_mouse_screen,
    discover_target, list_selectable_windows, record_external_frontmost_app, region_target,
    scrawl_log, selectable_window_at, shortcut_from_key_and_mods, start_global_input_tap,
};
use crate::laser_overlay::MetalLaserOverlay as LaserOverlay;
use crate::model::{CaptureMode, ScrawlBundle, TargetInfo, format_clock};
use crate::sketch::timeline;

const SIDEBAR_BG: u32 = 0x111111;
const CANVAS_BG: u32 = 0x161616;
const SURFACE_BG: u32 = 0x1d1d1d;
const SURFACE_HOVER: u32 = 0x252525;
const SURFACE_SELECTED: u32 = 0x2e2e2e;
const LINE_COLOR: u32 = 0x2a2a2a;
const TEXT_COLOR: u32 = 0xf2f2f2;
const TEXT_SOFT: u32 = 0xd0d0d0;
const MUTED_COLOR: u32 = 0x969696;
const ACCENT_COLOR: u32 = 0x3b5cf6;
const ACCENT_HOVER: u32 = 0x4b6bff;
const INK_RED: u32 = 0xff3b30;
pub(crate) mod motion;

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

/// The switch chips' clock: the animation clock, so they stay open as long in slow motion
/// (`motion::time_scale`). The held switcher keeps the real clock: it times key presses.
fn reveal_clock_ms() -> u64 {
    (motion::now() * 1000.0) as u64
}

/// What Bespoke's tray menu asks for (`crate::host::menu`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MenuAction {
    ToggleCapture,
    ChooseTarget,
    OpenLatest,
    Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionMode {
    Area,
    Window,
    /// The whole display the picker covers.
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
        crate::handoff::default_captures_dir()
            .parent()
            .map(|dir| dir.join("last-mode"))
    }

    /// The mode the last recording used; the picker opens on it.
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

    /// Modes shown in the HUD, in order.
    pub(crate) fn available() -> &'static [SelectionMode] {
        &Self::ALL
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Area => "Area",
            Self::Window => "Window",
            Self::Screen => "Screen",
        }
    }

    /// Screen needs no pointer gesture, so the HUD offers a Record chip for it.
    pub(crate) fn shows_record(self) -> bool {
        self == Self::Screen
    }

    /// `1`–`3` pick a mode by its position in the HUD.
    fn from_digit(key: &str) -> Option<Self> {
        let index = key.parse::<usize>().ok()?.checked_sub(1)?;
        Self::available().get(index).copied()
    }

    /// The neighbouring mode in HUD order, wrapping at either end.
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
    },
    Dismissing,
}

enum CaptureSource {
    Screen {
        session: ScreenCaptureSession,
        audio: Option<AudioRecorder>,
        /// Screen mode only: which windows were where, for `scrawl autocrop`.
        windows: Option<crate::windows::WindowLogger>,
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

/// Shared coordinator owned by the GPUI app.
pub struct ScrawlCoordinator {
    telemetry: Arc<SharedCaptureTelemetry>,
    active_recording: Option<ActiveRecording>,
    picker_metal_overlay: Option<LaserOverlay>,
    native_pill: Option<crate::native::pill::NativePill>,
    island_phase: IslandPhase,
    /// Switch chips on a tap-started recording (`crate::hold::Reveal`).
    reveal: crate::hold::Reveal,
    /// macOS Reduce motion, read when a reveal starts.
    reduced_motion: bool,
    /// The recording clock last shown, so the lead keeps its time while it leaves.
    last_elapsed_ms: u64,
    selectable_windows: Vec<TargetInfo>,
    selection_generation: u64,
    windows_loading: bool,
    selection_pointer: Option<[f32; 2]>,
    hovered_window: Option<TargetInfo>,
    animated_window_rect: Option<[f32; 4]>,
    selection_drag: Option<([f32; 2], [f32; 2])>,
    /// Global frame of the display the picker covers, highlighted in `SelectionMode::Screen`.
    display_rect: [f32; 4],
    review_bundle: Option<(PathBuf, ScrawlBundle)>,
    review_status: Option<String>,
    selected_tab: ReviewTab,
    selected_keyframe: usize,
    /// The bundle the Completed island is showing.
    completed_dir: Option<PathBuf>,
    /// The mode the last recording used; the picker opens on it.
    last_mode: SelectionMode,
    /// The last fresh read of what macOS has granted (at launch, then when a permission is
    /// missing).
    grants: Option<crate::permissions::Grants>,
    /// Holds Bespoke's Annotate page until the shortcut's modifiers are up.
    gate: crate::permissions::Gate,
    /// ⌥⇧S held as a ⌘Tab-style switcher (`crate::hold`).
    switcher: crate::hold::Switcher,
    /// The modifiers held when the shortcut went down; releasing any of them commits.
    hold_mods: u64,
    /// The open picker is the held switcher, not the menu bar's picker.
    hold_picker: bool,
    clock: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewTab {
    ContactSheet,
    Keyframes,
}

impl ScrawlCoordinator {
    fn new(telemetry: Arc<SharedCaptureTelemetry>) -> Self {
        Self {
            telemetry,
            active_recording: None,
            picker_metal_overlay: None,
            native_pill: None,
            island_phase: IslandPhase::Hidden,
            reveal: crate::hold::Reveal::default(),
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
            review_bundle: None,
            review_status: None,
            selected_tab: ReviewTab::ContactSheet,
            selected_keyframe: 0,
            completed_dir: None,
            last_mode: SelectionMode::load_last_used(),
            grants: None,
            gate: crate::permissions::Gate::default(),
            switcher: crate::hold::Switcher::default(),
            hold_mods: 0,
            hold_picker: false,
            clock: Instant::now(),
        }
    }

    /// Remembers the mode a recording is starting in, so the next picker opens on it.
    fn remember_mode(&mut self, mode: SelectionMode) {
        if self.last_mode != mode {
            self.last_mode = mode;
            mode.save_last_used();
        }
    }

    /// What the paste shortcut pastes: the bundle on the Completed island, else the latest.
    fn paste_target(&self) -> PathBuf {
        matches!(self.island_phase, IslandPhase::Completed { .. })
            .then(|| self.completed_dir.clone())
            .flatten()
            .unwrap_or_else(|| PathBuf::from("latest"))
    }

    pub fn paste_latest(&mut self, cx: &mut Context<Self>) {
        // Without Accessibility macOS drops the posted ⌘V (it can be turned off after it was
        // granted); bring the Annotate page instead.
        if !crate::capture::accessibility_granted() {
            scrawl_log("Paste shortcut without Accessibility; asking for it");
            self.gate
                .defer(vec![crate::permissions::Permission::Accessibility]);
            return;
        }
        let dir = self.paste_target();
        if matches!(self.island_phase, IslandPhase::Completed { .. }) {
            self.dismiss_island(cx);
        }
        std::thread::spawn(move || {
            if let Err(err) = crate::handoff::paste_bundle_image_and_text(&dir) {
                scrawl_log(format!("paste_bundle_image_and_text error: {err}"));
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

    /// Opens Bespoke's Annotate page, where Scrawl's settings and permissions live.
    pub fn open_settings(&mut self) {
        crate::host::open_settings();
    }

    /// Keeps a fresh read of the grants. (The key tap follows Accessibility by itself, in the
    /// HID poller.)
    fn note_grants(&mut self, grants: crate::permissions::Grants) {
        self.grants = Some(grants);
    }

    /// Required permissions that keep a capture from starting now.
    fn missing_to_record(&mut self) -> Vec<crate::permissions::Permission> {
        use crate::permissions::{Permission, missing_to_record};
        let screen = crate::capture::screen_capture_usable(false);
        let accessibility = crate::capture::accessibility_granted();
        let missing = missing_to_record(self.grants.as_ref(), screen, accessibility);
        // The last read may be old; read again before standing in the way.
        if missing.contains(&Permission::Microphone)
            && let Some(fresh) = crate::permissions::read_grants()
        {
            self.note_grants(fresh);
            return missing_to_record(self.grants.as_ref(), screen, accessibility);
        }
        missing
    }

    /// Whether this may go on: with a required permission off it starts nothing, and the
    /// Annotate page comes forward once the modifiers are up.
    fn ready_to_record(&mut self) -> bool {
        let missing = self.missing_to_record();
        if missing.is_empty() {
            return true;
        }
        scrawl_log(format!(
            "Not recording: {missing:?} off; opening the Annotate page"
        ));
        self.gate.defer(missing);
        false
    }

    /// Polled while the gate holds a request: shows the Annotate page once ⌥⇧ is up.
    pub fn tick_gate(&mut self) {
        if self
            .gate
            .tick(crate::capture::modifier_flags() != 0)
            .is_some()
        {
            self.open_settings();
        }
    }

    fn activity(&self) -> crate::hold::Activity {
        use crate::hold::Activity;
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

    /// The capture shortcut went down (from Carbon, the event tap or the HID poller).
    pub fn press_shortcut(&mut self, cx: &mut Context<Self>) {
        let activity = self.activity();
        if self.gate.is_pending() {
            return;
        }
        if !self.switcher.is_held() && activity == crate::hold::Activity::Free {
            // No switcher, picker or overlay before the permissions are on: they sit above
            // macOS's permission prompts and take the clicks meant for them.
            let missing = self.missing_to_record();
            if !self.gate.press(missing) {
                scrawl_log("Shortcut held back: a required permission is off");
                return;
            }
            self.hold_mods = crate::capture::modifier_flags();
            // An Esc from before this hold doesn't cancel it.
            self.telemetry.take_escape();
        }
        if let Some(mode) = self.selection_mode() {
            self.switcher.follow(mode);
        }
        let action = self.switcher.press(self.clock_ms(), activity);
        self.apply_switcher(action, cx);
    }

    /// Polled from the UI loop while the shortcut is held: watches for ⌥⇧ coming up and Esc,
    /// whichever app or window has focus.
    pub fn tick_switcher(&mut self, cx: &mut Context<Self>) {
        if !self.switcher.is_held() {
            return;
        }
        if self.switcher.is_switching() && !self.is_selecting() {
            // Closed some other way (✕, a click on a chip).
            self.switcher.reset();
            return;
        }
        let action = if self.telemetry.take_escape() {
            self.switcher.escape()
        } else {
            let mods = crate::capture::modifier_flags();
            let held = self.hold_mods != 0 && mods & self.hold_mods == self.hold_mods;
            self.switcher.tick(self.clock_ms(), held)
        };
        self.apply_switcher(action, cx);
    }

    fn apply_switcher(&mut self, action: crate::hold::Action, cx: &mut Context<Self>) {
        use crate::hold::Action;
        if action != Action::None {
            scrawl_log(format!("switcher -> {action:?}"));
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

    /// The menu's Record Screen item, the review window's Record button and `app --record`.
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

    /// ⌥⇧ released: record what the switcher shows. A drag too small for an area records
    /// nothing.
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

    /// Records the display under the cursor straight away (no picker), auto-cropped.
    pub fn record_screen_now(&mut self, cx: &mut Context<Self>) {
        self.start_recording_with_target(discover_target(None), cx);
        if self.is_recording() {
            self.show_reveal(cx);
        }
    }

    /// Activity that means the user got to work: clicks, drags, scrolls and strokes. Clicks on
    /// Scrawl's own windows (the capsule and its chips) are never recorded, so they don't count.
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

    /// The recording just started from a tap: the capsule opens onto the switch chips.
    fn show_reveal(&mut self, cx: &mut Context<Self>) {
        self.reduced_motion = reduce_motion();
        let activity = self.reveal_activity();
        self.reveal.show(reveal_clock_ms(), activity);
        self.sync_native_pill();
        cx.notify();
    }

    /// The chips fold back into the recording pill (timeout, work started, or a chip chosen).
    fn fold_reveal(&mut self) {
        if self.reveal.is_open() {
            self.reveal.close();
            self.play_fold();
        }
    }

    fn play_fold(&mut self) {
        self.sync_native_pill();
    }

    /// A switch chip on a tap-started recording. Screen keeps it. Anything else drops the few
    /// seconds recorded so far and starts over on the new target, exactly as if it had been
    /// chosen first: Window and Area morph the capsule into the picker (hover and click a window,
    /// or drag an area).
    pub fn switch_recording(&mut self, mode: SelectionMode, cx: &mut Context<Self>) {
        if !self.is_recording() {
            return;
        }
        if mode == SelectionMode::Screen {
            self.fold_reveal();
            cx.notify();
            return;
        }
        scrawl_log(format!("switch_recording -> {mode:?}"));
        let Some((_, overlay)) = self.discard_recording() else {
            return;
        };
        // Window and Area: the chips stay up while the capsule morphs into the picker.
        // The recording's overlay becomes the picker's (moved if the picker opens on another
        // display), so the border never swaps windows (as the picker → recording path does).
        self.picker_metal_overlay = overlay;
        self.open_picker(Some(mode), false, cx);
    }

    /// Stops the active recording and deletes its bundle, leaving the island as it is. Returns
    /// its target and overlay.
    fn discard_recording(&mut self) -> Option<(TargetInfo, Option<LaserOverlay>)> {
        let rec = self.active_recording.take()?;
        let _ = self.telemetry.finish_session();
        let CaptureSource::Screen { session, audio, .. } = rec.source;
        let _ = session.stop();
        if let Some(audio) = audio {
            let _ = audio.stop();
        }
        let _ = std::fs::remove_dir_all(rec.bundle_dir);
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

    pub fn set_review_bundle(&mut self, bundle: (PathBuf, ScrawlBundle), status: Option<String>) {
        self.review_bundle = Some(bundle);
        self.review_status = status;
        self.selected_tab = ReviewTab::ContactSheet;
        self.selected_keyframe = 0;
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
        if let IslandPhase::Selecting { mode: prev_mode } = self.island_phase {
            if prev_mode != mode {
                // Crisp tactile Dynamic Island scale micro-pulse when Space toggles camera/crosshair mode.
            }
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

    /// Switches the picker to `mode`.
    pub fn choose_selection_mode(&mut self, mode: SelectionMode, cx: &mut Context<Self>) {
        if self.selection_mode().is_none() {
            return;
        }
        self.set_selection_mode(mode, cx);
    }

    /// A HUD segment click: picks the mode, or records when Screen is already selected.
    pub fn click_selection_mode(&mut self, mode: SelectionMode, cx: &mut Context<Self>) {
        if self.selection_mode() == Some(mode) && mode.shows_record() {
            self.record_selected_mode(cx);
        } else {
            self.choose_selection_mode(mode, cx);
        }
    }

    /// Return, `s`, the capture shortcut, or Record: records the full screen.
    pub fn record_selected_mode(&mut self, cx: &mut Context<Self>) {
        if self.selection_mode().is_some() {
            self.start_recording_full_screen(cx);
        }
    }

    /// Picker keyboard map, shared by the picker window and the HUD pill. Returns whether the
    /// key was handled.
    /// ⌘Q and ⌘, in Scrawl's own windows: a menu bar (accessory) app has no main menu to
    /// carry them (Ctrl+Q and Ctrl+, on Windows).
    pub fn handle_app_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let mods = keystroke.modifiers;
        // `secondary` is ⌘ on macOS and Ctrl elsewhere; the other of the two must be up.
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
        // Esc cancels even with ⌥⇧ still held from the switcher.
        if keystroke.key == "escape" {
            self.switcher.reset();
            self.cancel_recording(cx);
            return true;
        }
        if self.handle_app_key(keystroke, cx) {
            return true;
        }
        // The shortcut itself, if it reaches a picker window before Bespoke's key tap.
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
        // The record shortcut's key ("S") also records on its own while the picker is up.
        let label = self.shortcut_label();
        let record_key = crate::capture::shortcut_parts(&label).1;
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

    /// Moves the island to `phase`; the native capsule animates the change.
    fn set_island_phase(&mut self, phase: IslandPhase) {
        let changed = std::mem::discriminant(&self.island_phase) != std::mem::discriminant(&phase);
        if changed {
            // A new phase replaces the whole content, and the switch chips go with it.
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

    /// Puts the capsule on the display in use: the one being recorded, the one the picker is on,
    /// else the one under the pointer.
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
        let Some(point) = anchor.or_else(crate::screens::pointer) else {
            return;
        };
        if let Some(ref pill) = self.native_pill {
            pill.place_on(point);
        }
    }

    /// While picking, the picker (its window, overlay and the capsule) follows the pointer to
    /// whichever display it's on, unless an area is being dragged (an area stays on one display).
    fn follow_pointer_display(&mut self, pointer: [f32; 2], cx: &mut Context<Self>) {
        if self.selection_drag.is_some() {
            return;
        }
        let p = pointer.map(f64::from);
        let current = self.display_rect.map(f64::from);
        if crate::screens::contains(current, p) {
            return;
        }
        let screens = crate::screens::current();
        let Some(i) = crate::screens::index_at(&screens, p) else {
            return;
        };
        let frame = screens[i].frame;
        if (0..4).all(|k| (frame[k] - current[k]).abs() < 1.0) {
            return;
        }
        scrawl_log(format!("picker follows the pointer to display {frame:?}"));
        self.display_rect = frame.map(|v| v as f32);
        if let Some(ref mut overlay) = self.picker_metal_overlay {
            overlay.move_to(frame);
        }
        platform::move_picker_window(frame, cx);
        self.place_capsule();
        self.sync_picker_overlay();
        // The picker draws relative to `display_rect`; with the native capsule nothing else
        // redraws it, and a same-size display doesn't resize its window.
        cx.notify();
    }

    /// The picker's mouse-up, inside its window or not: ends an area drag.
    fn picker_mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        let [sx, sy] = current_mouse_screen()
            .unwrap_or([f32::from(event.position.x), f32::from(event.position.y)]);
        // The held switcher keeps the area until ⌥⇧ is released.
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
        match crate::native::pill::NativePill::open() {
            Ok(pill) => {
                let num = pill.window_number();
                if num > 0 {
                    self.telemetry
                        .excluded_window_ids
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(num);
                }
                self.native_pill = Some(pill);
            }
            Err(err) => {
                scrawl_log(format!("NativePill::open error: {err}"));
            }
        }
    }

    fn pill_snapshot(&self) -> crate::native::pill::PillSnapshot {
        use crate::native::pill::{PillSnapshot, PillStyle, model::Pres};
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
            IslandPhase::Completed { .. } => Pres::Completed {
                label: "Copied".to_owned(),
            },
        };
        PillSnapshot {
            pres,
            clock,
            reduced_motion: self.reduced_motion,
            style: PillStyle::current(),
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
        action: crate::native::pill::PillAction,
        cx: &mut Context<Self>,
    ) {
        use crate::native::pill::PillAction;
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
            open_review_window(&entity, cx);
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
        if !crate::capture::accessibility_granted() {
            scrawl_log("Paste chip without Accessibility; asking for it");
            self.gate
                .defer(vec![crate::permissions::Permission::Accessibility]);
            return;
        }
        std::thread::spawn(move || {
            if let Err(err) = crate::handoff::paste_bundle_at_target(&dir, click_target) {
                scrawl_log(format!("paste_bundle_at_target error: {err}"));
            }
        });
    }

    /// The UI loop's tick, every few ms while the island shows: the native capsule's clicks, the
    /// Completed island's timeout, the picker following the pointer, the switch chips folding,
    /// and closing up once the capsule has faded out.
    pub fn island_tick(&mut self, cx: &mut Context<Self>) {
        if let Some(actions) = self.native_pill.as_ref().map(|p| p.drain_actions()) {
            for action in actions {
                self.apply_pill_action(action, cx);
            }
        }
        if let IslandPhase::Completed { finished_at, .. } = self.island_phase
            && finished_at.elapsed() >= Duration::from_millis(25_000)
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

    /// The menu bar's picker, opening on the mode the last recording used.
    pub fn begin_target_selection(&mut self, cx: &mut Context<Self>) {
        if self.ready_to_record() {
            self.open_picker(None, false, cx);
        }
    }

    /// Opens the picker on `mode`, else on the mode the last recording used. `hold`: it is the
    /// held switcher (`crate::hold`), which records on release.
    fn open_picker(&mut self, mode: Option<SelectionMode>, hold: bool, cx: &mut Context<Self>) {
        if self.is_recording() || self.is_processing() {
            return;
        }
        scrawl_log(format!(
            "open_picker invoked (mode: {mode:?}, hold: {hold})"
        ));
        self.hold_picker = hold;
        let started = Instant::now();
        self.selectable_windows.clear();
        self.hovered_window = None;
        self.animated_window_rect = None;
        self.selection_pointer = current_mouse_screen();
        // The picker opens on the display under the pointer and follows it there
        // (`follow_pointer_display`); Screen mode highlights all of it.
        let screens = crate::screens::current();
        if let Some(i) =
            crate::screens::pointer().and_then(|p| crate::screens::index_at(&screens, p))
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
                // A previous picker request must not overwrite a reopened picker or recording.
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
                start_metric("picker_ready_ms", started);
                cx.notify();
            });
        })
        .detach();
        self.selection_drag = None;
        if matches!(self.island_phase, IslandPhase::Dismissing) {
            // A capsule still fading out opens again from nothing.
            self.island_phase = IslandPhase::Hidden;
        }
        // Open on the mode the last recording used; Window starts on Area's crosshair until the
        // window list arrives.
        let opening = mode.unwrap_or(self.last_mode);
        self.set_island_phase(IslandPhase::Selecting { mode: opening });
        self.telemetry.begin_picker_session();
        self.sync_picker_overlay();
        if self.picker_metal_overlay.is_none() {
            let [x, y, w, h] = self.display_rect.map(f64::from);
            let overlay =
                platform::open_overlay(Arc::clone(&self.telemetry), Some([x, y, w, h]), cx)
                    .map_err(|error| scrawl_log(format!("Metal overlay failed: {error}")))
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
        cx.defer(move |cx| {
            close_review_window(cx);
            open_picker_windows(&entity, cx);
            start_metric("picker_open_ms", started);
        });
        cx.notify();
        start_metric("picker_prep_ms", started);
    }

    /// From the picker: the full screen it highlights.
    pub fn start_recording_full_screen(&mut self, cx: &mut Context<Self>) {
        let [x, y, w, h] = self.display_rect;
        let centre = [f64::from(x + w / 2.0), f64::from(y + h / 2.0)];
        let target = crate::capture::discover_target_at(None, Some(centre));
        self.start_recording_with_target(target, cx);
    }

    pub fn start_recording_with_target(&mut self, target: TargetInfo, cx: &mut Context<Self>) {
        if self.is_recording() || self.is_processing() {
            return;
        }
        // No surprise prompt mid-capture: the permissions are granted on the Annotate page.
        if !self.ready_to_record() {
            self.cancel_recording(cx);
            return;
        }
        let started = Instant::now();
        scrawl_log(format!(
            "start_recording_with_target invoked: mode={:?} frame={:?}",
            target.mode, target.frame
        ));
        // Without the picker the start needs its own cue: the overlay flashes an edge glow.
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
        let id = crate::make_bundle_id();
        let root = std::env::var_os("SCRAWL_BENCH_START")
            .map(PathBuf::from)
            .unwrap_or_else(crate::handoff::default_captures_dir);
        let bundle_dir = root.join(format!("{id}.scrawl"));
        let _ = std::fs::create_dir_all(&bundle_dir);
        let video_path = bundle_dir.join("recording.mp4");

        self.telemetry.set_capture_mode(target.mode);
        self.telemetry.reset_for_session(target.frame);
        let stage = Instant::now();
        // Reuse the continuous 120Hz Metal overlay opened in `begin_target_selection` so the
        // cutout and border never swap windows or jump a single pixel when recording starts.
        // It moves to the target's display if that's another one (a window picked there).
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
            let ov = platform::open_overlay(Arc::clone(&self.telemetry), Some(target_rect), cx)
                .map_err(|error| scrawl_log(format!("Metal overlay failed: {error}")))
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
        start_metric("metal_open_ms", stage);
        let stage = Instant::now();
        let audio_recorder = AudioRecorder::start().ok();
        start_metric("audio_start_ms", stage);
        let stage = Instant::now();
        let screen_session = ScreenCaptureSession::start(
            target.clone(),
            Some(video_path),
            Arc::clone(&self.telemetry.excluded_window_ids),
        );
        start_metric("sck_start_ms", stage);
        let window_log = (target.mode == CaptureMode::Display).then(|| {
            let started_at = *self
                .telemetry
                .started_at
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            crate::windows::WindowLogger::start(
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
        start_metric("recording_start_ms", started);
    }

    /// The island turns into the recording pill: from the picker it morphs, from nothing it opens
    /// from a compact capsule, Dynamic Island style (the native capsule animates both).
    fn present_recording_island(&mut self, cx: &mut Context<Self>) {
        self.set_island_phase(IslandPhase::Recording);
        let entity = cx.entity().clone();
        cx.defer(move |cx| {
            close_review_window(cx);
            close_picker_window(cx);
            ensure_pill_open(&entity, cx);
        });
        cx.notify();
    }

    pub fn cancel_recording(&mut self, cx: &mut Context<Self>) {
        scrawl_log("cancel_recording invoked");
        self.discard_recording();
        self.dismiss_island(cx);
    }

    pub fn finish_recording(&mut self, cx: &mut Context<Self>) {
        scrawl_log("finish_recording invoked");
        let Some(rec) = self.active_recording.take() else {
            return;
        };
        let (events, strokes) = self.telemetry.finish_session();
        let bundle_dir = rec.bundle_dir;
        let target = rec.target;
        let source = rec.source;
        // Drop the Metal overlay immediately on the main thread so the screen frame clears,
        // and morph the Dynamic Island into the compact spinning "Processing…" state while
        // media teardown, transcription, and PNG encoding run on a background worker.
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
                    let (recording, recorded_audio, mut activity) = std::thread::scope(|s| {
                        let screen_handle = s.spawn(|| screen_session.stop());
                        let win_handle = s.spawn(|| {
                            window_log
                                .map(crate::windows::WindowLogger::stop)
                                .unwrap_or_default()
                        });
                        let audio = audio_recorder.and_then(|r| r.stop().ok());
                        (
                            screen_handle.join().unwrap(),
                            audio,
                            win_handle.join().unwrap_or_default(),
                        )
                    });
                    activity.stop_started = Some(stop_started);
                    activity.stop_ms = Some(stop_started.elapsed().as_millis() as u64);
                    let duration_ms = recording.duration_ms;
                    let finalized = crate::finalize_capture_bundle(
                        &bundle_dir,
                        target,
                        recording,
                        strokes,
                        events,
                        recorded_audio,
                        None,
                        activity,
                    );
                    (finalized, duration_ms)
                })
                .await;

            let _ = entity.update(cx, |coord, cx| {
                let (finalized, duration_ms) = result;
                let mut frame_count = 1;
                coord.completed_dir = None;
                if let Ok((dir, bundle, _)) = finalized {
                    frame_count = bundle.keyframes.len();
                    let _ = crate::handoff::copy_bundle_to_clipboard(&dir);
                    coord.completed_dir = Some(dir.clone());
                    coord.set_review_bundle((dir, bundle), None);
                }
                coord.set_island_phase(IslandPhase::Completed {
                    finished_at: Instant::now(),
                    frame_count,
                    duration_ms,
                });
                cx.notify();
            });
        })
        .detach();
    }
}

/// CleanShot-style floating camera cursor rendered at `(x, y)` in `SelectionMode::Window`.
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
            // Top viewfinder hump
            div()
                .w(px(9.0))
                .h(px(2.5))
                .rounded_t(px(1.5))
                .bg(rgb(0xf5f5f7)),
        )
        .child(
            // Camera body + lens
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
    coordinator: Entity<ScrawlCoordinator>,
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
        // The picker covers one display; pointer, windows and areas are global points.
        let origin = [coord.display_rect[0], coord.display_rect[1]];
        // Global coordinates are points on macOS and pixels on Windows; this window lays out in points.
        let k = platform::points_per_pixel(window);
        let screen = [
            f32::from(win_bounds.size.width),
            f32::from(win_bounds.size.height),
        ];

        let hold = coord.hold_picker;
        let shortcut = coord.shortcut_label();
        let (hold_mods, hold_key) = crate::capture::shortcut_parts(&shortcut);

        // Selection rect in window-local coordinates, clipped to the screen, plus its badge.
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

        // The 120Hz `LaserOverlay` draws the backdrop dim, selection cutout, window tint,
        // and inset border continuously across both `Selecting` and `Recording` so there is zero
        // window swap or border jump on recording start. `TargetPickerWindow` only provides the
        // hit-test surface, cursor style, camera cursor follower, and tabular dimension badge.
        let mut root = div()
            .id("scrawl-target-picker")
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
                    .bg(rgba(0x1c1c1ef2))
                    .border_1()
                    .border_color(rgba(0xffffff21))
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
                    // The held switcher: a drag selects an area; releasing ⌥⇧ records.
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
        // A drag released past the picker's display (on another one) ends there too, at the
        // display's edge: the window that saw the press gets the release, outside its bounds.
        .on_mouse_up_out(
            MouseButton::Left,
            move |event: &MouseUpEvent, _window, cx| {
                c_up.update(cx, |coord, cx| coord.picker_mouse_up(event, cx));
            },
        )
    }
}

const BADGE_H: f32 = 22.0;
const BADGE_PX: f32 = 8.0;
const BADGE_TEXT: f32 = 12.0;
const BADGE_GAP: f32 = 8.0;
const BADGE_MARGIN: f32 = 8.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BadgeAnchor {
    /// Just outside the bottom-right corner (Area drags), flipping above or inside near edges.
    BelowTrailing,
    /// Centered over the target (Window hover).
    Center,
}

/// The smallest area the picker records, points a side (`screens::MIN_AREA` is the floor
/// after trimming).
const MIN_AREA_SIDE: f32 = 36.0;

/// A point dragged past the picker's display (global points), pulled back onto its edge: an
/// area belongs to the display the drag started on, which the picker stays on while dragging.
fn clamp_point([x, y]: [f32; 2], [dx, dy, dw, dh]: [f32; 4]) -> [f32; 2] {
    [x.clamp(dx, dx + dw), y.clamp(dy, dy + dh)]
}

/// A dragged area in global points, kept on the picker's `display` (so the cutout, the size
/// badge and the recording agree), if it's big enough to record.
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

/// Positions a badge of `size` relative to `rect`, keeping it fully on screen.
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

pub struct BundleReviewWindow {
    coordinator: Entity<ScrawlCoordinator>,
    focus: FocusHandle,
    cached_sheet_path: Option<PathBuf>,
    cached_sheet_image: Option<Arc<Image>>,
    cached_frames: Vec<Option<Arc<Image>>>,
}

impl BundleReviewWindow {
    fn new(coordinator: Entity<ScrawlCoordinator>, focus: FocusHandle) -> Self {
        Self {
            coordinator,
            focus,
            cached_sheet_path: None,
            cached_sheet_image: None,
            cached_frames: Vec::new(),
        }
    }

    fn ensure_images(&mut self, dir: &std::path::Path, bundle: &ScrawlBundle) {
        let sheet_path = dir.join(&bundle.contact_sheet);
        if self.cached_sheet_path.as_ref() != Some(&sheet_path) {
            self.cached_sheet_path = Some(sheet_path.clone());
            self.cached_sheet_image = std::fs::read(&sheet_path)
                .ok()
                .map(|bytes| Arc::new(Image::from_bytes(ImageFormat::Png, bytes)));
            self.cached_frames = bundle
                .keyframes
                .iter()
                .map(|kf| {
                    let bytes = std::fs::read(dir.join(&kf.file)).ok()?;
                    Some(Arc::new(Image::from_bytes(ImageFormat::Png, bytes)))
                })
                .collect();
        }
    }
}

impl Render for BundleReviewWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let stage_w = (f32::from(window.bounds().size.width) - 320.0 - 44.0).max(320.0);
        let coord_read = self.coordinator.read(cx);
        let state = coord_read.review_bundle.clone();
        let status = coord_read.review_status.clone();
        let tab = coord_read.selected_tab;
        let selected_kf = coord_read.selected_keyframe;
        let shortcut_label = coord_read.shortcut_label();

        let Some((dir, bundle)) = state else {
            let c_start = self.coordinator.clone();
            let c_keydown = self.coordinator.clone();
            return div()
                .id("scrawl-review-empty")
                .track_focus(&self.focus)
                .on_key_down(move |event: &KeyDownEvent, window, cx| {
                    let handled = c_keydown
                        .update(cx, |coord, cx| coord.handle_app_key(&event.keystroke, cx));
                    let key = event.keystroke.key.as_str();
                    if !handled
                        && (key == "escape"
                            || (event.keystroke.modifiers.secondary()
                                && key.eq_ignore_ascii_case("w")))
                    {
                        window.remove_window();
                    } else if !handled {
                        return;
                    }
                    cx.stop_propagation();
                })
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .bg(rgb(CANVAS_BG))
                .text_color(rgb(TEXT_COLOR))
                .child(
                    div()
                        .text_size(px(16.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Annotate"),
                )
                .child(
                    hint_button("empty-record", "Record", Some(shortcut_label), true).on_click(
                        move |_, _, cx| {
                            c_start.update(cx, |coord, cx| coord.record_or_finish(cx));
                        },
                    ),
                )
                .into_any_element();
        };

        self.ensure_images(&dir, &bundle);

        let target_label = match (&bundle.target.app, &bundle.target.window_title) {
            (Some(app), Some(win)) if !win.is_empty() && win != app => {
                format!("{app} — “{win}”")
            }
            (Some(app), _) => app.clone(),
            _ => format!("Display {}", bundle.target.display_id),
        };
        let timeline = timeline(&bundle);

        let c_keydown = self.coordinator.clone();
        let c_shortcut = self.coordinator.clone();
        let c_copy = self.coordinator.clone();
        let c_send = self.coordinator.clone();
        let c_tab_sheet = self.coordinator.clone();
        let c_tab_frames = self.coordinator.clone();
        let copy_dir = dir.clone();
        let send_dir = dir.clone();

        div()
            .id("scrawl-review-root")
            .track_focus(&self.focus)
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                let mods = event.keystroke.modifiers;
                let handled = c_keydown.update(cx, |coord, cx| {
                    if coord.handle_app_key(&event.keystroke, cx) {
                        return true;
                    }
                    if let Some(cfg) = shortcut_from_key_and_mods(
                        key,
                        mods.platform,
                        mods.control,
                        mods.alt,
                        mods.shift,
                    ) && coord.telemetry.matches_shortcut(
                        cfg.key_code as u16,
                        mods.platform,
                        mods.control,
                        mods.alt,
                        mods.shift,
                    ) {
                        coord.telemetry.emit_capture_shortcut();
                        return true;
                    }
                    false
                });
                if !handled
                    && (key == "escape" || (mods.secondary() && key.eq_ignore_ascii_case("w")))
                {
                    window.remove_window();
                } else if !handled {
                    return;
                }
                cx.stop_propagation();
            })
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(CANVAS_BG))
            .text_color(rgb(TEXT_COLOR))
            .child(
                div()
                    .h(px(48.0))
                    .pl(px(platform::HEADER_LEAD))
                    .pr(px(platform::HEADER_TRAIL))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .bg(rgb(SIDEBAR_BG))
                    .border_b_1()
                    .border_color(rgb(LINE_COLOR))
                    .map(platform::window_drag_region)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                // Segmented control: R_outer 8 = R_tab 6 + border 1 + pad 1.
                                div()
                                    .flex_none()
                                    .h(px(28.0))
                                    .p(px(1.0))
                                    .flex()
                                    .items_center()
                                    .gap(px(2.0))
                                    .rounded(px(8.0))
                                    .bg(rgb(CANVAS_BG))
                                    .border_1()
                                    .border_color(rgb(LINE_COLOR))
                                    .child(
                                        tab_chip(
                                            "tab-contact-sheet",
                                            "Contact Sheet",
                                            tab == ReviewTab::ContactSheet,
                                        )
                                        .on_click(
                                            move |_, _, cx| {
                                                c_tab_sheet.update(cx, |coord, cx| {
                                                    coord.selected_tab = ReviewTab::ContactSheet;
                                                    cx.notify();
                                                });
                                            },
                                        ),
                                    )
                                    .child(
                                        tab_chip(
                                            "tab-keyframes",
                                            "Keyframes",
                                            tab == ReviewTab::Keyframes,
                                        )
                                        .on_click(
                                            move |_, _, cx| {
                                                c_tab_frames.update(cx, |coord, cx| {
                                                    coord.selected_tab = ReviewTab::Keyframes;
                                                    cx.notify();
                                                });
                                            },
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.0))
                                    .text_color(rgb(MUTED_COLOR))
                                    .child(format!(
                                        "{} · {} · {} {}",
                                        target_label,
                                        format_clock(bundle.duration_ms),
                                        bundle.keyframes.len(),
                                        if bundle.keyframes.len() == 1 {
                                            "keyframe"
                                        } else {
                                            "keyframes"
                                        }
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                // Bespoke's Annotate page changes it.
                                hint_button(
                                    "btn-shortcut",
                                    "Shortcut",
                                    Some(shortcut_label),
                                    false,
                                )
                                .on_click(move |_, _, cx| {
                                    c_shortcut.update(cx, |coord, _| coord.open_settings());
                                }),
                            )
                            .child(
                                action_button(
                                    "btn-reveal",
                                    &format!("Show in {}", platform::FILE_MANAGER),
                                    false,
                                )
                                .on_click(move |_, _, cx| {
                                    let _ = crate::handoff::open_path(&send_dir);
                                    c_send.update(cx, |coord, cx| {
                                        coord.review_status =
                                            Some(format!("Opened in {}", platform::FILE_MANAGER));
                                        cx.notify();
                                    });
                                }),
                            )
                            .child(
                                action_button(
                                    "btn-copy",
                                    if status.as_deref() == Some("Copied ✓") {
                                        "Copied"
                                    } else {
                                        "Copy"
                                    },
                                    true,
                                )
                                .on_click(move |_, _, cx| {
                                    let res = crate::handoff::copy_bundle_to_clipboard(&copy_dir);
                                    c_copy.update(cx, |coord, cx| {
                                        coord.review_status = Some(match res {
                                            Ok(()) => "Copied ✓".into(),
                                            Err(e) => format!("Copy failed: {e}"),
                                        });
                                        cx.notify();
                                    });
                                }),
                            ),
                    )
                    .children(platform::caption_buttons()),
            )
            .child(
                // Main Workspace: Stage on Left + Timeline Sidebar on Right
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .id("review-stage-scroll")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_y_scroll()
                            .overflow_x_hidden()
                            .p_5()
                            .child(match tab {
                                ReviewTab::ContactSheet => self.render_contact_sheet_stage(stage_w),
                                ReviewTab::Keyframes => {
                                    self.render_keyframes_stage(&bundle, selected_kf, stage_w)
                                }
                            }),
                    )
                    .child(
                        div()
                            .id("review-timeline-sidebar")
                            .w(px(320.0))
                            .h_full()
                            .flex_none()
                            .overflow_y_scroll()
                            .overflow_x_hidden()
                            .bg(rgb(SIDEBAR_BG))
                            .border_l_1()
                            .border_color(rgb(LINE_COLOR))
                            .px_4()
                            .py(px(18.0))
                            .flex()
                            .flex_col()
                            .child(sidebar_heading("Transcript"))
                            .child(
                                div()
                                    .p_3()
                                    .rounded(px(8.0))
                                    .bg(rgb(SURFACE_BG))
                                    .border_1()
                                    .border_color(rgb(LINE_COLOR))
                                    .text_size(px(13.0))
                                    .line_height(relative(1.45))
                                    .map(|d| {
                                        let text = bundle.transcript.text.trim();
                                        if text.is_empty() {
                                            d.text_color(rgb(MUTED_COLOR)).child("No speech")
                                        } else {
                                            d.text_color(rgb(TEXT_COLOR)).child(format!("“{text}”"))
                                        }
                                    }),
                            )
                            .child(div().mt(px(20.0)).child(sidebar_heading("Timeline")))
                            .child(
                                div().mx(px(-8.0)).flex().flex_col().gap(px(1.0)).children(
                                    timeline
                                        .into_iter()
                                        .enumerate()
                                        .map(|(i, line)| timeline_row(i, &line)),
                                ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl BundleReviewWindow {
    fn render_contact_sheet_stage(&self, stage_w: f32) -> AnyElement {
        if let Some(ref image) = self.cached_sheet_image {
            div()
                .w_full()
                .min_w_0()
                .flex()
                .justify_center()
                .child(
                    img(Arc::clone(image))
                        .w(px(stage_w))
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(rgb(LINE_COLOR)),
                )
                .into_any_element()
        } else {
            div()
                .p_6()
                .text_color(rgb(MUTED_COLOR))
                .child("Contact sheet image unavailable.")
                .into_any_element()
        }
    }

    fn render_keyframes_stage(
        &self,
        bundle: &ScrawlBundle,
        selected_kf: usize,
        stage_w: f32,
    ) -> AnyElement {
        // Card: border 1 + pad 8 around the frame, so R_card 12 = R_frame 3 + 9.
        let card_img_w = (stage_w - 18.0).max(240.0);
        let cards = bundle
            .keyframes
            .iter()
            .enumerate()
            .map(|(idx, kf)| {
                let is_selected = idx == selected_kf;
                let frame_img = self.cached_frames.get(idx).and_then(Clone::clone);
                let c_select = self.coordinator.clone();
                div()
                    .id(("kf-card", idx))
                    .p(px(8.0))
                    .rounded(px(12.0))
                    .bg(rgb(SURFACE_BG))
                    .border_1()
                    .border_color(if is_selected {
                        rgb(ACCENT_COLOR)
                    } else {
                        rgb(LINE_COLOR)
                    })
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .cursor_pointer()
                    .when(!is_selected, |d| d.hover(|s| s.border_color(rgb(0x3a3a3a))))
                    .on_click(move |_, _, cx| {
                        c_select.update(cx, |coord, cx| {
                            coord.selected_keyframe = idx;
                            cx.notify();
                        });
                    })
                    .when_some(frame_img, |d, image| {
                        d.child(img(image).w(px(card_img_w)).rounded(px(3.0)))
                    })
                    .child(
                        div()
                            .px(px(4.0))
                            .pb(px(2.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .text_size(px(12.5))
                            .child(
                                div()
                                    .h(px(20.0))
                                    .px(px(7.0))
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .rounded(px(10.0))
                                    .bg(rgb(CANVAS_BG))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(px(11.5))
                                    .text_color(rgb(TEXT_COLOR))
                                    .child(format!("#{} · {}", kf.index, format_clock(kf.t))),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(rgb(TEXT_SOFT))
                                    .child(kf.event.clone()),
                            ),
                    )
                    .when_some(kf.said.clone(), |d, said| {
                        d.child(
                            div()
                                .px(px(4.0))
                                .pb(px(2.0))
                                .text_size(px(12.0))
                                .line_height(relative(1.4))
                                .text_color(rgb(MUTED_COLOR))
                                .child(format!("“{said}”")),
                        )
                    })
            })
            .collect::<Vec<_>>();

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .children(cards)
            .into_any_element()
    }
}

fn sidebar_heading(label: &'static str) -> Div {
    div()
        .mb(px(8.0))
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(MUTED_COLOR))
        .child(label)
}

/// Splits a `sketch::timeline` line ("0:03 · Clicked…", "0:04 ✎ Circle…", "0:05 “said”") into
/// a time column and a body, flagging annotation marks.
fn split_timeline_line(line: &str) -> (&str, &str, bool) {
    let (time, rest) = line.split_once(' ').unwrap_or(("", line));
    if let Some(body) = rest.strip_prefix("✎ ") {
        (time, body, true)
    } else {
        (time, rest.strip_prefix("· ").unwrap_or(rest), false)
    }
}

fn timeline_row(index: usize, line: &str) -> gpui::Stateful<Div> {
    let (time, body, is_mark) = split_timeline_line(line);
    div()
        .id(("timeline-row", index))
        .px(px(8.0))
        .py(px(6.0))
        .rounded(px(6.0))
        .flex()
        .items_start()
        .gap(px(10.0))
        .text_size(px(12.5))
        .line_height(relative(1.4))
        .hover(|s| s.bg(rgb(SURFACE_BG)))
        .child(
            div()
                .w(px(32.0))
                .flex_none()
                .text_color(rgb(MUTED_COLOR))
                .child(time.to_string()),
        )
        .when(is_mark, |d| {
            d.child(div().flex_none().text_color(rgb(INK_RED)).child("✎"))
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(if is_mark {
                    rgb(TEXT_COLOR)
                } else {
                    rgb(TEXT_SOFT)
                })
                .child(body.to_string()),
        )
}

fn tab_chip(id: &'static str, label: &'static str, selected: bool) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(24.0))
        .px(px(10.0))
        .rounded(px(6.0))
        .flex()
        .items_center()
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .when(selected, |d| {
            d.bg(rgb(SURFACE_SELECTED)).shadow(vec![BoxShadow {
                color: gpui::hsla(0.0, 0.0, 0.0, 0.25),
                offset: point(px(0.0), px(1.0)),
                blur_radius: px(2.0),
                spread_radius: px(0.0),
            }])
        })
        .text_color(if selected {
            rgb(TEXT_COLOR)
        } else {
            rgb(MUTED_COLOR)
        })
        .when(!selected, |d| {
            d.cursor_pointer().hover(|s| s.text_color(rgb(TEXT_SOFT)))
        })
        .child(label)
}

fn button_base(id: &'static str, primary: bool) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(28.0))
        .px(px(12.0))
        .rounded(px(7.0))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.0))
        .whitespace_nowrap()
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .border_1()
        .border_color(if primary {
            rgb(ACCENT_COLOR)
        } else {
            rgb(LINE_COLOR)
        })
        .bg(if primary {
            rgb(ACCENT_COLOR)
        } else {
            rgb(SURFACE_BG)
        })
        .text_color(rgb(TEXT_COLOR))
        .cursor_pointer()
        .hover(move |s| {
            if primary {
                s.bg(rgb(ACCENT_HOVER)).border_color(rgb(ACCENT_HOVER))
            } else {
                s.bg(rgb(SURFACE_HOVER))
            }
        })
}

fn action_button(id: &'static str, label: &str, primary: bool) -> gpui::Stateful<Div> {
    button_base(id, primary).child(SharedString::from(label.to_string()))
}

/// Button with a trailing muted hint, e.g. a keyboard shortcut.
fn hint_button(
    id: &'static str,
    label: &str,
    hint: Option<String>,
    primary: bool,
) -> gpui::Stateful<Div> {
    button_base(id, primary)
        .child(SharedString::from(label.to_string()))
        .when_some(hint, |d, hint| {
            d.child(
                div()
                    .text_color(if primary {
                        rgba(0xffffffb3)
                    } else {
                        rgb(MUTED_COLOR)
                    })
                    .child(hint),
            )
        })
}

fn open_picker_windows(coord_entity: &Entity<ScrawlCoordinator>, cx: &mut App) {
    close_picker_window(cx);
    let screen = coord_entity.read(cx).display_rect;
    let display = platform::display_at(screen, cx);
    // Windows wants the display's own origin, or GPUI puts the picker at its default bounds.
    let picker_bounds = Bounds {
        origin: platform::window_origin(display.as_deref()),
        size: platform::layout_size(screen, display.as_deref()),
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
            // The display the picker was opened for, or has since followed the pointer to.
            let frame = picker_coord.read(cx).display_rect.map(f64::from);
            if let Some(num) = platform::configure_picker_window(window, frame, cx) {
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
        platform::take_keys(cx);
        platform::focus_picker_soon(cx);
    }

    ensure_pill_open(coord_entity, cx);
}

/// Opens the native capsule if it isn't up, and brings it to the coordinator's state.
fn ensure_pill_open(coord_entity: &Entity<ScrawlCoordinator>, cx: &mut App) {
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

fn close_review_window(cx: &mut App) {
    for handle in cx.windows() {
        if handle.downcast::<BundleReviewWindow>().is_some() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }
}

/// Opens the review window in front with the keys. A menu bar (accessory) app isn't active when
/// its menu is used, and the cooperative `NSApp.activate()` may leave the window behind the app
/// in front, so it activates ignoring other apps once the window exists.
fn open_review_window(coord_entity: &Entity<ScrawlCoordinator>, cx: &mut App) {
    for handle in cx.windows() {
        if let Some(review) = handle.downcast::<BundleReviewWindow>() {
            platform::take_keys(cx);
            let _ = review.update(cx, |_, window, cx| {
                window.activate_window();
                cx.notify();
            });
            return;
        }
    }
    let bounds = Bounds::centered(None, size(px(1120.0), px(740.0)), cx);
    let coord_clone = coord_entity.clone();
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui::TitlebarOptions {
                // The taskbar and Alt+Tab show it on Windows; macOS draws no title here.
                title: cfg!(target_os = "windows").then(|| "Scrawl".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(16.0), px(16.0))),
            }),
            app_id: Some("dev.kitlangton.scrawl".into()),
            focus: true,
            show: true,
            kind: WindowKind::Normal,
            is_resizable: true,
            window_background: WindowBackgroundAppearance::Opaque,
            ..Default::default()
        },
        move |window, cx| {
            platform::decorate_app_window(window);
            let focus = cx.focus_handle();
            window.focus(&focus);
            cx.new(|cx| {
                cx.observe(&coord_clone, |_, _, cx| cx.notify()).detach();
                BundleReviewWindow::new(coord_clone, focus)
            })
        },
    );
    platform::take_keys(cx);
}

pub(crate) fn start_metric(name: &str, started: Instant) {
    if std::env::var_os("SCRAWL_BENCH_START").is_some() {
        eprintln!(
            "START_METRIC {name}={:.6}",
            started.elapsed().as_secs_f64() * 1_000.0
        );
    }
}

/// Starts Scrawl inside the host app (Bespoke): the key tap, retention, the speech model, and
/// the loop that drives the capsule, picker and review windows. Bespoke owns the shortcuts, the
/// tray menu and the settings, and reaches Scrawl through `crate::host`.
pub fn start(cx: &mut App) {
    scrawl_log(format!("start pid={}", std::process::id()));
    // ⌘W, Esc, the close button or Scrawl itself: every way a window closes ends up here.
    cx.on_window_closed(|cx| cx.defer(platform::hand_back_keys))
        .detach();
    let (ui_sender, ui_receiver) = flume::unbounded();
    let telemetry = SharedCaptureTelemetry::new(ui_sender);
    crate::host::set_telemetry(Arc::clone(&telemetry));
    start_global_input_tap(Arc::clone(&telemetry));
    crate::retention::start();
    crate::speech_model::ensure();
    let menu_rx = install_status_bar();

    // The permission check runs once, so the first shortcut press knows what's missing.
    {
        let sender = telemetry.ui_sender.clone();
        std::thread::spawn(move || {
            if let Some(grants) = crate::permissions::read_grants() {
                let _ = sender.send(LiveOverlayEvent::Grants(grants));
            }
        });
    }
    let coordinator = cx.new(|_| ScrawlCoordinator::new(telemetry));

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
                                if let Ok((dir, bundle)) =
                                    crate::handoff::load_bundle(std::path::Path::new("latest"))
                                {
                                    loop_coord.update(cx, |coord, cx| {
                                        coord.set_review_bundle((dir, bundle), None);
                                        cx.notify();
                                    });
                                    open_review_window(&loop_coord, cx);
                                }
                            }
                            MenuAction::Settings => {
                                loop_coord.update(cx, |coord, _| coord.open_settings());
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
                    // The picker may not have the keys (another app is active), but the
                    // key tap sees Esc anyway.
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

    /// Kit's review: a drag that starts at x -150 on a left display and overshoots to x 160 on
    /// the laptop keeps to the left display, so it's recorded there (`screens::area_on_display`).
    #[test]
    fn an_area_drag_stays_on_the_display_it_started_on() {
        let left = [-1920.0, 37.0, 1920.0, 1080.0];
        assert_eq!(
            area_rect([-150.0, 200.0], [160.0, 400.0], left),
            Some([-150.0, 200.0, 150.0, 200.0])
        );
        // Past its bottom-left corner too.
        assert_eq!(
            area_rect([-1800.0, 1000.0], [-2100.0, 1300.0], left),
            Some([-1920.0, 1000.0, 120.0, 117.0])
        );
        // What's left on the display can be too small to record.
        assert_eq!(area_rect([-20.0, 200.0], [400.0, 400.0], left), None);
        let screens = [
            crate::screens::Screen {
                id: 1,
                frame: [0.0, 0.0, 1728.0, 1117.0],
                visible: [0.0, 0.0, 1728.0, 1117.0],
                scale: 2.0,
                safe_top: 0.0,
            },
            crate::screens::Screen {
                id: 2,
                frame: left.map(f64::from),
                visible: left.map(f64::from),
                scale: 1.0,
                safe_top: 0.0,
            },
        ];
        let area = area_rect([-150.0, 200.0], [160.0, 400.0], left).unwrap();
        let (i, rect) = crate::screens::area_on_display(&screens, area).unwrap();
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
        assert_eq!([bx, by], [400.0, 289.0]);

        let clipped = clip_rect([-300.0, -50.0, 400.0, 200.0], screen);
        assert_eq!(clipped, [0.0, 0.0, 100.0, 150.0]);
    }

    #[test]
    fn timeline_lines_split_into_time_and_body() {
        assert_eq!(
            split_timeline_line("0:04 ✎ Circle around button — frame #2"),
            ("0:04", "Circle around button — frame #2", true)
        );
        assert_eq!(
            split_timeline_line("0:05 · Clicked Save"),
            ("0:05", "Clicked Save", false)
        );
        assert_eq!(
            split_timeline_line("0:01 “hello”"),
            ("0:01", "“hello”", false)
        );
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
}
