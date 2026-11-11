use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, AnyElement, App, Bounds, Context, DisplayId, Div, Entity, Global,
    HighlightStyle, Pixels, ScrollHandle, SharedString, Size, StyledText, Subscription, Task,
    Window, WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowHandle, WindowKind,
    WindowOptions, canvas, div, linear_color_stop, linear_gradient, point, prelude::*, px, size,
};

use crate::app_handle::{AppEvent, AppHandle};
use crate::i18n::t;
use crate::managers::transcription::{
    StreamPhase, StreamPhaseEvent, StreamTextEvent, StreamWorkKind,
};
use crate::overlay::MIC_LEVEL_EVENT;
use crate::settings::OverlayPosition;
use crate::settings::get_settings;
use crate::ui::capsule::{self, ChipKind, Colors, Fold, MORPH, Tween, ease, ease_morph};
use crate::ui::clay::keycap;
use crate::ui::components::{icon, width_tracker};
use crate::ui::event_bus::event_bus;
use crate::ui::platform::capsule::{self as native, CapsuleWindow};
use crate::ui::theme::Alpha;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayState {
    Recording,
    Streaming,
    Transcribing,
    Processing,
    /// The text landed. Shows a check for a moment.
    Done,
    /// The paste failed. Stays until dismissed or `FAILED_HOLD` passes.
    PasteFailed,
}

const COMPACT_AREA: (f32, f32) = (320., BASE_H);
const STREAM_AREA: (f32, f32) = (OPEN_W + 6., 132.);

const PAD_X: f32 = 18.;
const PAD_TOP: f32 = 10.;
const PAD_BOTTOM: f32 = 18.;

const TOP_OFFSET: f32 = 46.;
const BOTTOM_OFFSET: f32 = 15.;

const HIDE_DELAY: Duration = Duration::from_millis(300);
const DONE_HOLD: Duration = Duration::from_millis(600);
const FAILED_HOLD: Duration = Duration::from_secs(25);

const BASE_H: f32 = capsule::HEIGHT;
const RADIUS: f32 = capsule::RADIUS;
const INSET: f32 = capsule::INSET;
const CHIP: f32 = capsule::CHIP;
const POLL: Duration = Duration::from_millis(33);
const LEAD_W: f32 = 2. * capsule::LEAD_PAD + capsule::DOT + capsule::LEAD_GAP + capsule::METER_W;
const REST_W: f32 = 2. * INSET + LEAD_W;
const WORK_BASE_W: f32 = 2. * INSET + 2. * capsule::LEAD_PAD + capsule::SPINNER + capsule::LEAD_GAP;
const WORK_LABEL_GUESS: f32 = 100.;
const DONE_W: f32 = 2. * INSET + 2. * capsule::LEAD_PAD + capsule::DONE;
const FAILED_BASE_W: f32 =
    2. * INSET + 2. * capsule::LEAD_PAD + capsule::WARN_ICON + capsule::LEAD_GAP + CHIP;
const OPEN_W: f32 = 440.;
const CAP_MAX_H: f32 = 64.;

fn window_size(state: OverlayState) -> Size<Pixels> {
    let (w, h) = if state == OverlayState::Streaming {
        STREAM_AREA
    } else {
        COMPACT_AREA
    };
    size(px(w + 2. * PAD_X), px(h + PAD_TOP + PAD_BOTTOM))
}

struct OverlayWindow {
    handle: WindowHandle<RecordingOverlay>,
    native: Option<CapsuleWindow>,
    display: Option<DisplayId>,
    bounds: Bounds<Pixels>,
}

