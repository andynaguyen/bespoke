use crate::audio_toolkit::{
    list_input_devices,
    vad::{
        frames_for_duration_ms, EarshotVad, SmoothedVad, VAD_OFFLINE_HANGOVER_MS, VAD_ONSET_MS,
        VAD_PREFILL_MS, VAD_STREAMING_HANGOVER_MS,
    },
    AudioRecorder, FrameObserver, SileroVad, VadPolicy, VoiceActivityDetector,
};
use crate::helpers::clamshell;
use crate::managers::transcription::StreamRouter;
use crate::settings::{get_settings, write_settings, AppSettings, VadBackend};
use crate::utils;
use log::{debug, error, info, trace, warn};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const SILERO_VAD_THRESHOLD: f32 = 0.3;
const EARSHOT_VAD_THRESHOLD: f32 = 0.5;

fn set_mute(mute: bool) {
    #[cfg(target_os = "windows")]
    {
        unsafe {
            use windows::Win32::{
                Media::Audio::{
                    eMultimedia, eRender, Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator,
                    MMDeviceEnumerator,
                },
                System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED},
            };

            macro_rules! unwrap_or_return {
                ($expr:expr) => {
                    match $expr {
                        Ok(val) => val,
                        Err(_) => return,
                    }
                };
            }

            // Initialize the COM library for this thread.
            // If already initialized (e.g., by another library like Tauri), this does nothing.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            let all_devices: IMMDeviceEnumerator =
                unwrap_or_return!(CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL));
            let default_device =
                unwrap_or_return!(all_devices.GetDefaultAudioEndpoint(eRender, eMultimedia));
            let volume_interface = unwrap_or_return!(
                default_device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
            );

            let _ = volume_interface.SetMute(mute, std::ptr::null());
        }
    }

    #[cfg(target_os = "linux")]
    {
        use std::process::Command;

        let mute_val = if mute { "1" } else { "0" };
        let amixer_state = if mute { "mute" } else { "unmute" };

        if Command::new("wpctl")
            .args(["set-mute", "@DEFAULT_AUDIO_SINK@", mute_val])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return;
        }

        if Command::new("pactl")
            .args(["set-sink-mute", "@DEFAULT_SINK@", mute_val])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return;
        }

        let _ = Command::new("amixer")
            .args(["set", "Master", amixer_state])
            .output();
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let script = format!(
            "set volume output muted {}",
            if mute { "true" } else { "false" }
        );
        let _ = Command::new("osascript").args(["-e", &script]).output();
    }
}

#[cfg(target_os = "windows")]
fn get_mute() -> Option<bool> {
    unsafe {
        use windows::Win32::{
            Media::Audio::{
                eMultimedia, eRender, Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator,
                MMDeviceEnumerator,
            },
            System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED},
        };

        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let all_devices: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let default_device = all_devices
            .GetDefaultAudioEndpoint(eRender, eMultimedia)
            .ok()?;
        let volume_interface = default_device
            .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
            .ok()?;

        Some(volume_interface.GetMute().ok()?.as_bool())
    }
}

#[cfg(target_os = "linux")]
fn get_mute() -> Option<bool> {
    use std::process::Command;

    if let Ok(out) = Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .output()
    {
        if out.status.success() {
            return Some(String::from_utf8_lossy(&out.stdout).contains("[MUTED]"));
        }
    }

    // 2. PulseAudio (pactl): prints "Mute: yes" / "Mute: no".
    // Force LC_ALL=C so a localized system still emits the parseable English
    // "yes"/"no" instead of e.g. "ja"/"nein".
    if let Ok(out) = Command::new("pactl")
        .env("LC_ALL", "C")
        .args(["get-sink-mute", "@DEFAULT_SINK@"])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).to_lowercase();
            if s.contains("yes") {
                return Some(true);
            }
            if s.contains("no") {
                return Some(false);
            }
        }
    }

    // 3. ALSA (amixer): prints "[off]" for muted channels, "[on]" otherwise.
    // LC_ALL=C keeps the "[on]"/"[off]" tokens stable across locales.
    if let Ok(out) = Command::new("amixer")
        .env("LC_ALL", "C")
        .args(["get", "Master"])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            if s.contains("[off]") {
                return Some(true);
            }
            if s.contains("[on]") {
                return Some(false);
            }
        }
    }

    None
}

