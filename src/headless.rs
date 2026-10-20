//! Headless one-shot transcription for `--transcribe-file`, `--list-devices`
//! and `--list-models`. Runs without any window, tray or microphone.

use std::sync::Arc;

use crate::app_handle::AppHandle;
use crate::cli::CliArgs;
use crate::managers::model::ModelManager;
use crate::managers::transcription::TranscriptionManager;
use crate::settings::get_settings;

/// Initializes only what transcription needs, runs the request and returns
/// the process exit code.
pub fn run(app: &AppHandle, args: &CliArgs) -> i32 {
    let model_manager =
        Arc::new(ModelManager::new(app).expect("Failed to initialize model manager"));
    let transcription_manager = Arc::new(
        TranscriptionManager::new(app, model_manager.clone())
            .expect("Failed to initialize transcription manager"),
    );
    app.manage(model_manager);
    app.manage(transcription_manager);
    crate::managers::transcription::init_transcribe_backend();
    crate::managers::transcription::apply_accelerator_settings(app);

    let code = run_headless_guarded(|| run_headless_transcription(app, args));
    // Drop the loaded engine before teardown: ggml-metal's global device free
    // asserts (SIGABRT) if a model's Metal resources are still alive at C++
    // static-destructor time.
    if let Some(tm) = app.try_state::<Arc<TranscriptionManager>>() {
        let _ = tm.unload_model();
    }
    code
}

/// Convert an unexpected panic in the headless run into a normal CLI failure
/// (exit code 1) instead of an abort.
pub fn run_headless_guarded<F>(operation: F) -> i32
where
    F: FnOnce() -> i32,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(code) => code,
        Err(payload) => {
            let message = if let Some(message) = payload.downcast_ref::<&str>() {
                (*message).to_string()
            } else if let Some(message) = payload.downcast_ref::<String>() {
                message.clone()
            } else {
                "unknown panic".to_string()
            };
            eprintln!("error: headless transcription panicked: {message}");
            1
        }
    }
}

#[cfg(test)]
mod headless_guard_tests {
    use super::run_headless_guarded;

    #[test]
    fn preserves_normal_exit_codes() {
        assert_eq!(run_headless_guarded(|| 2), 2);
    }

    #[test]
    fn converts_worker_panics_to_runtime_failures() {
        assert_eq!(run_headless_guarded(|| panic!("simulated failure")), 1);
    }
}

