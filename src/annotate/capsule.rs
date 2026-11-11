use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::annotate::audio::MIC_LEVEL_EVENT;
use crate::annotate::capsule_link::{
    CapsuleCommand, CapsuleLink, CapsuleState, PillAction, PillSnapshot, Pres,
};
use crate::annotate::coordinator::SelectionMode;
use gpui::{
    Animation, AnimationExt, AnyElement, App, Bounds, Context, FontWeight, Pixels, SharedString,
    Subscription, Task, Window, WindowBackgroundAppearance, WindowBounds, WindowDecorations,
    WindowKind, WindowOptions, canvas, div, prelude::*, px, size,
};

use crate::app_handle::{AppEvent, AppHandle};
use crate::i18n::t;
use crate::settings::{OverlayPosition, get_settings};
use crate::ui::capsule::{self, ChipKind, Colors, Fold};
use crate::ui::clay::keycap;
use crate::ui::components::icon;
use crate::ui::event_bus::event_bus;
use crate::ui::platform::capsule::{self as native, CapsuleWindow};
use crate::ui::theme::{Alpha, mix};

const WINDOW_W: f32 = 720.;
const PAD_X: f32 = 18.;
const PAD_TOP: f32 = 10.;
const PAD_BOTTOM: f32 = 18.;
const WINDOW_H: f32 = capsule::HEIGHT + PAD_TOP + PAD_BOTTOM;
const EDGE_GAP: f64 = 12.;
const POLL: Duration = Duration::from_millis(33);
const PASTE_DRAG: f64 = 24.;

#[cfg(debug_assertions)]
#[derive(Default)]
struct LastWindow(Option<gpui::AnyWindowHandle>);

#[cfg(debug_assertions)]
impl gpui::Global for LastWindow {}

#[cfg(debug_assertions)]
pub fn window_handle(cx: &App) -> Option<gpui::AnyWindowHandle> {
    cx.try_global::<LastWindow>().and_then(|last| last.0)
}

#[cfg(debug_assertions)]
static DEV_FOLDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(debug_assertions)]
pub fn dev_preview(kind: &str, cx: &mut App) {
    DEV_FOLDED.store(kind == "recording-folded", Ordering::SeqCst);
    let pres = match kind {
        "choose-screen" => Pres::Selecting {
            mode: SelectionMode::Screen,
            hold: false,
        },
        "recording" => Pres::Recording {
            chips: false,
            stroke_count: 3,
        },
        "recording-folded" => Pres::Recording {
            chips: false,
            stroke_count: 2,
        },
        "switch" => Pres::Recording {
            chips: true,
            stroke_count: 3,
        },
        "processing" => Pres::Processing,
        "done" => Pres::Completed { copied: true },
        "saved" => Pres::Completed { copied: false },
        "failed" => Pres::Failed,
        _ => Pres::Selecting {
            mode: SelectionMode::Area,
            hold: false,
        },
    };
    let anchor = native::pointer().map_or([0., 0.], |(p, _)| p);
    crate::annotate::capsule_link::preview_capsule(
        PillSnapshot {
            pres,
            clock: "0:21".into(),
            reduced_motion: false,
            shortcut: "⌥⇧S".into(),
            cancel: "esc".into(),
        },
        anchor,
    );
    let _ = cx;
}

/// Opens a window that draws the capsule `link` drives, on the UI thread.
pub fn open(app: &AppHandle, link: CapsuleLink) {
    app.with_ui(move |cx| open_window(link, cx));
}

