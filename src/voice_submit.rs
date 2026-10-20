//! Hands-free submit keyword. While a recording is armed, this watches for a
//! pause after speech, checks whether the audio before the pause ends with the
//! keyword, and asks the coordinator to stop the recording. The final
//! transcript then decides whether to remove the keyword and press submit.

use crate::app_handle::AppHandle;
use crate::audio_toolkit::FrameObserver;
use crate::managers::transcription::TranscriptionManager;
use crate::transcription_coordinator::TranscriptionCoordinator;
use log::debug;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

const SAMPLES_PER_MS: usize = 16;
/// Silence after speech that ends an utterance.
const PAUSE_SAMPLES: usize = 600 * SAMPLES_PER_MS;
/// Voiced audio needed in a row to count as speech, so one noisy frame
/// neither starts speech nor resets the pause.
const ONSET_SAMPLES: usize = 60 * SAMPLES_PER_MS;
/// Audio kept for a check; enough for a sentence ending in the keyword.
const TAIL_SAMPLES: usize = 8_000 * SAMPLES_PER_MS;

#[derive(Debug, PartialEq, Eq)]
enum FrameEvent {
    None,
    SpeechStarted,
    Paused,
}

/// Speech and pause state for one recording, fed one frame at a time.
#[derive(Default)]
struct PauseTracker {
    tail: VecDeque<f32>,
    voiced_run: usize,
    silence: usize,
    in_speech: bool,
}

impl PauseTracker {
    fn push(&mut self, frame: &[f32], voiced: bool) -> FrameEvent {
        self.tail.extend(frame.iter().copied());
        let excess = self.tail.len().saturating_sub(TAIL_SAMPLES);
        self.tail.drain(..excess);

        if voiced {
            self.voiced_run += frame.len();
        } else {
            self.voiced_run = 0;
        }

        if self.voiced_run >= ONSET_SAMPLES {
            self.silence = 0;
            if !self.in_speech {
                self.in_speech = true;
                return FrameEvent::SpeechStarted;
            }
        } else if !voiced {
            self.silence += frame.len();
            if self.in_speech && self.silence >= PAUSE_SAMPLES {
                self.in_speech = false;
                return FrameEvent::Paused;
            }
        }
        FrameEvent::None
    }
}

struct Session {
    id: u64,
    keyword: String,
    /// The model streams, so the live transcript is checked instead of
    /// decoding the audio tail (the stream holds the engine).
    streaming: bool,
}

/// A pause worth checking. `revision` identifies the speech before it.
struct Pause {
    session: u64,
    revision: u64,
    tail: Vec<f32>,
}

pub struct VoiceSubmit {
    app: AppHandle,
    armed: AtomicBool,
    session_id: AtomicU64,
    /// Bumped whenever speech starts. A check that began before the user kept
    /// talking carries an older revision and can no longer stop recording.
    revision: AtomicU64,
    session: Mutex<Option<Session>>,
    tracker: Mutex<PauseTracker>,
    pauses: mpsc::Sender<Pause>,
}

impl VoiceSubmit {
    pub fn new(app: &AppHandle) -> Arc<Self> {
        let (pauses, rx) = mpsc::channel();
        let voice_submit = Arc::new(Self {
            app: app.clone(),
            armed: AtomicBool::new(false),
            session_id: AtomicU64::new(0),
            revision: AtomicU64::new(0),
            session: Mutex::new(None),
            tracker: Mutex::new(PauseTracker::default()),
            pauses,
        });
        let worker = Arc::clone(&voice_submit);
        thread::spawn(move || worker.run(rx));
        voice_submit
    }

    /// Start watching a new recording. Call before capture begins so the
    /// recorder resets its detector for this session.
    pub fn arm(&self, keyword: &str, streaming: bool) {
        let id = self.session_id.fetch_add(1, Ordering::AcqRel) + 1;
        *self.tracker.lock().unwrap() = PauseTracker::default();
        self.revision.store(0, Ordering::Release);
        *self.session.lock().unwrap() = Some(Session {
            id,
            keyword: keyword.to_string(),
            streaming,
        });
        self.armed.store(true, Ordering::Release);
        debug!("Voice submit armed (streaming={streaming})");
    }

    /// Stop watching. Pending and in-flight checks become stale.
    pub fn disarm(&self) {
        self.armed.store(false, Ordering::Release);
        *self.session.lock().unwrap() = None;
    }

    /// Whether a check for `session` at `revision` may still stop recording:
    /// the same recording is armed and no speech has started since.
    pub fn is_current(&self, session: u64, revision: u64) -> bool {
        self.armed.load(Ordering::Acquire)
            && self.session_id.load(Ordering::Acquire) == session
            && self.revision.load(Ordering::Acquire) == revision
    }

