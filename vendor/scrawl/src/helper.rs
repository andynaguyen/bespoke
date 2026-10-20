//! `scrawl-helper`, the Swift executable shipped beside `bespoke` (see `helper/main.swift`).
//!
//! It owns the Apple frameworks that are Swift-only or absent from the objc2 bindings Scrawl
//! uses: SpeechAnalyzer transcription, AVFoundation movie decoding, and the permission card.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

use crate::model::{RgbaFrame, TranscriptData, TranscriptSegment, WordTiming};

pub fn path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("SCRAWL_HELPER") {
        return Ok(PathBuf::from(path));
    }
    let raw_exe = std::env::current_exe().context("Could not locate the app binary")?;
    let exe = std::fs::canonicalize(&raw_exe).unwrap_or(raw_exe);
    // Beside the app binary, or one level up from `target/*/deps` for tests.
    exe.ancestors()
        .skip(1)
        .take(2)
        .map(|dir| dir.join("scrawl-helper"))
        .find(|path| path.is_file())
        .context("scrawl-helper is missing; rebuild Bespoke")
}

/// Runs a helper command to completion and parses its JSON output.
fn run<T: for<'de> Deserialize<'de>>(args: &[&std::ffi::OsStr], timeout: Duration) -> Result<T> {
    run_program(&path()?, args, timeout)
}

fn run_program<T: for<'de> Deserialize<'de>>(
    program: &Path,
    args: &[&std::ffi::OsStr],
    timeout: Duration,
) -> Result<T> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Could not start scrawl-helper")?;
    let mut stdout = child.stdout.take().context("No helper stdout")?;
    let mut stderr = child.stderr.take().context("No helper stderr")?;
    let caller = std::thread::current();
    let eof = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader_eof = std::sync::Arc::clone(&eof);
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        reader_eof.store(true, std::sync::atomic::Ordering::Release);
        caller.unpark();
        out
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if eof.load(std::sync::atomic::Ordering::Acquire) {
            break child.wait()?;
        }
        let now = Instant::now();
        if now >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("scrawl-helper {:?} timed out", args.first());
        }
        std::thread::park_timeout(deadline - now);
    };
    let out = reader.join().unwrap_or_default();
    if !status.success() {
        let mut message = String::new();
        let _ = stderr.read_to_string(&mut message);
        bail!("{}", message.trim());
    }
    serde_json::from_slice(&out).context("Invalid scrawl-helper output")
}

fn os(s: &str) -> &std::ffi::OsStr {
    std::ffi::OsStr::new(s)
}

/// What macOS has granted, from a fresh helper process (a long-running one can miss a Screen
/// Recording grant).
pub fn permissions()
-> Result<std::collections::BTreeMap<crate::permissions::Permission, crate::permissions::Status>> {
    run(&[os("permissions")], Duration::from_secs(5))
}

#[derive(Deserialize)]
struct Transcript {
    #[serde(default)]
    engine: Option<String>,
    locale: String,
    text: String,
    segments: Vec<TranscriptSegment>,
    words: Vec<WordTiming>,
    /// No usable speech model: `downloading`, `unsupported` or `failed` (with `error`).
    #[serde(default)]
    missing: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// On-device SpeechAnalyzer transcription with measured word timings. Without a speech model
/// the transcript is empty and `missing` says why.
pub fn transcribe(audio: &Path) -> Result<TranscriptData> {
    let transcript: Transcript = run(
        &[os("transcribe"), audio.as_os_str()],
        Duration::from_secs(180),
    )?;
    let engine = transcript
        .engine
        .unwrap_or_else(|| "SpeechTranscriber".into());
    let missing = transcript.missing.map(|why| {
        crate::speech_model::missing(&why, &transcript.locale, transcript.error.as_deref())
    });
    Ok(TranscriptData {
        source: if missing.is_some() {
            "offline".into()
        } else {
            "speech-analyzer".into()
        },
        model: Some(format!("{engine} {}", transcript.locale)),
        text: transcript.text,
        segments: transcript.segments,
        words: transcript.words,
        estimated: false,
        missing,
    })
}

/// One line of `scrawl-helper speech-model [--install]`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct SpeechModelLine {
    pub status: String,
    #[serde(default)]
    pub engine: Option<String>,
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub progress: Option<f64>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Runs `scrawl-helper speech-model --install`, handing each line to `line` as it comes. The
/// download can take minutes; the helper exits when it's done or has failed.
pub fn install_speech_model(mut line: impl FnMut(SpeechModelLine)) -> Result<()> {
    use std::io::BufRead;
    let mut child = Command::new(path()?)
        .args(["speech-model", "--install"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Could not start scrawl-helper")?;
    let stdout = child.stdout.take().context("No helper stdout")?;
    for text in std::io::BufReader::new(stdout)
        .lines()
        .map_while(Result::ok)
    {
        if let Ok(parsed) = serde_json::from_str(&text) {
            line(parsed);
        }
    }
    let status = child.wait()?;
    if !status.success() {
        let mut message = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut message);
        }
        bail!("{}", message.trim());
    }
    Ok(())
}

#[derive(Deserialize)]
struct FrameFile {
    t: u64,
    width: u32,
    height: u32,
    file: PathBuf,
}

/// Decodes the movie's frames at exactly these times into memory; `max_edge` 0 keeps native
/// size.
pub fn frames(movie: &Path, times: &[u64], max_edge: u32) -> Result<Vec<RgbaFrame>> {
    if times.is_empty() {
        return Ok(Vec::new());
    }
    let dir = std::env::temp_dir().join(format!("scrawl-frames-{}", crate::make_bundle_id()));
    let at = times
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let edge = max_edge.to_string();
    let result: Result<Vec<FrameFile>> = run(
        &[
            os("frames"),
            movie.as_os_str(),
            os("--out-dir"),
            dir.as_os_str(),
            os("--at"),
            os(&at),
            os("--max-edge"),
            os(&edge),
        ],
        Duration::from_secs(300),
    );
    let frames = result.and_then(|files| {
        if files.len() <= 1 {
            files
                .into_iter()
                .map(|file| {
                    let rgba = std::fs::read(&file.file)?;
                    ensure!(
                        rgba.len() == file.width as usize * file.height as usize * 4,
                        "Truncated helper frame"
                    );
                    Ok(RgbaFrame::new(file.t, file.width, file.height, rgba))
                })
                .collect()
        } else {
            let chunk_size = files.len().div_ceil(6).max(1);
            std::thread::scope(|scope| -> Result<Vec<RgbaFrame>> {
                let workers: Vec<_> = files
                    .chunks(chunk_size)
                    .map(|chunk| {
                        scope.spawn(move || -> Result<Vec<RgbaFrame>> {
                            chunk
                                .iter()
                                .map(|file| {
                                    let rgba = std::fs::read(&file.file)?;
                                    ensure!(
                                        rgba.len()
                                            == file.width as usize * file.height as usize * 4,
                                        "Truncated helper frame"
                                    );
                                    Ok(RgbaFrame::new(file.t, file.width, file.height, rgba))
                                })
                                .collect()
                        })
                    })
                    .collect();
                let mut out = Vec::with_capacity(files.len());
                for worker in workers {
                    out.extend(
                        worker
                            .join()
                            .map_err(|_| anyhow::anyhow!("Frame read worker panicked"))??,
                    );
                }
                Ok(out)
            })
        }
    });
    std::thread::spawn(move || {
        let _ = std::fs::remove_dir_all(dir);
    });
    frames
}
