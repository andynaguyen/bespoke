use std::io::Cursor;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail, ensure};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const SAMPLE_RATE: u32 = 16_000;

#[derive(Clone, Debug)]
pub struct RecordedAudio {
    pub sample_count: usize,
    pub wav_bytes: Vec<u8>,
}

pub struct AudioRecorder {
    worker: Option<std::thread::JoinHandle<Result<()>>>,
    stop_sender: Option<std::sync::mpsc::Sender<()>>,
    samples: Arc<Mutex<Vec<f32>>>,
    error: Arc<Mutex<Option<String>>>,
    first_sample: Arc<OnceLock<Instant>>,
}

/// What the input callback writes into: the 16 kHz samples, the stream's last error, and when
/// the first sample was heard.
#[derive(Clone)]
struct Sink {
    samples: Arc<Mutex<Vec<f32>>>,
    error: Arc<Mutex<Option<String>>>,
    first_sample: Arc<OnceLock<Instant>>,
}

impl AudioRecorder {
    pub fn start() -> Result<Self> {
        let samples = Arc::new(Mutex::new(Vec::new()));
        let error = Arc::new(Mutex::new(None));
        let first_sample = Arc::new(OnceLock::new());
        let (stop_sender, stop_receiver) = std::sync::mpsc::channel();
        let sink = Sink {
            samples: Arc::clone(&samples),
            error: Arc::clone(&error),
            first_sample: Arc::clone(&first_sample),
        };
        let worker = std::thread::Builder::new()
            .name("annotate-audio".into())
            .spawn(move || {
                let stream = Self::open_stream(sink).inspect_err(|error| {
                    crate::annotate::capture::annotate_log(format!(
                        "Microphone startup failed: {error:#}"
                    ));
                })?;
                let _ = stop_receiver.recv();
                drop(stream);
                Ok(())
            })
            .context("Could not spawn microphone worker")?;
        Ok(Self {
            worker: Some(worker),
            stop_sender: Some(stop_sender),
            samples,
            error,
            first_sample,
        })
    }

    fn open_stream(sink: Sink) -> Result<cpal::Stream> {
        let wanted = crate::app_handle::AppHandle::try_global().and_then(|app| {
            crate::managers::audio::preferred_input_device(&crate::settings::get_settings(app))
                .map(|(name, _)| name)
        });
        Self::open_named(wanted.as_deref(), sink)
    }

    /// Opens the input named on Bespoke's Sound page, or the system default for `None` or a
    /// device that isn't connected or won't open.
    fn open_named(name: Option<&str>, sink: Sink) -> Result<cpal::Stream> {
        use cpal::traits::HostTrait;
        let Some(name) = name else {
            return Self::open_default(sink);
        };
        let device = cpal::default_host()
            .input_devices()
            .ok()
            .and_then(|mut devices| devices.find(|d| device_name(d).as_deref() == Some(name)));
        match device {
            Some(device) => match Self::open_device(&device, sink.clone()) {
                Ok(stream) => {
                    crate::annotate::capture::annotate_log(format!("Microphone: {name}"));
                    return Ok(stream);
                }
                Err(e) => crate::annotate::capture::annotate_log(format!(
                    "Microphone: could not open {name} ({e:#}); using the default"
                )),
            },
            None => crate::annotate::capture::annotate_log(format!(
                "Microphone: {name} isn't connected; using the default"
            )),
        }
        Self::open_default(sink)
    }

    fn open_default(sink: Sink) -> Result<cpal::Stream> {
        let device = cpal::default_host()
            .default_input_device()
            .context("No microphone is available")?;
        let stream = Self::open_device(&device, sink)?;
        crate::annotate::capture::annotate_log(format!(
            "Microphone: {} (default)",
            device_name(&device).unwrap_or_default()
        ));
        Ok(stream)
    }

