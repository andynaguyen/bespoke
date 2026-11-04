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

#[cfg(target_os = "macos")]
mod imp {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use anyhow::Result;
    use scrawl::host::{SpeechState, TranscriptData};

    use super::Status;

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
        match scrawl::host::speech_status().state {
            SpeechState::Unknown | SpeechState::Checking => Status::Unchecked,
            SpeechState::Downloading => Status::Downloading,
            SpeechState::Ready => Status::Installed,
            SpeechState::Failed => Status::Failed,
            SpeechState::Unavailable => Status::Unsupported,
        }
    }

    pub fn attempts() -> u32 {
        scrawl::host::speech_status().attempts
    }

    /// Download progress from 0 to 1, while macOS downloads the model.
    pub fn progress() -> f64 {
        scrawl::host::speech_status().progress
    }

    pub fn error() -> Option<String> {
        scrawl::host::speech_status().error
    }

    /// Starts an install, or the next try now when an earlier one failed. Returns the
    /// `attempts` number of the try that answers this call.
    pub fn install() -> u32 {
        scrawl::host::ensure_speech_model()
    }

    /// Runs `changed` when the model's state changes. Download progress alone doesn't count.
    pub fn on_change(changed: impl Fn() + Send + Sync + 'static) {
        let last = std::sync::Mutex::new(None::<SpeechState>);
        scrawl::host::on_speech_status(move |status| {
            let mut last = last
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if last.as_ref() != Some(&status.state) {
                *last = Some(status.state.clone());
                drop(last);
                changed();
            }
        });
    }

    /// Transcribes a WAV file with word times. `missing` is set when the model can't run yet.
    pub fn transcribe_file(wav: &Path, custom_words: &[String]) -> Result<TranscriptData> {
        scrawl::host::apple_speech(wav, custom_words)
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

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::Status;

    pub fn status() -> Status {
        Status::Unsupported
    }

    pub fn progress() -> f64 {
        0.0
    }

    pub fn error() -> Option<String> {
        None
    }

    pub fn install() -> u32 {
        0
    }

    pub fn attempts() -> u32 {
        0
    }
}

pub use imp::*;
