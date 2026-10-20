//! The recording overlay (`overlay/RecordingOverlay.tsx` and its CSS): a
//! transparent always-on-top panel near the top or bottom edge of the screen
//! under the cursor. The compact form is a pill with a waveform, or a
//! spinner and label while transcribing. The Live form opens into a panel
//! that shows streaming text.
//!
//! The panel window exists only while the overlay shows. `hide` clears the
//! card at once and closes the window 300ms later, unless a newer `show`
//! arrived in the meantime.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    canvas, div, linear_color_stop, linear_gradient, percentage, point, prelude::*, px, size,
    Animation, AnimationExt, AnyElement, App, Bounds, Context, DisplayId, Entity, FontFeatures,
    FontStyle, Global, HighlightStyle, Hsla, Pixels, ScrollHandle, SharedString, Size, StyledText,
    Subscription, Task, Window, WindowBackgroundAppearance, WindowBounds, WindowDecorations,
    WindowHandle, WindowKind, WindowOptions,
};

use crate::app_handle::{AppEvent, AppHandle};
use crate::i18n::t;
use crate::managers::transcription::{
    StreamPhase, StreamPhaseEvent, StreamTextEvent, StreamWorkKind,
};
use crate::overlay::MIC_LEVEL_EVENT;
use crate::settings::OverlayPosition;
use crate::ui::components::icon;
use crate::ui::event_bus::event_bus;
use crate::ui::theme::{palette, Alpha, HandyTheme};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayState {
    Recording,
    Streaming,
    Transcribing,
    Processing,
}

// Window sizes. The window must hold the card's widest state (the compact
// pill grows to 218 wide, the Live panel to 394x118) since the card sits flush
// against the screen-edge side of the window.
const COMPACT_WINDOW: (f32, f32) = (256., 50.);
const STREAM_WINDOW: (f32, f32) = (400., 120.);

#[cfg(target_os = "macos")]
const TOP_OFFSET: f32 = 46.;
#[cfg(not(target_os = "macos"))]
const TOP_OFFSET: f32 = 4.;
#[cfg(target_os = "macos")]
const BOTTOM_OFFSET: f32 = 15.;
#[cfg(not(target_os = "macos"))]
const BOTTOM_OFFSET: f32 = 40.;

const HIDE_DELAY: Duration = Duration::from_millis(300);

// Card geometry (`--ov-*`). CSS sizes the card's content box, so the 1px
// border adds 2px to each.
const REST_W: f32 = 172.;
const PILL_W: f32 = 184.;
const WORK_W: f32 = 216.;
const OPEN_W: f32 = 392.;
const BASE_H: f32 = 40.;
const CAP_MAX_H: f32 = 64.;
const WAVE_BARS: usize = 9;
const MORPH: Duration = Duration::from_millis(460);

fn window_size(state: OverlayState) -> Size<Pixels> {
    let (w, h) = if state == OverlayState::Streaming {
        STREAM_WINDOW
    } else {
        COMPACT_WINDOW
    };
    size(px(w), px(h))
}

