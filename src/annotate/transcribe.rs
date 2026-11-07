use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::annotate::audio::RecordedAudio;
use crate::annotate::host::{TranscriptData, TranscriptSegment, WordTiming};

use crate::app_handle::AppHandle;
use crate::audio_toolkit::{TimedText, apply_custom_words, apply_custom_words_to_timed};
use crate::managers::model::{EngineType, ModelManager};
use crate::managers::transcription::{TimedTranscript, TranscriptionManager};
use crate::settings::{AppSettings, get_settings};

const SAMPLE_RATE: usize = 16_000;
const MAX_CHUNK_SECS: usize = 28;
const CUT_WINDOW_SECS: usize = 8;
const QUIET_SPAN: usize = SAMPLE_RATE / 20;
/// Shortest last piece. A capture's trailing silence is often its quietest place, and a cut
/// there would leave a sliver of silence, where Whisper tends to invent text.
const MIN_TAIL: usize = 2 * SAMPLE_RATE;

pub fn init(app: &AppHandle) {
    let transcribing = app.clone();
    crate::annotate::host::on_transcribe(move |wav| with_selected_model(&transcribing, wav));
    let refreshing = app.clone();
    crate::apple_speech::on_change(move || {
        refreshing
            .state::<Arc<ModelManager>>()
            .refresh_system_models();
    });
}

pub fn transcribe(wav: Option<&Path>, audio: Option<&RecordedAudio>) -> TranscriptData {
    let (Some(wav), Some(_)) = (wav, audio.filter(|audio| audio.sample_count > 0)) else {
        return TranscriptData {
            source: "none".into(),
            ..Default::default()
        };
    };
    let transcript = run(wav).unwrap_or_else(|error| {
        crate::annotate::capture::annotate_log(format!("Transcription failed: {error:#}"));
        TranscriptData {
            source: "offline".into(),
            missing: Some(crate::annotate::model::MissingTranscript {
                reason: format!("Transcription failed ({error:#})."),
                retry: false,
            }),
            ..Default::default()
        }
    });
    if let Some(missing) = &transcript.missing {
        crate::annotate::capture::annotate_log(format!("No transcript: {}", missing.reason));
        if missing.retry {
            crate::annotate::speech_model::ensure();
        }
    }
    transcript
}

pub fn run(wav: &Path) -> Result<TranscriptData> {
    match crate::annotate::host::transcriber() {
        Some(transcribe) => transcribe(wav),
        None => crate::annotate::helper::transcribe(wav, &[]),
    }
}

fn with_selected_model(app: &AppHandle, wav: &Path) -> Result<TranscriptData> {
    let settings = get_settings(app);
    let local = app
        .state::<Arc<ModelManager>>()
        .get_model_info(&settings.selected_model)
        .filter(|model| {
            model.is_downloaded && !matches!(model.engine_type, EngineType::AppleSpeech)
        });
    let Some(model) = local else {
        return apple_speech(wav, &settings);
    };
    match transcribe_pieces(app, wav, &settings) {
        Ok(timed) => {
            log::info!("Annotate transcribed {} with {}", wav.display(), model.name);
            Ok(timed.into_transcript(model.name, &settings))
        }
        Err(error) => {
            log::warn!(
                "Annotate couldn't transcribe with {}, so Apple Speech will: {error:#}",
                model.name
            );
            apple_speech(wav, &settings)
        }
    }
}

fn apple_speech(wav: &Path, settings: &AppSettings) -> Result<TranscriptData> {
    let mut transcript = crate::apple_speech::transcribe_file(wav, &settings.custom_words)?;
    if transcript.missing.is_none() && !settings.custom_words.is_empty() {
        let correct = |text: &str| {
            apply_custom_words(
                text,
                &settings.custom_words,
                settings.word_correction_threshold,
            )
        };
        transcript.text = correct(&transcript.text);
        for segment in &mut transcript.segments {
            segment.text = correct(&segment.text);
        }
        let words: Vec<TimedText> = transcript
            .words
            .iter()
            .map(|w| TimedText {
                t0: w.t0,
                t1: w.t1,
                text: w.text.clone(),
            })
            .collect();
        transcript.words = apply_custom_words_to_timed(
            &words,
            &settings.custom_words,
            settings.word_correction_threshold,
        )
        .into_iter()
        .map(word_timing)
        .collect();
    }
    Ok(transcript)
}