#[derive(Default)]
struct Overlay {
    view: Option<Entity<RecordingOverlay>>,
    window: Option<OverlayWindow>,
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

fn placement(
    size: Size<Pixels>,
    position: OverlayPosition,
    cx: &App,
) -> (Option<DisplayId>, gpui::Point<Pixels>) {
    if let Some(screen) = crate::ui::platform::CursorScreen::current() {
        let x = (screen.width as f32 - f32::from(size.width)) / 2.;
        let y = match position {
            OverlayPosition::Top => TOP_OFFSET - PAD_TOP,
            OverlayPosition::Bottom => {
                screen.visible_bottom as f32 - f32::from(size.height) - BOTTOM_OFFSET + PAD_BOTTOM
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
        OverlayPosition::Top => px(TOP_OFFSET - PAD_TOP),
        OverlayPosition::Bottom => {
            bounds.size.height - size.height - px(BOTTOM_OFFSET - PAD_BOTTOM)
        }
    };
    (Some(display.id()), point(x, y))
}

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
            let native = handle
                .update(cx, |_, window, _| CapsuleWindow::of(window))
                .ok()
                .flatten();
            if let Some(native) = &native {
                native.set_ignores_mouse(true);
            }
            cx.global_mut::<Overlay>().window = Some(OverlayWindow {
                handle,
                native,
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
    let result = matches!(state, OverlayState::Done | OverlayState::PasteFailed);
    // A result shows in the window it follows, so the live text's window isn't reopened.
    let current = cx
        .try_global::<Overlay>()
        .and_then(|o| o.window.as_ref())
        .map(|w| w.bounds.size);
    let size = match current {
        Some(size) if result => size,
        _ => window_size(state),
    };
    place_window(&view, size, position, cx);
    cx.global_mut::<Overlay>().generation += 1;
    view.update(cx, |overlay, cx| overlay.show(state, position, cx));
}

/// Shows a result, then hides the overlay after a while unless a newer session took it.
pub fn show_result(state: OverlayState, position: OverlayPosition, cx: &mut App) {
    // Annotate's capsule sits in the same spot.
    if crate::annotate::capsule::is_showing() {
        hide(cx);
        return;
    }
    show(state, position, cx);
    let hold = if state == OverlayState::PasteFailed {
        FAILED_HOLD
    } else {
        DONE_HOLD
    };
    let scheduled = cx.global::<Overlay>().generation;
    cx.spawn(async move |cx| {
        cx.background_executor().timer(hold).await;
        let _ = cx.update(|cx| {
            if cx.global::<Overlay>().generation == scheduled {
                hide(cx);
            }
        });
    })
    .detach();
}

pub fn hide(cx: &mut App) {
    let Some(view) = existing_view(cx) else {
        return;
    };
    view.update(cx, |overlay, cx| overlay.hide(cx));
    if let Some(native) = cx
        .global::<Overlay>()
        .window
        .as_ref()
        .and_then(|w| w.native.as_ref())
    {
        native.set_ignores_mouse(true);
    }
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

/// Hides a paste result, so it doesn't sit under Annotate's capsule.
pub fn dismiss_result(cx: &mut App) {
    let showing_result = existing_view(cx).is_some_and(|view| {
        let overlay = view.read(cx);
        overlay.visible
            && matches!(
                overlay.state,
                OverlayState::Done | OverlayState::PasteFailed
            )
    });
    if showing_result {
        hide(cx);
    }
}

pub fn recording_ready(cx: &mut App) {
    if let Some(view) = existing_view(cx) {
        view.update(cx, |overlay, cx| overlay.recording_ready(cx));
    }
}

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

#[cfg(debug_assertions)]
pub fn window_handle(cx: &App) -> Option<gpui::AnyWindowHandle> {
    cx.try_global::<Overlay>()
        .and_then(|o| o.window.as_ref())
        .map(|w| w.handle.into())
}

#[cfg(debug_assertions)]
pub fn dev_preview(kind: &str, cx: &mut App) {
    let (kind, position) = match kind.strip_suffix("-top") {
        Some(kind) => (kind, OverlayPosition::Top),
        None => (kind, OverlayPosition::Bottom),
    };
    let state = match kind {
        "transcribing" => OverlayState::Transcribing,
        "done" => OverlayState::Done,
        "failed" => OverlayState::PasteFailed,
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
        overlay.smoothed = [0.2, 0.8, 0.5, 0.9, 0.4];
        if kind == "recording-folded" {
            overlay.fold.close(true);
        }
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
    capture_ready: bool,
    smoothed: [f32; capsule::METER_BARS],
    text: StreamTextEvent,
    phase: StreamPhase,
    work_kind: StreamWorkKind,
    elapsed: u64,
    session: u64,
    pinned: bool,
    cap_scroll: ScrollHandle,
    snap: bool,
    width: Tween,
    text_open: Tween,
    text_fade: Tween,
    text_height: Pixels,
    caret_on: bool,
    fold: Fold,
    card: Bounds<Pixels>,
    label_w: Pixels,
    failed_label_w: Pixels,
    cancel_is_escape: bool,
    elapsed_timer: Option<Task<()>>,
    caret_blink: Option<Task<()>>,
    pointer_poll: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl RecordingOverlay {
    fn new(cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.subscribe(&event_bus(cx), |this, _, event: &AppEvent, cx| {
                this.handle_event(event, cx)
            }),
        ];
        Self {
            visible: false,
            state: OverlayState::Recording,
            position: OverlayPosition::Bottom,
            capture_ready: false,
            smoothed: [0.; capsule::METER_BARS],
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
            width: Tween::new(REST_W, MORPH, ease_morph),
            text_open: Tween::new(0., Duration::from_millis(440), ease_morph),
            text_fade: Tween::new(0., Duration::from_millis(260), ease),
            text_height: px(0.),
            caret_on: true,
            fold: Fold::new(),
            card: Bounds::default(),
            label_w: px(0.),
            failed_label_w: px(0.),
            cancel_is_escape: true,
            elapsed_timer: None,
            caret_blink: None,
            pointer_poll: None,
            _subscriptions: subscriptions,
        }
    }

    fn handle_event(&mut self, event: &AppEvent, cx: &mut Context<Self>) {
        match event.name.as_str() {
            MIC_LEVEL_EVENT => {
                let Some(levels) = event.parse::<Vec<f32>>() else {
                    return;
                };
                capsule::smooth_levels(&mut self.smoothed, &levels);
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
                    if self.phase == StreamPhase::Working {
                        self.fold.close(crate::ui::platform::reduce_motion());
                    }
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
        let result = matches!(state, OverlayState::Done | OverlayState::PasteFailed);
        let switched = streaming != (self.state == OverlayState::Streaming);
        // A result morphs out of whatever showed before it.
        if !self.visible || streaming || (switched && !result) {
            self.snap = true;
        }
        if matches!(state, OverlayState::Recording | OverlayState::Streaming) {
            self.capture_ready = false;
            self.smoothed = [0.; capsule::METER_BARS];
            self.text = StreamTextEvent {
                committed: String::new(),
                tentative: String::new(),
            };
            self.fold.start(Instant::now());
        } else {
            self.fold
                .close(!self.visible || crate::ui::platform::reduce_motion());
        }
        self.cancel_is_escape = get_settings(AppHandle::global())
            .bindings
            .get("cancel")
            .is_some_and(|binding| binding.current_binding == "escape");
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

    fn check_pointer(&mut self, cx: &mut Context<Self>) {
        let Some(window) = cx
            .try_global::<Overlay>()
            .and_then(|o| o.window.as_ref())
            .and_then(|w| w.native.clone())
        else {
            return;
        };
        let Some((pointer, _)) = native::pointer() else {
            return;
        };
        let Some(inside) = capsule::pointer_on(&window, self.card, pointer) else {
            return;
        };
        window.set_ignores_mouse(!inside);
        let snap = crate::ui::platform::reduce_motion();
        if self.fold.tick(Instant::now(), inside, snap) {
            cx.notify();
        }
    }

    fn sync_tasks(&mut self, cx: &mut Context<Self>) {
        if !self.visible {
            self.pointer_poll = None;
        } else if self.pointer_poll.is_none() {
            self.pointer_poll = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(POLL).await;
                    if this.update(cx, |this, cx| this.check_pointer(cx)).is_err() {
                        break;
                    }
                }
            }));
        }

        let live = self.visible && self.state == OverlayState::Streaming;
        if live && self.capture_ready {
            if self.elapsed_timer.is_none() {
                self.elapsed_timer = Some(cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(Duration::from_secs(1)).await;
                        let ticked = this.update(cx, |this, cx| {
                            this.elapsed += 1;
                            cx.notify();
                        });
                        if ticked.is_err() {
                            break;
                        }
                    }
                }));
            }
        } else {
            self.elapsed_timer = None;
        }

        if live && self.phase != StreamPhase::Working {
            if self.caret_blink.is_none() {
                self.caret_on = true;
                self.caret_blink = Some(cx.spawn(async move |this, cx| {
                    loop {
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

    fn cancel_button(&self, c: &Colors) -> impl IntoElement {
        // Stopping a recording unregisters Esc, so the hint shows only while it cancels.
        let listening = match self.state {
            OverlayState::Recording => true,
            OverlayState::Streaming => self.phase != StreamPhase::Working,
            _ => false,
        };
        let escape = self.cancel_is_escape && listening;
        capsule::chip("overlay-cancel", ChipKind::Plain, c)
            .on_click(|_, _, _| {
                let app = AppHandle::global().clone();
                crate::runtime::spawn_blocking(move || {
                    crate::utils::cancel_current_operation(&app)
                });
            })
            .child(icon("overlay-cancel").size(px(11.)))
            .when(escape, |el| {
                el.child(keycap(
                    "esc",
                    capsule::KEYCAP,
                    c.keycap,
                    c.text.a(0.75),
                    c.dark,
                ))
            })
    }

    fn base_row(
        &self,
        lead: Div,
        extra: Option<AnyElement>,
        c: &Colors,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let rtl = crate::i18n::is_rtl();
        div()
            .flex_none()
            .h(px(BASE_H - 2.))
            .px(px(INSET - 1.))
            .flex()
            .items_center()
            .when(rtl, |el| el.flex_row_reverse())
            .child(lead.flex_1().min_w_0().h(px(CHIP)))
            .children(extra)
            .child(
                self.fold.element(
                    div()
                        .flex()
                        .items_center()
                        .when(rtl, |el| el.flex_row_reverse())
                        .child(capsule::divider(c))
                        .child(self.cancel_button(c)),
                    width_tracker(cx.entity().downgrade(), |this: &mut Self| {
                        &mut this.fold.measured
                    }),
                ),
            )
    }

    fn listening_row(&self, show_timer: bool, c: &Colors, cx: &Context<Self>) -> impl IntoElement {
        let lead = capsule::lead()
            .child(capsule::live_dot(self.capture_ready, c))
            .child(capsule::meter(&self.smoothed, self.capture_ready, c));
        let timer = show_timer.then(|| {
            let secs = self.elapsed;
            div()
                .flex_none()
                .px(px(8.))
                .child(capsule::clock(format!("{}:{:02}", secs / 60, secs % 60), c))
                .into_any_element()
        });
        self.base_row(lead, timer, c, cx)
    }

    fn working_row(&self, label: SharedString, c: &Colors, cx: &Context<Self>) -> impl IntoElement {
        let lead = capsule::lead()
            .child(capsule::spinner(c))
            .child(self.measured_label(label, |this| &mut this.label_w, c, cx));
        self.base_row(lead, None, c, cx)
    }

    fn measured_label(
        &self,
        label: SharedString,
        slot: fn(&mut Self) -> &mut Pixels,
        c: &Colors,
        cx: &Context<Self>,
    ) -> Div {
        div()
            .relative()
            .flex_none()
            .child(capsule::label(label, c))
            .child(width_tracker(cx.entity().downgrade(), slot))
    }

    fn result_row(&self, c: &Colors, cx: &Context<Self>) -> impl IntoElement {
        let rtl = crate::i18n::is_rtl();
        let row = div()
            .flex_none()
            .h(px(BASE_H - 2.))
            .px(px(INSET - 1.))
            .flex()
            .items_center()
            .when(rtl, |el| el.flex_row_reverse());
        if self.state == OverlayState::Done {
            return row.child(capsule::lead().child(capsule::done_mark(c)));
        }
        row.child(
            capsule::lead()
                .flex_1()
                .min_w_0()
                .child(capsule::warn_icon(c))
                .child(self.measured_label(
                    t("overlay.pasteFailed"),
                    |this| &mut this.failed_label_w,
                    c,
                    cx,
                )),
        )
        .child(
            capsule::chip("overlay-dismiss", ChipKind::Icon, c)
                .on_click(|_, _, cx| hide(cx))
                .child(icon("overlay-cancel").size(px(11.))),
        )
    }

    fn failed_width(&self) -> f32 {
        let label = f32::from(self.failed_label_w);
        FAILED_BASE_W + if label > 0. { label } else { WORK_LABEL_GUESS }
    }

    fn work_width(&self) -> f32 {
        let label = f32::from(self.label_w);
        WORK_BASE_W + if label > 0. { label } else { WORK_LABEL_GUESS }
    }

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
                    // paints the caret nearly transparent instead.
                    color: Some(if self.caret_on {
                        c.accent
                    } else {
                        c.text.a(0.01)
                    }),
                    ..Default::default()
                },
            ));
        }

        let area_h = f32::from(window.bounds().size.height) - PAD_TOP - PAD_BOTTOM;
        let cap_max = (area_h - BASE_H - 20.).min(CAP_MAX_H) + 12.;
        let overflowing = self.cap_scroll.max_offset().height > px(1.);
        let rtl = crate::i18n::is_rtl();
        let cap = div()
            .id(SharedString::from(format!("stext-cap-{}", self.session)))
            .max_h(px(cap_max))
            .overflow_y_scroll()
            .track_scroll(&self.cap_scroll)
            .on_scroll_wheel(cx.listener(|this, _, _, cx| {
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
            .line_height(px(15. * 1.35))
            .text_color(c.text)
            .when(rtl, |el| el.text_right())
            .child(
                div()
                    .when(!top, |el| el.mt_auto())
                    .child(StyledText::new(content).with_highlights(highlights)),
            );

        // CSS fades the top 18px with a mask: always at the bottom of the
        // screen, only on overflow at the top. A surface-colored gradient
        // gives the same look on the flat card.
        //
        // At the bottom of the screen the fade runs along the card's top
        // edge. GPUI clips children to a rectangle, so the fade takes the
        // card's inner corners, and GPUI caps a radius at half the height, so
        // it grows tall enough for them and its gradient still ends at 18px.
        // Over native glass a colored fade would show as a band, so the
        // text clips at the padding instead.
        let flat = !crate::ui::glass::native_available();
        let fade = (flat && (!top || overflowing)).then(|| {
            let corner = RADIUS - 1.;
            let height = if top { 18. } else { (2. * corner).max(18.) };
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(height))
                .when(!top, |el| el.rounded_t(px(corner)))
                .bg(linear_gradient(
                    180.,
                    linear_color_stop(c.tint.a(1.), 0.),
                    linear_color_stop(c.tint.a(0.), 18. / height),
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

    fn card(&self, extra: Pixels, c: &Colors, cx: &mut Context<Self>) -> Div {
        let tracked = cx.entity().downgrade();
        capsule::surface("overlay-capsule", px(RADIUS), c)
            .w(px(self.width.value()) + extra)
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let _ = tracked.update(cx, |this, _| this.card = bounds);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
    }

    fn render_compact(&mut self, c: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let working = matches!(
            self.state,
            OverlayState::Transcribing | OverlayState::Processing
        );
        let snap = std::mem::take(&mut self.snap) || crate::ui::platform::reduce_motion();
        let width = match self.state {
            OverlayState::Done => DONE_W,
            OverlayState::PasteFailed => self.failed_width(),
            _ if working => self.work_width(),
            _ => REST_W,
        };
        self.width.set(width, snap);
        if matches!(self.state, OverlayState::Done | OverlayState::PasteFailed) {
            // The buttons are still folding closed, so their width stays until they finish.
            return self
                .card(self.fold.width(), c, cx)
                .flex_col()
                .child(self.result_row(c, cx))
                .into_any_element();
        }
        let label = if self.state == OverlayState::Processing {
            t("overlay.processing")
        } else {
            t("overlay.transcribing")
        };
        self.card(self.fold.width(), c, cx)
            .flex_col()
            .child(if working {
                self.working_row(label, c, cx).into_any_element()
            } else {
                self.listening_row(false, c, cx).into_any_element()
            })
            .into_any_element()
    }

    fn render_live(&mut self, window: &Window, c: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let has_text = self.has_text();
        let working = self.phase == StreamPhase::Working;
        let open = has_text;
        let collapsed = working && !has_text;
        let width = if open {
            OPEN_W
        } else if collapsed {
            self.work_width()
        } else {
            REST_W
        };
        let snap = std::mem::take(&mut self.snap) || crate::ui::platform::reduce_motion();
        self.width.set(width, snap);
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
            self.working_row(label, c, cx).into_any_element()
        } else {
            self.listening_row(open, c, cx).into_any_element()
        };

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

        let extra = self.fold.width() * (1. - self.text_open.value());
        self.card(extra, c, cx)
            .when(top, |el| el.flex_col_reverse())
            .when(!top, |el| el.flex_col())
            .child(region)
            .child(row)
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
        let stage = div()
            .size_full()
            .flex()
            .justify_center()
            .px(px(PAD_X))
            .pt(px(PAD_TOP))
            .pb(px(PAD_BOTTOM));
        if !self.visible {
            return stage;
        }
        let c = Colors::current(cx);
        let card = if self.state == OverlayState::Streaming {
            self.render_live(window, &c, cx)
        } else {
            self.render_compact(&c, cx)
        };
        if self.width.running()
            || self.text_open.running()
            || self.text_fade.running()
            || self.fold.running()
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
