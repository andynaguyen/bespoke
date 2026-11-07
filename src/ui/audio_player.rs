use std::cell::RefCell;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{
    App, Bounds, DispatchPhase, ElementId, Entity, MouseButton, MouseMoveEvent, MouseUpEvent,
    Pixels, SharedString, Window, canvas, div, prelude::*, px, rems,
};

use crate::ui::components::{KeyboardFocus, Typography, icon};
use crate::ui::theme::{Alpha, palette};

struct Playback {
    _stream: rodio::OutputStream,
    sink: rodio::Sink,
    path: PathBuf,
}

thread_local! {
    static PLAYBACK: RefCell<Option<Playback>> = const { RefCell::new(None) };
}

pub fn wav_duration(path: &Path) -> Option<Duration> {
    let reader = hound::WavReader::open(path).ok()?;
    let spec = reader.spec();
    Some(Duration::from_secs_f64(
        reader.duration() as f64 / spec.sample_rate as f64,
    ))
}

fn open(path: &Path, from: Duration) -> Result<Playback, String> {
    let stream = rodio::OutputStreamBuilder::from_default_device()
        .and_then(|b| b.open_stream())
        .map_err(|e| e.to_string())?;
    let file = File::open(path).map_err(|e| e.to_string())?;
    let sink = rodio::play(stream.mixer(), BufReader::new(file)).map_err(|e| e.to_string())?;
    if !from.is_zero() {
        let _ = sink.try_seek(from);
    }
    Ok(Playback {
        _stream: stream,
        sink,
        path: path.to_path_buf(),
    })
}

pub fn is_playing(path: &Path) -> bool {
    PLAYBACK.with(|p| {
        p.borrow()
            .as_ref()
            .is_some_and(|p| p.path == path && !p.sink.is_paused() && !p.sink.empty())
    })
}

pub fn position(path: &Path) -> Option<Duration> {
    PLAYBACK.with(|p| {
        p.borrow()
            .as_ref()
            .filter(|p| p.path == path)
            .map(|p| p.sink.get_pos())
    })
}

fn ended(path: &Path) -> bool {
    PLAYBACK.with(|p| {
        p.borrow()
            .as_ref()
            .is_some_and(|p| p.path == path && p.sink.empty())
    })
}

pub fn play(path: &Path, from: Duration) {
    PLAYBACK.with(|p| {
        let mut slot = p.borrow_mut();
        if let Some(current) = slot.as_ref()
            && current.path == path
            && !current.sink.empty()
        {
            let _ = current.sink.try_seek(from);
            current.sink.play();
            return;
        }
        *slot = match open(path, from) {
            Ok(playback) => Some(playback),
            Err(e) => {
                log::error!("Failed to play {}: {e}", path.display());
                None
            }
        };
    });
}

pub fn pause(path: &Path) {
    PLAYBACK.with(|p| {
        if let Some(current) = p.borrow().as_ref().filter(|p| p.path == path) {
            current.sink.pause();
        }
    });
}

pub fn seek(path: &Path, to: Duration) {
    PLAYBACK.with(|p| {
        let mut slot = p.borrow_mut();
        match slot.as_ref().filter(|p| p.path == path) {
            Some(current) if current.sink.empty() => *slot = None,
            Some(current) => {
                let _ = current.sink.try_seek(to);
            }
            None => {}
        }
    });
}

