use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

use crate::annotate::capture::annotate_log;
use crate::app_handle::AppHandle;
use crate::audio_feedback::{SoundType, play_feedback_sound, play_feedback_sound_blocking};
use crate::audio_toolkit::{AudioRecorder as Microphone, RecordingStart, VadPolicy};
use crate::settings::get_settings;
use crate::system_output::{self, Hold};

const SAMPLE_RATE: u32 = 16_000;

const WAIT_POLL: Duration = Duration::from_millis(20);

/// Records the microphone for one capture with the device and input channel picked on
/// Bespoke's Sound page. Opening and closing the device happen on the recorder's own thread,
/// so a slow Bluetooth microphone never holds up the UI. Dropping it stops and closes the
/// microphone in the background.
pub struct AudioRecorder {
    worker: JoinHandle<Result<Heard>>,
    stop: mpsc::Sender<()>,
}

/// What the microphone heard: 16 kHz samples, and where they begin.
struct Heard {
    samples: Vec<f32>,
    start: Option<RecordingStart>,
}

impl AudioRecorder {
    pub fn start() -> Result<Self> {
        let (stop, stopped) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("annotate-audio".into())
            .spawn(move || record(&stopped))
            .context("Could not spawn microphone worker")?;
        Ok(Self { worker, stop })
    }

    /// Stops the microphone and returns its 16 kHz samples, lined up so that sample 0 is at
    /// `started_at`, the capture's clock.
    pub fn stop(self, started_at: Instant) -> Result<Vec<f32>> {
        let Self { worker, stop } = self;
        let _ = stop.send(());
        let Heard { mut samples, start } = worker
            .join()
            .map_err(|_| anyhow!("Microphone worker panicked"))??;
        if samples.is_empty() {
            bail!("Microphone heard nothing");
        }
        let Some(start) = start else {
            return Ok(samples);
        };
        samples.drain(..start.leading_samples.min(samples.len()));
        let lag_ms = match start.heard_at.checked_duration_since(started_at) {
            Some(late) => late.as_millis() as i128,
            None => -(started_at.duration_since(start.heard_at).as_millis() as i128),
        };
        annotate_log(format!(
            "Microphone: first sample {lag_ms} ms after the capture started"
        ));
        Ok(align_to_clock(samples, start.heard_at, started_at))
    }
}

/// Opens the microphone, records until `stopped` fires or its sender is dropped, then closes
/// the microphone so the system's indicator turns off.
fn record(stopped: &mpsc::Receiver<()>) -> Result<Heard> {
    let app = AppHandle::try_global();
    let settings = app.map(get_settings);
    let channel = settings.as_ref().and_then(|s| s.selected_channel);
    let preferred = settings
        .as_ref()
        .and_then(crate::managers::audio::preferred_input_device);
    let mut microphone = open(preferred, channel)?;
    let started = microphone
        .start(VadPolicy::Disabled)
        .map_err(|e| anyhow!("Could not start the microphone: {e}"));
    let ended = match &started {
        Ok(ready) => hold_until_stopped(ready, stopped, app),
        Err(_) => Ended::silently(),
    };
    let samples = started.and_then(|_| {
        microphone
            .stop()
            .map_err(|e| anyhow!("Could not stop the microphone: {e}"))
    });
    let start = microphone.recording_start();
    ended.restore_output_then_play_stop_sound(app);
    microphone.close();
    Ok(Heard {
        samples: samples?,
        start,
    })
}

fn hold_until_stopped(
    ready: &mpsc::Receiver<()>,
    stopped: &mpsc::Receiver<()>,
    app: Option<&AppHandle>,
) -> Ended {
    if wait_for(ready, stopped) != Waited::Arrived {
        return Ended::silently();
    }
    let Some(app) = app else {
        return Ended::after(None, wait_for_end(stopped));
    };
    match wait_for(&spawn_start_sound(app), stopped) {
        Waited::Arrived => {}
        waited => return Ended::after(None, waited),
    }
    let output_hold = system_output::lower(get_settings(app).recording_output_volume);
    Ended::after(output_hold, wait_for_end(stopped))
}

