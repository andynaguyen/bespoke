//! Annotate's capsule, drawn by Bespoke with the capsule dictation uses
//! (`ui::capsule`). Scrawl's coordinator decides what it shows and sends a
//! snapshot over a `CapsuleLink` whenever that changes; clicks go back over
//! the link as `PillAction`s.
//!
//! The window is wider than any form of the capsule, so outside the capsule
//! it lets clicks through to the apps below. A timer polls the pointer for
//! that, as Scrawl's Metal capsule did, and reports hovering, which keeps the
//! switch chips open while recording.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    canvas, div, prelude::*, px, size, Animation, AnimationExt, AnyElement, App, Bounds, Context,
    FontWeight, Pixels, SharedString, Task, Window, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowKind, WindowOptions,
};
use scrawl::host::{
    CapsuleActions, CapsuleCommand, CapsuleLink, CapsuleState, PillAction, PillSnapshot, Pres,
    SelectionMode,
};

use crate::app_handle::AppHandle;
use crate::i18n::{t, t_count};
use crate::settings::{get_settings, OverlayPosition};
use crate::ui::capsule::{self, ChipKind, Colors};
use crate::ui::clay::keycap;
use crate::ui::components::icon;
use crate::ui::platform::capsule::{self as native, CapsuleWindow};
use crate::ui::theme::{mix, Alpha};

/// Wide enough for the widest form, recording with the switch chips open.
const WINDOW_W: f32 = 720.;
/// Margins that keep the glass's shadow inside the window.
const PAD_X: f32 = 18.;
const PAD_TOP: f32 = 10.;
const PAD_BOTTOM: f32 = 18.;
const WINDOW_H: f32 = capsule::HEIGHT + PAD_TOP + PAD_BOTTOM;
/// The capsule's distance from the menu bar, or from the Dock.
const EDGE_GAP: f64 = 12.;
/// How often the pointer is checked while the capsule shows.
const POLL: Duration = Duration::from_millis(33);
/// A Paste press that moves this far drags the capture to where it ends.
const PASTE_DRAG: f64 = 24.;

/// Registers Bespoke as the host that draws Annotate's capsules. Call before
/// `scrawl::start`.
/// The window of the capsule opened last, for dev snapshots.
#[derive(Default)]
struct LastWindow(Option<gpui::AnyWindowHandle>);

impl gpui::Global for LastWindow {}

pub fn window_handle(cx: &App) -> Option<gpui::AnyWindowHandle> {
    cx.try_global::<LastWindow>().and_then(|last| last.0)
}

/// Debug builds: shows the capsule in one of its forms with sample data.
/// `kind` is `choose`, `choose-screen`, `recording`, `switch`,
/// `processing` or `done`.
#[cfg(debug_assertions)]
pub fn dev_preview(kind: &str, cx: &mut App) {
    let pres = match kind {
        "choose-screen" => Pres::Selecting {
            mode: SelectionMode::Screen,
            hold: false,
        },
        "recording" => Pres::Recording {
            chips: false,
            stroke_count: 3,
        },
        "switch" => Pres::Recording {
            chips: true,
            stroke_count: 3,
        },
        "processing" => Pres::Processing,
        "done" => Pres::Completed {
            label: "Copied".into(),
        },
        _ => Pres::Selecting {
            mode: SelectionMode::Area,
            hold: false,
        },
    };
    let anchor = native::pointer().map_or([0., 0.], |(p, _)| p);
    scrawl::host::preview_capsule(
        PillSnapshot {
            pres,
            clock: "0:21".into(),
            reduced_motion: false,
            shortcut: "⌥⇧S".into(),
        },
        anchor,
    );
    let _ = cx;
}

pub fn init(app: &AppHandle) {
    let ui = app.clone();
    scrawl::host::on_capsule(move |link| {
        ui.with_ui(move |cx| open(link, cx));
    });
}

fn open(link: CapsuleLink, cx: &mut App) {
    let CapsuleLink {
        commands,
        actions,
        state,
        excluded_window_ids,
    } = link;
    let view = cx.new(|_| AnnotateCapsule::new(actions, Arc::clone(&state)));
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
    cx.default_global::<LastWindow>().0 = Some(handle.into());

    // The link closes when the coordinator drops the capsule; the window goes
    // with it.
    cx.spawn(async move |cx| {
        while let Ok(command) = commands.recv_async().await {
            let _ = cx.update(|cx| view.update(cx, |capsule, cx| capsule.apply(command, cx)));
        }
        let _ = cx.update(|cx| {
            view.update(cx, |capsule, _| capsule.poll = None);
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        });
    })
    .detach();
}