#[derive(Default)]
struct CaptureTranscript {
    texts: Vec<String>,
    segments: Vec<TimedText>,
    words: Vec<TimedText>,
    estimated: bool,
    dictionary_prompted: bool,
}

impl CaptureTranscript {
    fn into_transcript(self, model: String, settings: &AppSettings) -> TranscriptData {
        let correct = !self.dictionary_prompted && !settings.custom_words.is_empty();
        let text = self.texts.join(" ");
        let (text, segments, words) = if correct {
            let fix = |text: &str| {
                apply_custom_words(
                    text,
                    &settings.custom_words,
                    settings.word_correction_threshold,
                )
            };
            (
                fix(&text),
                self.segments
                    .into_iter()
                    .map(|s| TimedText {
                        text: fix(&s.text),
                        ..s
                    })
                    .collect(),
                apply_custom_words_to_timed(
                    &self.words,
                    &settings.custom_words,
                    settings.word_correction_threshold,
                ),
            )
        } else {
            (text, self.segments, self.words)
        };
        TranscriptData {
            source: "bespoke".into(),
            model: Some(model),
            text,
            segments: segments
                .into_iter()
                .map(|s| TranscriptSegment {
                    t0: s.t0,
                    t1: s.t1,
                    text: s.text,
                })
                .collect(),
            words: words.into_iter().map(word_timing).collect(),
            estimated: self.estimated,
            missing: None,
        }
    }
}

fn word_timing(word: TimedText) -> WordTiming {
    WordTiming {
        t0: word.t0,
        t1: word.t1,
        text: word.text,
    }
}

fn transcribe_pieces(
    app: &AppHandle,
    wav: &Path,
    settings: &AppSettings,
) -> Result<CaptureTranscript> {
    let samples = crate::audio_toolkit::read_wav_samples(wav)?;
    let manager = app.state::<Arc<TranscriptionManager>>();
    let mut timed = CaptureTranscript::default();
    let mut transcribed = 0;
    let mut last_error = None;
    for range in chunks(&samples) {
        let piece = &samples[range.clone()];
        let part = match manager.transcribe_timed(piece) {
            Ok(part) => {
                transcribed += 1;
                part
            }
            Err(error) => {
                log::warn!(
                    "Annotate couldn't transcribe {:.1}s to {:.1}s with the picked model: {error:#}",
                    range.start as f64 / SAMPLE_RATE as f64,
                    range.end as f64 / SAMPLE_RATE as f64
                );
                last_error = Some(error);
                match apple_speech_piece(piece, settings) {
                    Some(part) => part,
                    None => continue,
                }
            }
        };
        timed.add(part, ms(range.start), ms(range.len()));
    }
    manager.maybe_unload_immediately("Annotate transcription");
    match last_error {
        Some(error) if transcribed == 0 => Err(error),
        _ => Ok(timed),
    }
}

fn apple_speech_piece(piece: &[f32], settings: &AppSettings) -> Option<TimedTranscript> {
    let transcript = crate::apple_speech::transcribe(piece, &settings.custom_words)
        .inspect_err(|error| log::warn!("Apple Speech couldn't transcribe it either: {error:#}"))
        .ok()
        .filter(|transcript| transcript.missing.is_none())?;
    let timed = |t0, t1, text: &str| TimedText {
        t0,
        t1,
        text: text.to_string(),
    };
    Some(TimedTranscript {
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
        dictionary_prompted: false,
    })
}