fn open_window(link: CapsuleLink, cx: &mut App) {
    let CapsuleLink {
        commands,
        actions,
        state,
        excluded_window_ids,
    } = link;
    let view = cx.new(|cx| AnnotateCapsule::new(actions, Arc::clone(&state), cx));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            gpui::point(px(0.), px(0.)),
            size(px(WINDOW_W), px(WINDOW_H)),
        ))),
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some(crate::app_handle::APP_IDENTIFIER.into()),
        window_decorations: Some(WindowDecorations::Client),
        ..Default::default()
    };
    let root = view.clone();
    let handle = match cx.open_window(options, move |window, _| {
        crate::ui::platform::prepare_overlay_window(window);
        root
    }) {
        Ok(handle) => handle,
        Err(e) => {
            log::error!("Failed to open Annotate's capsule: {e}");
            return;
        }
    };
    let native = handle
        .update(cx, |_, window, _| CapsuleWindow::of(window))
        .ok()
        .flatten();
    if let Some(native) = &native {
        let number = native.float();
        native.set_ignores_mouse(true);
        state.window_number.store(number, Ordering::SeqCst);
        excluded_window_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(number);
    }
    view.update(cx, |capsule, _| capsule.window = native);
    #[cfg(debug_assertions)]
    {
        cx.default_global::<LastWindow>().0 = Some(handle.into());
    }

    cx.spawn(async move |cx| {
        while let Ok(command) = commands.recv_async().await {
            let _ = cx.update(|cx| view.update(cx, |capsule, cx| capsule.apply(command, cx)));
        }
        let _ = cx.update(|cx| {
            view.update(cx, |capsule, _| capsule.poll = None);
            capsule::spot(cx).update(cx, |spot, cx| spot.set_annotate_showing(false, cx));
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        });
    })
    .detach();
}

pub struct AnnotateCapsule {
    actions: flume::Sender<PillAction>,
    state: Arc<CapsuleState>,
    window: Option<CapsuleWindow>,
    snapshot: Option<PillSnapshot>,
    anchor: Option<[f64; 2]>,
    shown: u64,
    card: Bounds<Pixels>,
    paste_press: Option<[f64; 2]>,
    poll: Option<Task<()>>,
    fold: Fold,
    chips_fold: Fold,
    levels: [f32; capsule::METER_BARS],
    at_top: bool,
    _subscription: Subscription,
}