fn format_time(duration: Duration) -> String {
    let secs = duration.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[derive(Default)]
pub struct PlayerState {
    pub duration: Option<Duration>,
    pub offset: Duration,
    track: Bounds<Pixels>,
    drag: Option<Duration>,
}

fn time_at(x: Pixels, track: Bounds<Pixels>, duration: Duration) -> Duration {
    let fraction = ((x - track.left()) / track.size.width.max(px(1.))).clamp(0., 1.);
    duration.mul_f32(fraction)
}

pub fn audio_player<I: Into<ElementId>, R: Fn(&mut App) -> Option<PathBuf> + 'static>(
    id: I,
    path: Option<PathBuf>,
    state: Entity<PlayerState>,
    resolve: R,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<I, R> {
    let p = palette(cx);
    let id: ElementId = id.into();
    let playing = path.as_deref().is_some_and(is_playing);
    let duration = state.read(cx).duration.unwrap_or_default();
    let elapsed = state
        .read(cx)
        .drag
        .or_else(|| path.as_deref().and_then(position))
        .unwrap_or(state.read(cx).offset)
        .min(duration);
    let progress = if duration.is_zero() {
        0.
    } else {
        elapsed.as_secs_f32() / duration.as_secs_f32()
    };
    let _ = window;

    let toggle_state = state.clone();
    let toggle_path = path.clone();
    let seek_state = state.clone();
    let bounds_state = state.clone();
    let drag_state = state.clone();
    let drag_path = path.clone();

    div()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .id((id.clone(), SharedString::from("play")))
                .flex_shrink_0()
                .cursor_pointer()
                .text_color(p.text.a(0.8))
                .hover(|s| s.text_color(p.text))
                .keyboard_focus(cx)
                .on_click(move |_, _, cx| {
                    let Some(path) = toggle_path.clone().or_else(|| resolve(cx)) else {
                        return;
                    };
                    if is_playing(&path) {
                        let pos = position(&path).unwrap_or_default();
                        pause(&path);
                        toggle_state.update(cx, |s, cx| {
                            s.offset = pos;
                            cx.notify();
                        });
                    } else {
                        let (duration, offset) = {
                            let s = toggle_state.read(cx);
                            (s.duration, s.offset)
                        };
                        let duration = duration.or_else(|| wav_duration(&path));
                        let from = if ended(&path) || duration.is_some_and(|d| offset >= d) {
                            Duration::ZERO
                        } else {
                            offset
                        };
                        play(&path, from);
                        toggle_state.update(cx, |s, cx| {
                            s.duration = duration;
                            cx.notify();
                        });
                    }
                })
                .child(
                    icon(if playing {
                        "pause-filled"
                    } else {
                        "play-filled"
                    })
                    .size(px(16.)),
                ),
        )
        .child(
            div()
                .xs_text()
                .text_color(p.text.a(0.5))
                .min_w(px(30.))
                .child(format_time(elapsed)),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .id((id.clone(), SharedString::from("seek")))
                        .relative()
                        .flex_1()
                        .h(px(16.))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                            let (track, duration) = {
                                let s = seek_state.read(cx);
                                (s.track, s.duration)
                            };
                            let Some(duration) = duration else {
                                return;
                            };
                            let to = time_at(event.position.x, track, duration);
                            seek_state.update(cx, |s, cx| {
                                s.drag = Some(to);
                                cx.notify();
                            });
                        })
                        .child(
                            div()
                                .w_full()
                                .h(rems(0.25))
                                .rounded_full()
                                .overflow_hidden()
                                .flex()
                                .bg(p.mid_gray.a(0.3))
                                .child(div().h_full().w(gpui::relative(progress)).bg(p.accent)),
                        )
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .left(
                                    (bounds_track_width(&state, cx) - px(16.)).max(px(0.))
                                        * progress,
                                )
                                .size(px(16.))
                                .rounded_full()
                                .bg(gpui::white())
                                .shadow(vec![
                                    crate::ui::theme::shadow(0., 0., 0., 1., 0.12),
                                    crate::ui::theme::shadow(0., 1., 3., 0., 0.2),
                                ]),
                        )
                        .child(
                            canvas(
                                move |bounds, _, cx| {
                                    bounds_state.update(cx, |s, _| s.track = bounds);
                                },
                                move |_, _, window, cx| {
                                    if drag_state.read(cx).drag.is_none() {
                                        return;
                                    }
                                    let move_state = drag_state.clone();
                                    window.on_mouse_event(
                                        move |event: &MouseMoveEvent, phase, _, cx| {
                                            if phase != DispatchPhase::Bubble {
                                                return;
                                            }
                                            move_state.update(cx, |s, cx| {
                                                let duration = s.duration.unwrap_or_default();
                                                s.drag = Some(time_at(
                                                    event.position.x,
                                                    s.track,
                                                    duration,
                                                ));
                                                cx.notify();
                                            });
                                        },
                                    );
                                    let up_state = drag_state.clone();
                                    let up_path = drag_path.clone();
                                    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                                        if phase != DispatchPhase::Bubble {
                                            return;
                                        }
                                        up_state.update(cx, |s, cx| {
                                            if let Some(to) = s.drag.take() {
                                                if let Some(path) = &up_path {
                                                    seek(path, to);
                                                }
                                                s.offset = to;
                                            }
                                            cx.notify();
                                        });
                                    });
                                },
                            )
                            .absolute()
                            .size_full(),
                        ),
                )
                .child(
                    div()
                        .xs_text()
                        .text_color(p.text.a(0.5))
                        .min_w(px(30.))
                        .child(format_time(duration)),
                ),
        )
}

fn bounds_track_width(state: &Entity<PlayerState>, cx: &App) -> Pixels {
    state.read(cx).track.size.width
}
