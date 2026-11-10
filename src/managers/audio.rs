use crate::audio_toolkit::{
    AudioRecorder, FrameObserver, SileroVad, VadPolicy, VoiceActivityDetector, list_input_devices,
    vad::{
        EarshotVad, SmoothedVad, VAD_OFFLINE_HANGOVER_MS, VAD_ONSET_MS, VAD_PREFILL_MS,
        VAD_STREAMING_HANGOVER_MS, frames_for_duration_ms,
    },
};
use crate::helpers::clamshell;
use crate::managers::transcription::StreamRouter;
use crate::settings::{AppSettings, VadBackend, get_settings};
use crate::utils;
use log::{debug, error, info, trace, warn};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const SILERO_VAD_THRESHOLD: f32 = 0.3;
const EARSHOT_VAD_THRESHOLD: f32 = 0.5;

fn set_mute(mute: bool) {
    use std::process::Command;
    let script = format!(
        "set volume output muted {}",
        if mute { "true" } else { "false" }
    );
    let _ = Command::new("osascript").args(["-e", &script]).output();
}

fn get_mute() -> Option<bool> {
    use std::process::Command;

    let out = Command::new("osascript")
        .args(["-e", "output muted of (get volume settings)"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    match String::from_utf8_lossy(&out.stdout).trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn restore_mute(prev_muted: Option<bool>) {
    if prev_muted != Some(true) {
        set_mute(false);
    }
}

const WHISPER_SAMPLE_RATE: usize = 16000;

#[derive(Clone, Debug)]
pub enum RecordingState {
    Idle,
    Recording { binding_id: String },
    Stopping,
}

#[derive(Clone, Debug)]
pub enum MicrophoneMode {
    AlwaysOn,
    OnDemand,
}

#[derive(Debug, Default, Clone, Copy)]
struct MuteState {
    did_mute: bool,
    prev_muted: Option<bool>,
}

impl MuteState {
    /// Mutes the output and remembers how it was, unless this already muted it.
    fn apply(&mut self, read: impl FnOnce() -> Option<bool>, mute: impl FnOnce()) {
        if self.did_mute {
            return;
        }
        self.prev_muted = read();
        mute();
        self.did_mute = true;
    }

    /// Puts the output back the way `apply` found it. Does nothing when `apply` did not mute.
    fn remove(&mut self, restore: impl FnOnce(Option<bool>)) {
        if self.did_mute {
            restore(self.prev_muted);
            self.did_mute = false;
        }
    }
}

enum DesiredMicrophone {
    Default,
    Priority(Vec<String>),
    Clamshell(String),
}

fn desired_microphone(settings: &AppSettings) -> DesiredMicrophone {
    if let Some(clamshell_microphone) = &settings.clamshell_microphone {
        let clamshell_started = Instant::now();
        let is_clamshell = clamshell::is_clamshell().unwrap_or(false);
        debug!(
            "device resolve: clamshell_check={:?} (clamshell={})",
            clamshell_started.elapsed(),
            is_clamshell
        );
        if is_clamshell {
            return DesiredMicrophone::Clamshell(clamshell_microphone.clone());
        }
    }
    if settings.microphone_priority.is_empty() {
        DesiredMicrophone::Default
    } else {
        DesiredMicrophone::Priority(settings.microphone_priority.clone())
    }
}

/// The first microphone in `priority` whose name is in `connected`, or `None` when none of them
/// are, which means the system default.
pub fn first_connected<'a, S: AsRef<str>>(
    priority: &'a [String],
    connected: &[S],
) -> Option<&'a str> {
    priority
        .iter()
        .find(|name| connected.iter().any(|c| c.as_ref() == name.as_str()))
        .map(String::as_str)
}

/// The input Bespoke records from under `settings`, or `None` for the system default. The
/// clamshell microphone wins while the lid is closed. Annotate asks for it at the start of each
/// recording, so Dictate and Annotate always use the same microphone.
pub fn preferred_input_device(settings: &AppSettings) -> Option<(String, cpal::Device)> {
    match desired_microphone(settings) {
        DesiredMicrophone::Default => None,
        DesiredMicrophone::Clamshell(name) => first_connected_device(&[name]),
        DesiredMicrophone::Priority(list) => first_connected_device(&list),
    }
}

/// The first microphone in `wanted` that is connected now.
fn first_connected_device(wanted: &[String]) -> Option<(String, cpal::Device)> {
    let devices = list_input_devices()
        .inspect_err(|e| debug!("Failed to list devices, using default: {e}"))
        .ok()?;
    let names: Vec<&str> = devices.iter().map(|d| d.name.as_str()).collect();
    let pick = first_connected(wanted, &names)?.to_string();
    devices
        .into_iter()
        .find(|d| d.name == pick)
        .map(|d| (pick, d.device))
}

fn create_audio_recorder(
    backend: VadBackend,
    app_handle: &crate::app_handle::AppHandle,
    selected_channel: Option<u16>,
    stream_router: Arc<StreamRouter>,
    frame_observer: Arc<dyn FrameObserver>,
) -> Result<AudioRecorder, anyhow::Error> {
    let detector: Box<dyn VoiceActivityDetector> = match backend {
        VadBackend::Silero => {
            let vad_path = app_handle
                .path()
                .resolve(
                    "resources/models/silero_vad_v4.onnx",
                    crate::app_handle::BaseDirectory::Resource,
                )
                .map_err(|e| anyhow::anyhow!("Failed to resolve VAD path: {e}"))?;
            Box::new(
                SileroVad::new(vad_path, SILERO_VAD_THRESHOLD)
                    .map_err(|e| anyhow::anyhow!("Failed to create SileroVad: {e}"))?,
            )
        }
        VadBackend::Earshot => Box::new(
            EarshotVad::new(EARSHOT_VAD_THRESHOLD)
                .map_err(|e| anyhow::anyhow!("Failed to create EarshotVad: {e}"))?,
        ),
    };

    let frame_samples = detector.frame_samples();
    let prefill_frames = frames_for_duration_ms(VAD_PREFILL_MS, frame_samples);
    let offline_hangover_frames = frames_for_duration_ms(VAD_OFFLINE_HANGOVER_MS, frame_samples);
    let streaming_hangover_frames =
        frames_for_duration_ms(VAD_STREAMING_HANGOVER_MS, frame_samples);
    let onset_frames = frames_for_duration_ms(VAD_ONSET_MS, frame_samples);
    let smoothed_vad = SmoothedVad::new(
        detector,
        prefill_frames,
        offline_hangover_frames,
        onset_frames,
    );

    info!(
        "Initialized {:?} VAD backend ({} samples/frame)",
        backend, frame_samples
    );

    let recorder = AudioRecorder::new()
        .with_vad(
            Box::new(smoothed_vad),
            offline_hangover_frames,
            streaming_hangover_frames,
        )
        .with_selected_channel(selected_channel)
        .with_level_callback({
            let app_handle = app_handle.clone();
            move |levels| {
                utils::emit_levels(&app_handle, &levels);
            }
        })
        .with_audio_callback({
            let router = stream_router;
            move |frame| {
                router.feed(frame);
            }
        })
        .with_frame_observer(frame_observer);

    Ok(recorder)
}

pub struct RecordingReadiness {
    receiver: mpsc::Receiver<()>,
    generation: u64,
}

impl RecordingReadiness {
    pub fn wait(self) -> bool {
        self.receiver.recv().is_ok()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }
}

#[derive(Clone)]
pub struct AudioRecordingManager {
    state: Arc<Mutex<RecordingState>>,
    mode: Arc<Mutex<MicrophoneMode>>,
    app_handle: crate::app_handle::AppHandle,

    recorder: Arc<Mutex<Option<AudioRecorder>>>,
    is_open: Arc<Mutex<bool>>,
    is_recording: Arc<Mutex<bool>>,
    mute_state: Arc<Mutex<MuteState>>,
    close_generation: Arc<AtomicU64>,
    cancel_generation: Arc<AtomicU64>,
    stream_router: Arc<StreamRouter>,
    frame_observer: Arc<dyn FrameObserver>,
    recording_active: Arc<AtomicBool>,
    capture_generation: Arc<AtomicU64>,
    cached_device: Arc<Mutex<Option<(String, cpal::Device)>>>,
    open_microphone: Arc<Mutex<Option<String>>>,
}

impl AudioRecordingManager {
    pub fn new(
        app: &crate::app_handle::AppHandle,
        stream_router: Arc<StreamRouter>,
        frame_observer: Arc<dyn FrameObserver>,
    ) -> Result<Self, anyhow::Error> {
        let settings = get_settings(app);
        let mode = if settings.always_on_microphone {
            MicrophoneMode::AlwaysOn
        } else {
            MicrophoneMode::OnDemand
        };

        let manager = Self {
            state: Arc::new(Mutex::new(RecordingState::Idle)),
            mode: Arc::new(Mutex::new(mode.clone())),
            app_handle: app.clone(),

            recorder: Arc::new(Mutex::new(None)),
            is_open: Arc::new(Mutex::new(false)),
            is_recording: Arc::new(Mutex::new(false)),
            mute_state: Arc::new(Mutex::new(MuteState::default())),
            close_generation: Arc::new(AtomicU64::new(0)),
            cancel_generation: Arc::new(AtomicU64::new(0)),
            stream_router,
            frame_observer,
            recording_active: Arc::new(AtomicBool::new(false)),
            capture_generation: Arc::new(AtomicU64::new(0)),
            cached_device: Arc::new(Mutex::new(None)),
            open_microphone: Arc::new(Mutex::new(None)),
        };

        if matches!(mode, MicrophoneMode::AlwaysOn) {
            manager.start_microphone_stream()?;
        }

        Ok(manager)
    }

    pub fn invalidate_device_cache(&self) {
        *self.cached_device.lock().unwrap() = None;
    }

    fn cached_top_choice(&self, wanted: &[String]) -> Option<(String, cpal::Device)> {
        self.cached_device
            .lock()
            .unwrap()
            .clone()
            .filter(|(name, _)| wanted.first() == Some(name))
    }

    fn resolve_microphone_device(&self, settings: &AppSettings) -> Option<(String, cpal::Device)> {
        let wanted = match desired_microphone(settings) {
            DesiredMicrophone::Default => {
                debug!("device resolve: no mic configured -> system default");
                return None;
            }
            DesiredMicrophone::Priority(list) => list,
            DesiredMicrophone::Clamshell(name) => vec![name],
        };

        if let Some(cached) = self.cached_top_choice(&wanted) {
            debug!("device resolve: cache hit for '{}'", cached.0);
            return Some(cached);
        }

        let enumerate_started = Instant::now();
        let found = first_connected_device(&wanted);
        debug!(
            "device resolve: enumerate={:?} (found={:?})",
            enumerate_started.elapsed(),
            found.as_ref().map(|(name, _)| name)
        );
        let found = found?;
        *self.cached_device.lock().unwrap() = Some(found.clone());
        Some(found)
    }

    fn schedule_lazy_close(&self) {
        let r#gen = self.close_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let app = self.app_handle.clone();
        std::thread::spawn(move || {
            std::thread::sleep(STREAM_IDLE_TIMEOUT);
            let rm = app.state::<Arc<AudioRecordingManager>>();
            let state = rm.state.lock().unwrap();
            if rm.close_generation.load(Ordering::SeqCst) == r#gen
                && matches!(*state, RecordingState::Idle)
            {
                info!(
                    "Closing idle microphone stream after {:?}",
                    STREAM_IDLE_TIMEOUT
                );
                rm.stop_microphone_stream();
            }
        });
    }

    /// Mutes the output for the recording that `generation` belongs to. It checks the
    /// generation while it holds `mute_state`, and stop and cancel bump the generation
    /// before they take that lock in `remove_mute`, so a mute never outlives its recording.
    pub fn apply_mute(&self, generation: u64) {
        let settings = get_settings(&self.app_handle);
        if !settings.mute_while_recording {
            return;
        }

        let is_open = self.is_open.lock().unwrap();
        let mut mute_guard = self.mute_state.lock().unwrap();
        if mute_guard.did_mute || !self.is_recording_readiness_current(generation) {
            return;
        }
        if *is_open {
            mute_guard.apply(get_mute, || set_mute(true));
            debug!("Mute applied (prev_muted={:?})", mute_guard.prev_muted);
        }
    }

    pub fn remove_mute(&self) {
        let mut mute_guard = self.mute_state.lock().unwrap();
        if mute_guard.did_mute {
            mute_guard.remove(restore_mute);
            debug!(
                "Mute removed (restored prev_muted={:?})",
                mute_guard.prev_muted
            );
        }
    }

    pub fn preload_vad(&self) -> Result<(), anyhow::Error> {
        let mut recorder_opt = self.recorder.lock().unwrap();
        if recorder_opt.is_none() {
            let settings = get_settings(&self.app_handle);
            *recorder_opt = Some(create_audio_recorder(
                settings.vad_backend,
                &self.app_handle,
                settings.selected_channel,
                Arc::clone(&self.stream_router),
                Arc::clone(&self.frame_observer),
            )?);
        }
        Ok(())
    }

    pub fn start_microphone_stream(&self) -> Result<(), anyhow::Error> {
        let mut open_flag = self.is_open.lock().unwrap();
        if *open_flag {
            let needs_reopen = self
                .recorder
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|rec| rec.needs_reopen());

            if needs_reopen {
                warn!("Microphone stream is no longer running (device disconnected?); reopening");
                self.invalidate_device_cache();
            } else {
                let picked = self
                    .resolve_microphone_device(&get_settings(&self.app_handle))
                    .map(|(name, _)| name);
                if picked == *self.open_microphone.lock().unwrap() {
                    trace!("Microphone stream already active");
                    return Ok(());
                }
                info!(
                    "Microphone changed to {}; reopening",
                    picked.as_deref().unwrap_or("the system default")
                );
            }

            {
                let mut mute_guard = self.mute_state.lock().unwrap();
                if mute_guard.did_mute {
                    restore_mute(mute_guard.prev_muted);
                    mute_guard.did_mute = false;
                }
            }
            if let Some(rec) = self.recorder.lock().unwrap().as_mut() {
                rec.close();
            }
            *self.is_recording.lock().unwrap() = false;
            *open_flag = false;
        }

        let start_time = Instant::now();

        {
            let mut mute_guard = self.mute_state.lock().unwrap();
            if mute_guard.did_mute {
                restore_mute(mute_guard.prev_muted);
                mute_guard.did_mute = false;
            }
        }

        let settings = get_settings(&self.app_handle);
        let resolve_started = Instant::now();
        let mut device = self.resolve_microphone_device(&settings);
        let resolve_elapsed = resolve_started.elapsed();

        let vad_started = Instant::now();
        self.preload_vad()?;
        let vad_elapsed = vad_started.elapsed();

        let open_started = Instant::now();
        let mut recorder_opt = self.recorder.lock().unwrap();
        if let Some(rec) = recorder_opt.as_mut()
            && let Err(first_err) = rec.open(device.as_ref().map(|(_, d)| d.clone()))
        {
            warn!("Recorder open failed ({first_err}); re-resolving device and retrying once");
            self.invalidate_device_cache();
            device = self.resolve_microphone_device(&settings);
            rec.open(device.as_ref().map(|(_, d)| d.clone()))
                .map_err(|e| anyhow::anyhow!("Failed to open recorder: {}", e))?;
        }
        *self.open_microphone.lock().unwrap() = device.map(|(name, _)| name);
        debug!(
            "mic stream breakdown: device_resolve={:?} vad_ensure={:?} open={:?}",
            resolve_elapsed,
            vad_elapsed,
            open_started.elapsed()
        );
        drop(recorder_opt);

        *open_flag = true;
        // This timing covers through cpal's stream.play() returning — i.e. the
        // point cpal surfaces as "stream running." It does NOT guarantee the
        // host audio device is producing samples yet; the first input callback
        // fires asynchronously one buffer period later (hardware dependent,
        // typically ~10–200ms on macOS, longer on Bluetooth/USB).
        info!(
            "Microphone stream initialized in {:?}",
            start_time.elapsed()
        );
        Ok(())
    }

    pub fn stop_microphone_stream(&self) {
        let mut open_flag = self.is_open.lock().unwrap();
        if !*open_flag {
            return;
        }

        {
            let mut mute_guard = self.mute_state.lock().unwrap();
            if mute_guard.did_mute {
                restore_mute(mute_guard.prev_muted);
            }
            mute_guard.did_mute = false;
        }

        if let Some(rec) = self.recorder.lock().unwrap().as_mut() {
            if *self.is_recording.lock().unwrap() {
                let _ = rec.stop();
                *self.is_recording.lock().unwrap() = false;
            }
            rec.close();
        }

        *open_flag = false;
        debug!("Microphone stream stopped");
    }

    pub fn update_mode(&self, new_mode: MicrophoneMode) -> Result<(), anyhow::Error> {
        let cur_mode = self.mode.lock().unwrap().clone();

        match (cur_mode, &new_mode) {
            (MicrophoneMode::AlwaysOn, MicrophoneMode::OnDemand) => {
                if matches!(*self.state.lock().unwrap(), RecordingState::Idle) {
                    self.close_generation.fetch_add(1, Ordering::SeqCst);
                    self.stop_microphone_stream();
                }
            }
            (MicrophoneMode::OnDemand, MicrophoneMode::AlwaysOn) => {
                self.close_generation.fetch_add(1, Ordering::SeqCst);
                self.start_microphone_stream()?;
            }
            _ => {}
        }

        *self.mode.lock().unwrap() = new_mode;
        Ok(())
    }

    fn set_state(&self, guard: &mut RecordingState, new_state: RecordingState) {
        *guard = new_state;
        self.recording_active.store(
            matches!(
                *guard,
                RecordingState::Recording { .. } | RecordingState::Stopping
            ),
            Ordering::SeqCst,
        );
    }

    pub fn try_start_recording(
        &self,
        binding_id: &str,
        vad_policy: VadPolicy,
    ) -> Result<RecordingReadiness, String> {
        let mut state = self.state.lock().unwrap();

        if let RecordingState::Idle = *state {
            self.close_generation.fetch_add(1, Ordering::SeqCst);
            if let Err(e) = self.start_microphone_stream() {
                let msg = format!("{e}");
                error!("Failed to open microphone stream: {msg}");
                return Err(msg);
            }

            if let Some(rec) = self.recorder.lock().unwrap().as_ref() {
                match rec.start(vad_policy) {
                    Ok(receiver) => {
                        let generation = self.capture_generation.fetch_add(1, Ordering::AcqRel) + 1;
                        *self.is_recording.lock().unwrap() = true;
                        self.set_state(
                            &mut state,
                            RecordingState::Recording {
                                binding_id: binding_id.to_string(),
                            },
                        );
                        debug!("Recording requested for binding {binding_id}");
                        return Ok(RecordingReadiness {
                            receiver,
                            generation,
                        });
                    }
                    Err(error) => return Err(format!("Failed to start recorder: {error}")),
                }
            }
            Err("Recorder not available".to_string())
        } else {
            Err("Already recording".to_string())
        }
    }

    pub fn update_vad_backend(&self, backend: VadBackend) -> Result<(), anyhow::Error> {
        let state = self.state.lock().unwrap();
        if !matches!(*state, RecordingState::Idle) {
            return Err(anyhow::anyhow!(
                "Cannot change the VAD backend while recording"
            ));
        }

        let settings = get_settings(&self.app_handle);
        let replacement = create_audio_recorder(
            backend,
            &self.app_handle,
            settings.selected_channel,
            Arc::clone(&self.stream_router),
            Arc::clone(&self.frame_observer),
        )?;
        let was_open = *self.is_open.lock().unwrap();

        self.close_generation.fetch_add(1, Ordering::SeqCst);
        if was_open {
            self.stop_microphone_stream();
        }

        let previous_recorder = self.recorder.lock().unwrap().replace(replacement);
        if was_open && let Err(change_error) = self.start_microphone_stream() {
            if let Some(recorder) = self.recorder.lock().unwrap().as_mut() {
                recorder.close();
            }
            *self.recorder.lock().unwrap() = previous_recorder;

            if let Err(rollback_error) = self.start_microphone_stream() {
                error!(
                    "Failed to restore microphone stream after VAD backend change failed: {rollback_error}"
                );
            }
            return Err(anyhow::anyhow!(
                "Failed to reopen microphone with {:?} VAD: {change_error}",
                backend
            ));
        }

        info!("VAD backend changed to {:?}", backend);
        drop(state);
        Ok(())
    }

    /// Applies a changed microphone setting to an open stream. A recording keeps its stream,
    /// because closing it drops what was said, and the next recording's start picks again. An
    /// idle stream reopens only when the pick changed.
    pub fn update_selected_device(&self) -> Result<(), anyhow::Error> {
        let state = self.state.lock().unwrap();
        if !matches!(*state, RecordingState::Idle) || !*self.is_open.lock().unwrap() {
            return Ok(());
        }
        self.start_microphone_stream()
    }

    pub fn update_selected_channel(
        &self,
        selected_channel: Option<u16>,
    ) -> Result<(), anyhow::Error> {
        let state = self.state.lock().unwrap();
        if !matches!(*state, RecordingState::Idle) {
            return Err(anyhow::anyhow!(
                "Cannot change the input channel while recording"
            ));
        }

        let previous_channel = get_settings(&self.app_handle).selected_channel;
        let was_open = *self.is_open.lock().unwrap();
        if was_open {
            self.close_generation.fetch_add(1, Ordering::SeqCst);
            self.stop_microphone_stream();
        }
        if let Some(recorder) = self.recorder.lock().unwrap().as_mut() {
            recorder.set_selected_channel(selected_channel);
        }
        if was_open && let Err(error) = self.start_microphone_stream() {
            if let Some(recorder) = self.recorder.lock().unwrap().as_mut() {
                recorder.set_selected_channel(previous_channel);
            }
            return Err(error);
        }
        drop(state);
        Ok(())
    }

    pub fn invalidate_recording_readiness(&self) {
        self.capture_generation.fetch_add(1, Ordering::AcqRel);
    }

    pub fn is_recording_readiness_current(&self, generation: u64) -> bool {
        self.capture_generation.load(Ordering::Acquire) == generation
    }

    pub fn cancel_generation(&self) -> u64 {
        self.cancel_generation.load(Ordering::Acquire)
    }

    pub fn was_cancelled_since(&self, generation: u64) -> bool {
        self.cancel_generation.load(Ordering::Acquire) != generation
    }

    pub fn stop_recording(&self, binding_id: &str, cancel_generation: u64) -> Option<Vec<f32>> {
        self.invalidate_recording_readiness();
        let mut state = self.state.lock().unwrap();

        match *state {
            RecordingState::Recording {
                binding_id: ref active,
            } if active == binding_id => {
                self.set_state(&mut state, RecordingState::Stopping);
                drop(state);

                let settings = get_settings(&self.app_handle);
                let buffer_ms = settings.extra_recording_buffer_ms;
                if buffer_ms > 0 {
                    debug!(
                        "Extra recording buffer: sleeping {}ms before stopping",
                        buffer_ms
                    );
                    let started = Instant::now();
                    let buffer = Duration::from_millis(buffer_ms);
                    while started.elapsed() < buffer {
                        if self.was_cancelled_since(cancel_generation) {
                            debug!("Recording stop cancelled during extra buffer");
                            break;
                        }
                        let remaining = buffer.saturating_sub(started.elapsed());
                        std::thread::sleep(remaining.min(Duration::from_millis(25)));
                    }
                }

                let samples = if let Some(rec) = self.recorder.lock().unwrap().as_ref() {
                    match rec.stop() {
                        Ok(buf) => buf,
                        Err(e) => {
                            error!("stop() failed: {e}");
                            Vec::new()
                        }
                    }
                } else {
                    error!("Recorder not available");
                    Vec::new()
                };

                *self.is_recording.lock().unwrap() = false;
                self.set_state(&mut self.state.lock().unwrap(), RecordingState::Idle);

                if matches!(*self.mode.lock().unwrap(), MicrophoneMode::OnDemand) {
                    if get_settings(&self.app_handle).lazy_stream_close {
                        self.schedule_lazy_close();
                    } else {
                        self.stop_microphone_stream();
                    }
                }

                if self.was_cancelled_since(cancel_generation) {
                    debug!("Recording stop cancelled; discarding captured samples");
                    return None;
                }

                let s_len = samples.len();
                if s_len < WHISPER_SAMPLE_RATE && s_len > 0 {
                    let mut padded = samples;
                    padded.resize(WHISPER_SAMPLE_RATE * 5 / 4, 0.0);
                    Some(padded)
                } else {
                    Some(samples)
                }
            }
            _ => None,
        }
    }
    pub fn is_recording(&self) -> bool {
        self.recording_active.load(Ordering::SeqCst)
    }

    pub fn cancel_recording(&self) {
        self.invalidate_recording_readiness();
        self.cancel_generation.fetch_add(1, Ordering::AcqRel);
        self.remove_mute();
        let mut state = self.state.lock().unwrap();

        match *state {
            RecordingState::Recording { .. } => {
                self.set_state(&mut state, RecordingState::Idle);
                drop(state);

                if let Some(rec) = self.recorder.lock().unwrap().as_ref() {
                    let _ = rec.stop();
                }

                *self.is_recording.lock().unwrap() = false;

                if matches!(*self.mode.lock().unwrap(), MicrophoneMode::OnDemand) {
                    if get_settings(&self.app_handle).lazy_stream_close {
                        self.schedule_lazy_close();
                    } else {
                        self.stop_microphone_stream();
                    }
                }
            }
            RecordingState::Stopping => {
                debug!("Cancellation requested while recording is stopping");
            }
            RecordingState::Idle => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MuteState, first_connected};
    use std::cell::{Cell, RefCell};

    #[test]
    fn muting_then_unmuting_restores_what_was_there() {
        let mut state = MuteState::default();
        let mutes = Cell::new(0);
        state.apply(|| Some(false), || mutes.set(mutes.get() + 1));
        assert_eq!(mutes.get(), 1);

        let restored = RefCell::new(Vec::new());
        state.remove(|prev| restored.borrow_mut().push(prev));
        assert_eq!(*restored.borrow(), vec![Some(false)]);
        assert!(!state.did_mute);
    }

    #[test]
    fn unmuting_without_a_mute_does_nothing() {
        let mut state = MuteState::default();
        let restores = Cell::new(0);
        state.remove(|_| restores.set(restores.get() + 1));
        assert_eq!(restores.get(), 0);
    }

    #[test]
    fn a_second_mute_keeps_the_first_reading() {
        let mut state = MuteState::default();
        state.apply(|| Some(false), || {});

        let second_read = Cell::new(false);
        let second_mute = Cell::new(false);
        state.apply(
            || {
                second_read.set(true);
                Some(true)
            },
            || second_mute.set(true),
        );
        assert!(!second_read.get());
        assert!(!second_mute.get());

        let restored = RefCell::new(Vec::new());
        state.remove(|prev| restored.borrow_mut().push(prev));
        assert_eq!(*restored.borrow(), vec![Some(false)]);
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn first_connected_skips_disconnected_microphones() {
        let priority = names(&["Shure MV7", "AirPods Pro", "MacBook Pro Microphone"]);
        let connected = ["MacBook Pro Microphone", "AirPods Pro"];
        assert_eq!(first_connected(&priority, &connected), Some("AirPods Pro"));
    }

    #[test]
    fn first_connected_is_none_when_nothing_in_the_list_is_connected() {
        let priority = names(&["Shure MV7"]);
        assert_eq!(first_connected(&priority, &["AirPods Pro"]), None);
        assert_eq!(first_connected::<&str>(&[], &["AirPods Pro"]), None);
    }
}