    fn open_device(device: &cpal::Device, sink: Sink) -> Result<cpal::Stream> {
        let supported = device.default_input_config()?;
        let config = supported.config();
        crate::annotate::capture::annotate_log(format!(
            "Microphone: {} Hz, {} ch, {:?}",
            config.sample_rate.0,
            config.channels,
            supported.sample_format()
        ));
        ensure!(
            config.channels > 0 && config.sample_rate.0 > 0,
            "Invalid microphone format"
        );
        macro_rules! stream {
            ($ty:ty) => {
                input_stream::<$ty>(&device, config, sink.clone())?
            };
        }
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => stream!(f32),
            cpal::SampleFormat::F64 => stream!(f64),
            cpal::SampleFormat::I8 => stream!(i8),
            cpal::SampleFormat::I16 => stream!(i16),
            cpal::SampleFormat::I24 => stream!(cpal::I24),
            cpal::SampleFormat::I32 => stream!(i32),
            cpal::SampleFormat::I64 => stream!(i64),
            cpal::SampleFormat::U8 => stream!(u8),
            cpal::SampleFormat::U16 => stream!(u16),
            cpal::SampleFormat::U32 => stream!(u32),
            cpal::SampleFormat::U64 => stream!(u64),
            format => bail!("Unsupported microphone format: {format}"),
        };
        stream.play().context("Could not start microphone")?;
        Ok(stream)
    }

    fn join_worker(&mut self) -> Result<()> {
        self.stop_sender.take();
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| anyhow!("Microphone worker panicked"))??;
        }
        Ok(())
    }

    /// Stops the microphone and returns what it heard, lined up so that sample 0 is at
    /// `started_at`, the capture's clock.
    pub fn stop(mut self, started_at: Instant) -> Result<RecordedAudio> {
        self.join_worker()?;
        let error = self
            .error
            .lock()
            .map_err(|_| anyhow!("Microphone error lock poisoned"))?
            .take();
        let samples_16k = std::mem::take(
            &mut *self
                .samples
                .lock()
                .map_err(|_| anyhow!("Microphone buffer lock poisoned"))?,
        );
        if let Some(error) = error {
            if samples_16k.is_empty() {
                bail!("Microphone capture failed: {error}");
            }
            crate::annotate::capture::annotate_log(format!(
                "Microphone: {error}; keeping what it heard"
            ));
        }
        let first_sample = self.first_sample.get().copied();
        if let Some(first) = first_sample {
            let lag_ms = match first.checked_duration_since(started_at) {
                Some(late) => late.as_millis() as i128,
                None => -(started_at.duration_since(first).as_millis() as i128),
            };
            crate::annotate::capture::annotate_log(format!(
                "Microphone: first sample {lag_ms} ms after the capture started"
            ));
        }
        let samples_16k = align_to_clock(samples_16k, first_sample, started_at);
        let sample_count = samples_16k.len();
        let wav_bytes = encode_wav_16k(&samples_16k)?;
        Ok(RecordedAudio {
            sample_count,
            wav_bytes,
        })
    }
}

impl Drop for AudioRecorder {
    fn drop(&mut self) {
        let _ = self.join_worker();
    }
}

fn device_name(device: &cpal::Device) -> Option<String> {
    device.name().ok()
}

/// Pads silence before `samples` when the first one was heard after `started_at`, or drops
/// the ones heard before it, so that sample 0 falls at `started_at`.
fn align_to_clock(
    mut samples: Vec<f32>,
    first_sample: Option<Instant>,
    started_at: Instant,
) -> Vec<f32> {
    let Some(first) = first_sample else {
        return samples;
    };
    let in_samples = |gap: Duration| (gap.as_secs_f64() * f64::from(SAMPLE_RATE)).round() as usize;
    match first.checked_duration_since(started_at) {
        Some(late) => {
            samples.splice(0..0, std::iter::repeat_n(0.0, in_samples(late)));
        }
        None => {
            let early = in_samples(started_at.duration_since(first));
            samples.drain(..early.min(samples.len()));
        }
    }
    samples
}

fn input_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    sink: Sink,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = usize::from(config.channels);
    let rate = config.sample_rate.0;
    let mut phase = 0_u32;
    let mut sum = 0.0_f64;
    let Sink {
        samples,
        error,
        first_sample,
    } = sink;
    Ok(device.build_input_stream::<T, _, _>(
        &config,
        move |data, _| {
            if first_sample.get().is_none() {
                let heard_for =
                    Duration::from_secs_f64((data.len() / channels) as f64 / f64::from(rate));
                let now = Instant::now();
                let _ = first_sample.set(now.checked_sub(heard_for).unwrap_or(now));
            }
            let Ok(mut output) = samples.lock() else {
                return;
            };
            for frame in data.chunks_exact(channels) {
                let mono = frame
                    .iter()
                    .map(|sample| sample.to_sample::<f32>() as f64)
                    .sum::<f64>()
                    / channels as f64;
                let mono = if mono.is_finite() { mono } else { 0.0 };
                let mut remaining = SAMPLE_RATE;
                while remaining > 0 {
                    let weight = remaining.min(rate - phase);
                    sum += mono * weight as f64;
                    phase += weight;
                    remaining -= weight;
                    if phase == rate {
                        output.push((sum / rate as f64) as f32);
                        phase = 0;
                        sum = 0.0;
                    }
                }
            }
        },
        move |err| {
            if let Ok(mut error) = error.lock() {
                *error = Some(err.to_string());
            }
        },
        None,
    )?)
}

