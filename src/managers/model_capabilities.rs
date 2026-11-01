use serde::{Deserialize, Serialize};
use std::path::Path;

use super::gguf_meta::{self, GgufError, GgufMetadata};

/// Architecture strings transcribe-cpp can load — the `.name` of each arch under
/// its `src/arch/`, which is exactly the value stored in `general.architecture`.
/// Keep this in sync with transcribe-cpp; an arch absent here still parses, it's
/// just surfaced as [`Compatibility::MaybeIncompatible`] rather than promised.
pub const KNOWN_ARCHES: &[&str] = &[
    "whisper",
    "parakeet",
    "qwen3_asr",
    "voxtral",
    "voxtral_realtime",
    "cohere",
    "cohere_asr",
    "canary",
    "canary_qwen",
    "moonshine",
    "moonshine_streaming",
    "sensevoice",
    "gigaam",
    "granite",
    "granite_speech",
    "granite_nar",
    "granite_speech_nar",
    "funasr_nano",
    "medasr",
    "moss",
    "sortformer",
];

const KEY_ARCH: &str = "general.architecture";
const KEY_NAME: &str = "general.name";
const KEY_VARIANT: &str = "stt.variant";
const KEY_LANGUAGES: &str = "general.languages";
const KEY_CAP_STREAMING: &str = "stt.capability.streaming";
const KEY_CAP_TRANSLATE: &str = "stt.capability.translate";
const KEY_CAP_LANG_DETECT: &str = "stt.capability.lang_detect";
const PROBE_KEYS: &[&str] = &[
    KEY_ARCH,
    KEY_NAME,
    KEY_VARIANT,
    KEY_LANGUAGES,
    KEY_CAP_STREAMING,
    KEY_CAP_TRANSLATE,
    KEY_CAP_LANG_DETECT,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compatibility {
    Compatible,
    MaybeIncompatible,
    Unsupported,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilityProbe {
    pub verdict: Compatibility,
    pub display_name: Option<String>,
    pub architecture: Option<String>,
    pub variant: Option<String>,
    pub languages: Option<Vec<String>>,
    pub supports_streaming: Option<bool>,
    pub supports_translation: Option<bool>,
    pub supports_language_detect: Option<bool>,
}

impl CapabilityProbe {
    pub fn unsupported() -> Self {
        CapabilityProbe {
            verdict: Compatibility::Unsupported,
            ..Default::default()
        }
    }

    /// Streaming for the parakeet family is *inferred* by transcribe-cpp's
    /// native loader from encoder hparams rather than a flat bool, so when the
    /// explicit `stt.capability.streaming` key is absent we leave it `None`
    /// (unknown) and let post-load reconciliation settle it — we never guess.
    pub fn from_metadata(meta: &GgufMetadata) -> Self {
        let architecture = meta.get_str(KEY_ARCH).map(str::to_string);
        let verdict = match architecture.as_deref() {
            Some(arch) if KNOWN_ARCHES.contains(&arch) => Compatibility::Compatible,
            _ => Compatibility::MaybeIncompatible,
        };
        CapabilityProbe {
            verdict,
            display_name: meta.get_str(KEY_NAME).map(str::to_string),
            architecture,
            variant: meta.get_str(KEY_VARIANT).map(str::to_string),
            languages: meta.get_string_array(KEY_LANGUAGES),
            supports_streaming: meta.get_bool(KEY_CAP_STREAMING),
            supports_translation: meta.get_bool(KEY_CAP_TRANSLATE),
            supports_language_detect: meta.get_bool(KEY_CAP_LANG_DETECT),
        }
    }
}

pub trait CapabilityProber: Send + Sync {
    fn probe_file(&self, path: &Path) -> CapabilityProbe;
}

pub struct GgufHeaderProber;

impl CapabilityProber for GgufHeaderProber {
    fn probe_file(&self, path: &Path) -> CapabilityProbe {
        match read_header_metadata(path) {
            Ok(meta) => CapabilityProbe::from_metadata(&meta),
            Err(_) => CapabilityProbe::unsupported(),
        }
    }
}

fn read_header_metadata(path: &Path) -> Result<GgufMetadata, GgufError> {
    // The KV metadata block precedes the tensor-info table. Shipping ASR models
    // place all of it well within the first 64 KiB, so that's the common-case
    // read. Older / community GGUFs may carry it deeper, so the loop grows the
    // prefix geometrically (jumping straight to the size the parser reports it
    // needs) up to a hard cap.
    const INITIAL_PREFIX: usize = 64 << 10;
    const MAX_PREFIX: usize = 16 << 20;

    fn read_prefix(path: &Path, size: usize) -> std::io::Result<Vec<u8>> {
        use std::io::Read;
        let mut file = std::fs::File::open(path)?;
        let mut buf = vec![0u8; size];
        let mut filled = 0;
        while filled < buf.len() {
            match file.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        buf.truncate(filled);
        Ok(buf)
    }

    let mut size = INITIAL_PREFIX;
    loop {
        let buf = read_prefix(path, size).map_err(|_| GgufError::Malformed("cannot read file"))?;
        let read_len = buf.len();
        match gguf_meta::parse_header(&buf, PROBE_KEYS) {
            Ok(meta) => return Ok(meta),
            Err(GgufError::Truncated { needed }) => {
                if read_len < size {
                    return Err(GgufError::Malformed("file shorter than its header"));
                }
                let next = needed.max(size.saturating_mul(2)).min(MAX_PREFIX);
                if next <= size {
                    return Err(GgufError::Truncated { needed });
                }
                size = next;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::gguf_meta::{GgufMetadata, GgufValue};
    use super::*;
    use std::collections::HashMap;

    fn meta_with(kvs: Vec<(&str, GgufValue)>) -> GgufMetadata {
        let mut kv = HashMap::new();
        for (k, v) in kvs {
            kv.insert(k.to_string(), v);
        }
        GgufMetadata { kv }
    }

    #[test]
    fn known_arch_is_compatible_and_reads_caps() {
        let meta = meta_with(vec![
            ("general.architecture", GgufValue::String("parakeet".into())),
            (
                "general.name",
                GgufValue::String("Parakeet Unified EN 0.6B".into()),
            ),
            ("stt.capability.streaming", GgufValue::Bool(true)),
            (
                "general.languages",
                GgufValue::Array(vec![GgufValue::String("en".into())]),
            ),
        ]);
        let probe = CapabilityProbe::from_metadata(&meta);
        assert_eq!(probe.verdict, Compatibility::Compatible);
        assert_eq!(
            probe.display_name.as_deref(),
            Some("Parakeet Unified EN 0.6B")
        );
        assert_eq!(probe.architecture.as_deref(), Some("parakeet"));
        assert_eq!(probe.supports_streaming, Some(true));
        assert_eq!(probe.languages, Some(vec!["en".to_string()]));
        assert_eq!(probe.supports_translation, None);
    }

    #[test]
    fn unknown_arch_is_maybe_incompatible() {
        let meta = meta_with(vec![(
            "general.architecture",
            GgufValue::String("llama".into()),
        )]);
        assert_eq!(
            CapabilityProbe::from_metadata(&meta).verdict,
            Compatibility::MaybeIncompatible
        );
    }
}
