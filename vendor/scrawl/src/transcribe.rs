use std::path::Path;

use anyhow::Result;

use crate::audio::RecordedAudio;
use crate::model::TranscriptData;

pub fn transcribe(wav: Option<&Path>, audio: Option<&RecordedAudio>) -> TranscriptData {
    let (Some(wav), Some(_)) = (wav, audio.filter(|audio| audio.sample_count > 0)) else {
        return TranscriptData {
            source: "none".into(),
            ..Default::default()
        };
    };
    let transcript = run(wav).unwrap_or_else(|error| {
        crate::capture::scrawl_log(format!("Transcription failed: {error:#}"));
        TranscriptData {
            source: "offline".into(),
            missing: Some(crate::model::MissingTranscript {
                reason: format!("Transcription failed ({error:#})."),
                retry: false,
            }),
            ..Default::default()
        }
    });
    if let Some(missing) = &transcript.missing {
        crate::capture::scrawl_log(format!("No transcript: {}", missing.reason));
        if missing.retry {
            crate::speech_model::ensure();
        }
    }
    transcript
}

pub fn run(wav: &Path) -> Result<TranscriptData> {
    match crate::host::transcriber() {
        Some(transcribe) => transcribe(wav),
        None => crate::helper::transcribe(wav, &[]),
    }
}
