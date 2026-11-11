use crate::managers::model::{EngineType, ModelInfo, ModelSource};

pub const MODEL_ID: &str = "apple-speech";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Unchecked,
    Downloading,
    Installed,
    Failed,
    Unsupported,
}

pub fn model_info() -> ModelInfo {
    ModelInfo {
        id: MODEL_ID.to_string(),
        name: "Apple Speech".to_string(),
        description: "Built into macOS. Runs on this Mac with Apple's speech model.".to_string(),
        filename: String::new(),
        source: ModelSource::System,
        size_mb: 0,
        is_downloaded: false,
        is_downloading: false,
        partial_size: 0,
        is_directory: false,
        engine_type: EngineType::AppleSpeech,
        accuracy_score: 0.70,
        speed_score: 0.85,
        supports_translation: false,
        is_recommended: false,
        supported_languages: Vec::new(),
        supports_language_selection: false,
        is_custom: false,
        supports_streaming: false,
        supports_language_detection: false,
    }
}

mod imp {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::annotate::model::TranscriptData;
    use crate::annotate::speech_model::{State as SpeechState, Status as SpeechStatus};
    use anyhow::Result;

    use super::Status;
    use crate::managers::model::ModelManager;

    /// SpeechAnalyzer, and the helper built against it, need macOS 26.
    pub fn supported_os() -> bool {
        objc2_foundation::NSProcessInfo::processInfo()
            .operatingSystemVersion()
            .majorVersion
            >= 26
    }

    pub fn status() -> Status {
        if !supported_os() {
            return Status::Unsupported;
        }
        match crate::annotate::speech_model::current().state {
            SpeechState::Unknown | SpeechState::Checking => Status::Unchecked,
            SpeechState::Downloading => Status::Downloading,
            SpeechState::Ready => Status::Installed,
            SpeechState::Failed => Status::Failed,
            SpeechState::Unavailable => Status::Unsupported,
        }
    }

    pub fn attempts() -> u32 {
        crate::annotate::speech_model::current().attempts
    }

    /// Download progress from 0 to 1, while macOS downloads the model.
    pub fn progress() -> f64 {
        crate::annotate::speech_model::current().progress
    }

    pub fn error() -> Option<String> {
        crate::annotate::speech_model::current().error
    }

    /// Starts an install, or the next try now when an earlier one failed. Returns the
    /// `attempts` number of the try that answers this call.
    pub fn install() -> u32 {
        crate::annotate::speech_model::ensure()
    }

    /// Refreshes the Models page's system models when the speech model's state changes.
    /// Download progress alone doesn't count.
    pub fn status_changed(status: &SpeechStatus) {
        static LAST: std::sync::Mutex<Option<SpeechState>> = std::sync::Mutex::new(None);
        let mut last = LAST
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if last.as_ref() != Some(&status.state) {
            *last = Some(status.state.clone());
            drop(last);
            if let Some(models) = crate::app_handle::AppHandle::try_global()
                .and_then(|app| app.try_state::<std::sync::Arc<ModelManager>>())
            {
                models.refresh_system_models();
            }
        }
    }

    /// Transcribes a WAV file with word times.
    pub fn transcribe_file(wav: &Path, custom_words: &[String]) -> Result<TranscriptData> {
        crate::annotate::helper::transcribe(wav, custom_words)
    }

    /// Transcribes 16 kHz mono samples.
    pub fn transcribe(audio: &[f32], custom_words: &[String]) -> Result<TranscriptData> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let wav: PathBuf = std::env::temp_dir().join(format!(
            "bespoke-apple-speech-{}-{}.wav",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        crate::audio_toolkit::save_wav_file(&wav, audio)?;
        let result = transcribe_file(&wav, custom_words);
        let _ = std::fs::remove_file(&wav);
        result
    }
}

pub use imp::*;