    pub fn keyword_for(&self, session: u64) -> Option<String> {
        self.session
            .lock()
            .unwrap()
            .as_ref()
            .filter(|s| s.id == session)
            .map(|s| s.keyword.clone())
    }

    /// The keyword was heard before a pause; ask the coordinator to stop. The
    /// coordinator checks `is_current` again on its own thread before acting.
    pub fn request_stop(&self, session: u64, revision: u64) {
        if !self.is_current(session, revision) {
            return;
        }
        debug!("Voice submit keyword heard before a pause; stopping recording");
        if let Some(coordinator) = self.app.try_state::<TranscriptionCoordinator>() {
            coordinator.notify_voice_stop(session, revision);
        }
    }

    fn run(&self, rx: mpsc::Receiver<Pause>) {
        while let Ok(mut pause) = rx.recv() {
            // Only the newest pause matters; older ones are superseded.
            while let Ok(newer) = rx.try_recv() {
                pause = newer;
            }
            if !self.is_current(pause.session, pause.revision) {
                continue;
            }
            let Some((keyword, streaming)) = self
                .session
                .lock()
                .unwrap()
                .as_ref()
                .filter(|s| s.id == pause.session)
                .map(|s| (s.keyword.clone(), s.streaming))
            else {
                continue;
            };

            let tm = self.app.state::<Arc<TranscriptionManager>>();
            if streaming {
                tm.request_voice_submit_check(pause.session, pause.revision);
            } else if tm.voice_submit_probe(pause.tail, &keyword) {
                self.request_stop(pause.session, pause.revision);
            }
        }
    }
}

impl FrameObserver for VoiceSubmit {
    fn is_active(&self) -> bool {
        self.armed.load(Ordering::Relaxed)
    }

    fn observe(&self, frame: &[f32], voiced: Option<bool>) {
        // Without a speech decision there is no pause to detect.
        let Some(voiced) = voiced else {
            return;
        };
        let mut tracker = self.tracker.lock().unwrap();
        match tracker.push(frame, voiced) {
            FrameEvent::SpeechStarted => {
                self.revision.fetch_add(1, Ordering::AcqRel);
            }
            FrameEvent::Paused => {
                let _ = self.pauses.send(Pause {
                    session: self.session_id.load(Ordering::Acquire),
                    revision: self.revision.load(Ordering::Acquire),
                    tail: tracker.tail.iter().copied().collect(),
                });
            }
            FrameEvent::None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: usize = 480; // 30 ms

    fn push_ms(tracker: &mut PauseTracker, ms: usize, voiced: bool) -> Vec<FrameEvent> {
        let frame = vec![0.0; FRAME];
        (0..ms / 30)
            .map(|_| tracker.push(&frame, voiced))
            .filter(|event| *event != FrameEvent::None)
            .collect()
    }

    #[test]
    fn pause_after_speech_fires_once() {
        let mut tracker = PauseTracker::default();
        assert_eq!(
            push_ms(&mut tracker, 900, true),
            vec![FrameEvent::SpeechStarted]
        );
        assert_eq!(push_ms(&mut tracker, 570, false), vec![]);
        assert_eq!(push_ms(&mut tracker, 30, false), vec![FrameEvent::Paused]);
        assert_eq!(push_ms(&mut tracker, 3000, false), vec![]);
    }

    #[test]
    fn silence_before_any_speech_never_fires() {
        let mut tracker = PauseTracker::default();
        assert_eq!(push_ms(&mut tracker, 5000, false), vec![]);
    }

    #[test]
    fn a_single_voiced_frame_does_not_reset_the_pause() {
        let mut tracker = PauseTracker::default();
        push_ms(&mut tracker, 900, true);
        push_ms(&mut tracker, 300, false);
        assert_eq!(push_ms(&mut tracker, 30, true), vec![]);
        assert_eq!(push_ms(&mut tracker, 300, false), vec![FrameEvent::Paused]);
    }

    #[test]
    fn resumed_speech_starts_a_new_segment() {
        let mut tracker = PauseTracker::default();
        push_ms(&mut tracker, 900, true);
        push_ms(&mut tracker, 600, false);
        assert_eq!(
            push_ms(&mut tracker, 300, true),
            vec![FrameEvent::SpeechStarted]
        );
        assert_eq!(push_ms(&mut tracker, 600, false), vec![FrameEvent::Paused]);
    }

    #[test]
    fn tail_keeps_only_the_most_recent_audio() {
        let mut tracker = PauseTracker::default();
        push_ms(&mut tracker, 12_000, true);
        assert_eq!(tracker.tail.len(), TAIL_SAMPLES);
    }
}
