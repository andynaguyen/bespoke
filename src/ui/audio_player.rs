use std::cell::RefCell;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{
    App, Bounds, DispatchPhase, ElementId, Entity, MouseButton, MouseMoveEvent, MouseUpEvent,
    Pixels, SharedString, canvas, div, prelude::*, px,
};

use crate::annotate::model::format_clock;
use crate::ui::components::{KeyboardFocus, RowDirection, Typography, icon};
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

pub const BARS: usize = 49;
const PLAY_GLYPH_OPTICAL_OFFSET: f32 = 1.5;

fn perceived_level(relative_rms: f32) -> f32 {
    relative_rms.sqrt()
}

/// The recording's loudness in `buckets` equal slices, each from 0.0 to 1.0, scaled so the
/// loudest slice is 1.0. Reads every sample, so run it off the main thread.
pub fn wav_peaks(path: &Path, buckets: usize) -> Option<Vec<f32>> {
    let mut reader = hound::WavReader::open(path).ok()?;
    let spec = reader.spec();
    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, _) => reader.samples::<f32>().filter_map(Result::ok).collect(),
        (hound::SampleFormat::Int, bits) => {
            let scale = (1_i64 << (bits.clamp(1, 32) - 1)) as f32;
            reader
                .samples::<i32>()
                .filter_map(Result::ok)
                .map(|v| v as f32 / scale)
                .collect()
        }
    };
    if samples.is_empty() || buckets == 0 {
        return None;
    }
    let per = samples.len().div_ceil(buckets);
    let rms: Vec<f32> = samples
        .chunks(per)
        .map(|chunk| (chunk.iter().map(|v| v * v).sum::<f32>() / chunk.len() as f32).sqrt())
        .collect();
    let loudest = rms.iter().copied().fold(0., f32::max);
    if loudest <= f32::EPSILON {
        return Some(vec![0.; rms.len()]);
    }
    Some(rms.iter().map(|v| perceived_level(v / loudest)).collect())
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

pub fn stop() {
    PLAYBACK.with(|p| *p.borrow_mut() = None);
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

#[derive(Default)]
pub struct PlayerState {
    pub duration: Option<Duration>,
    pub peaks: Option<Vec<f32>>,
    pub offset: Duration,
    track: Bounds<Pixels>,
    drag: Option<Duration>,
}

fn finish_drag(state: &Entity<PlayerState>, path: Option<&Path>, cx: &mut App) {
    state.update(cx, |s, cx| {
        if let Some(to) = s.drag.take() {
            if let Some(path) = path {
                seek(path, to);
            }
            s.offset = to;
            cx.notify();
        }
    });
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

    let toggle_state = state.clone();
    let toggle_path = path.clone();
    let seek_state = state.clone();
    let bounds_state = state.clone();
    let drag_state = state.clone();
    let drag_path = path.clone();

    let peaks = state.read(cx).peaks.clone();
    let bars = peaks.as_ref().map_or(BARS, Vec::len).max(1);
    let shown = if playing || !elapsed.is_zero() {
        elapsed
    } else {
        duration
    };

    div()
        .flex()
        .row_dir()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .id((id.clone(), SharedString::from("play")))
                .flex_none()
                .size(px(30.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(p.accent)
                .text_color(p.accent_contrast)
                .cursor_pointer()
                .hover(|s| s.bg(p.accent.a(0.85)))
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
                    .size(px(12.))
                    .when(!playing, |el| el.ml(px(PLAY_GLYPH_OPTICAL_OFFSET))),
                ),
        )
        .child(
            div()
                .id((id.clone(), SharedString::from("seek")))
                .relative()
                .flex_1()
                .min_w_0()
                .h(px(28.))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
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
                .children((0..bars).map(|i| {
                    let level = peaks.as_ref().and_then(|p| p.get(i)).copied().unwrap_or(0.);
                    let played = (i as f32 + 0.5) / bars as f32 <= progress;
                    div()
                        .flex_none()
                        .w(px(2.))
                        .h(px(3. + 25. * level))
                        .rounded_full()
                        .bg(if played { p.accent } else { p.mid_gray.a(0.45) })
                }))
                .child(
                    canvas(
                        move |bounds, _, cx| {
                            bounds_state.update(cx, |s, _| s.track = bounds);
                        },
                        move |_, _, window, _| {
                            // Listen on every paint, not only mid-drag: a trackpad tap can send
                            // down and up within one frame, before a drag-only listener exists.
                            let move_state = drag_state.clone();
                            let move_path = drag_path.clone();
                            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                                if phase != DispatchPhase::Bubble
                                    || move_state.read(cx).drag.is_none()
                                {
                                    return;
                                }
                                if event.pressed_button != Some(MouseButton::Left) {
                                    finish_drag(&move_state, move_path.as_deref(), cx);
                                    return;
                                }
                                move_state.update(cx, |s, cx| {
                                    let duration = s.duration.unwrap_or_default();
                                    s.drag = Some(time_at(event.position.x, s.track, duration));
                                    cx.notify();
                                });
                            });
                            let up_state = drag_state.clone();
                            let up_path = drag_path.clone();
                            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                                if phase == DispatchPhase::Bubble {
                                    finish_drag(&up_state, up_path.as_deref(), cx);
                                }
                            });
                        },
                    )
                    .absolute()
                    .size_full(),
                ),
        )
        .child(
            div()
                .flex_none()
                .min_w(px(30.))
                .flex()
                .row_dir()
                .justify_flex_end()
                .xs_text()
                .text_color(p.text.a(0.55))
                .child(format_clock(shown.as_millis() as u64)),
        )
}