struct Ended {
    output_hold: Option<Hold>,
    play_stop_sound: bool,
}

impl Ended {
    fn silently() -> Self {
        Self {
            output_hold: None,
            play_stop_sound: false,
        }
    }

    fn after(output_hold: Option<Hold>, waited: Waited) -> Self {
        Self {
            output_hold,
            play_stop_sound: waited == Waited::Finished,
        }
    }

    fn restore_output_then_play_stop_sound(self, app: Option<&AppHandle>) {
        drop(self.output_hold);
        if self.play_stop_sound
            && let Some(app) = app
        {
            play_feedback_sound(app, SoundType::Stop);
        }
    }
}

fn spawn_start_sound(app: &AppHandle) -> mpsc::Receiver<()> {
    let (done, ended) = mpsc::channel();
    let app = app.clone();
    std::thread::spawn(move || {
        play_feedback_sound_blocking(&app, SoundType::Start);
        let _ = done.send(());
    });
    ended
}

#[derive(Debug, PartialEq, Eq)]
enum Waited {
    Arrived,
    Finished,
    Discarded,
}

fn wait_for(signal: &mpsc::Receiver<()>, stopped: &mpsc::Receiver<()>) -> Waited {
    loop {
        match stopped.try_recv() {
            Ok(()) => return Waited::Finished,
            Err(mpsc::TryRecvError::Disconnected) => return Waited::Discarded,
            Err(mpsc::TryRecvError::Empty) => {}
        }
        match signal.recv_timeout(WAIT_POLL) {
            Ok(()) => return Waited::Arrived,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return wait_for_end(stopped),
        }
    }
}

fn wait_for_end(stopped: &mpsc::Receiver<()>) -> Waited {
    match stopped.recv() {
        Ok(()) => Waited::Finished,
        Err(mpsc::RecvError) => Waited::Discarded,
    }
}

/// Opens `preferred`, or the system default when there is none or it won't open.
fn open(preferred: Option<(String, cpal::Device)>, channel: Option<u16>) -> Result<Microphone> {
    if let Some((name, device)) = preferred {
        let mut microphone = Microphone::new().with_selected_channel(channel);
        match microphone.open(Some(device)) {
            Ok(()) => {
                annotate_log(format!("Microphone: {name}"));
                return Ok(microphone);
            }
            Err(error) => annotate_log(format!(
                "Microphone: could not open {name} ({error}); using the default"
            )),
        }
    }
    let mut microphone = Microphone::new().with_selected_channel(channel);
    microphone
        .open(None)
        .map_err(|e| anyhow!("Could not open the default microphone: {e}"))?;
    annotate_log("Microphone: the system default");
    Ok(microphone)
}