pub fn pcm16(sample: f32) -> i16 {
    let sample = if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    (sample * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16
}

pub fn encode_wav_16k(samples_16k: &[f32]) -> Result<Vec<u8>> {
    let mut bytes = Cursor::new(Vec::with_capacity(44 + samples_16k.len() * 2));
    {
        let mut writer = hound::WavWriter::new(
            &mut bytes,
            hound::WavSpec {
                channels: 1,
                sample_rate: SAMPLE_RATE,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )?;
        let mut i16_writer = writer.get_i16_writer(samples_16k.len() as u32);
        for &sample in samples_16k {
            i16_writer.write_sample(pcm16(sample));
        }
        i16_writer.flush()?;
        writer.finalize()?;
    }
    Ok(bytes.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn stop_joins_worker_before_reading_its_last_samples() {
        let samples = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::clone(&samples);
        let (stop_sender, stop_receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            assert!(matches!(
                stop_receiver.recv_timeout(Duration::from_secs(2)),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
            ));
            output.lock().unwrap().extend([0.1, 0.2, 0.3]);
            Ok(())
        });
        let audio = AudioRecorder {
            worker: Some(worker),
            stop_sender: Some(stop_sender),
            samples,
            error: Arc::new(Mutex::new(None)),
            first_sample: Arc::default(),
        }
        .stop(Instant::now())
        .unwrap();
        assert_eq!(audio.sample_count, 3);
        assert_eq!(
            hound::WavReader::new(Cursor::new(audio.wav_bytes))
                .unwrap()
                .duration(),
            3
        );
    }

    #[test]
    fn a_device_lost_partway_keeps_what_it_heard() {
        let recorder = |samples: Vec<f32>| AudioRecorder {
            worker: None,
            stop_sender: None,
            samples: Arc::new(Mutex::new(samples)),
            error: Arc::new(Mutex::new(Some("Device disconnected".into()))),
            first_sample: Arc::default(),
        };
        assert_eq!(
            recorder(vec![0.1; 160])
                .stop(Instant::now())
                .unwrap()
                .sample_count,
            160
        );
        assert!(recorder(Vec::new()).stop(Instant::now()).is_err());
    }

    #[test]
    fn stop_propagates_background_initialization_failure() {
        let recorder = AudioRecorder {
            worker: Some(std::thread::spawn(|| bail!("device unavailable"))),
            stop_sender: None,
            samples: Arc::new(Mutex::new(Vec::new())),
            error: Arc::new(Mutex::new(None)),
            first_sample: Arc::default(),
        };
        assert!(
            recorder
                .stop(Instant::now())
                .unwrap_err()
                .to_string()
                .contains("device unavailable")
        );
    }

    #[test]
    fn a_microphone_that_started_late_gets_silence_in_front() {
        let started_at = Instant::now();
        let first = started_at + Duration::from_millis(250);
        let aligned = align_to_clock(vec![0.5; 100], Some(first), started_at);
        assert_eq!(aligned.len(), 4_000 + 100);
        assert!(aligned[..4_000].iter().all(|&s| s == 0.0));
        assert_eq!(aligned[4_000], 0.5);
    }

    #[test]
    fn a_microphone_that_started_early_loses_what_came_before_the_capture() {
        let first = Instant::now();
        let started_at = first + Duration::from_millis(100);
        let mut heard = vec![0.1; 1_600];
        heard.extend([0.9; 50]);
        let aligned = align_to_clock(heard, Some(first), started_at);
        assert_eq!(aligned, vec![0.9; 50]);

        let short = align_to_clock(vec![0.1; 10], Some(first), started_at);
        assert!(short.is_empty());
    }

    #[test]
    fn audio_with_no_first_sample_is_left_alone() {
        let samples = vec![0.2; 30];
        assert_eq!(
            align_to_clock(samples.clone(), None, Instant::now()),
            samples
        );
    }

    #[test]
    fn stop_lines_the_audio_up_with_the_capture_clock() {
        let started_at = Instant::now();
        let first_sample = Arc::new(OnceLock::new());
        first_sample
            .set(started_at + Duration::from_millis(500))
            .unwrap();
        let audio = AudioRecorder {
            worker: None,
            stop_sender: None,
            samples: Arc::new(Mutex::new(vec![0.3; 160])),
            error: Arc::new(Mutex::new(None)),
            first_sample,
        }
        .stop(started_at)
        .unwrap();
        assert_eq!(audio.sample_count, 8_000 + 160);
    }

    #[test]
    fn wav_is_mono_16k_pcm_and_clips_safely() {
        let bytes = encode_wav_16k(&[-2.0, -0.5, 0.0, 0.5, 2.0, f32::NAN]).unwrap();
        let mut reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(reader.spec().sample_format, hound::SampleFormat::Int);
        assert_eq!(
            reader
                .samples::<i16>()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap(),
            vec![-32_768, -16_384, 0, 16_384, 32_767, 0]
        );
    }
}
