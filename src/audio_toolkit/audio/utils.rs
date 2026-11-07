use anyhow::Result;
use hound::{WavReader, WavSpec, WavWriter};
use log::debug;
use std::path::Path;

pub fn read_wav_samples<P: AsRef<Path>>(file_path: P) -> Result<Vec<f32>> {
    let reader = WavReader::open(file_path.as_ref())?;
    let samples = reader
        .into_samples::<i16>()
        .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
        .collect::<Result<Vec<f32>, _>>()?;
    Ok(samples)
}

pub fn verify_wav_file<P: AsRef<Path>>(file_path: P, expected_samples: usize) -> Result<()> {
    let reader = WavReader::open(file_path.as_ref())?;
    let actual_samples = reader.len() as usize;
    if actual_samples != expected_samples {
        anyhow::bail!(
            "WAV sample count mismatch: expected {}, got {}",
            expected_samples,
            actual_samples
        );
    }
    Ok(())
}

pub fn save_wav_file<P: AsRef<Path>>(file_path: P, samples: &[f32]) -> Result<()> {
    let spec = WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = WavWriter::create(file_path.as_ref(), spec)?;

    for &sample in samples {
        writer.write_sample(pcm16(sample))?;
    }

    writer.finalize()?;
    debug!("Saved WAV file: {:?}", file_path.as_ref());
    Ok(())
}

/// A sample as 16-bit PCM: clipped to -1 to 1, rounded, and scaled by `i16::MAX` so that
/// `read_wav_samples` reads it back as the same value. Anything not finite becomes silence.
pub fn pcm16(sample: f32) -> i16 {
    let sample = if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    (sample * f32::from(i16::MAX)).round() as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_is_mono_16k_pcm_and_clips_safely() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.wav");
        save_wav_file(&path, &[-2.0, -0.5, 0.0, 0.5, 2.0, f32::NAN]).unwrap();
        let mut reader = WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(reader.spec().sample_format, hound::SampleFormat::Int);
        assert_eq!(
            reader
                .samples::<i16>()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap(),
            vec![-32_767, -16_384, 0, 16_384, 32_767, 0]
        );
    }

    #[test]
    fn samples_read_back_as_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.wav");
        let written = [-1.0, -0.25, 0.0, 0.125, 1.0];
        save_wav_file(&path, &written).unwrap();
        for (read, wrote) in read_wav_samples(&path).unwrap().iter().zip(written) {
            assert!((read - wrote).abs() <= 0.5 / 32_767.0, "{read} vs {wrote}");
        }
    }
}
