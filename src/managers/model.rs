use super::model_capabilities::{
    CapabilityProbe, CapabilityProber, Compatibility, GgufHeaderProber,
};
use crate::app_handle::AppHandle;
use crate::settings::{get_settings, write_settings};
use anyhow::Result;
use hf_hub::api::tokio::{ApiBuilder, CancellationToken, Progress};
use hf_hub::{Cache, Repo, RepoType};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod download;

use download::{DOWNLOAD_STALL_TIMEOUT, HttpDownloadOutcome};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EngineType {
    TranscribeCpp,
    Parakeet,
    Moonshine,
    MoonshineStreaming,
    SenseVoice,
    GigaAM,
    Canary,
    Cohere,
    AppleSpeech,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModelSource {
    Url {
        url: String,
        sha256: Option<String>,
    },
    HuggingFace {
        repo_id: String,
        revision: String,
    },
    Local,
    /// Part of the OS. macOS downloads and stores it, so Bespoke has no file to delete.
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub filename: String,
    pub source: ModelSource,
    pub size_mb: u64,
    pub is_downloaded: bool,
    pub is_downloading: bool,
    pub partial_size: u64,
    pub is_directory: bool,
    pub engine_type: EngineType,
    pub accuracy_score: f32,
    pub speed_score: f32,
    pub supports_translation: bool,
    pub is_recommended: bool,
    pub supported_languages: Vec<String>,
    pub supports_language_selection: bool,
    pub is_custom: bool,
    pub supports_streaming: bool,
    pub supports_language_detection: bool,
}

const CHINESE_LANGUAGE_CODE: &str = "zh";

pub(crate) fn recognition_language(language: &str) -> &str {
    match language {
        "zh-Hans" | "zh-Hant" => CHINESE_LANGUAGE_CODE,
        other => other,
    }
}

fn base_language(language: &str) -> &str {
    language.split(&['-', '_'][..]).next().unwrap_or(language)
}

pub(crate) fn canonical_language_code(language: &str) -> &str {
    match base_language(language) {
        "nb" => "no",
        "fil" => "tl",
        base => base,
    }
}

fn canonicalize_supported_languages(languages: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut canonical = Vec::with_capacity(languages.len());

    for language in languages {
        let language = recognition_language(&language).to_string();
        if seen.insert(language.clone()) {
            canonical.push(language);
        }
    }

    canonical
}

#[derive(Debug, Clone, Deserialize)]
pub struct QuantFile {
    pub filename: String,
    pub quant: String,
    pub size_bytes: u64,
    #[serde(default)]
    pub sha256: Option<String>,
}

pub(crate) fn default_quant_file<'a>(
    files: &'a [QuantFile],
    default_quant: Option<&str>,
) -> Option<&'a QuantFile> {
    files
        .iter()
        .find(|f| Some(f.quant.as_str()) == default_quant)
        .or_else(|| files.first())
}

#[derive(Debug, Clone, Default)]
pub struct DiskStatus {
    pub is_downloaded: bool,
    pub is_downloading: bool,
    pub partial_size: u64,
}

#[derive(Debug, Clone)]
pub struct ModelDescriptor {
    pub id: String,
    pub source: ModelSource,
    pub name: String,
    pub description: String,
    pub engine_type: EngineType,
    pub caps: CapabilityProbe,
    pub files: Vec<QuantFile>,
    pub default_quant: Option<String>,
    pub speed_score: f32,
    pub accuracy_score: f32,
    pub recommended_rank: Option<u32>,
    pub recommended: bool,
}

impl ModelDescriptor {
    fn default_file(&self) -> Option<&QuantFile> {
        default_quant_file(&self.files, self.default_quant.as_deref())
    }

    pub fn to_model_info(&self, status: &DiskStatus) -> ModelInfo {
        self.render_model_info(self.default_file(), status)
    }

    pub fn to_model_info_for_file(&self, file: &QuantFile, status: &DiskStatus) -> ModelInfo {
        self.render_model_info(Some(file), status)
    }

    fn render_model_info(&self, file: Option<&QuantFile>, status: &DiskStatus) -> ModelInfo {
        let is_default = match (file, self.default_file()) {
            (Some(f), Some(d)) => f.filename == d.filename,
            _ => true,
        };
        let id = match (&self.source, file) {
            (ModelSource::HuggingFace { repo_id, .. }, Some(f)) => {
                format!("{}/{}", repo_id, f.filename)
            }
            _ => self.id.clone(),
        };
        let name = if is_default {
            self.name.clone()
        } else {
            format!(
                "{} ({})",
                self.name,
                file.map(|f| f.quant.as_str()).unwrap_or("")
            )
        };
        let languages =
            canonicalize_supported_languages(self.caps.languages.clone().unwrap_or_default());
        ModelInfo {
            id,
            name,
            description: self.description.clone(),
            filename: file.map(|f| f.filename.clone()).unwrap_or_default(),
            source: self.source.clone(),
            size_mb: file.map(|f| f.size_bytes / (1024 * 1024)).unwrap_or(0),
            is_downloaded: status.is_downloaded,
            is_downloading: status.is_downloading,
            partial_size: status.partial_size,
            is_directory: false,
            engine_type: self.engine_type.clone(),
            accuracy_score: self.accuracy_score,
            speed_score: self.speed_score,
            supports_translation: self.caps.supports_translation.unwrap_or(false),
            is_recommended: self.recommended && is_default,
            supports_language_selection: languages.len() > 1,
            supported_languages: languages,
            is_custom: false,
            supports_streaming: self.caps.supports_streaming.unwrap_or(false),
            supports_language_detection: self.caps.supports_language_detect.unwrap_or(false),
        }
    }
}

pub fn effective_language(
    intent: &str,
    supported_languages: &[String],
    supports_language_detection: bool,
) -> String {
    if supported_languages.is_empty() {
        return intent.to_string();
    }

    if intent != "auto" {
        let exact_base_match = supported_languages
            .iter()
            .find(|language| base_language(language) == base_language(intent));
        let equivalent_match = || {
            supported_languages.iter().find(|language| {
                canonical_language_code(language) == canonical_language_code(intent)
            })
        };

        if let Some(code) = exact_base_match.or_else(equivalent_match) {
            if intent == "zh-Hans" || intent == "zh-Hant" {
                return intent.to_string();
            }
            return code.clone();
        }
    }

    if supports_language_detection {
        return "auto".to_string();
    }

    if let Some(en) = supported_languages
        .iter()
        .find(|language| base_language(language) == "en")
    {
        return en.clone();
    }
    recognition_language(&supported_languages[0]).to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub model_id: String,
    pub downloaded: u64,
    pub total: u64,
    pub percentage: f64,
}

/// hf-hub resolves purely through `refs/<revision>`. Pinned downloads write
/// `refs/<commit-sha>`, but caches populated before pinning — or by other
/// tools, which download via `main` — only have `refs/main`, so lookup falls
/// back to it. Grandfathered `main` copies may predate the pin; per policy a
/// working local model is never invalidated by routine catalog regeneration.
fn hf_cached_path(repo_id: &str, revision: &str, filename: &str) -> Option<PathBuf> {
    let get = |rev: &str| {
        Cache::from_env()
            .repo(Repo::with_revision(
                repo_id.to_string(),
                RepoType::Model,
                rev.to_string(),
            ))
            .get(filename)
    };
    get(revision).or_else(|| (revision != "main").then(|| get("main")).flatten())
}