impl CaptureTranscript {
    fn add(&mut self, part: TimedTranscript, start_ms: u64, length_ms: u64) {
        self.dictionary_prompted |= part.dictionary_prompted;
        let text = part.text.trim();
        if text.is_empty() {
            return;
        }
        let mut words: Vec<TimedText> = part
            .words
            .into_iter()
            .filter(|w| !w.text.trim().is_empty())
            .collect();
        // transcribe-cpp reports Whisper's `max_timestamp_kind` as Segment and rejects a Word
        // request, and Moonshine, GigaAM, Canary and Cohere in transcribe-rs report no times.
        if words.is_empty() {
            words = if part.segments.is_empty() {
                spread_words(text, 0, length_ms)
            } else {
                part.segments
                    .iter()
                    .flat_map(|s| spread_words(&s.text, s.t0, s.t1))
                    .collect()
            };
            self.estimated = true;
        }
        let shift = |t: TimedText| TimedText {
            t0: t.t0 + start_ms,
            t1: t.t1 + start_ms,
            text: t.text,
        };
        self.segments.extend(part.segments.into_iter().map(shift));
        self.words.extend(words.into_iter().map(shift));
        self.texts.push(text.to_string());
    }
}

fn ms(samples: usize) -> u64 {
    (samples * 1000 / SAMPLE_RATE) as u64
}

fn spread_words(text: &str, t0: u64, t1: u64) -> Vec<TimedText> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let each = t1.saturating_sub(t0) / words.len().max(1) as u64;
    words
        .into_iter()
        .enumerate()
        .map(|(i, word)| TimedText {
            t0: t0 + i as u64 * each,
            t1: t0 + (i as u64 + 1) * each,
            text: word.to_string(),
        })
        .collect()
}

fn chunks(samples: &[f32]) -> Vec<Range<usize>> {
    let max = MAX_CHUNK_SECS * SAMPLE_RATE;
    let window = CUT_WINDOW_SECS * SAMPLE_RATE;
    let mut ranges = Vec::new();
    let mut start = 0;
    while samples.len() - start > max {
        let search = (start + max - window)..(start + max).min(samples.len() - MIN_TAIL);
        let cut = quietest(samples, search);
        ranges.push(start..cut);
        start = cut;
    }
    if start < samples.len() {
        ranges.push(start..samples.len());
    }
    ranges
}

fn quietest(samples: &[f32], search: Range<usize>) -> usize {
    let energy = |at: usize| -> f32 { samples[at..at + QUIET_SPAN].iter().map(|s| s * s).sum() };
    (search.start..search.end - QUIET_SPAN)
        .step_by(QUIET_SPAN / 2)
        .min_by(|a, b| energy(*a).total_cmp(&energy(*b)))
        .map_or(search.end, |at| at + QUIET_SPAN / 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_audio_is_one_chunk() {
        let audio = vec![0.1; 10 * SAMPLE_RATE];
        assert_eq!(chunks(&audio), vec![0..audio.len()]);
        assert!(chunks(&[]).is_empty());
    }

    #[test]
    fn long_audio_is_cut_at_its_quietest_place() {
        let mut audio = vec![0.5; 70 * SAMPLE_RATE];
        let pause = 24 * SAMPLE_RATE;
        audio[pause..pause + SAMPLE_RATE / 5].fill(0.0);
        let ranges = chunks(&audio);
        assert!(ranges.len() >= 3);
        let first_cut = ranges[0].end;
        assert!((pause..pause + SAMPLE_RATE / 5).contains(&first_cut));
        for pair in ranges.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
        assert_eq!(ranges.last().map(|r| r.end), Some(audio.len()));
        assert!(
            ranges
                .iter()
                .all(|r| r.len() <= MAX_CHUNK_SECS * SAMPLE_RATE)
        );
    }

    #[test]
    fn a_cut_never_leaves_a_sliver_at_the_end() {
        let mut audio = vec![0.5; MAX_CHUNK_SECS * SAMPLE_RATE + SAMPLE_RATE / 5];
        let silence = audio.len() - SAMPLE_RATE;
        audio[silence..].fill(0.0);
        let ranges = chunks(&audio);
        assert_eq!(ranges.len(), 2);
        assert!(ranges[1].len() >= MIN_TAIL);
        assert!(ranges[0].len() <= MAX_CHUNK_SECS * SAMPLE_RATE);
    }

    #[test]
    fn untimed_words_spread_across_their_chunk() {
        let words = spread_words("one two three four", 1000, 3000);
        assert_eq!(words.len(), 4);
        assert_eq!((words[0].t0, words[0].t1), (1000, 1500));
        assert_eq!((words[3].t0, words[3].t1), (2500, 3000));
    }
}