pub struct AnnotateCapsule {
    actions: CapsuleActions,
    state: Arc<CapsuleState>,
    window: Option<CapsuleWindow>,
    snapshot: Option<PillSnapshot>,
    /// The point the coordinator anchors the capsule to.
    anchor: Option<[f64; 2]>,
    /// Bumped each time the capsule appears, to replay its fade-in.
    shown: u64,
    /// The capsule's bounds in the window, from the last frame.
    card: Bounds<Pixels>,
    /// Where a press on Paste started, to tell a click from a drag.
    paste_press: Option<[f64; 2]>,
    poll: Option<Task<()>>,
}

impl AnnotateCapsule {
    fn new(actions: CapsuleActions, state: Arc<CapsuleState>) -> Self {
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
        }
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

    /// Centers the window on the anchor's display, near the edge the
    /// dictation overlay uses.
    fn place(&self, cx: &App) {
        let Some(window) = &self.window else {
            return;
        };
        let anchor = self.anchor.or_else(|| native::pointer().map(|(p, _)| p));
        let Some(anchor) = anchor else {
            return;
        };
        let at_top = self.at_top();
        let size = [f64::from(WINDOW_W), f64::from(WINDOW_H)];
        let Some(mut origin) = scrawl::host::capsule_origin(anchor, size, at_top, EDGE_GAP) else {
            return;
        };
        // The window's margins sit between the capsule and the edge.
        if at_top {
            origin[1] -= f64::from(PAD_TOP);
        } else {
            origin[1] += f64::from(PAD_BOTTOM);
        }
        window.move_to(origin, cx);
    }

    fn at_top(&self) -> bool {
        get_settings(AppHandle::global()).overlay_position == OverlayPosition::Top
    }

    fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        let Some(window) = &self.window else {
            return;
        };
        window.set_visible(visible, cx);
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
        self.poll = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL).await;
            if this.update(cx, |this, _| this.check_pointer()).is_err() {
                break;
            }
        }));
    }

    /// Lets clicks through outside the capsule, reports hovering, and ends a
    /// Paste drag.
    fn check_pointer(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let Some((pointer, down)) = native::pointer() else {
            return;
        };
        let card = self.card;
        let Some([x, y, _, _]) = window.frame() else {
            return;
        };
        let local = [pointer[0] - x, pointer[1] - y];
        let inside = card.contains(&gpui::point(px(local[0] as f32), px(local[1] as f32)));
        self.state.hovered.store(inside, Ordering::SeqCst);

        if let Some(start) = self.paste_press {
            if !down {
                self.paste_press = None;
                let moved = (pointer[0] - start[0]).hypot(pointer[1] - start[1]);
                if !inside && moved >= PASTE_DRAG {
                    self.send(PillAction::Paste(Some([
                        pointer[0] as f32,
                        pointer[1] as f32,
                    ])));
                }
            }
        }
        let ignore = !inside && self.paste_press.is_none();
        window.set_ignores_mouse(ignore);
    }

    fn send(&self, action: PillAction) {
        let _ = self.actions.send(action);
    }

    /// A chip that sends `action` when clicked.
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

    fn label(&self, text: SharedString, c: &Colors) -> impl IntoElement {
        div()
            .flex_none()
            .text_size(px(13.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(c.text)
            .whitespace_nowrap()
            .child(text)
    }

    fn secondary(&self, text: impl Into<SharedString>, c: &Colors) -> impl IntoElement {
        div()
            .flex_none()
            .text_size(px(13.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(c.secondary)
            .whitespace_nowrap()
            .child(text.into())
    }

    /// A shortcut hint on a chip: a clay keycap in `color` labeled in `label`.
    fn hint(
        &self,
        text: &str,
        color: gpui::Hsla,
        label: gpui::Hsla,
        c: &Colors,
    ) -> impl IntoElement {
        keycap(text.to_string(), 18., color, label, c.dark)
    }

    /// The Area, Window and Screen chips as a segmented control. `active`
    /// is the highlighted one; `switch` picks what a click sends.
    fn modes(
        &self,
        active: Option<SelectionMode>,
        switch: bool,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .rounded_full()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .when(on, |el| el.bg(raised).text_color(text))
                    .when(!on, |el| {
                        el.text_color(secondary).hover(move |s| s.text_color(text))
                    })
                    .on_click(cx.listener(move |this, _, _, _| this.send(action)))
                    .child(icon(icon_name).size(px(15.)))
                    .child(t(key))
            }))
    }

    fn render_form(&mut self, c: &Colors, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(snapshot) = self.snapshot.clone() else {
            return Vec::new();
        };
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
                let mut items = vec![div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .pl(px(10.))
                    .pr(px(4.))
                    .child(capsule::live_dot(true, c))
                    .child(self.secondary(snapshot.clock.clone(), c))
                    .when(stroke_count > 0, |el| {
                        el.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(5.))
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(c.live)
                                .child(icon("pen-line").size(px(13.)))
                                .child(t_count("annotate.capsule.marks", stroke_count as i64, &[])),
                        )
                    })
                    .into_any_element()];
                items.push(capsule::divider(c).into_any_element());
                if chips {
                    items.push(
                        self.modes(Some(SelectionMode::Screen), true, c, cx)
                            .into_any_element(),
                    );
                    items.push(capsule::divider(c).into_any_element());
                    items.push(close(self, PillAction::Cancel, cx));
                } else {
                    let can_undo = stroke_count > 0;
                    items.push(
                        capsule::chip("capsule-undo", ChipKind::Plain, c)
                            .when(can_undo, |el| {
                                el.on_click(
                                    cx.listener(|this, _, _, _| this.send(PillAction::Undo)),
                                )
                            })
                            .when(!can_undo, |el| el.opacity(0.4).cursor_default())
                            .child(icon("undo-2").size(px(14.)))
                            .child(t("annotate.capsule.undo"))
                            .into_any_element(),
                    );
                    items.push(
                        self.action_chip(
                            "capsule-cancel",
                            ChipKind::Plain,
                            PillAction::Cancel,
                            c,
                            cx,
                        )
                        .child(t("annotate.capsule.cancel"))
                        .child(self.hint("esc", c.keycap, c.text.a(0.75), c))
                        .into_any_element(),
                    );
                    items.push(
                        self.action_chip(
                            "capsule-finish",
                            ChipKind::Primary,
                            PillAction::Finish,
                            c,
                            cx,
                        )
                        .child(icon("check").size(px(14.)))
                        .child(t("annotate.capsule.finish"))
                        .child(self.hint(
                            &snapshot.shortcut,
                            mix(c.accent, gpui::white(), 0.2),
                            gpui::white(),
                            c,
                        ))
                        .into_any_element(),
                    );
                }
                items
            }
            Pres::Processing => vec![div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(10.))
                .child(capsule::spinner(c))
                .child(self.label(t("annotate.capsule.processing"), c))
                .into_any_element()],
            Pres::Completed { .. } => vec![
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .pl(px(6.))
                    .pr(px(4.))
                    .child(capsule::done_mark(c))
                    .child(self.label(t("annotate.capsule.copied"), c))
                    .child(self.secondary(snapshot.clock.clone(), c))
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
                    .child(icon("clipboard-paste").size(px(14.)))
                    .child(t("annotate.capsule.paste"))
                    .into_any_element(),
                self.action_chip(
                    "capsule-inspect",
                    ChipKind::Primary,
                    PillAction::Inspect,
                    c,
                    cx,
                )
                .child(icon("scan-eye").size(px(14.)))
                .child(t("annotate.capsule.inspect"))
                .into_any_element(),
                close(self, PillAction::DismissCompleted, cx),
            ],
        }
    }
}

impl Render for AnnotateCapsule {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let at_top = self.at_top();
        let stage = div()
            .size_full()
            .flex()
            .justify_center()
            .px(px(PAD_X))
            .pt(px(PAD_TOP))
            .pb(px(PAD_BOTTOM))
            .when(at_top, |el| el.items_start())
            .when(!at_top, |el| el.items_end());
        if *self.pres() == Pres::Hidden {
            return stage;
        }
        let c = Colors::current(cx);
        let items = self.render_form(&c, cx);
        let tracked = cx.entity().downgrade();
        let card = capsule::surface("annotate-capsule", px(capsule::RADIUS), &c)
            .h(px(capsule::HEIGHT))
            .items_center()
            .px(px(capsule::INSET - 1.))
            .gap(px(2.))
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