/// CSS `cubic-bezier(x1, y1, x2, y2)`.
fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let curve = |a: f32, b: f32, s: f32| {
        let u = 1. - s;
        3. * u * u * s * a + 3. * u * s * s * b + s * s * s
    };
    // Solve x(s) = t by bisection; 20 steps is far below a pixel.
    let (mut lo, mut hi) = (0f32, 1f32);
    for _ in 0..20 {
        let mid = (lo + hi) / 2.;
        if curve(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    curve(y1, y2, (lo + hi) / 2.)
}

/// `cubic-bezier(0.22, 1, 0.36, 1)`, the card's morph curve.
fn ease_morph(t: f32) -> f32 {
    cubic_bezier(0.22, 1., 0.36, 1., t)
}

/// CSS `ease`.
fn ease(t: f32) -> f32 {
    cubic_bezier(0.25, 0.1, 0.25, 1., t)
}

/// CSS `ease-in-out`.
fn ease_in_out(t: f32) -> f32 {
    cubic_bezier(0.42, 0., 0.58, 1., t)
}

/// A CSS transition on one number.
#[derive(Clone, Copy)]
struct Tween {
    from: f32,
    to: f32,
    /// When the running transition started; `None` once settled.
    start: Option<Instant>,
    duration: Duration,
    ease: fn(f32) -> f32,
}

impl Tween {
    fn new(value: f32, duration: Duration, ease: fn(f32) -> f32) -> Self {
        Self {
            from: value,
            to: value,
            start: None,
            duration,
            ease,
        }
    }

    fn progress(&self) -> f32 {
        match self.start {
            Some(start) if !self.duration.is_zero() => {
                (start.elapsed().as_secs_f32() / self.duration.as_secs_f32()).min(1.)
            }
            _ => 1.,
        }
    }

    fn value(&self) -> f32 {
        self.from + (self.to - self.from) * (self.ease)(self.progress())
    }

    fn running(&self) -> bool {
        self.progress() < 1.
    }

    /// Animates from the current value to `to`, or jumps there if `snap`.
    fn set(&mut self, to: f32, snap: bool) {
        if snap {
            self.from = to;
            self.to = to;
            self.start = None;
        } else if to != self.to {
            self.from = self.value();
            self.to = to;
            self.start = Some(Instant::now());
        }
    }
}

/// Overlay colors (`--s-*`), light and dark.
struct Colors {
    surface: Hsla,
    accent: Hsla,
    accent_soft: Hsla,
    muted: Hsla,
    faint: Hsla,
    border: Hsla,
    hair: Hsla,
    text: Hsla,
}

fn colors(cx: &App) -> Colors {
    let p = palette(cx);
    let hex = |value: u32| -> Hsla { gpui::rgb(value).into() };
    if p.is_dark {
        Colors {
            surface: p.background.a(0.98),
            accent: p.logo_primary,
            accent_soft: p.logo_primary.a(0.2),
            muted: hex(0xa3a09a),
            faint: hex(0x6f6c66),
            border: p.text.a(0.12),
            hair: p.text.a(0.08),
            text: p.text,
        }
    } else {
        Colors {
            surface: p.background.a(0.98),
            accent: p.logo_primary,
            accent_soft: p.background_ui.a(0.16),
            muted: hex(0x6e6e6e),
            faint: hex(0x9a9a9a),
            border: p.mid_gray.a(0.22),
            hair: p.mid_gray.a(0.12),
            text: p.text,
        }
    }
}

struct OverlayWindow {
    handle: WindowHandle<RecordingOverlay>,
    display: Option<DisplayId>,
    bounds: Bounds<Pixels>,
}

#[derive(Default)]
struct Overlay {
    view: Option<Entity<RecordingOverlay>>,
    window: Option<OverlayWindow>,
    /// Bumped by every show, so a delayed close from an earlier hide never
    /// takes down a session that started in the meantime.
    generation: u64,
}

impl Global for Overlay {}

fn overlay_view(cx: &mut App) -> Entity<RecordingOverlay> {
    if let Some(view) = cx.default_global::<Overlay>().view.clone() {
        return view;
    }
    let view = cx.new(RecordingOverlay::new);
    cx.global_mut::<Overlay>().view = Some(view.clone());
    view
}

fn existing_view(cx: &App) -> Option<Entity<RecordingOverlay>> {
    cx.try_global::<Overlay>().and_then(|o| o.view.clone())
}

/// The display under the cursor and the window origin on it, in that
/// display's top-left coordinates.
fn placement(
    size: Size<Pixels>,
    position: OverlayPosition,
    cx: &App,
) -> (Option<DisplayId>, gpui::Point<Pixels>) {
    #[cfg(target_os = "macos")]
    if let Some(screen) = crate::ui::platform::CursorScreen::current() {
        let x = (screen.width as f32 - f32::from(size.width)) / 2.;
        let y = match position {
            OverlayPosition::Top => TOP_OFFSET,
            // The visible frame keeps the overlay above the Dock.
            OverlayPosition::Bottom => {
                screen.visible_bottom as f32 - f32::from(size.height) - BOTTOM_OFFSET
            }
        };
        let display = cx
            .displays()
            .into_iter()
            .map(|d| d.id())
            .find(|id| u32::from(*id) == screen.display_id);
        return (display, point(px(x), px(y)));
    }

    let cursor = crate::input::get_cursor_position(AppHandle::global());
    let displays = cx.displays();
    let display = cursor
        .and_then(|(x, y)| {
            let cursor = point(px(x as f32), px(y as f32));
            displays
                .iter()
                .find(|d| d.bounds().contains(&cursor))
                .cloned()
        })
        .or_else(|| cx.primary_display());
    let Some(display) = display else {
        return (None, point(px(0.), px(0.)));
    };
    let bounds = display.bounds();
    let x = (bounds.size.width - size.width) / 2.;
    let y = match position {
        OverlayPosition::Top => px(TOP_OFFSET),
        OverlayPosition::Bottom => bounds.size.height - size.height - px(BOTTOM_OFFSET),
    };
    (Some(display.id()), point(x, y))
}

/// Opens the overlay window at the right place for `size`, or keeps the
/// current one if it is already there.
fn place_window(
    view: &Entity<RecordingOverlay>,
    size: Size<Pixels>,
    position: OverlayPosition,
    cx: &mut App,
) {
    let (display, origin) = placement(size, position, cx);
    let bounds = Bounds::new(origin, size);
    let current = cx
        .global::<Overlay>()
        .window
        .as_ref()
        .map(|w| (w.display, w.bounds));
    if current == Some((display, bounds)) {
        return;
    }
    close_window(cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: display,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some(crate::app_handle::APP_IDENTIFIER.into()),
        window_decorations: Some(WindowDecorations::Client),
        ..Default::default()
    };
    let root = view.clone();
    match cx.open_window(options, move |window, _| {
        crate::ui::platform::prepare_overlay_window(window);
        root
    }) {
        Ok(handle) => {
            cx.global_mut::<Overlay>().window = Some(OverlayWindow {
                handle,
                display,
                bounds,
            })
        }
        Err(e) => log::error!("Failed to open the recording overlay: {e}"),
    }
}

fn close_window(cx: &mut App) {
    if let Some(window) = cx.default_global::<Overlay>().window.take() {
        let _ = window
            .handle
            .update(cx, |_, window, _| window.remove_window());
    }
}

pub fn show(state: OverlayState, position: OverlayPosition, cx: &mut App) {
    let view = overlay_view(cx);
    place_window(&view, window_size(state), position, cx);
    cx.global_mut::<Overlay>().generation += 1;
    view.update(cx, |overlay, cx| overlay.show(state, position, cx));
}

pub fn hide(cx: &mut App) {
    let Some(view) = existing_view(cx) else {
        return;
    };
    view.update(cx, |overlay, cx| overlay.hide(cx));
    let scheduled = cx.global::<Overlay>().generation;
    cx.spawn(async move |cx| {
        cx.background_executor().timer(HIDE_DELAY).await;
        let _ = cx.update(|cx| {
            if cx.global::<Overlay>().generation == scheduled {
                close_window(cx);
            } else {
                log::debug!("Skipping stale overlay hide: a newer session is showing the overlay");
            }
        });
    })
    .detach();
}

pub fn recording_ready(cx: &mut App) {
    if let Some(view) = existing_view(cx) {
        view.update(cx, |overlay, cx| overlay.recording_ready(cx));
    }
}

/// Moves a showing overlay after the position setting changes.
pub fn reposition(position: OverlayPosition, cx: &mut App) {
    let Some(view) = existing_view(cx) else {
        return;
    };
    let Some(size) = cx
        .global::<Overlay>()
        .window
        .as_ref()
        .map(|w| w.bounds.size)
    else {
        return;
    };
    place_window(&view, size, position, cx);
    view.update(cx, |overlay, cx| {
        overlay.position = position;
        cx.notify();
    });
}

/// The overlay window, for dev snapshots.
#[cfg(debug_assertions)]
pub fn window_handle(cx: &App) -> Option<gpui::AnyWindowHandle> {
    cx.try_global::<Overlay>()
        .and_then(|o| o.window.as_ref())
        .map(|w| w.handle.into())
}

/// Dev snapshots: shows the overlay with sample data. `kind` is `arming`,
/// `recording`, `transcribing`, `streaming`, `streaming-long` or
/// `streaming-working`, with an optional `-top` suffix.
#[cfg(debug_assertions)]
pub fn dev_preview(kind: &str, cx: &mut App) {
    let (kind, position) = match kind.strip_suffix("-top") {
        Some(kind) => (kind, OverlayPosition::Top),
        None => (kind, OverlayPosition::Bottom),
    };
    let state = match kind {
        "transcribing" => OverlayState::Transcribing,
        k if k.starts_with("streaming") => OverlayState::Streaming,
        _ => OverlayState::Recording,
    };
    show(state, position, cx);
    let Some(view) = existing_view(cx) else {
        return;
    };
    view.update(cx, |overlay, cx| {
        if kind != "arming" {
            overlay.capture_ready = true;
        }
        overlay.smoothed = [
            0.2, 0.5, 0.8, 0.6, 0.9, 0.4, 0.7, 0.3, 0.5, 0., 0., 0., 0., 0., 0., 0.,
        ];
        match kind {
            "streaming" | "streaming-working" => {
                overlay.text = StreamTextEvent {
                    committed: "Let's meet at the coffee place on Fifth".into(),
                    tentative: "around ten tomorrow".into(),
                };
                overlay.elapsed = 7;
            }
            "streaming-long" => {
                overlay.text = StreamTextEvent {
                    committed: "So the plan for next week is to finish the migration first, then \
                                review the dashboards with the team on Tuesday and ship the \
                                onboarding changes"
                        .into(),
                    tentative: "by the end of the week".into(),
                };
                overlay.elapsed = 42;
            }
            _ => {}
        }
        if kind == "streaming-working" {
            overlay.phase = StreamPhase::Working;
        }
        overlay.cap_scroll.scroll_to_bottom();
        overlay.sync_tasks(cx);
        cx.notify();
    });
}

pub struct RecordingOverlay {
    visible: bool,
    state: OverlayState,
    position: OverlayPosition,
    /// False until the recorder delivers its first samples; the card shows a
    /// muted arming pulse until then.
    capture_ready: bool,
    smoothed: [f32; 16],
    text: StreamTextEvent,
    phase: StreamPhase,
    work_kind: StreamWorkKind,
    elapsed: u64,
    session: u64,
    /// Whether the live text follows the newest line.
    pinned: bool,
    cap_scroll: ScrollHandle,
    /// Jump to the next targets instead of animating, as a fresh mount does.
    snap: bool,
    width: Tween,
    radius: Tween,
    text_open: Tween,
    text_fade: Tween,
    text_height: Pixels,
    caret_on: bool,
    elapsed_timer: Option<Task<()>>,
    caret_blink: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl RecordingOverlay {
    fn new(cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.subscribe(&event_bus(cx), |this, _, event: &AppEvent, cx| {
                this.handle_event(event, cx)
            }),
            cx.observe_global::<HandyTheme>(|_, cx| cx.notify()),
        ];
        Self {
            visible: false,
            state: OverlayState::Recording,
            position: OverlayPosition::Bottom,
            capture_ready: false,
            smoothed: [0.; 16],
            text: StreamTextEvent {
                committed: String::new(),
                tentative: String::new(),
            },
            phase: StreamPhase::Listening,
            work_kind: StreamWorkKind::Transcribing,
            elapsed: 0,
            session: 0,
            pinned: true,
            cap_scroll: ScrollHandle::new(),
            snap: true,
            width: Tween::new(REST_W + 2., MORPH, ease_morph),
            radius: Tween::new(24., MORPH, ease_morph),
            text_open: Tween::new(0., Duration::from_millis(440), ease_morph),
            text_fade: Tween::new(0., Duration::from_millis(260), ease),
            text_height: px(0.),
            caret_on: true,
            elapsed_timer: None,
            caret_blink: None,
            _subscriptions: subscriptions,
        }
    }

    fn handle_event(&mut self, event: &AppEvent, cx: &mut Context<Self>) {
        match event.name.as_str() {
            MIC_LEVEL_EVENT => {
                let Some(levels) = event.parse::<Vec<f32>>() else {
                    return;
                };
                for (i, level) in self.smoothed.iter_mut().enumerate() {
                    *level = *level * 0.7 + levels.get(i).copied().unwrap_or(0.) * 0.3;
                }
                if self.visible {
                    cx.notify();
                }
            }
            StreamTextEvent::EVENT => {
                if let Some(text) = event.parse::<StreamTextEvent>() {
                    self.text = text;
                    if self.pinned {
                        self.cap_scroll.scroll_to_bottom();
                    }
                    cx.notify();
                }
            }
            StreamPhaseEvent::EVENT => {
                if let Some(event) = event.parse::<StreamPhaseEvent>() {
                    self.phase = event.phase;
                    if let Some(kind) = event.kind {
                        self.work_kind = kind;
                    }
                    self.sync_tasks(cx);
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    fn show(&mut self, state: OverlayState, position: OverlayPosition, cx: &mut Context<Self>) {
        let streaming = state == OverlayState::Streaming;
        // React swaps the whole card when the form changes and remounts the
        // Live card for each session, so neither animates from old sizes.
        if !self.visible || streaming || streaming != (self.state == OverlayState::Streaming) {
            self.snap = true;
        }
        if matches!(state, OverlayState::Recording | OverlayState::Streaming) {
            self.capture_ready = false;
            self.smoothed = [0.; 16];
            self.text = StreamTextEvent {
                committed: String::new(),
                tentative: String::new(),
            };
        }
        self.position = position;
        if streaming {
            self.phase = StreamPhase::Listening;
            self.work_kind = StreamWorkKind::Transcribing;
            self.elapsed = 0;
            self.session += 1;
            self.pinned = true;
            self.cap_scroll.set_offset(point(px(0.), px(0.)));
            self.text_height = px(0.);
        }
        self.state = state;
        self.visible = true;
        self.sync_tasks(cx);
        cx.notify();
    }

    fn hide(&mut self, cx: &mut Context<Self>) {
        self.visible = false;
        self.capture_ready = false;
        self.sync_tasks(cx);
        cx.notify();
    }

    fn recording_ready(&mut self, cx: &mut Context<Self>) {
        self.elapsed = 0;
        self.capture_ready = true;
        self.sync_tasks(cx);
        cx.notify();
    }

    /// Starts or stops the elapsed-time counter and the caret blink.
    fn sync_tasks(&mut self, cx: &mut Context<Self>) {
        let live = self.visible && self.state == OverlayState::Streaming;
        if live && self.capture_ready {
            if self.elapsed_timer.is_none() {
                self.elapsed_timer = Some(cx.spawn(async move |this, cx| loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let ticked = this.update(cx, |this, cx| {
                        this.elapsed += 1;
                        cx.notify();
                    });
                    if ticked.is_err() {
                        break;
                    }
                }));
            }
        } else {
            self.elapsed_timer = None;
        }

        // `scaret-blink 1.05s steps(1)`: on for half, off for half.
        if live && self.phase != StreamPhase::Working {
            if self.caret_blink.is_none() {
                self.caret_on = true;
                self.caret_blink = Some(cx.spawn(async move |this, cx| loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(525))
                        .await;
                    let toggled = this.update(cx, |this, cx| {
                        this.caret_on = !this.caret_on;
                        cx.notify();
                    });
                    if toggled.is_err() {
                        break;
                    }
                }));
            }
        } else {
            self.caret_blink = None;
            self.caret_on = true;
        }
    }

    fn has_text(&self) -> bool {
        !self.text.committed.is_empty() || !self.text.tentative.is_empty()
    }

    fn waveform(&self, c: &Colors) -> impl IntoElement {
        let ready = self.capture_ready;
        let muted = c.muted;
        let accent = c.accent;
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(3.))
            .pr(px(8.))
            .h(px(18.))
            .children(
                self.smoothed[..WAVE_BARS]
                    .iter()
                    .enumerate()
                    .map(move |(i, level)| {
                        let bar = div().w(px(4.)).rounded(px(2.)).flex_none();
                        if ready {
                            let height = (3. + level.max(0.).powf(0.7) * 15.).clamp(3., 18.);
                            bar.h(px(height)).bg(accent).into_any_element()
                        } else {
                            // `swave-arming`: a muted pulse travelling out from the
                            // middle bar.
                            let delay = [0., 75., 150., 225., 300., 225., 150., 75., 0.][i] / 900.;
                            bar.h(px(6.))
                                .bg(muted)
                                .with_animation(
                                    SharedString::from(format!("swave-arming-{i}")),
                                    Animation::new(Duration::from_millis(900)).repeat(),
                                    move |bar, delta| {
                                        let t = (delta - delay).rem_euclid(1.);
                                        let (scale, opacity) = if t < 0.5 {
                                            let e = ease_in_out(t * 2.);
                                            (0.55 + 0.95 * e, 0.25 + 0.45 * e)
                                        } else {
                                            let e = ease_in_out((t - 0.5) * 2.);
                                            (1.5 - 0.95 * e, 0.7 - 0.45 * e)
                                        };
                                        bar.h(px(6. * scale)).opacity(opacity)
                                    },
                                )
                                .into_any_element()
                        }
                    }),
            )
    }

    fn dot(&self, c: &Colors) -> impl IntoElement {
        let ready = self.capture_ready;
        let soft = c.accent_soft;
        div()
            .relative()
            .size(px(7.))
            .flex_none()
            .when(ready, |el| {
                // `sdot-pulse`: a soft ring spreads 7px and fades by 70%.
                el.child(div().absolute().rounded_full().with_animation(
                    "sdot-pulse",
                    Animation::new(Duration::from_millis(1900)).repeat(),
                    move |ring, delta| {
                        let p = if delta < 0.7 { ease(delta / 0.7) } else { 1. };
                        let spread = px(7. * p);
                        ring.left(-spread)
                            .top(-spread)
                            .size(px(7.) + spread * 2.)
                            .bg(soft.a(1. - p))
                    },
                ))
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded_full()
                    .when(ready, |el| el.bg(c.accent))
                    .when(!ready, |el| el.bg(c.muted).opacity(0.45)),
            )
    }

    fn cancel_button(&self, c: &Colors) -> impl IntoElement {
        let (soft, text) = (c.accent_soft, c.text);
        div()
            .id("overlay-cancel")
            .size(px(22.))
            .flex_none()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(c.muted)
            .bg(c.hair)
            .cursor_pointer()
            .hover(move |s| s.bg(soft).text_color(text))
            .on_click(|_, _, _| {
                let app = AppHandle::global().clone();
                crate::runtime::spawn_blocking(move || crate::commands::cancel_operation(app));
            })
            .child(icon("overlay-cancel").size(px(9.)))
    }

    fn spinner(&self, c: &Colors) -> impl IntoElement {
        div()
            .relative()
            .size(px(13.))
            .flex_none()
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded_full()
                    .border_2()
                    .border_color(c.hair),
            )
            .child(div().absolute().inset_0().text_color(c.accent).child(
                icon("spinner-arc").size(px(13.)).with_animation(
                    "sspin",
                    Animation::new(Duration::from_millis(700)).repeat(),
                    |icon, delta| icon.rotate(percentage(delta)),
                ),
            ))
    }

    /// The 40px control row: left | center | right, with equal side columns
    /// so the center stays centered.
    fn base_row(
        &self,
        left: AnyElement,
        center: AnyElement,
        right: Vec<AnyElement>,
    ) -> impl IntoElement {
        let rtl = crate::i18n::is_rtl();
        div()
            .flex_none()
            .h(px(BASE_H))
            .px(px(10.))
            .flex()
            .items_center()
            .when(rtl, |el| el.flex_row_reverse())
            .child(
                div()
                    .flex_1()
                    .min_w(px(22.))
                    .flex()
                    .items_center()
                    .when(rtl, |el| el.justify_end().pr(px(5.)))
                    .when(!rtl, |el| el.pl(px(5.)))
                    .child(left),
            )
            .child(center)
            .child(
                div()
                    .flex_1()
                    .min_w(px(22.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .when(rtl, |el| el.flex_row_reverse().justify_end())
                    .when(!rtl, |el| el.justify_end())
                    .children(right),
            )
    }

    fn listening_row(&self, show_timer: bool, c: &Colors) -> impl IntoElement {
        let mut right = Vec::new();
        if show_timer {
            let secs = self.elapsed;
            let mut timer = div()
                .text_size(px(12.))
                .line_height(px(15.))
                .text_color(c.faint)
                .whitespace_nowrap()
                .child(format!("{}:{:02}", secs / 60, secs % 60));
            timer
                .text_style()
                .get_or_insert_with(Default::default)
                .font_features = Some(FontFeatures(Arc::new(vec![("tnum".into(), 1)])));
            right.push(timer.into_any_element());
        }
        right.push(self.cancel_button(c).into_any_element());
        self.base_row(
            self.dot(c).into_any_element(),
            self.waveform(c).into_any_element(),
            right,
        )
    }

    fn working_row(&self, label: SharedString, c: &Colors) -> impl IntoElement {
        self.base_row(
            self.spinner(c).into_any_element(),
            div()
                .min_w_0()
                .px(px(8.))
                .text_size(px(12.))
                .line_height(px(15.))
                .text_color(c.muted)
                .truncate()
                .child(label)
                .into_any_element(),
            vec![self.cancel_button(c).into_any_element()],
        )
    }

    /// The scrolling live text with the blinking caret.
    fn live_text(
        &self,
        working: bool,
        top: bool,
        window: &Window,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut content = String::new();
        if !self.text.committed.is_empty() {
            content.push_str(&self.text.committed);
            content.push(' ');
        }
        content.push_str(&self.text.tentative);
        let mut highlights = Vec::new();
        if !working {
            let start = content.len();
            content.push('\u{258F}');
            highlights.push((
                start..content.len(),
                HighlightStyle {
                    // GPUI draws a zero-alpha glyph dark, so the off phase
                    // paints the caret in the card color instead.
                    color: Some(if self.caret_on { c.accent } else { c.surface }),
                    font_style: Some(FontStyle::Normal),
                    ..Default::default()
                },
            ));
        }

        // `min(--ov-cap-max-h, 100vh - --ov-base-h - 20px)`, a content-box
        // height, so the 12px padding comes on top.
        let cap_max = (f32::from(window.bounds().size.height) - BASE_H - 20.).min(CAP_MAX_H) + 12.;
        let overflowing = self.cap_scroll.max_offset().height > px(1.);
        let rtl = crate::i18n::is_rtl();
        let cap = div()
            .id(SharedString::from(format!("stext-cap-{}", self.session)))
            .max_h(px(cap_max))
            .overflow_y_scroll()
            .track_scroll(&self.cap_scroll)
            .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                // Re-pin within about a line of the bottom.
                let max = this.cap_scroll.max_offset().height;
                let offset = -this.cap_scroll.offset().y;
                this.pinned = max - offset <= px(16.);
                cx.notify();
            }))
            .flex()
            .flex_col()
            .px(px(14.))
            .when(top, |el| el.pb(px(12.)))
            .when(!top, |el| el.pt(px(12.)))
            .text_size(px(15.))
            .italic()
            .line_height(px(15. * 1.35))
            .text_color(c.text.a(0.9))
            .when(rtl, |el| el.text_right())
            .child(
                div()
                    .when(!top, |el| el.mt_auto())
                    .child(StyledText::new(content).with_highlights(highlights)),
            );

        // CSS fades the top 18px with a mask: always at the bottom of the
        // screen, only on overflow at the top. A surface-colored gradient
        // gives the same look on the flat card.
        let fade = (!top || overflowing).then(|| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(18.))
                .bg(linear_gradient(
                    180.,
                    linear_color_stop(c.surface, 0.),
                    linear_color_stop(c.surface.a(0.), 1.),
                ))
        });

        let tracked = cx.entity().downgrade();
        div()
            .relative()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .child(cap)
            .children(fade)
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let changed = tracked
                            .update(cx, |this, _| {
                                let changed = this.text_height != bounds.size.height;
                                this.text_height = bounds.size.height;
                                changed
                            })
                            .unwrap_or(false);
                        if changed {
                            cx.defer(move |cx| {
                                let _ = tracked.update(cx, |_, cx| cx.notify());
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
    }

    fn render_compact(&mut self, c: &Colors) -> AnyElement {
        let working = matches!(
            self.state,
            OverlayState::Transcribing | OverlayState::Processing
        );
        let snap = std::mem::take(&mut self.snap);
        self.width
            .set(if working { WORK_W } else { REST_W } + 2., snap);
        self.radius.set(24., snap);
        let label = if self.state == OverlayState::Processing {
            t("overlay.processing")
        } else {
            t("overlay.transcribing")
        };
        div()
            .flex_none()
            .flex()
            .flex_col()
            .w(px(self.width.value()))
            .rounded(px(self.radius.value()))
            .overflow_hidden()
            .bg(c.surface)
            .border_1()
            .border_color(c.border)
            .child(if working {
                self.working_row(label, c).into_any_element()
            } else {
                self.listening_row(false, c).into_any_element()
            })
            .into_any_element()
    }

    fn render_live(&mut self, window: &Window, c: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let has_text = self.has_text();
        let working = self.phase == StreamPhase::Working;
        // The panel stays open while finalizing so the text doesn't jump.
        let open = has_text;
        let collapsed = working && !has_text;
        let (width, radius) = if open {
            (OPEN_W, 16.)
        } else if collapsed {
            (WORK_W, 18.)
        } else {
            (PILL_W, 24.)
        };
        let snap = std::mem::take(&mut self.snap);
        self.width.set(width + 2., snap);
        self.radius.set(radius, snap);
        let target = if open { 1. } else { 0. };
        self.text_open.set(target, snap);
        self.text_fade.set(target, snap);

        let top = self.position == OverlayPosition::Top;
        let label = if self.work_kind == StreamWorkKind::Polishing {
            t("overlay.processing")
        } else {
            t("overlay.transcribing")
        };
        let row = if working {
            self.working_row(label, c).into_any_element()
        } else {
            self.listening_row(open, c).into_any_element()
        };

        // `grid-template-rows: 0fr -> 1fr`: grow from nothing to the text's
        // own height, then let it size itself.
        let progress = self.text_open.value();
        let region = div()
            .flex_none()
            .flex()
            .flex_col()
            .overflow_hidden()
            .opacity(self.text_fade.value())
            .when(self.text_open.running() || target == 0., |el| {
                el.h(self.text_height * progress)
            })
            .child(self.live_text(working, top, window, c, cx));

        div()
            .flex_none()
            .flex()
            .when(top, |el| el.flex_col_reverse())
            .when(!top, |el| el.flex_col())
            .w(px(self.width.value()))
            .rounded(px(self.radius.value()))
            .overflow_hidden()
            .bg(c.surface)
            .border_1()
            .border_color(c.border)
            .child(region)
            .child(row)
            // `scard-pop`: fade in once per session.
            .with_animation(
                SharedString::from(format!("scard-pop-{}", self.session)),
                Animation::new(MORPH).with_easing(ease_morph),
                |card, delta| card.opacity(delta),
            )
            .into_any_element()
    }
}

impl Render for RecordingOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let stage = div().size_full().flex().justify_center();
        if !self.visible {
            return stage;
        }
        let c = colors(cx);
        let card = if self.state == OverlayState::Streaming {
            self.render_live(window, &c, cx)
        } else {
            self.render_compact(&c)
        };
        if self.width.running()
            || self.radius.running()
            || self.text_open.running()
            || self.text_fade.running()
        {
            window.request_animation_frame();
        }
        stage
            .when(self.position == OverlayPosition::Top, |el| el.items_start())
            .when(self.position == OverlayPosition::Bottom, |el| {
                el.items_end()
            })
            .child(card)
    }
}