/// Headless one-shot transcription for the `--transcribe-file` / `--list-devices`
/// path. Drives the same `TranscriptionManager::transcribe` the app uses; no
/// mic, no VAD, no download. Returns a process exit code (0 ok, 1 runtime
/// failure, 2 bad input/usage).
pub fn run_headless_transcription(app: &AppHandle, args: &CliArgs) -> i32 {
    use std::time::Instant;

    // --list-devices: print registered compute devices (with indices) and exit.
    // Useful on multi-GPU machines to discover the index for --device-index.
    if args.list_devices {
        let devices = crate::managers::transcription::describe_compute_devices();
        if devices.is_empty() {
            println!("No transcribe-cpp compute devices registered.");
        } else {
            println!("transcribe-cpp compute devices:");
            for d in &devices {
                println!("  {}", d);
            }
        }
        if args.transcribe_file.is_none() {
            return 0;
        }
    }

    // --list-models: print the model registry (catalog + on-disk + custom) with
    // their ids — the same ids `--model` accepts — then exit. `--json` emits the
    // full ModelInfo array for scripting.
    if args.list_models {
        let model_manager = app.state::<Arc<ModelManager>>();
        let models = model_manager.get_available_models();
        if args.json {
            match serde_json::to_string_pretty(&models) {
                Ok(s) => println!("{}", s),
                Err(e) => {
                    eprintln!("error: failed to serialize models: {}", e);
                    return 1;
                }
            }
        } else if models.is_empty() {
            println!("No models available.");
        } else {
            println!("Available models (✓ = installed):");
            let width = models.iter().map(|m| m.id.len()).max().unwrap_or(0);
            for m in &models {
                let mark = if m.is_downloaded { "✓" } else { " " };
                let rec = if m.is_recommended {
                    "  [recommended]"
                } else {
                    ""
                };
                println!(
                    "  {}  {:<width$}  {}{}",
                    mark,
                    m.id,
                    m.name,
                    rec,
                    width = width
                );
            }
        }
        if args.transcribe_file.is_none() {
            return 0;
        }
    }

    let Some(wav) = args.transcribe_file.clone() else {
        return 0;
    };

    // read_wav_samples reads 16-bit int samples and does no validation; the app
    // only ever saves 16 kHz mono 16-bit PCM, so reject anything else rather than
    // transcribe garbage / mis-time / mis-decode.
    match hound::WavReader::open(&wav) {
        Ok(reader) => {
            let spec = reader.spec();
            if spec.sample_rate != 16_000
                || spec.channels != 1
                || spec.bits_per_sample != 16
                || spec.sample_format != hound::SampleFormat::Int
            {
                eprintln!(
                    "error: expected 16 kHz mono 16-bit PCM WAV, got {} Hz / {} ch / {}-bit {:?}",
                    spec.sample_rate, spec.channels, spec.bits_per_sample, spec.sample_format
                );
                return 2;
            }
        }
        Err(e) => {
            eprintln!("error: cannot open {}: {}", wav.display(), e);
            return 2;
        }
    }

    let samples = match crate::audio_toolkit::read_wav_samples(&wav) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to read {}: {}", wav.display(), e);
            return 2;
        }
    };
    let audio_secs = samples.len() as f64 / 16_000.0;

    let tm = app.state::<Arc<TranscriptionManager>>();

    let model_id = args
        .model
        .clone()
        .unwrap_or_else(|| get_settings(app).selected_model);
    if model_id.is_empty() {
        eprintln!("error: no model selected (pass --model or pick one in the app)");
        return 2;
    }

    // --device-index hard-selects a compute device by its --list-devices registry
    // index (transcribe-cpp / whisper-family models only; not persisted). Omit it
    // to use the persisted accelerator setting.
    let device_index = args.device_index;
    let requested_device = match device_index {
        Some(idx) => format!("index {}", idx),
        None => "settings".to_string(),
    };

    // Cold load (timed).
    let load_start = Instant::now();
    if let Err(e) = tm.load_model_with_device(&model_id, device_index) {
        eprintln!("error: load_model('{}') failed: {}", model_id, e);
        return 1;
    }
    let load_ms = load_start.elapsed().as_millis() as u64;
    let bound_backend = tm.current_backend();

    let runs = args.repeat.unwrap_or(1).max(1);
    let mut times_ms: Vec<u64> = Vec::new();
    let mut text = String::new();
    for i in 0..runs {
        // If the model's unload-timeout is "Immediately", transcribe() unloads
        // the engine after each run; reload (untimed) so repeats keep working
        // and the inference timing below stays clean.
        if !tm.is_model_loaded() {
            if let Err(e) = tm.load_model_with_device(&model_id, device_index) {
                eprintln!("error: reload before run {} failed: {}", i + 1, e);
                return 1;
            }
        }
        let t = Instant::now();
        match tm.transcribe(samples.clone()) {
            Ok(out) => text = out,
            Err(e) => {
                eprintln!("error: transcribe failed: {}", e);
                return 1;
            }
        }
        times_ms.push(t.elapsed().as_millis() as u64);
    }
    let best_ms = times_ms.iter().copied().min().unwrap_or(0);
    let rtf = if best_ms > 0 {
        audio_secs / (best_ms as f64 / 1000.0)
    } else {
        0.0
    };

    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "model": model_id,
                "requested_device": requested_device,
                "bound_backend": bound_backend,
                "audio_secs": audio_secs,
                "load_ms": load_ms,
                "transcribe_ms": times_ms,
                "best_ms": best_ms,
                "rtf": rtf,
                "text": text,
            })
        );
    } else {
        println!(
            "model={} device={} backend={} audio={:.2}s load={}ms best={}ms rtf={:.2}x",
            model_id,
            requested_device,
            bound_backend.as_deref().unwrap_or("?"),
            audio_secs,
            load_ms,
            best_ms,
            rtf,
        );
        println!("text: {}", text);
    }
    0
}