#[cfg(target_os = "macos")]
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

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn get_mute() -> Option<bool> {
    None
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

enum DesiredMicrophone {
    Default,
    Selected(String),
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
    match &settings.selected_microphone {
        Some(name) => DesiredMicrophone::Selected(name.clone()),
        None => DesiredMicrophone::Default,
    }
}

/// The input Bespoke records from under `settings`, by device name, with `None` for the system
/// default. The clamshell microphone wins while the lid is closed. Annotate asks for it at the
/// start of each recording, so Dictate and Annotate always use the same microphone.
pub fn preferred_microphone(settings: &AppSettings) -> Option<String> {
    match desired_microphone(settings) {
        DesiredMicrophone::Default => None,
        DesiredMicrophone::Selected(name) | DesiredMicrophone::Clamshell(name) => Some(name),
    }
}

struct MicrophoneResolution {
    device: Option<cpal::Device>,
    unavailable_selected_microphone: Option<String>,
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
        };

        if matches!(mode, MicrophoneMode::AlwaysOn) {
            manager.start_microphone_stream()?;
        }

        Ok(manager)
    }

    pub fn invalidate_device_cache(&self) {
        *self.cached_device.lock().unwrap() = None;
    }

    fn resolve_microphone_device(&self, settings: &AppSettings) -> MicrophoneResolution {
        let desired = desired_microphone(settings);
        let (device_name, selected_microphone) = match desired {
            DesiredMicrophone::Default => {
                debug!("device resolve: no mic configured -> system default");
                return MicrophoneResolution {
                    device: None,
                    unavailable_selected_microphone: None,
                };
            }
            DesiredMicrophone::Selected(name) => (name.clone(), Some(name)),
            DesiredMicrophone::Clamshell(name) => (name, None),
        };

        if let Some((cached_name, device)) = self.cached_device.lock().unwrap().as_ref() {
            if *cached_name == device_name {
                debug!("device resolve: cache hit for '{}'", device_name);
                return MicrophoneResolution {
                    device: Some(device.clone()),
                    unavailable_selected_microphone: None,
                };
            }
        }

        let enumerate_started = Instant::now();
        let (device, enumeration_succeeded) = match list_input_devices() {
            Ok(devices) => (
                devices
                    .into_iter()
                    .find(|d| d.name == device_name)
                    .map(|d| d.device),
                true,
            ),
            Err(e) => {
                debug!("Failed to list devices, using default: {}", e);
                (None, false)
            }
        };
        debug!(
            "device resolve: enumerate={:?} (found={})",
            enumerate_started.elapsed(),
            device.is_some()
        );
        if let Some(d) = &device {
            *self.cached_device.lock().unwrap() = Some((device_name, d.clone()));
        }

        let unavailable_selected_microphone = if enumeration_succeeded && device.is_none() {
            selected_microphone
        } else {
            None
        };
        MicrophoneResolution {
            device,
            unavailable_selected_microphone,
        }
    }

    fn persist_default_microphone_after_fallback(&self, unavailable_name: &str) {
        let mut settings = get_settings(&self.app_handle);
        if settings.selected_microphone.as_deref() != Some(unavailable_name) {
            return;
        }

        settings.selected_microphone = None;
        write_settings(&self.app_handle, settings);
        let _ = self.app_handle.emit(
            "settings-changed",
            serde_json::json!({
                "setting": "selected_microphone",
                "value": "Default"
            }),
        );
    }

    fn schedule_lazy_close(&self) {
        let gen = self.close_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let app = self.app_handle.clone();
        std::thread::spawn(move || {
            std::thread::sleep(STREAM_IDLE_TIMEOUT);
            let rm = app.state::<Arc<AudioRecordingManager>>();
            let state = rm.state.lock().unwrap();
            if rm.close_generation.load(Ordering::SeqCst) == gen
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

    pub fn apply_mute(&self) {
        let settings = get_settings(&self.app_handle);
        if !settings.mute_while_recording {
            return;
        }

        let is_open = self.is_open.lock().unwrap();
        let mut mute_guard = self.mute_state.lock().unwrap();
        if mute_guard.did_mute {
            return;
        }
        if *is_open {
            mute_guard.prev_muted = get_mute();
            set_mute(true);
            mute_guard.did_mute = true;
            debug!("Mute applied (prev_muted={:?})", mute_guard.prev_muted);
        }
    }

    pub fn remove_mute(&self) {
        let mut mute_guard = self.mute_state.lock().unwrap();
        if mute_guard.did_mute {
            restore_mute(mute_guard.prev_muted);
            mute_guard.did_mute = false;
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

            if !needs_reopen {
                trace!("Microphone stream already active");
                return Ok(());
            }

            warn!("Microphone stream is no longer running (device disconnected?); reopening");

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
            self.invalidate_device_cache();
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
        let mut resolution = self.resolve_microphone_device(&settings);
        let resolve_elapsed = resolve_started.elapsed();

        let vad_started = Instant::now();
        self.preload_vad()?;
        let vad_elapsed = vad_started.elapsed();

        let open_started = Instant::now();
        let mut recorder_opt = self.recorder.lock().unwrap();
        if let Some(rec) = recorder_opt.as_mut() {
            if let Err(first_err) = rec.open(resolution.device.clone()) {
                warn!("Recorder open failed ({first_err}); re-resolving device and retrying once");
                self.invalidate_device_cache();
                resolution = self.resolve_microphone_device(&settings);
                rec.open(resolution.device.clone())
                    .map_err(|e| anyhow::anyhow!("Failed to open recorder: {}", e))?;
            }
        }
        debug!(
            "mic stream breakdown: device_resolve={:?} vad_ensure={:?} open={:?}",
            resolve_elapsed,
            vad_elapsed,
            open_started.elapsed()
        );
        drop(recorder_opt);

        *open_flag = true;
        if let Some(unavailable_name) = resolution.unavailable_selected_microphone {
            self.persist_default_microphone_after_fallback(&unavailable_name);
        }
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
        if was_open {
            if let Err(change_error) = self.start_microphone_stream() {
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
        }

        info!("VAD backend changed to {:?}", backend);
        drop(state);
        Ok(())
    }

    pub fn update_selected_device(&self) -> Result<(), anyhow::Error> {
        self.invalidate_device_cache();
        let was_open = *self.is_open.lock().unwrap();
        if was_open {
            self.close_generation.fetch_add(1, Ordering::SeqCst);
            self.stop_microphone_stream();
            self.start_microphone_stream()?;
        }
        Ok(())
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
        if was_open {
            if let Err(error) = self.start_microphone_stream() {
                if let Some(recorder) = self.recorder.lock().unwrap().as_mut() {
                    recorder.set_selected_channel(previous_channel);
                }
                return Err(error);
            }
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