/// Pads silence before `samples` when the first one was heard after `started_at`, or drops
/// the ones heard before it, so that sample 0 falls at `started_at`.
fn align_to_clock(mut samples: Vec<f32>, heard_at: Instant, started_at: Instant) -> Vec<f32> {
    let in_samples = |gap: Duration| (gap.as_secs_f64() * f64::from(SAMPLE_RATE)).round() as usize;
    match heard_at.checked_duration_since(started_at) {
        Some(late) => {
            samples.splice(0..0, std::iter::repeat_n(0.0, in_samples(late)));
        }
        None => {
            let early = in_samples(started_at.duration_since(heard_at));
            samples.drain(..early.min(samples.len()));
        }
    }
    samples
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorder(heard: Result<Heard>) -> (AudioRecorder, mpsc::Receiver<()>) {
        let (stop, stopped) = mpsc::channel();
        let (seen, saw_stop) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = stopped.recv();
            let _ = seen.send(());
            heard
        });
        (AudioRecorder { worker, stop }, saw_stop)
    }

    #[test]
    fn stop_waits_for_the_worker_and_lines_its_samples_up() {
        let started_at = Instant::now();
        let mut samples = vec![9.0; 3];
        samples.extend([0.3; 160]);
        let (recorder, saw_stop) = recorder(Ok(Heard {
            samples,
            start: Some(RecordingStart {
                heard_at: started_at + Duration::from_millis(500),
                leading_samples: 3,
            }),
        }));
        let audio = recorder.stop(started_at).unwrap();
        assert!(saw_stop.try_recv().is_ok());
        assert_eq!(audio.len(), 8_000 + 160);
        assert!(audio[..8_000].iter().all(|&s| s == 0.0));
        assert!(audio[8_000..].iter().all(|&s| s == 0.3));
    }

    #[test]
    fn stop_reports_a_microphone_that_failed_or_heard_nothing() {
        let (failed, _) = recorder(Err(anyhow!("device unavailable")));
        let error = failed.stop(Instant::now()).unwrap_err();
        assert!(error.to_string().contains("device unavailable"));

        let (silent, _) = recorder(Ok(Heard {
            samples: Vec::new(),
            start: None,
        }));
        assert!(silent.stop(Instant::now()).is_err());
    }

    #[test]
    fn stopping_signals_the_worker_but_dropping_does_not() {
        let (stop, stopped) = mpsc::channel();
        let (seen, saw) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = seen.send(stopped.recv().is_ok());
            Ok(Heard {
                samples: vec![0.1],
                start: None,
            })
        });
        AudioRecorder { worker, stop }.stop(Instant::now()).unwrap();
        assert!(saw.recv().unwrap());

        let (stop, stopped) = mpsc::channel();
        let (seen, saw) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = seen.send(stopped.recv().is_ok());
            Ok(Heard {
                samples: Vec::new(),
                start: None,
            })
        });
        drop(AudioRecorder { worker, stop });
        assert!(!saw.recv().unwrap());
    }

    #[test]
    fn a_signal_arrives_while_the_capture_runs() {
        let (signal_tx, signal) = mpsc::channel();
        let (_stop, stopped) = mpsc::channel::<()>();
        signal_tx.send(()).unwrap();
        assert_eq!(wait_for(&signal, &stopped), Waited::Arrived);
    }

    #[test]
    fn a_wait_ends_when_the_capture_does() {
        let (_signal_tx, signal) = mpsc::channel::<()>();
        let (stop, stopped) = mpsc::channel();
        stop.send(()).unwrap();
        assert_eq!(wait_for(&signal, &stopped), Waited::Finished);

        let (_signal_tx, signal) = mpsc::channel::<()>();
        let (stop, stopped) = mpsc::channel::<()>();
        drop(stop);
        assert_eq!(wait_for(&signal, &stopped), Waited::Discarded);
    }

    #[test]
    fn a_signal_that_can_never_fire_waits_for_the_capture_to_end() {
        let (signal_tx, signal) = mpsc::channel::<()>();
        drop(signal_tx);
        let (stop, stopped) = mpsc::channel();
        let waiter = std::thread::spawn(move || wait_for(&signal, &stopped));
        std::thread::sleep(Duration::from_millis(50));
        assert!(!waiter.is_finished());
        stop.send(()).unwrap();
        assert_eq!(waiter.join().unwrap(), Waited::Finished);
    }

    #[test]
    fn a_microphone_that_started_late_gets_silence_in_front() {
        let started_at = Instant::now();
        let heard_at = started_at + Duration::from_millis(250);
        let aligned = align_to_clock(vec![0.5; 100], heard_at, started_at);
        assert_eq!(aligned.len(), 4_000 + 100);
        assert!(aligned[..4_000].iter().all(|&s| s == 0.0));
        assert_eq!(aligned[4_000], 0.5);
    }

    #[test]
    fn a_microphone_that_started_early_loses_what_came_before_the_capture() {
        let heard_at = Instant::now();
        let started_at = heard_at + Duration::from_millis(100);
        let mut heard = vec![0.1; 1_600];
        heard.extend([0.9; 50]);
        assert_eq!(align_to_clock(heard, heard_at, started_at), vec![0.9; 50]);
        assert!(align_to_clock(vec![0.1; 10], heard_at, started_at).is_empty());
    }
}