/// Where the capsule's window goes on the screen under `point`: at the top (upstream's
/// `pill_origin`) or above the Dock.
fn capsule_origin(point: [f64; 2], size: [f64; 2], at_top: bool, gap: f64) -> Option<[f64; 2]> {
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

impl AnnotateCapsule {
    fn new(
        actions: flume::Sender<PillAction>,
        state: Arc<CapsuleState>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.subscribe(&event_bus(cx), |this, _, event: &AppEvent, cx| {
            if event.name == MIC_LEVEL_EVENT
                && let Some(levels) = event.parse::<Vec<f32>>()
            {
                capsule::smooth_levels(&mut this.levels, &levels);
                if matches!(this.pres(), Pres::Recording { .. }) {
                    cx.notify();
                }
            }
        });
        Self {
            actions,
            state,
            window: None,
            snapshot: None,
            anchor: None,
            shown: 0,
            card: Bounds::default(),
            paste_press: None,
            poll: None,
            fold: Fold::new(),
            chips_fold: Fold::new(),
            levels: [0.; capsule::METER_BARS],
            at_top: false,
            _subscription: subscription,
        }
    }

    fn chips_showing(&self) -> bool {
        matches!(self.pres(), Pres::Recording { chips: true, .. })
    }

    fn pres(&self) -> &Pres {
        self.snapshot.as_ref().map_or(&Pres::Hidden, |s| &s.pres)
    }

    fn apply(&mut self, command: CapsuleCommand, cx: &mut Context<Self>) {
        match command {
            CapsuleCommand::Place(point) => {
                self.anchor = Some(point);
                self.place(cx);
            }
            CapsuleCommand::Sync(snapshot) => {
                let was_hidden = *self.pres() == Pres::Hidden;
                let hidden = snapshot.pres == Pres::Hidden;
                let was_recording = matches!(self.pres(), Pres::Recording { .. });
                let had_chips = self.chips_showing();
                if let Pres::Recording { chips, .. } = snapshot.pres {
                    let now = Instant::now();
                    if !was_recording {
                        self.levels = [0.; capsule::METER_BARS];
                        self.fold.start(now);
                    }
                    let snap = crate::ui::platform::reduce_motion();
                    if chips {
                        if !had_chips {
                            self.chips_fold.start(now);
                        }
                        self.fold.cover();
                    } else if had_chips {
                        self.chips_fold.close(snap);
                        self.fold.uncover(now, snap);
                    }
                }
                #[cfg(debug_assertions)]
                self.apply_dev_preview();
                self.snapshot = Some(snapshot);
                if was_hidden && !hidden {
                    self.shown += 1;
                    self.place(cx);
                    self.set_visible(true, cx);
                } else if hidden && !was_hidden {
                    self.set_visible(false, cx);
                }
                cx.notify();
            }
        }
    }

    #[cfg(debug_assertions)]
    fn apply_dev_preview(&mut self) {
        if std::env::var_os("BESPOKE_DEV_SNAPSHOT").is_none() {
            return;
        }
        self.levels = [0.2, 0.8, 0.5, 0.9, 0.4];
        if DEV_FOLDED.load(Ordering::SeqCst) {
            self.fold.close(true);
        }
    }

    fn place(&mut self, cx: &App) {
        let Some(window) = &self.window else {
            return;
        };
        let anchor = self.anchor.or_else(|| native::pointer().map(|(p, _)| p));
        let Some(anchor) = anchor else {
            return;
        };
        let at_top = get_settings(AppHandle::global()).overlay_position == OverlayPosition::Top;
        self.at_top = at_top;
        let size = [f64::from(WINDOW_W), f64::from(WINDOW_H)];
        let Some(mut origin) = capsule_origin(anchor, size, at_top, EDGE_GAP) else {
            return;
        };
        if at_top {
            origin[1] -= f64::from(PAD_TOP);
        } else {
            origin[1] += f64::from(PAD_BOTTOM);
        }
        window.move_to(origin, cx);
    }

    fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        let Some(window) = &self.window else {
            return;
        };
        window.set_visible(visible, cx);
        capsule::spot(cx).update(cx, |spot, cx| spot.set_annotate_showing(visible, cx));
        if !visible {
            window.set_ignores_mouse(true);
        }
        if visible {
            self.start_polling(cx);
        } else {
            self.poll = None;
            self.paste_press = None;
            self.state.hovered.store(false, Ordering::SeqCst);
            self.state.hidden.store(true, Ordering::SeqCst);
        }
    }

    fn start_polling(&mut self, cx: &mut Context<Self>) {
        if self.poll.is_some() {
            return;
        }
        self.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if this.update(cx, |this, cx| this.check_pointer(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    fn check_pointer(&mut self, cx: &mut Context<Self>) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let Some((pointer, down)) = native::pointer() else {
            return;
        };
        let Some(inside) = capsule::pointer_on(&window, self.card, pointer) else {
            return;
        };
        self.state
            .hovered
            .store(inside || self.paste_press.is_some(), Ordering::SeqCst);
        if self
            .fold
            .tick(Instant::now(), inside, crate::ui::platform::reduce_motion())
        {
            cx.notify();
        }

        if let Some(start) = self.paste_press
            && !down
        {
            self.paste_press = None;
            let moved = (pointer[0] - start[0]).hypot(pointer[1] - start[1]);
            if !inside && moved >= PASTE_DRAG {
                self.send(PillAction::Paste(Some([
                    pointer[0] as f32,
                    pointer[1] as f32,
                ])));
            }
        }
        let ignore = !inside && self.paste_press.is_none();
        window.set_ignores_mouse(ignore);
    }

    fn send(&self, action: PillAction) {
        let _ = self.actions.send(action);
    }

    fn action_chip(
        &self,
        id: &'static str,
        kind: ChipKind,
        action: PillAction,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        capsule::chip(id, kind, c).on_click(cx.listener(move |this, _, _, _| this.send(action)))
    }

    fn hint(
        &self,
        text: &str,
        color: gpui::Hsla,
        label: gpui::Hsla,
        c: &Colors,
    ) -> impl IntoElement {
        keycap(text.to_string(), capsule::KEYCAP, color, label, c.dark)
    }

    fn modes(
        &self,
        active: Option<SelectionMode>,
        switch: bool,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rtl = crate::i18n::is_rtl();
        let raised = if crate::ui::theme::palette(cx).is_dark {
            gpui::white().a(0.18)
        } else {
            gpui::white().a(0.9)
        };
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(2.))
            .p(px(2.))
            .rounded_full()
            .bg(c.chip)
            .when(rtl, |el| el.flex_row_reverse())
            .children(SelectionMode::available().iter().map(|&mode| {
                let (key, icon_name) = match mode {
                    SelectionMode::Area => ("annotate.capsule.area", "square-dashed"),
                    SelectionMode::Window => ("annotate.capsule.window", "app-window"),
                    SelectionMode::Screen => ("annotate.capsule.screen", "monitor"),
                };
                let on = active == Some(mode);
                let action = if switch {
                    PillAction::SwitchRecording(mode)
                } else {
                    PillAction::SelectMode(mode)
                };
                let (text, secondary) = (c.text, c.secondary);
                div()
                    .id(SharedString::from(format!("mode-{}", mode.label())))
                    .h(px(capsule::CHIP - 4.))
                    .px(px(9.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .rounded_full()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .when(rtl, |el| el.flex_row_reverse())
                    .when(on, |el| el.bg(raised).text_color(text))
                    .when(!on, |el| {
                        el.text_color(secondary).hover(move |s| s.text_color(text))
                    })
                    .on_click(cx.listener(move |this, _, _, _| this.send(action)))
                    .child(icon(icon_name).size(px(13.)))
                    .child(t(key))
            }))
    }

    fn render_form(
        &mut self,
        snapshot: PillSnapshot,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let close = |this: &Self, action: PillAction, cx: &mut Context<Self>| {
            this.action_chip("capsule-close", ChipKind::Icon, action, c, cx)
                .child(icon("overlay-cancel").size(px(12.)))
                .into_any_element()
        };
        match snapshot.pres {
            Pres::Hidden => Vec::new(),
            Pres::Selecting { mode, hold } => {
                let mut items = vec![self.modes(Some(mode), false, c, cx).into_any_element()];
                if mode.shows_record() && !hold {
                    items.push(
                        self.action_chip(
                            "capsule-record",
                            ChipKind::Live,
                            PillAction::RecordSelected,
                            c,
                            cx,
                        )
                        .child(div().size(px(8.)).rounded_full().bg(gpui::white()))
                        .child(t("annotate.capsule.record"))
                        .into_any_element(),
                    );
                }
                items.push(capsule::divider(c).into_any_element());
                items.push(close(self, PillAction::Cancel, cx));
                items
            }
            Pres::Recording {
                chips,
                stroke_count,
            } => {
                let lead = capsule::status_row()
                    .child(capsule::live_dot(true, c))
                    .child(capsule::meter(&self.levels, true, c))
                    .child(capsule::clock(snapshot.clock.clone(), c))
                    .when(stroke_count > 0, |el| {
                        el.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(3.))
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(c.secondary)
                                .when(crate::i18n::is_rtl(), |el| el.flex_row_reverse())
                                .child(icon("pen-line").size(px(13.)))
                                .child(stroke_count.to_string()),
                        )
                    });
                let rtl = crate::i18n::is_rtl();
                let mode_chips = (chips || self.chips_fold.running()).then(|| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .when(rtl, |el| el.flex_row_reverse())
                        .child(capsule::divider(c))
                        .child(self.modes(Some(SelectionMode::Screen), true, c, cx))
                        .child(capsule::divider(c))
                        .child(close(self, PillAction::Cancel, cx))
                });
                let mode_chips = mode_chips.map(|chips| {
                    self.chips_fold
                        .element(chips, cx.entity().downgrade(), |this: &mut Self| {
                            &mut this.chips_fold
                        })
                });
                let can_undo = stroke_count > 0;
                let buttons = div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .when(rtl, |el| el.flex_row_reverse())
                    .child(capsule::divider(c))
                    .child(
                        capsule::chip("capsule-undo", ChipKind::Icon, c)
                            .when(can_undo, |el| {
                                el.on_click(
                                    cx.listener(|this, _, _, _| this.send(PillAction::Undo)),
                                )
                            })
                            .when(!can_undo, |el| el.opacity(0.4).cursor_default())
                            .child(icon("undo-2").size(px(13.))),
                    )
                    .child(
                        self.action_chip(
                            "capsule-cancel",
                            ChipKind::Plain,
                            PillAction::Cancel,
                            c,
                            cx,
                        )
                        .child(icon("overlay-cancel").size(px(11.)))
                        .child(self.hint(
                            &snapshot.cancel,
                            c.keycap,
                            c.text.a(0.75),
                            c,
                        )),
                    )
                    .child(
                        self.action_chip(
                            "capsule-finish",
                            ChipKind::Primary,
                            PillAction::Finish,
                            c,
                            cx,
                        )
                        .child(icon("check").size(px(13.)))
                        .child(t("annotate.capsule.finish"))
                        .child(self.hint(
                            &snapshot.shortcut,
                            mix(c.accent, gpui::white(), 0.2),
                            gpui::white(),
                            c,
                        )),
                    );
                vec![
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .when(rtl, |el| el.flex_row_reverse())
                        .child(lead)
                        .children(mode_chips)
                        .child(self.fold.element(
                            buttons,
                            cx.entity().downgrade(),
                            |this: &mut Self| &mut this.fold,
                        ))
                        .into_any_element(),
                ]
            }
            Pres::Processing => vec![
                capsule::status_row()
                    .child(capsule::spinner(c))
                    .child(capsule::label(t("annotate.capsule.processing"), c))
                    .into_any_element(),
            ],
            Pres::Completed { copied } => vec![
                capsule::status_row()
                    .child(capsule::done_mark(c))
                    .child(capsule::label(
                        if copied {
                            t("annotate.capsule.copied")
                        } else {
                            t("annotate.capsule.saved")
                        },
                        c,
                    ))
                    .into_any_element(),
                capsule::divider(c).into_any_element(),
                capsule::chip("capsule-paste", ChipKind::Plain, c)
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, _, _| {
                            this.paste_press = native::pointer().map(|(p, _)| p);
                        }),
                    )
                    .on_click(cx.listener(|this, _, _, _| {
                        this.paste_press = None;
                        this.send(PillAction::Paste(None));
                    }))
                    .child(icon("clipboard-paste").size(px(13.)))
                    .child(t("annotate.capsule.paste"))
                    .into_any_element(),
                self.action_chip(
                    "capsule-inspect",
                    ChipKind::Plain,
                    PillAction::Inspect,
                    c,
                    cx,
                )
                .child(icon("scan-eye").size(px(13.)))
                .child(t("annotate.capsule.inspect"))
                .into_any_element(),
                close(self, PillAction::DismissCompleted, cx),
            ],
            Pres::Failed => vec![
                capsule::status_row()
                    .child(capsule::warn_icon(c))
                    .child(capsule::label(t("annotate.capsule.failed"), c))
                    .into_any_element(),
                close(self, PillAction::DismissCompleted, cx),
            ],
        }
    }
}

impl Render for AnnotateCapsule {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.fold.running() || self.chips_fold.running() {
            window.request_animation_frame();
        }
        let at_top = self.at_top;
        let stage = div()
            .size_full()
            .flex()
            .justify_center()
            .px(px(PAD_X))
            .pt(px(PAD_TOP))
            .pb(px(PAD_BOTTOM))
            .when(at_top, |el| el.items_start())
            .when(!at_top, |el| el.items_end());
        let Some(snapshot) = self.snapshot.clone().filter(|s| s.pres != Pres::Hidden) else {
            return stage;
        };
        let c = Colors::current(cx);
        let items = self.render_form(snapshot, &c, cx);
        let tracked = cx.entity().downgrade();
        let card = capsule::surface("annotate-capsule", px(capsule::RADIUS), &c)
            .h(px(capsule::HEIGHT))
            .items_center()
            .px(px(capsule::INSET - 1.))
            .gap(px(2.))
            .when(crate::i18n::is_rtl(), |el| el.flex_row_reverse())
            .children(items)
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
            .with_animation(
                SharedString::from(format!("annotate-capsule-in-{}", self.shown)),
                Animation::new(Duration::from_millis(220)),
                |card, delta| card.opacity(delta),
            );
        stage.child(card)
    }
}