fn probed_display_name(probe: &CapabilityProbe) -> Option<String> {
    probe
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

struct LocalCaps {
    supports_streaming: bool,
    supports_translation: bool,
    supports_language_selection: bool,
    supports_language_detection: bool,
    supported_languages: Vec<String>,
}

fn local_caps(probe: &CapabilityProbe) -> LocalCaps {
    let languages = canonicalize_supported_languages(probe.languages.clone().unwrap_or_default());
    LocalCaps {
        supports_streaming: probe.supports_streaming.unwrap_or(false),
        supports_translation: probe.supports_translation.unwrap_or(false),
        supports_language_selection: languages.len() > 1,
        supports_language_detection: probe.supports_language_detect.unwrap_or(false),
        supported_languages: languages,
    }
}

#[derive(Clone)]
struct HfDownloadProgress {
    app_handle: AppHandle,
    model_id: String,
    state: Arc<Mutex<HfProgressState>>,
}

struct HfProgressState {
    total: u64,
    downloaded: u64,
    last_emit: Instant,
    last_activity: Instant,
}

impl HfDownloadProgress {
    fn new(app_handle: AppHandle, model_id: String) -> Self {
        Self {
            app_handle,
            model_id,
            state: Arc::new(Mutex::new(HfProgressState {
                total: 0,
                downloaded: 0,
                last_emit: Instant::now(),
                last_activity: Instant::now(),
            })),
        }
    }

    fn last_activity(&self) -> Instant {
        self.state.lock().unwrap().last_activity
    }

    fn emit(&self, downloaded: u64, total: u64) {
        let percentage = if total > 0 {
            (downloaded as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        let _ = self.app_handle.emit(
            "model-download-progress",
            &DownloadProgress {
                model_id: self.model_id.clone(),
                downloaded,
                total,
                percentage,
            },
        );
    }
}

impl Progress for HfDownloadProgress {
    async fn init(&mut self, size: usize, _filename: &str) {
        {
            let mut st = self.state.lock().unwrap();
            st.total = size as u64;
            st.downloaded = 0;
            st.last_emit = Instant::now();
            st.last_activity = Instant::now();
        }
        self.emit(0, size as u64);
    }

    async fn update(&mut self, size: usize) {
        let (downloaded, total, emit) = {
            let mut st = self.state.lock().unwrap();
            st.downloaded = st.downloaded.saturating_add(size as u64);
            let now = Instant::now();
            st.last_activity = now;
            let emit = now.duration_since(st.last_emit) >= Duration::from_millis(100)
                || (st.total > 0 && st.downloaded >= st.total);
            if emit {
                st.last_emit = now;
            }
            (st.downloaded, st.total, emit)
        };
        if emit {
            self.emit(downloaded, total);
        }
    }

    async fn finish(&mut self) {
        let total = {
            let st = self.state.lock().unwrap();
            st.total.max(st.downloaded)
        };
        self.emit(total, total);
    }
}

struct RescanGuard {
    flag: Arc<AtomicBool>,
}

impl Drop for RescanGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

struct DownloadCleanup<'a> {
    available_models: &'a Mutex<HashMap<String, ModelInfo>>,
    cancel_flags: &'a Arc<Mutex<HashMap<String, CancellationToken>>>,
    model_id: String,
    disarmed: bool,
}

impl<'a> Drop for DownloadCleanup<'a> {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        {
            let mut models = self.available_models.lock().unwrap();
            if let Some(model) = models.get_mut(self.model_id.as_str()) {
                model.is_downloading = false;
            }
        }
        self.cancel_flags.lock().unwrap().remove(&self.model_id);
    }
}

pub struct ModelManager {
    app_handle: AppHandle,
    models_dir: PathBuf,
    available_models: Mutex<HashMap<String, ModelInfo>>,
    cancel_flags: Arc<Mutex<HashMap<String, CancellationToken>>>,
    is_rescanning: Arc<AtomicBool>,
}

impl ModelManager {
    #[cfg(test)]
    pub fn empty(app_handle: &AppHandle) -> Self {
        Self {
            app_handle: app_handle.clone(),
            models_dir: PathBuf::new(),
            available_models: Mutex::new(HashMap::new()),
            cancel_flags: Arc::new(Mutex::new(HashMap::new())),
            is_rescanning: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn new(app_handle: &AppHandle) -> Result<Self> {
        let models_dir = crate::portable::app_data_dir(app_handle)
            .map_err(|e| anyhow::anyhow!("Failed to get app data dir: {}", e))?
            .join("models");

        if !models_dir.exists() {
            fs::create_dir_all(&models_dir)?;
        }

        let mut available_models = HashMap::new();

        let whisper_languages: Vec<String> = vec![
            "en", "zh", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr", "pl", "ca", "nl", "ar",
            "sv", "it", "id", "hi", "fi", "vi", "he", "uk", "el", "ms", "cs", "ro", "da", "hu",
            "ta", "no", "th", "ur", "hr", "bg", "lt", "la", "mi", "ml", "cy", "sk", "te", "fa",
            "lv", "bn", "sr", "az", "sl", "kn", "et", "mk", "br", "eu", "is", "hy", "ne", "mn",
            "bs", "kk", "sq", "sw", "gl", "mr", "pa", "si", "km", "sn", "yo", "so", "af", "oc",
            "ka", "be", "tg", "sd", "gu", "am", "yi", "lo", "uz", "fo", "ht", "ps", "tk", "nn",
            "mt", "sa", "lb", "my", "bo", "tl", "mg", "as", "tt", "haw", "ln", "ha", "ba", "jw",
            "su", "yue",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        available_models.insert(
            "small".to_string(),
            ModelInfo {
                id: "small".to_string(),
                name: "Whisper Small".to_string(),
                description: "Fast and fairly accurate.".to_string(),
                filename: "ggml-small.bin".to_string(),
                source: ModelSource::Url {
                    url: "https://blob.handy.computer/ggml-small.bin".to_string(),
                    sha256: Some(
                        "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b"
                            .to_string(),
                    ),
                },
                size_mb: 465,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: false,
                engine_type: EngineType::TranscribeCpp,
                accuracy_score: 0.60,
                speed_score: 0.85,
                supports_translation: true,
                is_recommended: false,
                supported_languages: whisper_languages.clone(),
                supports_language_selection: true,
                is_custom: false,
                supports_streaming: false,
                supports_language_detection: true,
            },
        );

        available_models.insert(
            "medium".to_string(),
            ModelInfo {
                id: "medium".to_string(),
                name: "Whisper Medium".to_string(),
                description: "Good accuracy, medium speed".to_string(),
                filename: "whisper-medium-q4_1.bin".to_string(),
                source: ModelSource::Url {
                    url: "https://blob.handy.computer/whisper-medium-q4_1.bin".to_string(),
                    sha256: Some(
                        "79283fc1f9fe12ca3248543fbd54b73292164d8df5a16e095e2bceeaaabddf57"
                            .to_string(),
                    ),
                },
                size_mb: 469,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: false,
                engine_type: EngineType::TranscribeCpp,
                accuracy_score: 0.75,
                speed_score: 0.60,
                supports_translation: true,
                is_recommended: false,
                supported_languages: whisper_languages.clone(),
                supports_language_selection: true,
                is_custom: false,
                supports_streaming: false,
                supports_language_detection: true,
            },
        );

        available_models.insert(
            "turbo".to_string(),
            ModelInfo {
                id: "turbo".to_string(),
                name: "Whisper Turbo".to_string(),
                description: "Balanced accuracy and speed.".to_string(),
                filename: "ggml-large-v3-turbo.bin".to_string(),
                source: ModelSource::Url {
                    url: "https://blob.handy.computer/ggml-large-v3-turbo.bin".to_string(),
                    sha256: Some(
                        "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69"
                            .to_string(),
                    ),
                },
                size_mb: 1549,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: false,
                engine_type: EngineType::TranscribeCpp,
                accuracy_score: 0.80,
                speed_score: 0.40,
                supports_translation: false,
                is_recommended: false,
                supported_languages: whisper_languages.clone(),
                supports_language_selection: true,
                is_custom: false,
                supports_streaming: false,
                supports_language_detection: true,
            },
        );

        available_models.insert(
            "large".to_string(),
            ModelInfo {
                id: "large".to_string(),
                name: "Whisper Large".to_string(),
                description: "Good accuracy, but slow.".to_string(),
                filename: "ggml-large-v3-q5_0.bin".to_string(),
                source: ModelSource::Url {
                    url: "https://blob.handy.computer/ggml-large-v3-q5_0.bin".to_string(),
                    sha256: Some(
                        "d75795ecff3f83b5faa89d1900604ad8c780abd5739fae406de19f23ecd98ad1"
                            .to_string(),
                    ),
                },
                size_mb: 1031,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: false,
                engine_type: EngineType::TranscribeCpp,
                accuracy_score: 0.85,
                speed_score: 0.30,
                supports_translation: true,
                is_recommended: false,
                supported_languages: whisper_languages.clone(),
                supports_language_selection: true,
                is_custom: false,
                supports_streaming: false,
                supports_language_detection: true,
            },
        );

        available_models.insert(
            "breeze-asr".to_string(),
            ModelInfo {
                id: "breeze-asr".to_string(),
                name: "Breeze ASR".to_string(),
                description: "Optimized for Taiwanese Mandarin. Code-switching support."
                    .to_string(),
                filename: "breeze-asr-q5_k.bin".to_string(),
                source: ModelSource::Url {
                    url: "https://blob.handy.computer/breeze-asr-q5_k.bin".to_string(),
                    sha256: Some(
                        "8efbf0ce8a3f50fe332b7617da787fb81354b358c288b008d3bdef8359df64c6"
                            .to_string(),
                    ),
                },
                size_mb: 1030,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: false,
                engine_type: EngineType::TranscribeCpp,
                accuracy_score: 0.85,
                speed_score: 0.35,
                supports_translation: false,
                is_recommended: false,
                supported_languages: whisper_languages,
                supports_language_selection: true,
                is_custom: false,
                supports_streaming: false,
                supports_language_detection: true,
            },
        );

        available_models.insert(
            "parakeet-tdt-0.6b-v2".to_string(),
            ModelInfo {
                id: "parakeet-tdt-0.6b-v2".to_string(),
                name: "Parakeet V2".to_string(),
                description: "English only. The best model for English speakers.".to_string(),
                filename: "parakeet-tdt-0.6b-v2-int8".to_string(),
                source: ModelSource::Url {
                    url: "https://blob.handy.computer/parakeet-v2-int8.tar.gz".to_string(),
                    sha256: Some(
                        "ac9b9429984dd565b25097337a887bb7f0f8ac393573661c651f0e7d31563991"
                            .to_string(),
                    ),
                },
                size_mb: 451,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::Parakeet,
                accuracy_score: 0.85,
                speed_score: 0.85,
                supports_translation: false,
                is_recommended: false,
                supported_languages: vec!["en".to_string()],
                supports_language_selection: false,
                is_custom: false,
                supports_streaming: false,
                supports_language_detection: true,
            },
        );

        let parakeet_v3_languages: Vec<String> = vec![
            "bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hu", "it", "lv",
            "lt", "mt", "pl", "pt", "ro", "sk", "sl", "es", "sv", "ru", "uk",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        available_models.insert(
            "parakeet-tdt-0.6b-v3".to_string(),
            ModelInfo {
                id: "parakeet-tdt-0.6b-v3".to_string(),
                name: "Parakeet V3".to_string(),
                description: "Fast and accurate. Supports 25 European languages.".to_string(),
                filename: "parakeet-tdt-0.6b-v3-int8".to_string(),
                source: ModelSource::Url {
                    url: "https://blob.handy.computer/parakeet-v3-int8.tar.gz".to_string(),
                    sha256: Some(
                        "43d37191602727524a7d8c6da0eef11c4ba24320f5b4730f1a2497befc2efa77"
                            .to_string(),
                    ),
                },
                size_mb: 456,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::Parakeet,
                accuracy_score: 0.80,
                speed_score: 0.85,
                supports_translation: false,
                is_recommended: true,
                supported_languages: parakeet_v3_languages,
                supports_language_selection: false,
                is_custom: false,
                supports_streaming: false,
                supports_language_detection: true,
            },
        );

        available_models.insert(
            "moonshine-base".to_string(),
            ModelInfo {
                id: "moonshine-base".to_string(),
                name: "Moonshine Base".to_string(),
                description: "Very fast, English only. Handles accents well.".to_string(),
                filename: "moonshine-base".to_string(),
                source: ModelSource::Url {
                    url: "https://blob.handy.computer/moonshine-base.tar.gz".to_string(),
                    sha256: Some(
                        "04bf6ab012cfceebd4ac7cf88c1b31d027bbdd3cd704649b692e2e935236b7e8"
                            .to_string(),
                    ),
                },
                size_mb: 55,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::Moonshine,
                accuracy_score: 0.70,
                speed_score: 0.90,
                supports_translation: false,
                is_recommended: false,
                supported_languages: vec!["en".to_string()],
                supports_language_selection: false,
                is_custom: false,
                supports_streaming: false,
                supports_language_detection: true,
            },
        );

        available_models.insert(
            "moonshine-tiny-streaming-en".to_string(),
            ModelInfo {
                id: "moonshine-tiny-streaming-en".to_string(),
                name: "Moonshine V2 Tiny".to_string(),
                description: "Ultra-fast, English only".to_string(),
                filename: "moonshine-tiny-streaming-en".to_string(),
                source: ModelSource::Url {
                    url: "https://blob.handy.computer/moonshine-tiny-streaming-en.tar.gz"
                        .to_string(),
                    sha256: Some(
                        "465addcfca9e86117415677dfdc98b21edc53537210333a3ecdb58509a80abaf"
                            .to_string(),
                    ),
                },
                size_mb: 31,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::MoonshineStreaming,
                accuracy_score: 0.55,
                speed_score: 0.95,
                supports_translation: false,
                is_recommended: false,
                supported_languages: vec!["en".to_string()],
                supports_language_selection: false,
                is_custom: false,
                supports_streaming: false,
                supports_language_detection: true,
            },
        );

        available_models.insert(
            "moonshine-small-streaming-en".to_string(),
            ModelInfo {
                id: "moonshine-small-streaming-en".to_string(),
                name: "Moonshine V2 Small".to_string(),
                description: "Fast, English only. Good balance of speed and accuracy.".to_string(),
                filename: "moonshine-small-streaming-en".to_string(),
                source: ModelSource::Url {
                    url: "https://blob.handy.computer/moonshine-small-streaming-en.tar.gz"
                        .to_string(),
                    sha256: Some(
                        "dbb3e1c1832bd88a4ac712f7449a136cc2c9a18c5fe33a12ed1b7cb1cfe9cdd5"
                            .to_string(),
                    ),
                },
                size_mb: 99,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::MoonshineStreaming,
                accuracy_score: 0.65,
                speed_score: 0.90,
                supports_translation: false,
                is_recommended: false,
                supported_languages: vec!["en".to_string()],
                supports_language_selection: false,
                is_custom: false,
                supports_streaming: false,
                supports_language_detection: true,
            },
        );

        available_models.insert(
            "moonshine-medium-streaming-en".to_string(),
            ModelInfo {
                id: "moonshine-medium-streaming-en".to_string(),
                name: "Moonshine V2 Medium".to_string(),
                description: "English only. High quality.".to_string(),
                filename: "moonshine-medium-streaming-en".to_string(),
                source: ModelSource::Url {
                    url: "https://blob.handy.computer/moonshine-medium-streaming-en.tar.gz"
                        .to_string(),
                    sha256: Some(
                        "07a66f3bff1c77e75a2f637e5a263928a08baae3c29c4c053fc968a9a9373d13"
                            .to_string(),
                    ),
                },
                size_mb: 192,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::MoonshineStreaming,
                accuracy_score: 0.75,
                speed_score: 0.80,
                supports_translation: false,
                is_recommended: false,
                supported_languages: vec!["en".to_string()],
                supports_language_selection: false,
                is_custom: false,
                supports_streaming: false,
                supports_language_detection: true,
            },
        );

        let sense_voice_languages: Vec<String> = vec!["zh", "en", "yue", "ja", "ko"]
            .into_iter()
            .map(String::from)
            .collect();

        available_models.insert(
            "sense-voice-int8".to_string(),
            ModelInfo {
                id: "sense-voice-int8".to_string(),
                name: "SenseVoice".to_string(),
                description: "Very fast. Chinese, English, Japanese, Korean, Cantonese."
                    .to_string(),
                filename: "sense-voice-int8".to_string(),
                source: ModelSource::Url {
                    url: "https://blob.handy.computer/sense-voice-int8.tar.gz".to_string(),
                    sha256: Some(
                        "171d611fe5d353a50bbb741b6f3ef42559b1565685684e9aa888ef563ba3e8a4"
                            .to_string(),
                    ),
                },
                size_mb: 152,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::SenseVoice,
                accuracy_score: 0.65,
                speed_score: 0.95,
                supports_translation: false,
                is_recommended: false,
                supported_languages: sense_voice_languages,
                supports_language_selection: true,
                is_custom: false,
                supports_streaming: false,
                supports_language_detection: true,
            },
        );

        let gigaam_languages: Vec<String> = vec!["ru"].into_iter().map(String::from).collect();

        available_models.insert(
            "gigaam-v3-e2e-ctc".to_string(),
            ModelInfo {
                id: "gigaam-v3-e2e-ctc".to_string(),
                name: "GigaAM v3".to_string(),
                description: "Russian speech recognition. Fast and accurate.".to_string(),
                filename: "giga-am-v3-int8".to_string(),
                source: ModelSource::Url {
                    url: "https://blob.handy.computer/giga-am-v3-int8.tar.gz".to_string(),
                    sha256: Some(
                        "d872462268430db140b69b72e0fc4b787b194c1dbe51b58de39444d55b6da45b"
                            .to_string(),
                    ),
                },
                size_mb: 151,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::GigaAM,
                accuracy_score: 0.85,
                speed_score: 0.75,
                supports_translation: false,
                is_recommended: false,
                supported_languages: gigaam_languages,
                supports_language_selection: false,
                is_custom: false,
                supports_streaming: false,
                supports_language_detection: true,
            },
        );

        let canary_flash_languages: Vec<String> = vec!["en", "de", "es", "fr"]
            .into_iter()
            .map(String::from)
            .collect();

        available_models.insert(
            "canary-180m-flash".to_string(),
            ModelInfo {
                id: "canary-180m-flash".to_string(),
                name: "Canary 180M Flash".to_string(),
                description: "Very fast. English, German, Spanish, French. Supports translation."
                    .to_string(),
                filename: "canary-180m-flash".to_string(),
                source: ModelSource::Url {
                    url: "https://blob.handy.computer/canary-180m-flash.tar.gz".to_string(),
                    sha256: Some(
                        "6d9cfca6118b296e196eaedc1c8fa9788305a7b0f1feafdb6dc91932ab6e53f7"
                            .to_string(),
                    ),
                },
                size_mb: 146,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::Canary,
                accuracy_score: 0.75,
                speed_score: 0.85,
                supports_translation: true,
                is_recommended: false,
                supported_languages: canary_flash_languages,
                supports_language_selection: true,
                is_custom: false,
                supports_streaming: false,
                supports_language_detection: false,
            },
        );

        let canary_1b_languages: Vec<String> = vec![
            "bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hu", "it", "lv",
            "lt", "mt", "pl", "pt", "ro", "sk", "sl", "es", "sv", "ru", "uk",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        available_models.insert(
            "canary-1b-v2".to_string(),
            ModelInfo {
                id: "canary-1b-v2".to_string(),
                name: "Canary 1B v2".to_string(),
                description: "Accurate multilingual. 25 European languages. Supports translation."
                    .to_string(),
                filename: "canary-1b-v2".to_string(),
                source: ModelSource::Url {
                    url: "https://blob.handy.computer/canary-1b-v2.tar.gz".to_string(),
                    sha256: Some(
                        "02305b2a25f9cf3e7deaffa7f94df00efa44f442cd55c101c2cb9c000f904666"
                            .to_string(),
                    ),
                },
                size_mb: 691,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::Canary,
                accuracy_score: 0.85,
                speed_score: 0.70,
                supports_translation: true,
                is_recommended: false,
                supported_languages: canary_1b_languages,
                supports_language_selection: true,
                is_custom: false,
                supports_streaming: false,
                supports_language_detection: false,
            },
        );

        let cohere_languages: Vec<String> = vec![
            "en", "fr", "de", "it", "es", "pt", "el", "nl", "pl", "zh", "ja", "ko", "vi", "ar",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        available_models.insert(
            "cohere-int8".to_string(),
            ModelInfo {
                id: "cohere-int8".to_string(),
                name: "Cohere".to_string(),
                description: "A large, slower, but very accurate multilingual model.".to_string(),
                filename: "cohere-int8".to_string(),
                source: ModelSource::Url {
                    url: "https://blob.handy.computer/cohere-int8.tar.gz".to_string(),
                    sha256: Some(
                        "ea2257d52434f3644574f187dcdcf666e302cd11b92866116ab8e14cd9c887f0"
                            .to_string(),
                    ),
                },
                size_mb: 1708,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::Cohere,
                accuracy_score: 0.90,
                speed_score: 0.60,
                supports_translation: false,
                is_recommended: false,
                supported_languages: cohere_languages,
                supports_language_selection: true,
                is_custom: false,
                supports_streaming: false,
                supports_language_detection: true,
            },
        );

        Self::seed_catalog_models(&mut available_models);

        if crate::apple_speech::status() != crate::apple_speech::Status::Unsupported {
            available_models.insert(
                crate::apple_speech::MODEL_ID.to_string(),
                crate::apple_speech::model_info(),
            );
        }

        if let Err(e) = Self::discover_custom_transcribe_models(&models_dir, &mut available_models)
        {
            warn!("Failed to discover custom models: {}", e);
        }

        Self::discover_hf_cache_models(&mut available_models);

        let manager = Self {
            app_handle: app_handle.clone(),
            models_dir,
            available_models: Mutex::new(available_models),
            cancel_flags: Arc::new(Mutex::new(HashMap::new())),
            is_rescanning: Arc::new(AtomicBool::new(false)),
        };

        manager.migrate_bundled_models()?;

        manager.migrate_gigaam_to_directory()?;

        manager.update_download_status();

        manager.auto_select_model_if_needed();

        Ok(manager)
    }

    pub fn get_available_models(&self) -> Vec<ModelInfo> {
        let mut list: Vec<ModelInfo> = {
            let models = self.available_models.lock().unwrap();
            models.values().cloned().collect()
        };
        list.sort_by(|a, b| {
            crate::catalog::rank_of(&a.id)
                .cmp(&crate::catalog::rank_of(&b.id))
                .then((!a.is_recommended).cmp(&(!b.is_recommended)))
                .then(b.accuracy_score.total_cmp(&a.accuracy_score))
                .then(b.speed_score.total_cmp(&a.speed_score))
                .then_with(|| a.name.cmp(&b.name))
        });
        list
    }

    fn seed_catalog_models(available_models: &mut HashMap<String, ModelInfo>) {
        use std::collections::hash_map::Entry;
        let mut added = 0usize;
        for desc in crate::catalog::CATALOG.iter() {
            if let Entry::Vacant(slot) = available_models.entry(desc.id.clone()) {
                slot.insert(desc.to_model_info(&DiskStatus::default()));
                added += 1;
            }
        }
        info!("Seeded {} catalog model(s) into the registry", added);
    }

    fn try_start_rescan(&self) -> Option<RescanGuard> {
        if self.is_rescanning.swap(true, Ordering::SeqCst) {
            None
        } else {
            Some(RescanGuard {
                flag: self.is_rescanning.clone(),
            })
        }
    }

    pub fn rescan_local_models(&self) {
        let _guard = match self.try_start_rescan() {
            Some(g) => g,
            None => {
                debug!("Model rescan already in progress; skipping");
                return;
            }
        };

        let mut snapshot = self.available_models.lock().unwrap().clone();
        if let Err(e) = Self::discover_custom_transcribe_models(&self.models_dir, &mut snapshot) {
            warn!("Rescan: failed to discover custom models: {}", e);
        }
        Self::discover_hf_cache_models(&mut snapshot);

        let mut added = 0usize;
        {
            let mut live = self.available_models.lock().unwrap();
            for (id, info) in snapshot {
                if let std::collections::hash_map::Entry::Vacant(entry) = live.entry(id) {
                    entry.insert(info);
                    added += 1;
                }
            }
        }

        self.update_download_status();
        self.auto_select_model_if_needed();
        if added > 0 {
            info!("Model rescan discovered {} new model(s)", added);
        }
        let _ = self.app_handle.emit("models-updated", ());
    }

    pub fn get_model_info(&self, model_id: &str) -> Option<ModelInfo> {
        let models = self.available_models.lock().unwrap();
        models.get(model_id).cloned()
    }

    pub fn set_runtime_capabilities(
        &self,
        model_id: &str,
        supports_streaming: bool,
        supports_translation: bool,
        supports_language_detection: bool,
        supported_languages: Vec<String>,
    ) {
        let supported_languages = canonicalize_supported_languages(supported_languages);
        let mut models = self.available_models.lock().unwrap();
        if let Some(model) = models.get_mut(model_id) {
            model.supports_streaming = supports_streaming;
            model.supports_translation = supports_translation;
            model.supports_language_detection = supports_language_detection;
            if !supported_languages.is_empty() {
                model.supports_language_selection = supported_languages.len() > 1;
                model.supported_languages = supported_languages;
            }
        }
    }

    fn migrate_bundled_models(&self) -> Result<()> {
        let bundled_models = ["ggml-small.bin"];

        for filename in &bundled_models {
            let bundled_path = self.app_handle.path().resolve(
                format!("resources/models/{}", filename),
                crate::app_handle::BaseDirectory::Resource,
            );

            if let Ok(bundled_path) = bundled_path
                && bundled_path.exists()
            {
                let user_path = self.models_dir.join(filename);

                if !user_path.exists() {
                    info!("Migrating bundled model {} to user directory", filename);
                    fs::copy(&bundled_path, &user_path)?;
                    info!("Successfully migrated {}", filename);
                }
            }
        }

        Ok(())
    }

    fn migrate_gigaam_to_directory(&self) -> Result<()> {
        let old_file = self.models_dir.join("giga-am-v3.int8.onnx");
        let new_dir = self.models_dir.join("giga-am-v3-int8");

        if !old_file.exists() || new_dir.exists() {
            return Ok(());
        }

        info!("Migrating GigaAM from single-file to directory format");

        let vocab_path = self
            .app_handle
            .path()
            .resolve(
                "resources/models/gigaam_vocab.txt",
                crate::app_handle::BaseDirectory::Resource,
            )
            .map_err(|e| anyhow::anyhow!("Failed to resolve GigaAM vocab path: {}", e))?;

        info!(
            "Resolved vocab path: {:?} (exists: {})",
            vocab_path,
            vocab_path.exists()
        );
        info!("Old file: {:?} (exists: {})", old_file, old_file.exists());
        info!("New dir: {:?} (exists: {})", new_dir, new_dir.exists());

        fs::create_dir_all(&new_dir)?;
        fs::rename(&old_file, new_dir.join("model.int8.onnx"))?;
        fs::copy(&vocab_path, new_dir.join("vocab.txt"))?;

        let old_partial = self.models_dir.join("giga-am-v3.int8.onnx.partial");
        if old_partial.exists() {
            let _ = fs::remove_file(&old_partial);
        }

        info!("GigaAM migration complete");
        Ok(())
    }

    fn update_download_status(&self) {
        let downloading_ids: HashSet<String> =
            self.cancel_flags.lock().unwrap().keys().cloned().collect();
        let mut models = self.available_models.lock().unwrap();
        let mut vanished_models: Vec<String> = Vec::new();

        for model in models.values_mut() {
            if let ModelSource::System = model.source {
                use crate::apple_speech::Status;
                let status = crate::apple_speech::status();
                model.is_downloaded = matches!(status, Status::Installed | Status::Unchecked);
                model.is_downloading = status == Status::Downloading;
                model.partial_size = 0;
                if status == Status::Unsupported {
                    vanished_models.push(model.id.clone());
                }
                continue;
            }
            if let ModelSource::HuggingFace { repo_id, revision } = &model.source {
                let local_path = self.models_dir.join(&model.filename);
                let partial_path = self.models_dir.join(format!("{}.partial", model.filename));
                model.is_downloaded = hf_cached_path(repo_id, revision, &model.filename).is_some()
                    || local_path.exists();
                model.is_downloading = false;
                model.partial_size = partial_path.metadata().map(|m| m.len()).unwrap_or(0);
                if !model.is_downloaded
                    && !downloading_ids.contains(&model.id)
                    && Self::disappears_when_missing(model)
                {
                    vanished_models.push(model.id.clone());
                }
                continue;
            }
            if model.is_directory {
                let model_path = self.models_dir.join(&model.filename);
                let partial_path = self.models_dir.join(format!("{}.partial", model.filename));

                model.is_downloaded = model_path.exists() && model_path.is_dir();
                model.is_downloading = false;

                if partial_path.exists() {
                    model.partial_size = partial_path.metadata().map(|m| m.len()).unwrap_or(0);
                } else {
                    model.partial_size = 0;
                }
            } else {
                let model_path = self.models_dir.join(&model.filename);
                let partial_path = self.models_dir.join(format!("{}.partial", model.filename));

                model.is_downloaded = model_path.exists();
                model.is_downloading = false;

                if partial_path.exists() {
                    model.partial_size = partial_path.metadata().map(|m| m.len()).unwrap_or(0);
                } else {
                    model.partial_size = 0;
                }
            }

            if !model.is_downloaded
                && !downloading_ids.contains(&model.id)
                && Self::disappears_when_missing(model)
            {
                vanished_models.push(model.id.clone());
            }
        }

        for id in vanished_models {
            models.remove(&id);
        }
    }

    fn is_catalog_alternate_quant(repo_id: &str, filename: &str) -> bool {
        crate::catalog::file_in_catalog(filename, Some(repo_id)).is_some_and(|(desc, file)| {
            desc.default_file()
                .is_some_and(|d| d.filename != file.filename)
        })
    }

    fn disappears_when_missing(model: &ModelInfo) -> bool {
        model.is_custom
            || match &model.source {
                ModelSource::HuggingFace { repo_id, .. } => {
                    Self::is_catalog_alternate_quant(repo_id, &model.filename)
                }
                _ => false,
            }
    }

    fn selected_model_is_available(models: &HashMap<String, ModelInfo>, model_id: &str) -> bool {
        models
            .get(model_id)
            .is_some_and(|model| model.is_downloaded)
    }

    fn delete_hf_cache_file(repo_id: &str, revision: &str, filename: &str) -> bool {
        let Some(pointer) = hf_cached_path(repo_id, revision, filename) else {
            return false;
        };
        // Resolve the blob before the pointer goes away. On Windows the
        // pointer may be a plain file (hf-hub's symlink fallback renames the
        // blob into the snapshot), in which case there is no separate blob.
        let is_symlink = fs::symlink_metadata(&pointer)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink && let Ok(blob) = fs::canonicalize(&pointer) {
            info!("Deleting HF cache blob at: {:?}", blob);
            let _ = fs::remove_file(&blob);
        }
        info!("Deleting HF cache file at: {:?}", pointer);
        fs::remove_file(&pointer).is_ok()
    }

    fn auto_select_model_if_needed(&self) {
        let mut settings = get_settings(&self.app_handle);

        if !settings.selected_model.is_empty() {
            let is_available = {
                let models = self.available_models.lock().unwrap();
                Self::selected_model_is_available(&models, &settings.selected_model)
            };

            if !is_available {
                info!(
                    "Selected model '{}' is not available on disk; clearing selection",
                    settings.selected_model
                );
                settings.selected_model = String::new();
                write_settings(&self.app_handle, settings.clone());
            }
        }

        if !settings.onboarding_completed {
            debug!("Skipping model auto-selection until onboarding is complete");
            return;
        }

        if settings.selected_model.is_empty()
            && let Some(available_model) = self
                .get_available_models()
                .into_iter()
                .find(|model| model.is_downloaded)
        {
            info!(
                "Auto-selecting model: {} ({})",
                available_model.id, available_model.name
            );

            let mut updated_settings = settings;
            updated_settings.selected_model = available_model.id.clone();
            write_settings(&self.app_handle, updated_settings);

            info!("Successfully auto-selected model: {}", available_model.id);
        }
    }

    fn discover_custom_transcribe_models(
        models_dir: &Path,
        available_models: &mut HashMap<String, ModelInfo>,
    ) -> Result<()> {
        if !models_dir.exists() {
            return Ok(());
        }

        let predefined_filenames: HashSet<String> = available_models
            .values()
            .filter(|m| matches!(m.engine_type, EngineType::TranscribeCpp) && !m.is_directory)
            .map(|m| m.filename.clone())
            .collect();

        for entry in fs::read_dir(models_dir)? {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warn!("Failed to read directory entry: {}", e);
                    continue;
                }
            };

            let path = entry.path();

            if !path.is_file() {
                continue;
            }

            let filename = match path.file_name().and_then(|s| s.to_str()) {
                Some(name) => name.to_string(),
                None => continue,
            };

            if filename.starts_with('.') {
                continue;
            }

            let (model_id, is_gguf) = if let Some(stem) = filename.strip_suffix(".bin") {
                (stem.to_string(), false)
            } else if let Some(stem) = filename.strip_suffix(".gguf") {
                (stem.to_string(), true)
            } else {
                continue;
            };

            if predefined_filenames.contains(&filename) {
                continue;
            }

            if let Some((desc, quant_file)) = crate::catalog::file_in_catalog(&filename, None) {
                let info = desc.to_model_info_for_file(
                    quant_file,
                    &DiskStatus {
                        is_downloaded: true,
                        ..Default::default()
                    },
                );
                if !available_models.contains_key(&info.id) {
                    info!(
                        "Discovered catalog quant in models dir: {} ({})",
                        info.id, filename
                    );
                    available_models.insert(info.id.clone(), info);
                }
                continue;
            }

            if available_models.contains_key(&model_id) {
                continue;
            }

            let fallback_display_name = model_id
                .replace(['-', '_'], " ")
                .split_whitespace()
                .map(|word| {
                    let mut chars = word.chars();
                    match chars.next() {
                        None => String::new(),
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");

            let size_mb = match path.metadata() {
                Ok(meta) => meta.len() / (1024 * 1024),
                Err(e) => {
                    warn!("Failed to get metadata for {}: {}", filename, e);
                    0
                }
            };

            let probe = if is_gguf {
                GgufHeaderProber.probe_file(&path)
            } else {
                CapabilityProbe::default()
            };
            let caps = local_caps(&probe);
            let display_name = probed_display_name(&probe).unwrap_or(fallback_display_name);

            info!(
                "Discovered custom transcribe-cpp model: {} ({}, {} MB, streaming={})",
                model_id, filename, size_mb, caps.supports_streaming
            );

            available_models.insert(
                model_id.clone(),
                ModelInfo {
                    id: model_id,
                    name: display_name,
                    description: "Not officially supported".to_string(),
                    filename,
                    source: ModelSource::Local,
                    size_mb,
                    is_downloaded: true,
                    is_downloading: false,
                    partial_size: 0,
                    is_directory: false,
                    engine_type: EngineType::TranscribeCpp,
                    accuracy_score: 0.0,
                    speed_score: 0.0,
                    supports_translation: caps.supports_translation,
                    is_recommended: false,
                    supported_languages: caps.supported_languages,
                    supports_language_selection: caps.supports_language_selection,
                    is_custom: true,
                    supports_streaming: caps.supports_streaming,
                    supports_language_detection: caps.supports_language_detection,
                },
            );
        }

        Ok(())
    }

    fn discover_hf_cache_models(available_models: &mut HashMap<String, ModelInfo>) {
        Self::discover_hf_cache_models_in(Cache::from_env().path(), available_models);
    }

    fn discover_hf_cache_models_in(
        cache_root: &Path,
        available_models: &mut HashMap<String, ModelInfo>,
    ) {
        if !cache_root.is_dir() {
            return;
        }

        let known_hf: HashSet<(String, String)> = available_models
            .values()
            .filter_map(|m| match &m.source {
                ModelSource::HuggingFace { repo_id, .. } => {
                    Some((repo_id.clone(), m.filename.clone()))
                }
                _ => None,
            })
            .collect();

        let prober = GgufHeaderProber;

        let entries = match fs::read_dir(cache_root) {
            Ok(entries) => entries,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let folder = entry.file_name();
            let folder = folder.to_string_lossy();
            let Some(rest) = folder.strip_prefix("models--") else {
                continue;
            };
            // Reverse hf-hub's `org/name` -> `models--org--name` folder naming.
            let repo_id = rest.replace("--", "/");

            let refs_dir = entry.path().join("refs");
            let Some(revision) = Self::pick_hf_revision(&refs_dir) else {
                continue;
            };
            let Ok(commit) = fs::read_to_string(refs_dir.join(&revision)) else {
                continue;
            };
            let snapshot = entry.path().join("snapshots").join(commit.trim());
            let Ok(files) = fs::read_dir(&snapshot) else {
                continue;
            };

            for file in files.flatten() {
                let fname = file.file_name().to_string_lossy().to_string();
                if !fname.ends_with(".gguf") {
                    continue;
                }
                if known_hf.contains(&(repo_id.clone(), fname.clone())) {
                    continue;
                }
                let model_id = format!("{}/{}", repo_id, fname);
                if available_models.contains_key(&model_id) {
                    continue;
                }

                if let Some((desc, quant_file)) =
                    crate::catalog::file_in_catalog(&fname, Some(&repo_id))
                {
                    let info = desc.to_model_info_for_file(
                        quant_file,
                        &DiskStatus {
                            is_downloaded: true,
                            ..Default::default()
                        },
                    );
                    info!(
                        "Discovered catalog quant in HF cache: {} ({})",
                        info.id, repo_id
                    );
                    available_models.insert(info.id.clone(), info);
                    continue;
                }

                let path = snapshot.join(&fname);
                let probe = prober.probe_file(&path);
                if probe.verdict != Compatibility::Compatible {
                    continue;
                }
                let caps = local_caps(&probe);

                let size_mb = path
                    .metadata()
                    .map(|m| m.len() / (1024 * 1024))
                    .unwrap_or(0);
                let display = probed_display_name(&probe)
                    .unwrap_or_else(|| fname.trim_end_matches(".gguf").to_string());

                info!("Discovered HF cache model: {} ({})", model_id, repo_id);
                available_models.insert(
                    model_id.clone(),
                    ModelInfo {
                        id: model_id,
                        name: display,
                        description: format!("From Hugging Face cache: {}", repo_id),
                        filename: fname,
                        source: ModelSource::HuggingFace {
                            repo_id: repo_id.clone(),
                            revision: revision.clone(),
                        },
                        size_mb,
                        is_downloaded: true,
                        is_downloading: false,
                        partial_size: 0,
                        is_directory: false,
                        engine_type: EngineType::TranscribeCpp,
                        accuracy_score: 0.0,
                        speed_score: 0.0,
                        supports_translation: caps.supports_translation,
                        is_recommended: false,
                        supported_languages: caps.supported_languages,
                        supports_language_selection: caps.supports_language_selection,
                        is_custom: false,
                        supports_streaming: caps.supports_streaming,
                        supports_language_detection: caps.supports_language_detection,
                    },
                );
            }
        }
    }

    fn pick_hf_revision(refs_dir: &Path) -> Option<String> {
        if refs_dir.join("main").is_file() {
            return Some("main".to_string());
        }
        fs::read_dir(refs_dir).ok()?.flatten().find_map(|e| {
            if e.path().is_file() {
                e.file_name().to_str().map(str::to_string)
            } else {
                None
            }
        })
    }

    async fn download_hf_model(
        &self,
        model_info: &ModelInfo,
        repo_id: String,
        revision: String,
    ) -> Result<()> {
        let model_id = model_info.id.clone();
        let filename = model_info.filename.clone();

        if hf_cached_path(&repo_id, &revision, &filename).is_some()
            || self.models_dir.join(&filename).exists()
        {
            self.update_download_status();
            let _ = self.app_handle.emit("model-download-complete", &model_id);
            return Ok(());
        }

        {
            let mut models = self.available_models.lock().unwrap();
            if let Some(model) = models.get_mut(&model_id) {
                model.is_downloading = true;
            }
        }

        let cancel_token = CancellationToken::new();
        {
            let mut flags = self.cancel_flags.lock().unwrap();
            flags.insert(model_id.clone(), cancel_token.clone());
        }

        let mut cleanup = DownloadCleanup {
            available_models: &self.available_models,
            cancel_flags: &self.cancel_flags,
            model_id: model_id.clone(),
            disarmed: false,
        };

        info!(
            "Downloading HF model {} from {}@{} ({})",
            model_id, repo_id, revision, filename
        );

        // hf-hub has no working internal retry (its retry knobs are hardcoded
        // to zero), so a single transient fault — dropped connection, a 429
        // from the resolve endpoint, a CDN blip — would otherwise fail the
        // whole download. Each attempt resumes from the `.sync.part`
        // committed-offset marker, so a retry only re-fetches what the failed
        // attempt hadn't finished.
        // Start moderately parallel for normal-network throughput, then stay
        // sequential after the first failure. Eight simultaneous connections
        // were all reset on an affected network in #1579, while one stream
        // succeeded; four is a less aggressive fast path, and every retry uses
        // the known-compatible request pattern.
        const ATTEMPT_STREAMS: [usize; 4] = [4, 1, 1, 1];
        let mut attempt: usize = 1;
        let hf_error = loop {
            let stream_count = ATTEMPT_STREAMS[attempt - 1];
            info!(
                "HF download attempt {}/{} for {} using {} concurrent stream(s)",
                attempt,
                ATTEMPT_STREAMS.len(),
                model_id,
                stream_count
            );

            let api = ApiBuilder::from_env()
                // Ignore cached and environment-provided credentials. A stale token
                // can make otherwise-public downloads fail authentication.
                .with_token(None)
                .with_progress(false)
                .with_max_files(stream_count)
                .build()
                .map_err(|e| anyhow::anyhow!("Failed to init Hugging Face API: {}", e))?;
            let repo = api.repo(Repo::with_revision(
                repo_id.clone(),
                RepoType::Model,
                revision.clone(),
            ));
            let progress = HfDownloadProgress::new(self.app_handle.clone(), model_id.clone());

            // hf-hub has no internal timeouts, so a wedged connection would
            // otherwise hang this attempt forever and neither the retry loop
            // nor the mirror fallback would ever fire. The watchdog cancels a
            // per-attempt child token when progress goes stale; a user cancel
            // on the parent propagates through the same child.
            let attempt_token = cancel_token.child_token();
            let watchdog = tokio::spawn({
                let probe = progress.clone();
                let attempt_token = attempt_token.clone();
                async move {
                    loop {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        if attempt_token.is_cancelled() {
                            break;
                        }
                        if probe.last_activity().elapsed() > DOWNLOAD_STALL_TIMEOUT {
                            attempt_token.cancel();
                            break;
                        }
                    }
                }
            });
            // hf-hub only observes its token inside the chunk loop — the
            // metadata/resolve request and cache lock run before it, so a hang
            // there would ignore the cancel entirely. Race the whole future
            // against the token: on cancel, grant a short grace so an attempt
            // that IS in the chunk loop can unwind gracefully (committing the
            // `.sync.part` resume offset), then drop the future outright,
            // which aborts whatever request it was wedged in.
            let mut download = std::pin::pin!(repo.download_with_progress_cancellable(
                &filename,
                progress,
                attempt_token.clone()
            ));
            let result = tokio::select! {
                r = &mut download => r,
                _ = attempt_token.cancelled() => {
                    match tokio::time::timeout(Duration::from_secs(5), &mut download).await {
                        Ok(r) => r,
                        Err(_) => Err(hf_hub::api::tokio::ApiError::Cancelled),
                    }
                }
            };
            watchdog.abort();

            match result {
                Ok(_) => break None,
                Err(hf_hub::api::tokio::ApiError::Cancelled) if cancel_token.is_cancelled() => {
                    // User cancelled. hf-hub leaves the partially downloaded
                    // `.sync.part` in the shared cache, so a later attempt resumes
                    // instead of restarting. The guard resets is_downloading and
                    info!("HF download cancelled for: {}", model_id);
                    return Ok(());
                }
                Err(hf_hub::api::tokio::ApiError::Cancelled) => {
                    let err = anyhow::anyhow!(
                        "transfer stalled: no progress for {}s",
                        DOWNLOAD_STALL_TIMEOUT.as_secs()
                    );
                    if stream_count == 1 || attempt >= ATTEMPT_STREAMS.len() {
                        break Some(err);
                    }
                    let delay = Duration::from_secs(1_u64 << attempt);
                    warn!(
                        "HF download attempt {}/{} stalled for {} using {} concurrent stream(s); retrying with {} stream(s) in {}s",
                        attempt,
                        ATTEMPT_STREAMS.len(),
                        model_id,
                        stream_count,
                        ATTEMPT_STREAMS[attempt],
                        delay.as_secs()
                    );
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = cancel_token.cancelled() => {
                            info!("HF download cancelled for: {}", model_id);
                            return Ok(());
                        }
                    }
                    attempt += 1;
                }
                Err(e) => {
                    // {:?} keeps the error source chain (reset vs TLS vs timeout);
                    // Display truncates it to "error sending request".
                    let err = anyhow::anyhow!("{:?}", e);
                    if attempt >= ATTEMPT_STREAMS.len() {
                        break Some(err);
                    }
                    let delay = Duration::from_secs(1_u64 << attempt);
                    warn!(
                        "HF download attempt {}/{} failed for {} using {} concurrent stream(s): {}; retrying with {} stream(s) in {}s",
                        attempt,
                        ATTEMPT_STREAMS.len(),
                        model_id,
                        stream_count,
                        err,
                        ATTEMPT_STREAMS[attempt],
                        delay.as_secs()
                    );
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = cancel_token.cancelled() => {
                            info!("HF download cancelled for: {}", model_id);
                            return Ok(());
                        }
                    }
                    attempt += 1;
                }
            }
        };

        if let Some(hf_error) = hf_error {
            error!(
                "HF download failed for {} after {} attempt(s): {:?}",
                model_id, attempt, hf_error
            );
            let mirrors = crate::catalog::mirror_fallbacks(&model_id);
            if mirrors.is_empty() {
                return Err(anyhow::anyhow!(
                    "Hugging Face download failed after {} attempt(s): {}",
                    attempt,
                    hf_error
                ));
            }
            let mut completed = false;
            for mirror in &mirrors {
                info!("Falling back to mirror for {}: {}", model_id, mirror.url);
                match self
                    .download_from_mirror(&model_id, &filename, mirror, cancel_token.clone())
                    .await
                {
                    Ok(true) => {
                        completed = true;
                        break;
                    }
                    Ok(false) => {
                        info!("Mirror download cancelled for: {}", model_id);
                        return Ok(());
                    }
                    Err(e) => {
                        warn!(
                            "Mirror download failed for {} from {}: {:?}",
                            model_id, mirror.url, e
                        );
                    }
                }
            }
            if !completed {
                return Err(anyhow::anyhow!(
                    "Download failed from Hugging Face ({}) and {} mirror(s)",
                    hf_error,
                    mirrors.len()
                ));
            }
        }

        cleanup.disarmed = true;
        self.update_download_status();
        self.cancel_flags.lock().unwrap().remove(&model_id);
        let _ = self.app_handle.emit("model-download-complete", &model_id);
        info!("HF model {} downloaded", model_id);
        Ok(())
    }

    async fn download_from_mirror(
        &self,
        model_id: &str,
        filename: &str,
        mirror: &crate::catalog::MirrorFile,
        cancel_token: CancellationToken,
    ) -> Result<bool> {
        fs::create_dir_all(&self.models_dir)?;
        let model_path = self.models_dir.join(filename);
        let partial_path = self.models_dir.join(format!("{}.partial", filename));

        if model_path.exists() {
            return Ok(true);
        }

        match self
            .download_http_resumable(
                model_id,
                &mirror.url,
                &partial_path,
                Some(mirror.size_bytes),
                Some(&mirror.sha256),
                &cancel_token,
            )
            .await?
        {
            HttpDownloadOutcome::Cancelled => Ok(false),
            HttpDownloadOutcome::Completed => {
                fs::rename(&partial_path, &model_path)?;
                info!(
                    "Mirror download of {} completed and verified ({:?})",
                    model_id, model_path
                );
                Ok(true)
            }
        }
    }

    pub async fn download_model(&self, model_id: &str) -> Result<()> {
        let model_info = {
            let models = self.available_models.lock().unwrap();
            models.get(model_id).cloned()
        };

        let model_info =
            model_info.ok_or_else(|| anyhow::anyhow!("Model not found: {}", model_id))?;

        match &model_info.source {
            ModelSource::HuggingFace { repo_id, revision } => {
                self.download_hf_model(&model_info, repo_id.clone(), revision.clone())
                    .await
            }
            ModelSource::System => self.install_system_model(&model_info.id).await,
            ModelSource::Url { .. } | ModelSource::Local => {
                Err(anyhow::anyhow!("No download source for model"))
            }
        }
    }

    async fn install_system_model(&self, model_id: &str) -> Result<()> {
        use crate::apple_speech::Status;
        let attempt = crate::apple_speech::install();
        loop {
            match crate::apple_speech::status() {
                Status::Installed => break,
                Status::Unsupported => {
                    self.refresh_system_models();
                    return Err(anyhow::anyhow!(
                        "This Mac has no Apple speech model for its language"
                    ));
                }
                Status::Failed if crate::apple_speech::attempts() >= attempt => {
                    return Err(anyhow::anyhow!(
                        crate::apple_speech::error().unwrap_or_else(|| {
                            "macOS couldn't install Apple's speech model".to_string()
                        })
                    ));
                }
                Status::Failed | Status::Unchecked | Status::Downloading => {}
            }
            let percentage = crate::apple_speech::progress() * 100.0;
            let _ = self.app_handle.emit(
                "model-download-progress",
                &DownloadProgress {
                    model_id: model_id.to_string(),
                    downloaded: percentage as u64,
                    total: 100,
                    percentage,
                },
            );
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        self.update_download_status();
        let _ = self.app_handle.emit("model-download-complete", model_id);
        Ok(())
    }

    pub fn refresh_system_models(&self) {
        self.update_download_status();
        let _ = self.app_handle.emit("models-updated", ());
    }

    pub fn delete_model(&self, model_id: &str) -> Result<()> {
        debug!("ModelManager: delete_model called for: {}", model_id);

        let model_info = {
            let models = self.available_models.lock().unwrap();
            models.get(model_id).cloned()
        };

        let model_info =
            model_info.ok_or_else(|| anyhow::anyhow!("Model not found: {}", model_id))?;

        debug!("ModelManager: Found model info: {:?}", model_info);

        if let ModelSource::System = model_info.source {
            return Err(anyhow::anyhow!("{} is part of macOS", model_info.name));
        }

        if let ModelSource::HuggingFace { repo_id, revision } = &model_info.source {
            let is_alternate_quant =
                Self::is_catalog_alternate_quant(repo_id, &model_info.filename);
            let mut deleted = false;
            if is_alternate_quant {
                deleted |= Self::delete_hf_cache_file(repo_id, revision, &model_info.filename);
            } else if let Some(file) = hf_cached_path(repo_id, revision, &model_info.filename)
                && let Some(repo_dir) = file.ancestors().nth(3)
                && repo_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("models--"))
            {
                info!("Deleting HF cache repo at: {:?}", repo_dir);
                fs::remove_dir_all(repo_dir)?;
                deleted = true;
            }
            for path in [
                self.models_dir.join(&model_info.filename),
                self.models_dir
                    .join(format!("{}.partial", model_info.filename)),
            ] {
                if path.exists() {
                    info!("Deleting model file at: {:?}", path);
                    fs::remove_file(&path)?;
                    deleted = true;
                }
            }
            if !deleted {
                debug!(
                    "ModelManager: no HF cache/model files found on disk for {}; clearing stale entry",
                    model_id
                );
            }
            if is_alternate_quant {
                self.available_models.lock().unwrap().remove(model_id);
            }
            self.update_download_status();
            let _ = self.app_handle.emit("model-deleted", model_id);
            return Ok(());
        }

        let model_path = self.models_dir.join(&model_info.filename);
        let partial_path = self
            .models_dir
            .join(format!("{}.partial", model_info.filename));
        debug!("ModelManager: Model path: {:?}", model_path);
        debug!("ModelManager: Partial path: {:?}", partial_path);

        let mut deleted_something = false;

        if model_info.is_directory {
            if model_path.exists() && model_path.is_dir() {
                info!("Deleting model directory at: {:?}", model_path);
                fs::remove_dir_all(&model_path)?;
                info!("Model directory deleted successfully");
                deleted_something = true;
            }
        } else {
            if model_path.exists() {
                info!("Deleting model file at: {:?}", model_path);
                fs::remove_file(&model_path)?;
                info!("Model file deleted successfully");
                deleted_something = true;
            }
        }

        if partial_path.exists() {
            info!("Deleting partial file at: {:?}", partial_path);
            fs::remove_file(&partial_path)?;
            info!("Partial file deleted successfully");
            deleted_something = true;
        }

        if !deleted_something {
            debug!(
                "ModelManager: no files found on disk for {}; clearing stale entry",
                model_id
            );
        }

        if model_info.is_custom {
            let mut models = self.available_models.lock().unwrap();
            models.remove(model_id);
            debug!("ModelManager: removed custom model from available models");
        } else {
            self.update_download_status();
            debug!("ModelManager: download status updated");
        }

        let _ = self.app_handle.emit("model-deleted", model_id);

        Ok(())
    }

    fn mark_model_unavailable(&self, model_id: &str) {
        let removed = {
            let mut models = self.available_models.lock().unwrap();
            let should_remove = models
                .get(model_id)
                .is_some_and(Self::disappears_when_missing);
            if should_remove {
                models.remove(model_id);
                true
            } else if let Some(model) = models.get_mut(model_id) {
                model.is_downloaded = false;
                false
            } else {
                return;
            }
        };

        if removed {
            info!("Removing vanished discovered model '{}'", model_id);
        } else {
            info!("Marking model '{}' as unavailable on disk", model_id);
        }
        let _ = self.app_handle.emit("models-updated", ());
    }

    pub fn get_model_path(&self, model_id: &str) -> Result<PathBuf> {
        let model_info = self
            .get_model_info(model_id)
            .ok_or_else(|| anyhow::anyhow!("Model not found: {}", model_id))?;

        if !model_info.is_downloaded {
            return Err(anyhow::anyhow!("Model not available: {}", model_id));
        }

        if model_info.is_downloading {
            return Err(anyhow::anyhow!(
                "Model is currently downloading: {}",
                model_id
            ));
        }

        if let ModelSource::HuggingFace { repo_id, revision } = &model_info.source {
            if let Some(path) = hf_cached_path(repo_id, revision, &model_info.filename) {
                return Ok(path);
            }
            let local_path = self.models_dir.join(&model_info.filename);
            if local_path.exists() {
                let partial_path = self
                    .models_dir
                    .join(format!("{}.partial", model_info.filename));
                if partial_path.exists() {
                    let _ = fs::remove_file(&partial_path);
                }
                return Ok(local_path);
            }
            self.mark_model_unavailable(model_id);
            return Err(anyhow::anyhow!(
                "Complete model file not found in HF cache or models dir: {}",
                model_id
            ));
        }

        let model_path = self.models_dir.join(&model_info.filename);
        let partial_path = self
            .models_dir
            .join(format!("{}.partial", model_info.filename));

        if model_info.is_directory {
            if !model_path.exists() || !model_path.is_dir() {
                self.mark_model_unavailable(model_id);
                return Err(anyhow::anyhow!(
                    "Complete model directory not found: {}",
                    model_id
                ));
            }
            if partial_path.exists() {
                return Err(anyhow::anyhow!(
                    "Model directory is incomplete: {}",
                    model_id
                ));
            }
            Ok(model_path)
        } else {
            if !model_path.exists() {
                self.mark_model_unavailable(model_id);
                return Err(anyhow::anyhow!(
                    "Complete model file not found: {}",
                    model_id
                ));
            }
            if partial_path.exists() {
                return Err(anyhow::anyhow!("Model file is incomplete: {}", model_id));
            }
            Ok(model_path)
        }
    }

    pub fn cancel_download(&self, model_id: &str) {
        debug!("ModelManager: cancel_download called for: {}", model_id);

        {
            let flags = self.cancel_flags.lock().unwrap();
            if let Some(token) = flags.get(model_id) {
                token.cancel();
                info!("Cancellation token triggered for: {}", model_id);
            } else {
                warn!("No active download found for: {}", model_id);
            }
        }

        {
            let mut models = self.available_models.lock().unwrap();
            if let Some(model) = models.get_mut(model_id) {
                model.is_downloading = false;
            }
        }

        self.update_download_status();

        let _ = self.app_handle.emit("model-download-cancelled", model_id);

        info!("Download cancellation initiated for: {}", model_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_effective_language_accepts_chinese_script_intent_for_zh_capability() {
        let languages = vec!["zh".to_string()];

        assert_eq!(effective_language("zh-Hans", &languages, false), "zh-Hans");
        assert_eq!(effective_language("zh-Hant", &languages, false), "zh-Hant");
    }

    #[test]
    fn test_effective_language_falls_back_to_canonical_chinese() {
        let languages = vec!["zh-Hant".to_string()];

        assert_eq!(effective_language("auto", &languages, false), "zh");
    }

    #[test]
    fn test_effective_language_resolves_bare_intent_to_concrete_locale() {
        let languages = vec![
            "en-US".to_string(),
            "en-GB".to_string(),
            "es-ES".to_string(),
            "zh-CN".to_string(),
            "ja-JP".to_string(),
        ];

        assert_eq!(effective_language("en", &languages, true), "en-US");
        assert_eq!(effective_language("es", &languages, true), "es-ES");
        assert_eq!(effective_language("zh", &languages, true), "zh-CN");
        assert_eq!(effective_language("ja", &languages, true), "ja-JP");
        assert_eq!(effective_language("fr", &languages, true), "auto");
    }

    #[test]
    fn test_effective_language_preserves_chinese_script_intent_for_locale_model() {
        let languages = vec!["en-US".to_string(), "zh-CN".to_string()];

        assert_eq!(effective_language("zh-Hans", &languages, true), "zh-Hans");
        assert_eq!(effective_language("zh-Hant", &languages, true), "zh-Hant");
    }

    #[test]
    fn test_effective_language_preserves_intent_across_model_code_variants() {
        assert_eq!(effective_language("no", &["nb".to_string()], true), "nb");
        assert_eq!(effective_language("nb", &["no".to_string()], true), "no");
        assert_eq!(effective_language("tl", &["fil".to_string()], true), "fil");
        assert_eq!(effective_language("fil", &["tl".to_string()], true), "tl");
    }

    #[test]
    fn test_effective_language_prefers_exact_norwegian_code() {
        let languages = vec!["no".to_string(), "nb".to_string()];

        assert_eq!(effective_language("nb", &languages, true), "nb");
        assert_eq!(effective_language("no", &languages, true), "no");
    }

    #[test]
    fn test_canonicalize_supported_languages_collapses_chinese_scripts() {
        let languages = canonicalize_supported_languages(
            vec!["en", "zh", "zh-Hans", "zh-Hant", "yue"]
                .into_iter()
                .map(String::from)
                .collect(),
        );

        assert_eq!(languages, vec!["en", "zh", "yue"]);
    }

    fn build_test_gguf_string_metadata(kvs: &[(&str, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"GGUF");
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes());
        out.extend_from_slice(&(kvs.len() as u64).to_le_bytes());
        for (key, value) in kvs {
            push_gguf_str(&mut out, key);
            out.extend_from_slice(&8u32.to_le_bytes());
            push_gguf_str(&mut out, value);
        }
        out
    }

    #[test]
    fn test_discover_custom_transcribe_models() {
        let temp_dir = TempDir::new().unwrap();
        let models_dir = temp_dir.path().to_path_buf();

        let mut custom_file = File::create(models_dir.join("my-custom-model.bin")).unwrap();
        custom_file.write_all(b"fake model data").unwrap();

        let mut another_file = File::create(models_dir.join("whisper_medical_v2.bin")).unwrap();
        another_file.write_all(b"another fake model").unwrap();

        let mut gguf_file = File::create(models_dir.join("my-gguf-model.gguf")).unwrap();
        gguf_file
            .write_all(&build_test_gguf_string_metadata(&[(
                "general.name",
                "Friendly GGUF Name",
            )]))
            .unwrap();

        File::create(models_dir.join(".hidden-model.bin")).unwrap();
        File::create(models_dir.join("readme.txt")).unwrap();
        File::create(models_dir.join("ggml-small.bin")).unwrap();
        File::create(models_dir.join("download.bin.partial")).unwrap();
        fs::create_dir(models_dir.join("some-directory.bin")).unwrap();

        let mut models = HashMap::new();
        models.insert(
            "small".to_string(),
            ModelInfo {
                id: "small".to_string(),
                name: "Whisper Small".to_string(),
                description: "Test".to_string(),
                filename: "ggml-small.bin".to_string(),
                source: ModelSource::Url {
                    url: "https://example.com".to_string(),
                    sha256: None,
                },
                size_mb: 100,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: false,
                engine_type: EngineType::TranscribeCpp,
                accuracy_score: 0.5,
                speed_score: 0.5,
                supports_translation: true,
                is_recommended: false,
                supported_languages: vec!["en".to_string()],
                supports_language_selection: true,
                is_custom: false,
                supports_streaming: false,
                supports_language_detection: true,
            },
        );

        ModelManager::discover_custom_transcribe_models(&models_dir, &mut models).unwrap();

        assert!(models.contains_key("my-custom-model"));
        assert!(models.contains_key("whisper_medical_v2"));

        let custom = models.get("my-custom-model").unwrap();
        assert_eq!(custom.name, "My Custom Model");
        assert_eq!(custom.filename, "my-custom-model.bin");
        assert!(matches!(custom.source, ModelSource::Local));
        assert!(custom.is_downloaded);
        assert!(custom.is_custom);
        assert!(ModelManager::disappears_when_missing(custom));
        assert_eq!(custom.accuracy_score, 0.0);
        assert_eq!(custom.speed_score, 0.0);
        assert!(custom.supported_languages.is_empty());

        let medical = models.get("whisper_medical_v2").unwrap();
        assert_eq!(medical.name, "Whisper Medical V2");

        assert!(models.contains_key("my-gguf-model"));
        let gguf = models.get("my-gguf-model").unwrap();
        assert_eq!(gguf.filename, "my-gguf-model.gguf");
        assert_eq!(gguf.name, "Friendly GGUF Name");
        assert!(gguf.is_custom);
        assert!(matches!(gguf.engine_type, EngineType::TranscribeCpp));

        assert!(!models.contains_key(".hidden-model"));
        assert!(!models.contains_key("readme"));
        assert!(!models.contains_key("download.bin"));
        assert!(!models.contains_key("some-directory"));
    }

    #[test]
    fn test_discover_custom_models_empty_dir() {
        let temp_dir = TempDir::new().unwrap();
        let models_dir = temp_dir.path().to_path_buf();

        let mut models = HashMap::new();
        let count_before = models.len();

        ModelManager::discover_custom_transcribe_models(&models_dir, &mut models).unwrap();

        assert_eq!(models.len(), count_before);
    }

    #[test]
    fn test_discover_custom_models_nonexistent_dir() {
        let models_dir = PathBuf::from("/nonexistent/path/that/does/not/exist");

        let mut models = HashMap::new();
        let count_before = models.len();

        let result = ModelManager::discover_custom_transcribe_models(&models_dir, &mut models);
        assert!(result.is_ok());
        assert_eq!(models.len(), count_before);
    }

    #[test]
    fn test_catalog_quant_rendering() {
        let desc = ModelDescriptor {
            id: "org/repo/model-Q8_0.gguf".to_string(),
            source: ModelSource::HuggingFace {
                repo_id: "org/repo".to_string(),
                revision: "main".to_string(),
            },
            name: "Model".to_string(),
            description: "desc".to_string(),
            engine_type: EngineType::TranscribeCpp,
            caps: CapabilityProbe::default(),
            files: vec![
                QuantFile {
                    filename: "model-Q4_K_M.gguf".to_string(),
                    quant: "Q4_K_M".to_string(),
                    size_bytes: 1,
                    sha256: None,
                },
                QuantFile {
                    filename: "model-Q8_0.gguf".to_string(),
                    quant: "Q8_0".to_string(),
                    size_bytes: 2,
                    sha256: None,
                },
            ],
            default_quant: Some("Q8_0".to_string()),
            speed_score: 0.5,
            accuracy_score: 0.5,
            recommended_rank: None,
            recommended: true,
        };
        let status = DiskStatus::default();

        let default_info = desc.to_model_info(&status);
        assert_eq!(default_info.name, "Model");
        assert!(default_info.is_recommended);

        let alt_info = desc.to_model_info_for_file(&desc.files[0], &status);
        assert_eq!(alt_info.id, "org/repo/model-Q4_K_M.gguf");
        assert_eq!(alt_info.name, "Model (Q4_K_M)");
        assert_eq!(alt_info.filename, "model-Q4_K_M.gguf");
        assert!(!alt_info.is_recommended);
        assert!(!alt_info.is_custom);

        assert!(!ModelManager::disappears_when_missing(&default_info));

        let mut selection_models = HashMap::new();
        selection_models.insert(default_info.id.clone(), default_info.clone());
        assert!(!ModelManager::selected_model_is_available(
            &selection_models,
            &default_info.id
        ));
        selection_models
            .get_mut(&default_info.id)
            .unwrap()
            .is_downloaded = true;
        assert!(ModelManager::selected_model_is_available(
            &selection_models,
            &default_info.id
        ));

        let same = desc.to_model_info_for_file(&desc.files[1], &status);
        assert_eq!(same.id, default_info.id);
        assert_eq!(same.name, default_info.name);
        assert_eq!(same.is_recommended, default_info.is_recommended);
    }

    #[test]
    fn test_discover_catalog_alternate_quant_in_models_dir() {
        let desc = crate::catalog::CATALOG
            .iter()
            .find(|d| d.files.len() > 1)
            .expect("catalog has multi-quant models");
        let default_filename = default_quant_file(&desc.files, desc.default_quant.as_deref())
            .unwrap()
            .filename
            .clone();
        let alt = desc
            .files
            .iter()
            .find(|f| f.filename != default_filename)
            .unwrap();

        let temp_dir = TempDir::new().unwrap();
        fs::write(temp_dir.path().join(&alt.filename), b"").unwrap();
        fs::write(temp_dir.path().join(&default_filename), b"").unwrap();

        let mut models = HashMap::new();
        ModelManager::seed_catalog_models(&mut models);
        let seeded = models.len();
        ModelManager::discover_custom_transcribe_models(temp_dir.path(), &mut models).unwrap();

        let ModelSource::HuggingFace { repo_id, .. } = &desc.source else {
            panic!("catalog descriptors are HF-sourced");
        };
        let alt_id = format!("{}/{}", repo_id, alt.filename);
        let info = models.get(&alt_id).expect("alternate quant discovered");
        assert_eq!(info.name, format!("{} ({})", desc.name, alt.quant));
        assert_eq!(info.description, desc.description);
        assert!(info.is_downloaded);
        assert!(!info.is_custom);
        assert!(matches!(info.source, ModelSource::HuggingFace { .. }));
        assert!(ModelManager::disappears_when_missing(info));

        assert_eq!(models.len(), seeded + 1);
        assert!(!models.contains_key(alt.filename.trim_end_matches(".gguf")));
        assert!(!models.contains_key(default_filename.trim_end_matches(".gguf")));
    }

    fn push_gguf_str(out: &mut Vec<u8>, val: &str) {
        out.extend_from_slice(&(val.len() as u64).to_le_bytes());
        out.extend_from_slice(val.as_bytes());
    }

    fn write_synthetic_gguf(path: &Path, arch: &str, languages: &[&str]) {
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(&0x4655_4747u32.to_le_bytes());
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes());
        out.extend_from_slice(&2u64.to_le_bytes());
        push_gguf_str(&mut out, "general.architecture");
        out.extend_from_slice(&8u32.to_le_bytes());
        push_gguf_str(&mut out, arch);
        push_gguf_str(&mut out, "general.languages");
        out.extend_from_slice(&9u32.to_le_bytes());
        out.extend_from_slice(&8u32.to_le_bytes());
        out.extend_from_slice(&(languages.len() as u64).to_le_bytes());
        for l in languages {
            push_gguf_str(&mut out, l);
        }
        fs::write(path, out).unwrap();
    }

    #[test]
    fn test_discover_hf_cache_models_filters_by_arch() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let repo = root.join("models--handy-computer--whisper-test");
        fs::create_dir_all(repo.join("snapshots").join("abc123")).unwrap();
        fs::create_dir_all(repo.join("refs")).unwrap();
        fs::write(repo.join("refs").join("main"), "abc123").unwrap();
        write_synthetic_gguf(
            &repo
                .join("snapshots")
                .join("abc123")
                .join("whisper-q8.gguf"),
            "whisper",
            &["en", "de"],
        );

        let repo2 = root.join("models--someone--llama-7b");
        fs::create_dir_all(repo2.join("snapshots").join("def456")).unwrap();
        fs::create_dir_all(repo2.join("refs")).unwrap();
        fs::write(repo2.join("refs").join("main"), "def456").unwrap();
        write_synthetic_gguf(
            &repo2.join("snapshots").join("def456").join("llama-q8.gguf"),
            "llama",
            &[],
        );

        let mut models = HashMap::new();
        ModelManager::discover_hf_cache_models_in(root, &mut models);

        let id = "handy-computer/whisper-test/whisper-q8.gguf";
        let m = models.get(id).expect("whisper gguf should be discovered");
        assert!(m.is_downloaded);
        assert!(
            matches!(&m.source, ModelSource::HuggingFace { repo_id, revision }
            if repo_id == "handy-computer/whisper-test" && revision == "main")
        );
        assert_eq!(
            m.supported_languages,
            vec!["en".to_string(), "de".to_string()]
        );
        assert!(
            !models.contains_key("someone/llama-7b/llama-q8.gguf"),
            "non-ASR gguf must be ignored"
        );
    }
}
