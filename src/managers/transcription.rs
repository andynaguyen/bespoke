use crate::app_handle::AppHandle;
use crate::audio_toolkit::{
    Fix, OutputLanguageEvidence, TimedText, apply_custom_words_with_fixes, detect_output_language,
    normalize_transcription_output, remove_filler_words, scratch_before_keyword,
    scrub_before_keyword, strip_trailing_keyword,
};
use crate::managers::model::{EngineType, ModelManager};
use crate::settings::{
    AppSettings, OrtAcceleratorSetting, TranscribeAcceleratorSetting, get_settings,
};
use crate::voice_submit::VoiceSubmit;
use anyhow::Result;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use transcribe_cpp::{
    Backend, Feature, Model, ModelOptions, RunExtension, RunOptions, Session, StreamOptions, Task,
    WhisperRunOptions,
};
use transcribe_rs::{
    SpeechModel, TranscribeOptions,
    onnx::{
        Quantization,
        canary::CanaryModel,
        cohere::CohereModel,
        gigaam::GigaAMModel,
        moonshine::{MoonshineModel, MoonshineVariant, StreamingModel},
        parakeet::{ParakeetModel, ParakeetParams, TimestampGranularity},
        sense_voice::{SenseVoiceModel, SenseVoiceParams},
    },
};

const STREAM_PERF_LOG_INTERVAL: Duration = Duration::from_secs(5);
const STREAM_FINALIZE_REPLY_TIMEOUT: Duration = Duration::from_secs(30);

fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelStateEvent {
    pub event_type: String,
    pub model_name: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamTextEvent {
    pub committed: String,
    pub tentative: String,
}

impl StreamTextEvent {
    pub const EVENT: &'static str = "stream-text-event";

    pub fn emit(&self, app: &AppHandle) -> Result<(), crate::app_handle::Error> {
        app.emit(Self::EVENT, self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamPhase {
    Listening,
    Working,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamWorkKind {
    Transcribing,
    Polishing,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamPhaseEvent {
    pub phase: StreamPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<StreamWorkKind>,
}

impl StreamPhaseEvent {
    pub const EVENT: &'static str = "stream-phase-event";

    pub fn emit(&self, app: &AppHandle) -> Result<(), crate::app_handle::Error> {
        app.emit(Self::EVENT, self)
    }
}

enum StreamCmd {
    Feed(Vec<f32>),
    Finalize(mpsc::Sender<Option<FinalizedStreamText>>),
    Cancel,
    VoiceSubmitCheck { session: u64, revision: u64 },
}

const VOICE_SUBMIT_MAX_BUFFERED_MS: i64 = 300;

struct FinalizedStreamText {
    text: String,
    output_language: OutputLanguageEvidence,
    supported_languages: Vec<String>,
}

pub struct StreamRouter {
    tx: Mutex<Option<mpsc::Sender<StreamCmd>>>,
    open: Arc<AtomicBool>,
}

impl StreamRouter {
    fn new() -> Self {
        Self {
            tx: Mutex::new(None),
            open: Arc::new(AtomicBool::new(false)),
        }
    }

    fn open(&self) -> mpsc::Receiver<StreamCmd> {
        let (tx, rx) = mpsc::channel::<StreamCmd>();
        *self.tx.lock().unwrap() = Some(tx);
        self.open.store(true, Ordering::Relaxed);
        rx
    }

    fn take(&self) -> Option<mpsc::Sender<StreamCmd>> {
        self.open.store(false, Ordering::Relaxed);
        self.tx.lock().unwrap().take()
    }

    fn clear(&self) {
        self.open.store(false, Ordering::Relaxed);
        *self.tx.lock().unwrap() = None;
    }

    pub fn feed(&self, frame: &[f32]) {
        if !self.open.load(Ordering::Relaxed) {
            return;
        }
        if let Some(tx) = self.tx.lock().unwrap().as_ref() {
            let _ = tx.send(StreamCmd::Feed(frame.to_vec()));
        }
    }

    fn send(&self, cmd: StreamCmd) {
        if !self.open.load(Ordering::Relaxed) {
            return;
        }
        if let Some(tx) = self.tx.lock().unwrap().as_ref() {
            let _ = tx.send(cmd);
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Relaxed)
    }
}

enum LoadedEngine {
    TranscribeCpp(Session),
    Parakeet(ParakeetModel),
    Moonshine(MoonshineModel),
    MoonshineStreaming(StreamingModel),
    SenseVoice(SenseVoiceModel),
    GigaAM(GigaAMModel),
    Canary(CanaryModel),
    Cohere(CohereModel),
    AppleSpeech,
}

struct LoadedModel {
    id: String,
    idle_engine: Option<LoadedEngine>,
}

#[derive(Default)]
struct ModelState {
    model: Option<LoadedModel>,
    loading: bool,
    dictating: bool,
    holds: usize,
}

struct LoadingGuard(TranscriptionManager);

impl Drop for LoadingGuard {
    fn drop(&mut self) {
        let mut state = self.0.lock_state();
        state.loading = false;
        self.0.loading_condvar.notify_all();
        self.0.unload_if_unwanted(state);
    }
}

/// Keeps the speech model in memory across transcriptions that aren't a dictation. The model
/// unloads when the last hold drops and no dictation needs it.
pub struct ModelHold(TranscriptionManager);

impl Drop for ModelHold {
    fn drop(&mut self) {
        let mut state = self.0.lock_state();
        state.holds -= 1;
        self.0.unload_if_unwanted(state);
    }
}

struct StreamWorkerGuard {
    worker_id: u64,
    active_stream_worker: Arc<AtomicU64>,
    active_engine_lease: Arc<AtomicU64>,
    stream_active: Arc<AtomicBool>,
}

impl Drop for StreamWorkerGuard {
    fn drop(&mut self) {
        if self.active_stream_worker.load(Ordering::Acquire) == self.worker_id {
            self.stream_active.store(false, Ordering::Release);
        }
        let _ = self.active_engine_lease.compare_exchange(
            self.worker_id,
            0,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        let _ = self.active_stream_worker.compare_exchange(
            self.worker_id,
            0,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

#[derive(Clone)]
pub struct TranscriptionManager {
    model_manager: Arc<ModelManager>,
    app_handle: AppHandle,
    state: Arc<Mutex<ModelState>>,
    loading_condvar: Arc<Condvar>,
    router: Arc<StreamRouter>,
    stream_active: Arc<AtomicBool>,
    next_stream_worker_id: Arc<AtomicU64>,
    active_stream_worker: Arc<AtomicU64>,
    active_engine_lease: Arc<AtomicU64>,
    inference_lock: Arc<Mutex<()>>,
}

#[derive(Debug, Default)]
pub struct RecordingTranscript {
    pub text: String,
    pub submit: bool,
    pub cut_by_voice: bool,
    /// The words the dictionary changed in `text`.
    pub fixes: Vec<Fix>,
}

struct RawTranscript {
    text: String,
    segments: Vec<TimedText>,
    words: Vec<TimedText>,
    output_language: OutputLanguageEvidence,
    model_languages: Vec<String>,
    model_is_whisper: bool,
}

#[derive(Default)]
struct EngineOutput {
    text: String,
    segments: Vec<TimedText>,
    words: Vec<TimedText>,
}

impl EngineOutput {
    fn from_cpp(transcript: transcribe_cpp::Transcript) -> Self {
        let timed = |t0: i64, t1: i64, text: &str| TimedText {
            t0: t0.max(0) as u64,
            t1: t1.max(0) as u64,
            text: text.trim().to_string(),
        };
        Self {
            segments: transcript
                .segments
                .iter()
                .map(|s| timed(s.t0_ms, s.t1_ms, &s.text))
                .collect(),
            words: transcript
                .words
                .iter()
                .map(|w| timed(w.t0_ms, w.t1_ms, &w.text))
                .collect(),
            text: transcript.text,
        }
    }

    fn from_rs(result: transcribe_rs::TranscriptionResult) -> Self {
        Self {
            segments: result
                .segments
                .unwrap_or_default()
                .iter()
                .map(|s| TimedText {
                    t0: (s.start.max(0.0) * 1000.0) as u64,
                    t1: (s.end.max(0.0) * 1000.0) as u64,
                    text: s.text.trim().to_string(),
                })
                .collect(),
            words: Vec::new(),
            text: result.text,
        }
    }

    /// For SenseVoice, whose segments are tokenizer pieces such as `▁hel` and `lo`, not words.
    fn without_times(self) -> Self {
        Self {
            text: self.text,
            ..Self::default()
        }
    }

    /// For Parakeet run with word granularity, whose segments are words.
    fn segments_as_words(mut self) -> Self {
        self.words = std::mem::take(&mut self.segments);
        self
    }

    fn from_apple(transcript: crate::annotate::model::TranscriptData) -> Result<Self> {
        if let Some(missing) = transcript.missing {
            anyhow::bail!("Apple Speech: {}", missing.reason);
        }
        let timed = |t0: u64, t1: u64, text: &str| TimedText {
            t0,
            t1,
            text: text.trim().to_string(),
        };
        Ok(Self {
            segments: transcript
                .segments
                .iter()
                .map(|s| timed(s.t0, s.t1, &s.text))
                .collect(),
            words: transcript
                .words
                .iter()
                .map(|w| timed(w.t0, w.t1, &w.text))
                .collect(),
            text: transcript.text,
        })
    }
}

fn run_apple_speech(audio: &[f32], settings: &AppSettings) -> Result<EngineOutput> {
    EngineOutput::from_apple(crate::apple_speech::transcribe(
        audio,
        &settings.custom_words,
    )?)
}

/// Times are milliseconds from the start of the audio.
pub struct TimedTranscript {
    pub text: String,
    pub segments: Vec<TimedText>,
    pub words: Vec<TimedText>,
    pub dictionary_prompted: bool,
}

fn split_voice_submit(raw: String, keyword: Option<&str>) -> (String, bool) {
    match keyword.and_then(|keyword| strip_trailing_keyword(&raw, keyword)) {
        Some(rest) => (rest, true),
        None => (raw, false),
    }
}

fn apply_cut(
    text: String,
    enabled: bool,
    keyword: &str,
    cut: fn(&str, &str) -> Option<String>,
) -> (String, bool) {
    match enabled.then(|| cut(&text, keyword)).flatten() {
        Some(rest) => (rest, true),
        None => (text, false),
    }
}

fn apply_spoken_cuts(text: String, settings: &AppSettings) -> (String, bool) {
    let (text, scrubbed) = apply_cut(
        text,
        settings.scrub_enabled,
        &settings.scrub_keyword,
        scrub_before_keyword,
    );
    let (text, scratched) = apply_cut(
        text,
        settings.scratch_enabled,
        &settings.scratch_keyword,
        scratch_before_keyword,
    );
    (text, scrubbed || scratched)
}

impl TranscriptionManager {
    pub fn new(app_handle: &AppHandle, model_manager: Arc<ModelManager>) -> Self {
        Self {
            model_manager,
            app_handle: app_handle.clone(),
            state: Arc::new(Mutex::new(ModelState::default())),
            loading_condvar: Arc::new(Condvar::new()),
            router: Arc::new(StreamRouter::new()),
            stream_active: Arc::new(AtomicBool::new(false)),
            next_stream_worker_id: Arc::new(AtomicU64::new(1)),
            active_stream_worker: Arc::new(AtomicU64::new(0)),
            active_engine_lease: Arc::new(AtomicU64::new(0)),
            inference_lock: Arc::new(Mutex::new(())),
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, ModelState> {
        self.state.lock().unwrap_or_else(|poisoned| {
            warn!("Model state mutex was poisoned by a previous panic, recovering");
            poisoned.into_inner()
        })
    }

    fn wait_for_load<'a>(&self, state: MutexGuard<'a, ModelState>) -> MutexGuard<'a, ModelState> {
        self.loading_condvar
            .wait_while(state, |state| state.loading)
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn take_engine(&self) -> Option<LoadedEngine> {
        self.lock_state().model.as_mut()?.idle_engine.take()
    }

    /// Starts loading the model and keeps it in memory until `end_dictation`.
    pub fn begin_dictation(&self) {
        self.lock_state().dictating = true;
        self.initiate_model_load();
    }

    /// Unloads the model unless something else still needs it.
    pub fn end_dictation(&self) {
        let mut state = self.lock_state();
        state.dictating = false;
        self.unload_if_unwanted(state);
    }

    /// Starts loading the model and keeps it in memory until the hold drops.
    pub fn hold_model(&self) -> ModelHold {
        self.lock_state().holds += 1;
        self.initiate_model_load();
        ModelHold(self.clone())
    }

    fn unload_if_unwanted(&self, state: MutexGuard<'_, ModelState>) {
        if !state.loading && !state.dictating && state.holds == 0 && state.model.is_some() {
            self.unload(state);
        }
    }

    pub fn unload_model(&self) {
        self.unload(self.lock_state());
    }

    fn unload(&self, mut state: MutexGuard<'_, ModelState>) {
        let unload_start = std::time::Instant::now();
        debug!("Starting to unload model");

        state.model = None;
        drop(state);

        let _ = self.app_handle.emit(
            "model-state-changed",
            ModelStateEvent {
                event_type: "unloaded".to_string(),
                model_name: None,
                error: None,
            },
        );

        let unload_duration = unload_start.elapsed();
        debug!(
            "Model unloaded manually (took {}ms)",
            unload_duration.as_millis()
        );
    }

    pub fn load_model(&self, model_id: &str) -> Result<()> {
        self.load_model_with_device(model_id, None)
    }

    pub fn load_model_with_device(
        &self,
        model_id: &str,
        device_index: Option<usize>,
    ) -> Result<()> {
        apply_accelerator_settings(&self.app_handle);

        let load_start = std::time::Instant::now();
        debug!("Starting to load model: {}", model_id);

        let _ = self.app_handle.emit(
            "model-state-changed",
            ModelStateEvent {
                event_type: "loading_started".to_string(),
                model_name: None,
                error: None,
            },
        );

        let model_info = match self.model_manager.get_model_info(model_id) {
            Some(model_info) => model_info,
            None => {
                let error_msg = format!("Model not found: {}", model_id);
                let _ = self.app_handle.emit(
                    "model-state-changed",
                    ModelStateEvent {
                        event_type: "loading_failed".to_string(),
                        model_name: None,
                        error: Some(error_msg.clone()),
                    },
                );
                return Err(anyhow::anyhow!(error_msg));
            }
        };

        let emit_loading_failed = |error_msg: &str| {
            let _ = self.app_handle.emit(
                "model-state-changed",
                ModelStateEvent {
                    event_type: "loading_failed".to_string(),
                    model_name: Some(model_info.name.clone()),
                    error: Some(error_msg.to_string()),
                },
            );
        };

        if !model_info.is_downloaded {
            let error_msg = "Model not downloaded";
            emit_loading_failed(error_msg);
            return Err(anyhow::anyhow!(error_msg));
        }

        let model_path = if matches!(model_info.engine_type, EngineType::AppleSpeech) {
            std::path::PathBuf::new()
        } else {
            self.model_manager
                .get_model_path(model_id)
                .inspect_err(|error| emit_loading_failed(&error.to_string()))?
        };

        self.lock_state().model = None;

        let loaded_engine = match model_info.engine_type {
            EngineType::TranscribeCpp => {
                transcribe_compute_devices();
                let (backend, device) = match device_index {
                    Some(index) => resolve_device_index(index).inspect_err(|e| {
                        emit_loading_failed(&e.to_string());
                    })?,
                    None => {
                        let settings = get_settings(&self.app_handle);
                        let accelerator = settings.transcribe_accelerator;
                        let device = resolve_gpu_device(
                            accelerator,
                            settings.transcribe_gpu_device.as_deref(),
                        );
                        let backend = if device.is_some() {
                            Backend::Auto
                        } else {
                            select_transcribe_backend(accelerator)
                        };
                        (backend, device)
                    }
                };
                let requested_device = device
                    .as_ref()
                    .map(transcribe_device_label)
                    .unwrap_or_else(|| "automatic".to_string());
                let model_options = ModelOptions { backend, device };
                let model = Model::load_with(&model_path, &model_options).map_err(|e| {
                    let error_msg = format!("Failed to load whisper model {}: {}", model_id, e);
                    emit_loading_failed(&error_msg);
                    anyhow::anyhow!(error_msg)
                })?;
                let bound_backend = model.backend();
                let session = model.session().map_err(|e| {
                    let error_msg = format!(
                        "Failed to create session for whisper model {}: {}",
                        model_id, e
                    );
                    emit_loading_failed(&error_msg);
                    anyhow::anyhow!(error_msg)
                })?;
                let caps = session.model().capabilities();
                self.model_manager.set_runtime_capabilities(
                    model_id,
                    caps.supports_streaming,
                    caps.supports_translate,
                    caps.supports_language_detect,
                    caps.languages.clone(),
                );
                let bound_device = model
                    .device()
                    .map(|device| transcribe_device_label(&device))
                    .unwrap_or_else(|_| "unknown".to_string());
                info!(
                    "Loaded whisper model '{}' (requested {:?}, requested device '{}', \
                     bound backend '{}', bound device '{}', supports_streaming={}, \
                     supports_translate={}, supports_language_detect={})",
                    model_id,
                    backend,
                    requested_device,
                    bound_backend,
                    bound_device,
                    caps.supports_streaming,
                    caps.supports_translate,
                    caps.supports_language_detect
                );
                LoadedEngine::TranscribeCpp(session)
            }
            EngineType::Parakeet => {
                let engine =
                    ParakeetModel::load(&model_path, &Quantization::Int8).map_err(|e| {
                        let error_msg =
                            format!("Failed to load parakeet model {}: {}", model_id, e);
                        emit_loading_failed(&error_msg);
                        anyhow::anyhow!(error_msg)
                    })?;
                LoadedEngine::Parakeet(engine)
            }
            EngineType::Moonshine => {
                let engine = MoonshineModel::load(
                    &model_path,
                    MoonshineVariant::Base,
                    &Quantization::default(),
                )
                .map_err(|e| {
                    let error_msg = format!("Failed to load moonshine model {}: {}", model_id, e);
                    emit_loading_failed(&error_msg);
                    anyhow::anyhow!(error_msg)
                })?;
                LoadedEngine::Moonshine(engine)
            }
            EngineType::MoonshineStreaming => {
                let engine = StreamingModel::load(&model_path, 0, &Quantization::default())
                    .map_err(|e| {
                        let error_msg = format!(
                            "Failed to load moonshine streaming model {}: {}",
                            model_id, e
                        );
                        emit_loading_failed(&error_msg);
                        anyhow::anyhow!(error_msg)
                    })?;
                LoadedEngine::MoonshineStreaming(engine)
            }
            EngineType::SenseVoice => {
                let engine =
                    SenseVoiceModel::load(&model_path, &Quantization::Int8).map_err(|e| {
                        let error_msg =
                            format!("Failed to load SenseVoice model {}: {}", model_id, e);
                        emit_loading_failed(&error_msg);
                        anyhow::anyhow!(error_msg)
                    })?;
                LoadedEngine::SenseVoice(engine)
            }
            EngineType::GigaAM => {
                let engine = GigaAMModel::load(&model_path, &Quantization::Int8).map_err(|e| {
                    let error_msg = format!("Failed to load gigaam model {}: {}", model_id, e);
                    emit_loading_failed(&error_msg);
                    anyhow::anyhow!(error_msg)
                })?;
                LoadedEngine::GigaAM(engine)
            }
            EngineType::Canary => {
                let engine = CanaryModel::load(&model_path, &Quantization::Int8).map_err(|e| {
                    let error_msg = format!("Failed to load canary model {}: {}", model_id, e);
                    emit_loading_failed(&error_msg);
                    anyhow::anyhow!(error_msg)
                })?;
                LoadedEngine::Canary(engine)
            }
            EngineType::Cohere => {
                let engine = CohereModel::load(&model_path, &Quantization::Int8).map_err(|e| {
                    let error_msg = format!("Failed to load cohere model {}: {}", model_id, e);
                    emit_loading_failed(&error_msg);
                    anyhow::anyhow!(error_msg)
                })?;
                LoadedEngine::Cohere(engine)
            }
            EngineType::AppleSpeech => LoadedEngine::AppleSpeech,
        };

        self.lock_state().model = Some(LoadedModel {
            id: model_id.to_string(),
            idle_engine: Some(loaded_engine),
        });

        let _ = self.app_handle.emit(
            "model-state-changed",
            ModelStateEvent {
                event_type: "loading_completed".to_string(),
                model_name: Some(model_info.name.clone()),
                error: None,
            },
        );

        let load_duration = load_start.elapsed();
        debug!(
            "Successfully loaded transcription model: {} (took {}ms)",
            model_id,
            load_duration.as_millis()
        );
        Ok(())
    }

    pub fn initiate_model_load(&self) {
        let mut state = self.lock_state();
        if state.loading || state.model.is_some() {
            return;
        }
        state.loading = true;
        let manager = self.clone();
        thread::spawn(move || {
            let _loading = LoadingGuard(manager.clone());
            let settings = get_settings(&manager.app_handle);
            if let Err(e) = manager.load_model(&settings.selected_model) {
                error!("Failed to load model: {}", e);
            }
        });
    }

    pub fn get_current_model(&self) -> Option<String> {
        self.lock_state()
            .model
            .as_ref()
            .map(|model| model.id.clone())
    }

    pub fn current_backend(&self) -> Option<String> {
        let state = self.lock_state();
        match state.model.as_ref()?.idle_engine.as_ref() {
            Some(LoadedEngine::TranscribeCpp(session)) => {
                Some(session.model().backend().to_string())
            }
            Some(LoadedEngine::AppleSpeech) => Some("apple-speech".to_string()),
            Some(_) => Some("onnx".to_string()),
            None => None,
        }
    }

    pub fn is_streaming(&self) -> bool {
        self.stream_active.load(Ordering::Acquire)
    }

    pub fn stream_router(&self) -> Arc<StreamRouter> {
        Arc::clone(&self.router)
    }

    pub fn start_stream(&self) {
        if self.router.is_open() || self.active_stream_worker.load(Ordering::Acquire) != 0 {
            warn!("start_stream called while a stream worker is already active");
            return;
        }
        let worker_id = self.next_stream_worker_id.fetch_add(1, Ordering::Relaxed);
        if self
            .active_stream_worker
            .compare_exchange(0, worker_id, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            warn!("start_stream lost a race with another stream worker");
            return;
        }
        let rx = self.router.open();
        self.stream_active.store(false, Ordering::Release);

        let manager = self.clone();
        thread::spawn(move || manager.run_stream_worker(rx, worker_id));
    }

    fn run_stream_worker(&self, rx: mpsc::Receiver<StreamCmd>, worker_id: u64) {
        let _worker = StreamWorkerGuard {
            worker_id,
            active_stream_worker: Arc::clone(&self.active_stream_worker),
            active_engine_lease: Arc::clone(&self.active_engine_lease),
            stream_active: Arc::clone(&self.stream_active),
        };

        drop(self.wait_for_load(self.lock_state()));

        let model_id = self.get_current_model().unwrap_or_default();

        if self
            .active_engine_lease
            .compare_exchange(0, worker_id, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            warn!("Live preview: another worker already holds the transcription engine");
            self.router.clear();
            drain_until_finalize(rx);
            return;
        }
        let mut engine = match self.take_engine() {
            Some(e) => e,
            None => {
                info!(
                    "Live preview: model '{}' was unloaded before streaming could begin; \
                     falling back to batch transcription",
                    model_id
                );
                let _ = self.active_engine_lease.compare_exchange(
                    worker_id,
                    0,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                self.router.clear();
                drain_until_finalize(rx);
                return;
            }
        };

        let (supports_streaming, supports_translate, languages) = match &engine {
            LoadedEngine::TranscribeCpp(session) => {
                let model = session.model();
                let caps = model.capabilities();
                info!(
                    "Live preview: model '{}' arch='{}' variant='{}' supports_streaming={} \
                     supports_translate={} languages={:?}",
                    model_id,
                    model.arch(),
                    model.variant(),
                    caps.supports_streaming,
                    caps.supports_translate,
                    caps.languages,
                );
                (
                    caps.supports_streaming,
                    caps.supports_translate,
                    caps.languages,
                )
            }
            _ => {
                info!(
                    "Live preview: model '{}' is not a transcribe-cpp model; \
                     streaming is unavailable, using batch transcription",
                    model_id
                );
                (false, false, Vec::new())
            }
        };

        if !supports_streaming {
            self.return_engine(engine, &model_id);
            self.router.clear();
            drain_until_finalize(rx);
            return;
        }

        let settings = get_settings(&self.app_handle);
        let effective_language =
            effective_language_for_model(&settings, self.model_manager.as_ref(), &model_id);
        let run_plan = transcribe_cpp_run_plan(
            settings.translate_to_english,
            &effective_language,
            &languages,
            supports_translate,
        );
        let output_language = resolve_output_language_evidence(
            &settings,
            run_plan.language.as_deref(),
            &languages,
            run_plan.target_language.as_deref() == Some("en"),
        );
        let run_options = RunOptions {
            task: run_plan.task,
            language: run_plan.language,
            target_language: run_plan.target_language,
            ..Default::default()
        };

        let mut finalize_reply: Option<mpsc::Sender<Option<FinalizedStreamText>>> = None;
        let mut finalize_result: Option<Option<FinalizedStreamText>> = None;
        let stream_started = 'stream: {
            let session = match &mut engine {
                LoadedEngine::TranscribeCpp(s) => s,
                _ => break 'stream false,
            };

            let backend = session.model().backend();

            let mut stream = match session.stream(&run_options, &StreamOptions::default()) {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to begin stream: {}", e);
                    break 'stream false;
                }
            };

            self.stream_active.store(true, Ordering::Release);
            info!(
                "Live streaming transcription started (model '{}', backend '{}')",
                model_id, backend
            );

            let mut perf = StreamPerf::new();
            let mut last_buffered_ms: i64 = 0;
            let mut pending_voice_submit: Option<(u64, u64)> = None;
            while let Ok(cmd) = rx.recv() {
                match cmd {
                    StreamCmd::VoiceSubmitCheck { session, revision } => {
                        pending_voice_submit = Some((session, revision));
                    }
                    StreamCmd::Feed(pcm) => {
                        perf.record_feed(pcm.len());
                        let feed_start = Instant::now();
                        match stream.feed(&pcm) {
                            Ok(update) => {
                                last_buffered_ms = update.buffered_ms;
                                perf.record_compute(feed_start.elapsed());
                                perf.record_update(
                                    update.revision,
                                    update.input_received_ms,
                                    update.audio_committed_ms,
                                    update.buffered_ms,
                                );
                                if update.committed_changed || update.tentative_changed {
                                    let text = stream.text();
                                    perf.record_emit();
                                    self.emit_stream_text(&text.committed, &text.tentative);
                                }
                                perf.maybe_log();
                            }
                            Err(e) => {
                                perf.record_compute(feed_start.elapsed());
                                warn!("stream feed failed: {}", e);
                            }
                        }
                    }
                    StreamCmd::Finalize(reply) => {
                        let finalize_start = Instant::now();
                        let result = match stream.finalize() {
                            Ok(update) => {
                                perf.record_compute(finalize_start.elapsed());
                                perf.record_update(
                                    update.revision,
                                    update.input_received_ms,
                                    update.audio_committed_ms,
                                    update.buffered_ms,
                                );
                                let output_language = match &output_language {
                                    OutputLanguageEvidence::Unknown => {
                                        with_model_detected_language(
                                            OutputLanguageEvidence::Unknown,
                                            stream.snapshot().language,
                                        )
                                    }
                                    resolved => resolved.clone(),
                                };
                                Some(FinalizedStreamText {
                                    text: stream.text().full,
                                    output_language,
                                    supported_languages: languages.clone(),
                                })
                            }
                            Err(e) => {
                                perf.record_compute(finalize_start.elapsed());
                                error!(
                                    "stream finalize failed: {}; falling back to batch transcription",
                                    e
                                );
                                None
                            }
                        };
                        let chars = match &result {
                            Some(finalized) => finalized.text.len(),
                            _ => 0,
                        };
                        perf.log_finalized(chars);
                        finalize_reply = Some(reply);
                        finalize_result = Some(result);
                        break;
                    }
                    StreamCmd::Cancel => {
                        stream.reset();
                        break;
                    }
                }

                if last_buffered_ms <= VOICE_SUBMIT_MAX_BUFFERED_MS
                    && let Some((session, revision)) = pending_voice_submit.take()
                {
                    self.check_stream_voice_submit(&stream.text().full, session, revision);
                }
            }

            true
        };

        if !stream_started {
            self.return_engine(engine, &model_id);
            drain_until_finalize(rx);
            return;
        }

        self.return_engine(engine, &model_id);
        if let (Some(reply), Some(result)) = (finalize_reply, finalize_result) {
            let _ = reply.send(result);
        }
    }

    fn return_engine(&self, engine: LoadedEngine, expected_model_id: &str) {
        let mut state = self.lock_state();
        match state
            .model
            .as_mut()
            .filter(|model| model.id == expected_model_id)
        {
            Some(model) => model.idle_engine = Some(engine),
            None => {
                drop(state);
                info!(
                    "Model changed/unloaded during transcription; dropping stale engine (was '{}')",
                    expected_model_id
                );
            }
        }
    }

    pub fn finalize_stream(
        &self,
        voice_submit_keyword: Option<&str>,
    ) -> Result<Option<RecordingTranscript>> {
        let Some(tx) = self.router.take() else {
            return Ok(None);
        };
        let (reply_tx, reply_rx) = mpsc::channel();
        if tx.send(StreamCmd::Finalize(reply_tx)).is_err() {
            return Ok(None);
        }
        let finalized = match reply_rx.recv_timeout(STREAM_FINALIZE_REPLY_TIMEOUT) {
            Ok(Some(finalized)) => finalized,
            Ok(None) => return Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(None),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.stream_active.store(false, Ordering::Release);
                return Err(anyhow::anyhow!(
                    "Timed out waiting {:?} for live transcription to finalize",
                    STREAM_FINALIZE_REPLY_TIMEOUT
                ));
            }
        };

        let settings = get_settings(&self.app_handle);
        let (text, submit) = split_voice_submit(finalized.text, voice_submit_keyword);
        let (text, cut_by_voice) = apply_spoken_cuts(text, &settings);
        let (filtered, fixes) = post_process_transcription_text_with_fixes(
            text,
            &settings,
            false,
            &finalized.output_language,
            &finalized.supported_languages,
        );

        Ok(Some(RecordingTranscript {
            text: filtered,
            submit,
            cut_by_voice,
            fixes,
        }))
    }

    pub fn request_voice_submit_check(&self, session: u64, revision: u64) {
        if self.is_streaming() {
            self.router
                .send(StreamCmd::VoiceSubmitCheck { session, revision });
        }
    }

    fn check_stream_voice_submit(&self, text: &str, session: u64, revision: u64) {
        let Some(voice_submit) = self.app_handle.try_state::<Arc<VoiceSubmit>>() else {
            return;
        };
        let Some(keyword) = voice_submit.keyword_for(session) else {
            return;
        };
        let matched = strip_trailing_keyword(text, &keyword).is_some();
        debug!("Voice submit check on live transcript: matched={matched}");
        if matched {
            voice_submit.request_stop(session, revision);
        }
    }

    pub fn cancel_stream(&self) {
        if let Some(tx) = self.router.take() {
            let _ = tx.send(StreamCmd::Cancel);
        }
        self.stream_active.store(false, Ordering::Release);
    }

    pub fn emit_stream_working(&self, kind: StreamWorkKind) {
        let _ = StreamPhaseEvent {
            phase: StreamPhase::Working,
            kind: Some(kind),
        }
        .emit(&self.app_handle);
    }

    fn emit_stream_text(&self, committed: &str, tentative: &str) {
        let _ = StreamTextEvent {
            committed: committed.to_string(),
            tentative: tentative.to_string(),
        }
        .emit(&self.app_handle);
    }

    pub fn transcribe(&self, audio: Vec<f32>) -> Result<String> {
        self.transcribe_recording(audio, None).map(|t| t.text)
    }

    pub fn transcribe_recording(
        &self,
        audio: Vec<f32>,
        voice_submit_keyword: Option<&str>,
    ) -> Result<RecordingTranscript> {
        #[cfg(debug_assertions)]
        if std::env::var("BESPOKE_FORCE_TRANSCRIPTION_FAILURE").is_ok() {
            return Err(anyhow::anyhow!(
                "Simulated transcription failure (BESPOKE_FORCE_TRANSCRIPTION_FAILURE)"
            ));
        }

        let st = std::time::Instant::now();
        let audio_len = audio.len();

        debug!("Audio vector length: {}", audio_len);

        if audio.is_empty() {
            debug!("Empty audio vector");
            return Ok(RecordingTranscript::default());
        }

        let settings = get_settings(&self.app_handle);
        let raw = self.run_engine(&audio, &settings, false)?;

        let (text, submit) = split_voice_submit(raw.text, voice_submit_keyword);
        let (text, cut_by_voice) = apply_spoken_cuts(text, &settings);
        let (filtered_result, fixes) = post_process_transcription_text_with_fixes(
            text,
            &settings,
            raw.model_is_whisper,
            &raw.output_language,
            &raw.model_languages,
        );

        let et = std::time::Instant::now();
        let translation_note = if settings.translate_to_english {
            " (translated)"
        } else {
            ""
        };
        let elapsed_secs = (et - st).as_secs_f64();
        let audio_secs = audio_len as f64 / 16_000.0;
        let speedup = real_time_factor(audio_secs, elapsed_secs);
        info!(
            "Transcription completed in {:.2}s for {:.2}s of audio ({:.2}x real-time){}",
            elapsed_secs, audio_secs, speedup, translation_note
        );

        let final_result = filtered_result;

        if final_result.is_empty() {
            info!("Transcription result is empty");
        } else {
            info!(
                "Transcription result: {}",
                crate::utils::redact_text(&final_result)
            );
        }

        Ok(RecordingTranscript {
            text: final_result,
            submit,
            cut_by_voice,
            fixes,
        })
    }

    /// Transcribes with segment and word times, for Annotate. Loads the selected model first
    /// when no model is loaded. Annotate calls this once per piece of a capture, under a
    /// `ModelHold` so the model stays loaded between pieces.
    pub fn transcribe_timed(&self, audio: &[f32]) -> Result<TimedTranscript> {
        self.initiate_model_load();
        let settings = get_settings(&self.app_handle);
        let raw = self.run_engine(audio, &settings, true)?;
        Ok(TimedTranscript {
            text: raw.text,
            segments: raw.segments,
            words: raw.words,
            dictionary_prompted: raw.model_is_whisper && !settings.custom_words.is_empty(),
        })
    }

    pub fn voice_submit_probe(&self, audio: Vec<f32>, keyword: &str) -> bool {
        let settings = get_settings(&self.app_handle);
        let started = Instant::now();
        match self.run_engine(&audio, &settings, false) {
            Ok(raw) => {
                let matched = strip_trailing_keyword(&raw.text, keyword).is_some();
                debug!(
                    "Voice submit probe on {:.2}s of audio took {:?}: matched={}",
                    audio.len() as f64 / 16_000.0,
                    started.elapsed(),
                    matched
                );
                matched
            }
            Err(e) => {
                debug!("Voice submit probe skipped: {}", e);
                false
            }
        }
    }

    fn run_engine(
        &self,
        audio: &[f32],
        settings: &AppSettings,
        word_times: bool,
    ) -> Result<RawTranscript> {
        let _inference = self
            .inference_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        {
            let state = self.wait_for_load(self.lock_state());

            if state
                .model
                .as_ref()
                .is_none_or(|model| model.idle_engine.is_none())
            {
                return Err(anyhow::anyhow!("Model is not loaded for transcription."));
            }
        }

        let active_model = self
            .get_current_model()
            .unwrap_or_else(|| settings.selected_model.clone());
        let validated_language =
            effective_language_for_model(settings, self.model_manager.as_ref(), &active_model);
        if validated_language != settings.selected_language {
            debug!(
                "Language intent '{}' resolved to '{}' for model '{}'",
                settings.selected_language, validated_language, active_model
            );
        }

        // Whether the loaded model is actually whisper-family (arch string).
        // Non-whisper archs (e.g. Voxtral Small) can advertise
        // Feature::InitialPrompt yet reject the whisper-kind run extension
        // with INVALID_ARG, so the whisper extension must be gated on the
        // arch, not on the feature (see #1601).
        let mut model_is_whisper = false;

        let (result, output_language, model_languages) = {
            let mut engine = match self.take_engine() {
                Some(e) => e,
                None => {
                    return Err(anyhow::anyhow!(
                        "Model failed to load after auto-load attempt. Please check your model settings."
                    ));
                }
            };

            let mut model_supports_translate = false;
            let mut model_languages = self
                .model_manager
                .get_model_info(&active_model)
                .map(|info| info.supported_languages)
                .unwrap_or_default();
            let mut output_was_translated = false;
            let mut applied_language_hint: Option<String> = None;
            let mut model_detected_language: Option<String> = None;
            if let LoadedEngine::TranscribeCpp(session) = &engine {
                let model = session.model();
                let caps = model.capabilities();
                model_is_whisper = model.arch() == "whisper";
                model_supports_translate = caps.supports_translate;
                model_languages = caps.languages;
                debug!(
                    "transcribe-cpp model '{}' on '{}': initial_prompt={}, translate={}, languages={:?}",
                    settings.selected_model,
                    model.backend(),
                    model.supports(Feature::InitialPrompt),
                    model_supports_translate,
                    model_languages
                );
            }

            let transcribe_result = catch_unwind(AssertUnwindSafe(|| -> Result<EngineOutput> {
                match &mut engine {
                    LoadedEngine::TranscribeCpp(session) => {
                        let family = if settings.custom_words.is_empty() || !model_is_whisper {
                            None
                        } else {
                            Some(RunExtension::Whisper(WhisperRunOptions {
                                initial_prompt: Some(settings.custom_words.join(", ")),
                                ..Default::default()
                            }))
                        };

                        let run_plan = transcribe_cpp_run_plan(
                            settings.translate_to_english,
                            &validated_language,
                            &model_languages,
                            model_supports_translate,
                        );
                        output_was_translated = run_plan.target_language.as_deref() == Some("en");
                        applied_language_hint = run_plan.language.clone();

                        let run_options = RunOptions {
                            task: run_plan.task,
                            language: run_plan.language,
                            target_language: run_plan.target_language,
                            family,
                            ..Default::default()
                        };

                        debug!(
                            "transcribe-cpp run: task={:?}, language={:?}, initial_prompt={}",
                            run_options.task,
                            run_options.language,
                            run_options.family.is_some()
                        );

                        session
                            .run(audio, &run_options)
                            .map(|mut t| {
                                model_detected_language = t.language.take();
                                EngineOutput::from_cpp(t)
                            })
                            .map_err(|e| {
                                anyhow::anyhow!("transcribe-cpp transcription failed: {}", e)
                            })
                    }
                    LoadedEngine::Parakeet(parakeet_engine) => {
                        let params = ParakeetParams {
                            timestamp_granularity: Some(if word_times {
                                TimestampGranularity::Word
                            } else {
                                TimestampGranularity::Segment
                            }),
                            ..Default::default()
                        };
                        parakeet_engine
                            .transcribe_with(audio, &params)
                            .map(|r| {
                                let output = EngineOutput::from_rs(r);
                                if word_times {
                                    output.segments_as_words()
                                } else {
                                    output
                                }
                            })
                            .map_err(|e| anyhow::anyhow!("Parakeet transcription failed: {}", e))
                    }
                    LoadedEngine::Moonshine(moonshine_engine) => moonshine_engine
                        .transcribe(audio, &TranscribeOptions::default())
                        .map(EngineOutput::from_rs)
                        .map_err(|e| anyhow::anyhow!("Moonshine transcription failed: {}", e)),
                    LoadedEngine::MoonshineStreaming(streaming_engine) => streaming_engine
                        .transcribe(audio, &TranscribeOptions::default())
                        .map(EngineOutput::from_rs)
                        .map_err(|e| {
                            anyhow::anyhow!("Moonshine streaming transcription failed: {}", e)
                        }),
                    LoadedEngine::SenseVoice(sense_voice_engine) => {
                        let language =
                            match crate::managers::model::recognition_language(&validated_language)
                            {
                                "zh" => Some("zh".to_string()),
                                "en" => Some("en".to_string()),
                                "ja" => Some("ja".to_string()),
                                "ko" => Some("ko".to_string()),
                                "yue" => Some("yue".to_string()),
                                _ => None,
                            };
                        applied_language_hint = language.clone();
                        let params = SenseVoiceParams {
                            language,
                            use_itn: Some(true),
                        };
                        sense_voice_engine
                            .transcribe_with(audio, &params)
                            .map(|r| EngineOutput::from_rs(r).without_times())
                            .map_err(|e| anyhow::anyhow!("SenseVoice transcription failed: {}", e))
                    }
                    LoadedEngine::GigaAM(gigaam_engine) => gigaam_engine
                        .transcribe(audio, &TranscribeOptions::default())
                        .map(EngineOutput::from_rs)
                        .map_err(|e| anyhow::anyhow!("GigaAM transcription failed: {}", e)),
                    LoadedEngine::Canary(canary_engine) => {
                        output_was_translated = settings.translate_to_english;
                        let lang = if validated_language == "auto" {
                            None
                        } else {
                            Some(validated_language.clone())
                        };
                        applied_language_hint = lang.clone();
                        let options = TranscribeOptions {
                            language: lang,
                            translate: settings.translate_to_english,
                            ..Default::default()
                        };
                        canary_engine
                            .transcribe(audio, &options)
                            .map(EngineOutput::from_rs)
                            .map_err(|e| anyhow::anyhow!("Canary transcription failed: {}", e))
                    }
                    LoadedEngine::AppleSpeech => run_apple_speech(audio, settings),
                    LoadedEngine::Cohere(cohere_engine) => {
                        let lang = if validated_language == "auto" {
                            None
                        } else {
                            Some(
                                crate::managers::model::recognition_language(&validated_language)
                                    .to_string(),
                            )
                        };
                        applied_language_hint = lang.clone();
                        let options = TranscribeOptions {
                            language: lang,
                            ..Default::default()
                        };
                        cohere_engine
                            .transcribe(audio, &options)
                            .map(EngineOutput::from_rs)
                            .map_err(|e| anyhow::anyhow!("Cohere transcription failed: {}", e))
                    }
                }
            }));

            let output = match transcribe_result {
                Ok(inner_result) => {
                    self.return_engine(engine, &active_model);
                    inner_result?
                }
                Err(panic_payload) => {
                    let panic_msg = panic_payload_message(panic_payload.as_ref());
                    error!(
                        "Transcription engine panicked: {}. Model has been unloaded.",
                        panic_msg
                    );

                    self.lock_state().model = None;

                    let _ = self.app_handle.emit(
                        "model-state-changed",
                        ModelStateEvent {
                            event_type: "unloaded".to_string(),
                            model_name: None,
                            error: Some(format!("Engine panicked: {}", panic_msg)),
                        },
                    );

                    return Err(anyhow::anyhow!(
                        "Transcription engine panicked: {}. The model has been unloaded and will reload on next attempt.",
                        panic_msg
                    ));
                }
            };

            let output_language = with_model_detected_language(
                resolve_output_language_evidence(
                    settings,
                    applied_language_hint.as_deref(),
                    &model_languages,
                    output_was_translated,
                ),
                model_detected_language,
            );
            debug!("Output language evidence: {:?}", output_language);

            (output, output_language, model_languages)
        };

        Ok(RawTranscript {
            text: result.text,
            segments: result.segments,
            words: result.words,
            output_language,
            model_languages,
            model_is_whisper,
        })
    }
}

struct StreamPerf {
    feed_count: u64,
    emit_count: u64,
    streamed_samples: u64,
    stream_compute_elapsed: Duration,
    last_log: Instant,
    latest_revision: i32,
    latest_input_received_ms: i64,
    latest_audio_committed_ms: i64,
    latest_buffered_ms: i64,
}

impl StreamPerf {
    fn new() -> Self {
        Self {
            feed_count: 0,
            emit_count: 0,
            streamed_samples: 0,
            stream_compute_elapsed: Duration::ZERO,
            last_log: Instant::now(),
            latest_revision: 0,
            latest_input_received_ms: 0,
            latest_audio_committed_ms: 0,
            latest_buffered_ms: 0,
        }
    }

    fn record_feed(&mut self, samples: usize) {
        self.feed_count += 1;
        self.streamed_samples += samples as u64;
    }

    fn record_compute(&mut self, elapsed: Duration) {
        self.stream_compute_elapsed += elapsed;
    }

    fn record_update(
        &mut self,
        revision: i32,
        input_received_ms: i64,
        audio_committed_ms: i64,
        buffered_ms: i64,
    ) {
        self.latest_revision = revision;
        self.latest_input_received_ms = input_received_ms;
        self.latest_audio_committed_ms = audio_committed_ms;
        self.latest_buffered_ms = buffered_ms;
    }

    fn record_emit(&mut self) {
        self.emit_count += 1;
    }

    fn maybe_log(&mut self) {
        if self.last_log.elapsed() < STREAM_PERF_LOG_INTERVAL {
            return;
        }

        let audio_secs = self.audio_secs();
        let compute_secs = self.compute_secs();
        debug!(
            "Live preview perf: {:.2}s streamed audio, {:.2}s model compute ({:.2}x real-time), \
             input_received={:.2}s, committed_audio={:.2}s, buffered={}ms, revision={}, \
             {} frames fed, {} updates emitted",
            audio_secs,
            compute_secs,
            real_time_factor(audio_secs, compute_secs),
            self.latest_input_received_ms as f64 / 1000.0,
            self.latest_audio_committed_ms as f64 / 1000.0,
            self.latest_buffered_ms,
            self.latest_revision,
            self.feed_count,
            self.emit_count,
        );
        self.last_log = Instant::now();
    }

    fn log_finalized(&self, chars: usize) {
        let audio_secs = self.audio_secs();
        let compute_secs = self.compute_secs();
        info!(
            "Live preview finalized in {:.2}s model compute for {:.2}s streamed audio ({:.2}x real-time): \
             input_received={:.2}s, committed_audio={:.2}s, buffered={}ms, revision={}, \
             {} frames fed, {} updates emitted, {} chars",
            compute_secs,
            audio_secs,
            real_time_factor(audio_secs, compute_secs),
            self.latest_input_received_ms as f64 / 1000.0,
            self.latest_audio_committed_ms as f64 / 1000.0,
            self.latest_buffered_ms,
            self.latest_revision,
            self.feed_count,
            self.emit_count,
            chars
        );
    }

    fn audio_secs(&self) -> f64 {
        self.streamed_samples as f64 / 16_000.0
    }

    fn compute_secs(&self) -> f64 {
        self.stream_compute_elapsed.as_secs_f64()
    }
}

fn real_time_factor(audio_secs: f64, compute_secs: f64) -> f64 {
    if compute_secs > 0.0 {
        audio_secs / compute_secs
    } else {
        0.0
    }
}

fn effective_language_for_model(
    settings: &AppSettings,
    model_manager: &ModelManager,
    model_id: &str,
) -> String {
    match model_manager.get_model_info(model_id) {
        Some(info) => crate::managers::model::effective_language(
            &settings.selected_language,
            &info.supported_languages,
            info.supports_language_detection,
        ),
        None => settings.selected_language.clone(),
    }
}

fn resolve_output_language_evidence(
    settings: &AppSettings,
    applied_language_hint: Option<&str>,
    supported_languages: &[String],
    translated_to_english: bool,
) -> OutputLanguageEvidence {
    if translated_to_english {
        return OutputLanguageEvidence::TranslatedToEnglish;
    }

    // Stored language intent is only evidence when this specific engine run
    // actually received the hint. Some multilingual engines (notably Parakeet
    // V3) always auto-detect and ignore Bespoke's selection; transcribe-cpp also
    // drops a requested hint when the loaded model does not advertise it.
    if let Some(language) = applied_language_hint.filter(|lang| !lang.is_empty() && *lang != "auto")
    {
        if settings.selected_language != "auto"
            && crate::managers::model::canonical_language_code(&settings.selected_language)
                == crate::managers::model::canonical_language_code(language)
        {
            return OutputLanguageEvidence::UserSelected(language.to_string());
        }

        return OutputLanguageEvidence::ModelConstrained(language.to_string());
    }

    if let [language] = supported_languages {
        return OutputLanguageEvidence::ModelConstrained(language.clone());
    }

    OutputLanguageEvidence::Unknown
}

fn with_model_detected_language(
    evidence: OutputLanguageEvidence,
    detected: Option<String>,
) -> OutputLanguageEvidence {
    match (evidence, detected) {
        (OutputLanguageEvidence::Unknown, Some(language))
            if !language.is_empty() && language != "auto" =>
        {
            OutputLanguageEvidence::ModelDetected(language)
        }
        (evidence, _) => evidence,
    }
}

struct TranscribeCppRunPlan {
    task: Task,
    language: Option<String>,
    target_language: Option<String>,
}

fn transcribe_cpp_run_plan(
    translate_to_english: bool,
    effective_language: &str,
    model_languages: &[String],
    model_supports_translate: bool,
) -> TranscribeCppRunPlan {
    let requested_language = match effective_language {
        "auto" => None,
        other => Some(crate::managers::model::recognition_language(other).to_string()),
    };
    let language = requested_language.filter(|lang| model_languages.iter().any(|l| l == lang));
    let (task, target_language) = cpp_translation_task(
        translate_to_english,
        model_supports_translate,
        language.as_deref(),
    );

    TranscribeCppRunPlan {
        task,
        language,
        target_language,
    }
}

fn post_process_transcription_text_with_fixes(
    raw: String,
    settings: &AppSettings,
    custom_words_already_prompted: bool,
    output_language: &OutputLanguageEvidence,
    supported_languages: &[String],
) -> (String, Vec<Fix>) {
    let mut fixes = Vec::new();
    let text = fail_open_text_transform(raw, |raw| {
        let corrected = if !settings.custom_words.is_empty() && !custom_words_already_prompted {
            let (corrected, found) = apply_custom_words_with_fixes(
                &raw,
                &settings.custom_words,
                settings.word_correction_threshold,
            );
            fixes = found;
            corrected
        } else {
            raw
        };

        let output_language = match output_language {
            OutputLanguageEvidence::Unknown
                if settings.filler_word_removal_enabled
                    && settings.custom_filler_words.is_none() =>
            {
                match detect_output_language(&corrected, supported_languages) {
                    Some(language) => {
                        debug!("Text-based language detection resolved '{}'", language);
                        OutputLanguageEvidence::TextDetected(language)
                    }
                    None => OutputLanguageEvidence::Unknown,
                }
            }
            other => other.clone(),
        };

        let without_fillers = remove_filler_words(
            &corrected,
            &output_language,
            &settings.custom_filler_words,
            settings.filler_word_removal_enabled,
        );

        normalize_transcription_output(&without_fillers)
    });
    (text, fixes)
}

fn fail_open_text_transform<F>(raw: String, transform: F) -> String
where
    F: FnOnce(String) -> String,
{
    let fallback = raw.clone();
    match catch_unwind(AssertUnwindSafe(|| transform(raw))) {
        Ok(processed) => processed,
        Err(payload) => {
            error!(
                "Optional transcription text post-processing panicked: {}; using the raw transcription",
                panic_payload_message(payload.as_ref())
            );
            fallback
        }
    }
}

/// Unlike transcribe-rs (which forces the target to English itself when its
/// `translate` flag is set), transcribe-cpp requires an explicit
/// `target_language`: a null target defaults to the *source*, so a non-English
/// source silently becomes e.g. es→es and Canary rejects the unadvertised pair.
fn cpp_translation_task(
    translate_to_english: bool,
    model_supports_translate: bool,
    source_language: Option<&str>,
) -> (Task, Option<String>) {
    let translate_to_en =
        translate_to_english && model_supports_translate && source_language != Some("en");
    if translate_to_en {
        (Task::Translate, Some("en".to_string()))
    } else {
        (Task::Transcribe, None)
    }
}

fn drain_until_finalize(rx: mpsc::Receiver<StreamCmd>) {
    while let Ok(cmd) = rx.recv() {
        match cmd {
            StreamCmd::Feed(_) | StreamCmd::VoiceSubmitCheck { .. } => {}
            StreamCmd::Finalize(reply) => {
                let _ = reply.send(None);
                break;
            }
            StreamCmd::Cancel => break,
        }
    }
}

pub fn describe_compute_devices() -> Vec<String> {
    transcribe_compute_devices()
        .iter()
        .map(|d| {
            let idx = d
                .index
                .map(|i| i.to_string())
                .unwrap_or_else(|| "-".to_string());
            let name = if d.description.is_empty() {
                &d.name
            } else {
                &d.description
            };
            let vram_mb = d.memory_total / (1024 * 1024);
            format!(
                "index={} kind={} name={} vram={}MB",
                idx, d.kind, name, vram_mb
            )
        })
        .collect()
}

fn resolve_device_index(index: usize) -> Result<(Backend, Option<transcribe_cpp::Device>)> {
    let device = transcribe_compute_devices()
        .iter()
        .find(|d| d.index == Some(index))
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!("No compute device with index {index} (see --list-devices)")
        })?;
    if matches!(
        device.device_type,
        transcribe_cpp::DeviceType::Accel | transcribe_cpp::DeviceType::Unknown
    ) {
        return Err(anyhow::anyhow!(
            "Device index {index} ({}) cannot host a model",
            device.kind
        ));
    }

    // 0.2's opaque handle makes every index, including zero, an exact
    // selection. Backend::Auto accepts any primary device and cannot conflict
    // with the selected device's vendor backend.
    Ok((Backend::Auto, Some(device)))
}

fn select_transcribe_backend(setting: TranscribeAcceleratorSetting) -> Backend {
    match setting {
        TranscribeAcceleratorSetting::Cpu => Backend::Cpu,
        TranscribeAcceleratorSetting::Auto | TranscribeAcceleratorSetting::Gpu => Backend::Auto,
    }
}

/// Resolve the user's persisted GPU identity to a fresh opaque 0.2 device
/// handle. Registry indices and handles are process-local, so settings store a
/// key based on the backend's stable `device_id` (falling back to name for
/// backends such as Metal that do not report one).
fn resolve_gpu_device(
    setting: TranscribeAcceleratorSetting,
    gpu_device: Option<&str>,
) -> Option<transcribe_cpp::Device> {
    if setting != TranscribeAcceleratorSetting::Gpu {
        return None;
    }
    let gpu_device = gpu_device?;
    let resolved = transcribe_compute_devices()
        .iter()
        .find(|device| {
            is_transcribe_gpu_device(device) && transcribe_device_key(device) == gpu_device
        })
        .cloned();
    if resolved.is_none() {
        warn!(
            "Stored transcribe GPU device '{}' is no longer available; using automatic device selection",
            gpu_device
        );
    }
    resolved
}

fn transcribe_device_key(device: &transcribe_cpp::Device) -> String {
    let (identity_kind, identity) = match device.device_id.as_deref() {
        Some(device_id) => ("id", device_id),
        None => ("name", device.name.as_str()),
    };
    serde_json::to_string(&(device.kind.as_str(), identity_kind, identity))
        .expect("transcribe device identity is always JSON serializable")
}

fn transcribe_device_label(device: &transcribe_cpp::Device) -> String {
    if device.description.is_empty() {
        device.name.clone()
    } else {
        device.description.clone()
    }
}

pub fn apply_accelerator_settings(app: &crate::app_handle::AppHandle) {
    use transcribe_rs::accel;

    let settings = get_settings(app);

    info!(
        "transcribe.cpp accelerator preference: {:?} (applied on next model load)",
        settings.transcribe_accelerator
    );

    let ort_pref = match settings.ort_accelerator {
        OrtAcceleratorSetting::Auto => accel::OrtAccelerator::Auto,
        OrtAcceleratorSetting::Cpu => accel::OrtAccelerator::CpuOnly,
        OrtAcceleratorSetting::Cuda => accel::OrtAccelerator::Cuda,
        OrtAcceleratorSetting::DirectMl => accel::OrtAccelerator::DirectMl,
        OrtAcceleratorSetting::Rocm => accel::OrtAccelerator::Rocm,
    };
    accel::set_ort_accelerator(ort_pref);
    info!("ORT accelerator set to: {}", ort_pref);
}

#[derive(Serialize, Clone, Debug)]
pub struct GpuDeviceOption {
    pub id: String,
    pub name: String,
    pub total_vram_mb: usize,
}

static GPU_DEVICES: OnceLock<Vec<GpuDeviceOption>> = OnceLock::new();

fn is_transcribe_gpu_device(device: &transcribe_cpp::Device) -> bool {
    matches!(
        device.device_type,
        transcribe_cpp::DeviceType::Gpu | transcribe_cpp::DeviceType::Igpu
    )
}

fn transcribe_compute_devices() -> &'static [transcribe_cpp::Device] {
    static DEVICES: OnceLock<Vec<transcribe_cpp::Device>> = OnceLock::new();
    DEVICES.get_or_init(|| {
        transcribe_cpp::init_logging();
        if let Err(error) = transcribe_cpp::init_backends_default() {
            warn!("Failed to initialize transcribe-cpp backends: {}", error);
        }
        let devices = transcribe_cpp::devices();
        info!(
            "transcribe-cpp has {} compute device(s): [{}]",
            devices.len(),
            devices
                .iter()
                .map(|device| format!("{} ({})", device.name, device.kind))
                .collect::<Vec<_>>()
                .join(", ")
        );
        devices
    })
}

fn cached_gpu_devices() -> &'static [GpuDeviceOption] {
    GPU_DEVICES.get_or_init(|| {
        transcribe_compute_devices()
            .iter()
            .filter(|device| is_transcribe_gpu_device(device))
            .map(|d| GpuDeviceOption {
                id: transcribe_device_key(d),
                name: transcribe_device_label(d),
                total_vram_mb: (d.memory_total / (1024 * 1024)) as usize,
            })
            .collect()
    })
}

#[derive(Serialize, Clone, Debug)]
pub struct AvailableAccelerators {
    pub transcribe: Vec<String>,
    pub ort: Vec<String>,
    pub gpu_devices: Vec<GpuDeviceOption>,
}

pub fn get_available_accelerators() -> AvailableAccelerators {
    use transcribe_rs::accel::OrtAccelerator;

    let ort_options: Vec<String> = OrtAccelerator::available()
        .into_iter()
        .map(|a| a.to_string())
        .collect();

    AvailableAccelerators {
        transcribe: vec!["auto".to_string(), "cpu".to_string(), "gpu".to_string()],
        ort: ort_options,
        gpu_devices: cached_gpu_devices().to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> TranscriptionManager {
        let app = AppHandle::detached();
        let models = Arc::new(ModelManager::empty(&app));
        TranscriptionManager::new(&app, models)
    }

    fn finish_load(manager: &TranscriptionManager) {
        manager.lock_state().model = Some(LoadedModel {
            id: "model".to_string(),
            idle_engine: Some(LoadedEngine::AppleSpeech),
        });
    }

    fn loaded_manager() -> TranscriptionManager {
        let manager = manager();
        finish_load(&manager);
        manager
    }

    fn engine_in_memory(manager: &TranscriptionManager) -> bool {
        manager
            .lock_state()
            .model
            .as_ref()
            .is_some_and(|model| model.idle_engine.is_some())
    }

    #[test]
    fn ending_a_dictation_unloads_the_model() {
        let manager = loaded_manager();
        manager.begin_dictation();
        assert!(engine_in_memory(&manager));

        manager.end_dictation();
        assert!(!engine_in_memory(&manager));
        assert_eq!(manager.get_current_model(), None);
    }

    #[test]
    fn a_hold_keeps_the_model_after_a_dictation_ends() {
        let manager = loaded_manager();
        manager.begin_dictation();
        let hold = manager.hold_model();

        manager.end_dictation();
        assert!(engine_in_memory(&manager));

        drop(hold);
        assert!(!engine_in_memory(&manager));
    }

    #[test]
    fn a_dictation_keeps_the_model_after_a_hold_drops() {
        let manager = loaded_manager();
        let hold = manager.hold_model();
        manager.begin_dictation();

        drop(hold);
        assert!(engine_in_memory(&manager));

        manager.end_dictation();
        assert!(!engine_in_memory(&manager));
    }

    #[test]
    fn a_load_that_outlives_its_dictation_unloads_when_it_ends() {
        let manager = manager();
        manager.lock_state().loading = true;
        manager.begin_dictation();
        manager.end_dictation();

        finish_load(&manager);
        assert!(engine_in_memory(&manager), "the load is still running");

        drop(LoadingGuard(manager.clone()));
        assert!(!engine_in_memory(&manager));
        assert!(!manager.lock_state().loading);
    }

    #[test]
    fn a_press_during_a_load_keeps_the_model_it_loads() {
        let manager = manager();
        manager.lock_state().loading = true;
        manager.begin_dictation();
        manager.end_dictation();
        manager.begin_dictation();

        finish_load(&manager);
        drop(LoadingGuard(manager.clone()));
        assert!(engine_in_memory(&manager));
    }

    #[test]
    fn an_engine_a_cancelled_worker_still_holds_does_not_stop_the_next_load() {
        let manager = loaded_manager();
        manager.take_engine();
        manager.begin_dictation();

        manager.end_dictation();

        assert_eq!(manager.get_current_model(), None);
    }

    #[test]
    fn an_engine_returned_after_an_unload_is_dropped() {
        let manager = loaded_manager();
        let engine = manager.take_engine().unwrap();

        manager.unload_model();
        manager.return_engine(engine, "model");

        assert!(!engine_in_memory(&manager));
    }

    fn languages(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|code| (*code).to_string()).collect()
    }

    #[test]
    fn concurrent_device_queries_share_the_registered_inventory() {
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|i| {
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    let (listed, available) = if i % 2 == 0 {
                        (describe_compute_devices(), get_available_accelerators())
                    } else {
                        let available = get_available_accelerators();
                        (describe_compute_devices(), available)
                    };
                    let gpu_ids = available
                        .gpu_devices
                        .into_iter()
                        .map(|device| device.id)
                        .collect::<Vec<_>>();
                    (listed, gpu_ids, transcribe_compute_devices())
                })
            })
            .collect();

        let devices = transcribe_compute_devices();
        let expected_gpu_ids: Vec<_> = devices
            .iter()
            .filter(|device| is_transcribe_gpu_device(device))
            .map(transcribe_device_key)
            .collect();
        let expected_listing = describe_compute_devices();
        for worker in workers {
            let (listed, gpu_ids, inventory) = worker.join().expect("device query panicked");
            assert!(std::ptr::eq(inventory, devices));
            assert_eq!(listed, expected_listing);
            assert_eq!(gpu_ids, expected_gpu_ids);
        }
    }

    #[test]
    fn registered_device_indices_resolve_to_the_same_handles() {
        for device in transcribe_compute_devices().iter().filter(|device| {
            !matches!(
                device.device_type,
                transcribe_cpp::DeviceType::Accel | transcribe_cpp::DeviceType::Unknown
            )
        }) {
            if let Some(index) = device.index {
                let (backend, resolved) = resolve_device_index(index).expect("device index");
                assert_eq!(backend, Backend::Auto);
                assert_eq!(
                    resolved.as_ref().map(transcribe_device_key),
                    Some(transcribe_device_key(device))
                );
            }
        }
        assert!(resolve_device_index(usize::MAX).is_err());
    }

    #[test]
    fn scrub_runs_only_when_turned_on() {
        let mut settings = crate::settings::get_default_settings();
        let text = || "wrong start scrub that right start".to_string();
        assert_eq!(
            apply_spoken_cuts(text(), &settings),
            ("Right start".to_string(), true)
        );

        settings.scrub_enabled = false;
        assert_eq!(apply_spoken_cuts(text(), &settings), (text(), false));
    }

    #[test]
    fn scratch_runs_only_when_turned_on() {
        let mut settings = crate::settings::get_default_settings();
        let text = || "Keep this. Drop this. Scratch that. Right start.".to_string();
        assert_eq!(
            apply_spoken_cuts(text(), &settings),
            ("Keep this. Right start.".to_string(), true)
        );

        settings.scratch_enabled = false;
        assert_eq!(apply_spoken_cuts(text(), &settings), (text(), false));
    }

    #[test]
    fn spoken_cuts_scrub_first_and_flag_an_empty_result() {
        let settings = crate::settings::get_default_settings();
        assert_eq!(
            apply_spoken_cuts(
                "Old. Scrub that. One. Two. Scratch that. Three.".to_string(),
                &settings
            ),
            ("One. Three.".to_string(), true)
        );
        assert_eq!(
            apply_spoken_cuts("Draft email. Scratch that.".to_string(), &settings),
            (String::new(), true)
        );
        assert_eq!(
            apply_spoken_cuts("Nothing to cut.".to_string(), &settings),
            ("Nothing to cut.".to_string(), false)
        );
    }

    #[test]
    fn only_cpu_forces_the_cpu_backend() {
        assert_eq!(
            select_transcribe_backend(TranscribeAcceleratorSetting::Auto),
            Backend::Auto
        );
        assert_eq!(
            select_transcribe_backend(TranscribeAcceleratorSetting::Cpu),
            Backend::Cpu
        );
        assert_eq!(
            select_transcribe_backend(TranscribeAcceleratorSetting::Gpu),
            Backend::Auto
        );
    }

    #[test]
    fn optional_text_transform_falls_back_to_raw_text_after_panic() {
        let raw = "原始轉錄。".to_string();
        let result = fail_open_text_transform(raw.clone(), |_| {
            panic!("simulated optional cleanup failure")
        });

        assert_eq!(result, raw);
    }

    #[test]
    fn portuguese_transcription_does_not_use_english_ui_filler_words() {
        let settings = AppSettings {
            app_language: "en".to_string(),
            selected_language: "pt-BR".to_string(),
            ..Default::default()
        };
        let supported = languages(&["en", "pt"]);
        let evidence = resolve_output_language_evidence(&settings, Some("pt"), &supported, false);

        let result = post_process_transcription_text_with_fixes(
            "eu vi um carro".to_string(),
            &settings,
            false,
            &evidence,
            &supported,
        )
        .0;

        assert_eq!(
            evidence,
            OutputLanguageEvidence::UserSelected("pt".to_string())
        );
        assert_eq!(result, "eu vi um carro");
    }

    #[test]
    fn norwegian_alias_is_recorded_as_user_selected_evidence() {
        let settings = AppSettings {
            selected_language: "no".to_string(),
            ..Default::default()
        };

        let evidence =
            resolve_output_language_evidence(&settings, Some("nb"), &languages(&["nb"]), false);

        assert_eq!(
            evidence,
            OutputLanguageEvidence::UserSelected("nb".to_string())
        );
    }

    #[test]
    fn auto_language_without_detection_skips_gated_filler_removal() {
        let settings = AppSettings {
            selected_language: "auto".to_string(),
            ..Default::default()
        };
        let evidence =
            resolve_output_language_evidence(&settings, None, &languages(&["en", "pt"]), false);

        let result = post_process_transcription_text_with_fixes(
            "um uhm ok".to_string(),
            &settings,
            false,
            &evidence,
            &languages(&["en", "pt"]),
        )
        .0;

        assert_eq!(evidence, OutputLanguageEvidence::Unknown);
        assert_eq!(result, "um ok");
    }

    #[test]
    fn unknown_evidence_with_confident_text_detection_removes_gated_fillers() {
        let settings = AppSettings {
            selected_language: "auto".to_string(),
            ..Default::default()
        };

        let result = post_process_transcription_text_with_fixes(
            "um so the weather forecast said it would probably rain throughout the whole weekend"
                .to_string(),
            &settings,
            false,
            &OutputLanguageEvidence::Unknown,
            &languages(&["en", "pt", "es", "de"]),
        )
        .0;

        assert_eq!(
            result,
            "so the weather forecast said it would probably rain throughout the whole weekend"
        );
    }

    #[test]
    fn unknown_evidence_with_portuguese_text_preserves_um() {
        let settings = AppSettings {
            selected_language: "auto".to_string(),
            ..Default::default()
        };

        let result = post_process_transcription_text_with_fixes(
            "eu vi um carro na rua ontem de manhã quando fui ao mercado".to_string(),
            &settings,
            false,
            &OutputLanguageEvidence::Unknown,
            &languages(&["en", "pt", "es", "de"]),
        )
        .0;

        assert_eq!(
            result,
            "eu vi um carro na rua ontem de manhã quando fui ao mercado"
        );
    }

    #[test]
    fn model_detected_language_upgrades_unknown_evidence_only() {
        assert_eq!(
            with_model_detected_language(OutputLanguageEvidence::Unknown, Some("en".to_string())),
            OutputLanguageEvidence::ModelDetected("en".to_string())
        );
        assert_eq!(
            with_model_detected_language(OutputLanguageEvidence::Unknown, Some("auto".to_string())),
            OutputLanguageEvidence::Unknown
        );
        assert_eq!(
            with_model_detected_language(OutputLanguageEvidence::Unknown, None),
            OutputLanguageEvidence::Unknown
        );
        assert_eq!(
            with_model_detected_language(
                OutputLanguageEvidence::UserSelected("pt".to_string()),
                Some("en".to_string())
            ),
            OutputLanguageEvidence::UserSelected("pt".to_string())
        );
    }

    #[test]
    fn auto_language_uses_single_language_model_as_evidence() {
        let settings = AppSettings {
            selected_language: "auto".to_string(),
            ..Default::default()
        };

        let evidence =
            resolve_output_language_evidence(&settings, None, &languages(&["en"]), false);

        assert_eq!(
            evidence,
            OutputLanguageEvidence::ModelConstrained("en".to_string())
        );
    }

    #[test]
    fn unsupported_explicit_language_uses_model_fallback_as_evidence() {
        let settings = AppSettings {
            selected_language: "pt".to_string(),
            ..Default::default()
        };

        let evidence = resolve_output_language_evidence(
            &settings,
            Some("en"),
            &languages(&["en", "de"]),
            false,
        );

        assert_eq!(
            evidence,
            OutputLanguageEvidence::ModelConstrained("en".to_string())
        );
    }

    #[test]
    fn ignored_user_language_is_not_output_evidence() {
        let settings = AppSettings {
            selected_language: "en".to_string(),
            ..Default::default()
        };
        let supported = languages(&["en", "de", "pt"]);

        let evidence = resolve_output_language_evidence(&settings, None, &supported, false);
        assert_eq!(evidence, OutputLanguageEvidence::Unknown);

        let result = post_process_transcription_text_with_fixes(
            "eu vi um carro".to_string(),
            &settings,
            false,
            &evidence,
            &supported,
        )
        .0;
        assert_eq!(result, "eu vi um carro");
    }

    #[test]
    fn unapplied_transcribe_cpp_language_is_not_output_evidence() {
        let settings = AppSettings {
            selected_language: "en".to_string(),
            ..Default::default()
        };
        let supported = languages(&[]);
        let plan = transcribe_cpp_run_plan(false, "en", &supported, false);

        assert_eq!(plan.language, None);
        assert_eq!(
            resolve_output_language_evidence(
                &settings,
                plan.language.as_deref(),
                &supported,
                false,
            ),
            OutputLanguageEvidence::Unknown
        );
    }

    #[test]
    fn translated_output_is_treated_as_english() {
        let settings = AppSettings {
            selected_language: "pt".to_string(),
            ..Default::default()
        };

        let evidence = resolve_output_language_evidence(
            &settings,
            Some("pt"),
            &languages(&["en", "pt"]),
            true,
        );

        assert_eq!(evidence, OutputLanguageEvidence::TranslatedToEnglish);
    }

    #[test]
    fn transcribe_cpp_run_plan_maps_chinese_variants() {
        let plan = transcribe_cpp_run_plan(false, "zh-Hant", &languages(&["zh"]), true);

        assert!(matches!(plan.task, Task::Transcribe));
        assert_eq!(plan.language.as_deref(), Some("zh"));
        assert_eq!(plan.target_language, None);
    }

    #[test]
    fn transcribe_cpp_run_plan_skips_english_translation() {
        let plan = transcribe_cpp_run_plan(true, "en", &languages(&["en", "es"]), true);

        assert!(matches!(plan.task, Task::Transcribe));
        assert_eq!(plan.language.as_deref(), Some("en"));
        assert_eq!(plan.target_language, None);
    }

    #[test]
    fn transcribe_cpp_run_plan_translates_supported_non_english() {
        let plan = transcribe_cpp_run_plan(true, "es", &languages(&["en", "es"]), true);

        assert!(matches!(plan.task, Task::Translate));
        assert_eq!(plan.language.as_deref(), Some("es"));
        assert_eq!(plan.target_language.as_deref(), Some("en"));
    }

    #[test]
    fn transcribe_cpp_run_plan_requires_model_translation_support() {
        let plan = transcribe_cpp_run_plan(true, "es", &languages(&["en", "es"]), false);

        assert!(matches!(plan.task, Task::Transcribe));
        assert_eq!(plan.language.as_deref(), Some("es"));
        assert_eq!(plan.target_language, None);
    }
}
