//! Apple's on-device speech model. A fresh Mac doesn't have it, and each app has to reserve it
//! for itself (`AssetInventory`, attributed to `scrawl-helper`), so Scrawl checks and installs
//! it in the background at launch (`scrawl-helper speech-model --install`) instead of during the
//! first capture. Onboarding and Settings show how that's going (`view`).

use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::annotate::model::MissingTranscript;

pub const PENDING: &str = ".transcript-pending";

const RETRY_AFTER: [Duration; 3] = [
    Duration::from_secs(60),
    Duration::from_secs(5 * 60),
    Duration::from_secs(30 * 60),
];

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    #[default]
    Unknown,
    Checking,
    Downloading,
    Ready,
    Unavailable,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    pub progress: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Install attempts started since launch.
    pub attempts: u32,
}

static STATUS: Mutex<Status> = Mutex::new(Status {
    state: State::Unknown,
    engine: None,
    locale: None,
    progress: 0.0,
    error: None,
    attempts: 0,
});
struct Worker {
    running: bool,
    retry_now: bool,
}

static WORKER: Mutex<Worker> = Mutex::new(Worker {
    running: false,
    retry_now: false,
});
static RETRY_WAKE: Condvar = Condvar::new();

pub fn current() -> Status {
    STATUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

fn update(change: impl FnOnce(&mut Status)) {
    let (before, after) = {
        let mut status = STATUS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = status.clone();
        change(&mut status);
        (before, status.clone())
    };
    if before != after {
        crate::apple_speech::status_changed(&after);
    }
}

fn lock_worker() -> std::sync::MutexGuard<'static, Worker> {
    WORKER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn ensure() -> u32 {
    let status = current();
    if matches!(status.state, State::Ready | State::Unavailable) {
        return status.attempts;
    }
    let mut worker = lock_worker();
    if worker.running {
        worker.retry_now = true;
        RETRY_WAKE.notify_all();
        return status.attempts + 1;
    }
    worker.running = true;
    drop(worker);
    let spawned = std::thread::Builder::new()
        .name("annotate-speech-model".into())
        .spawn(work);
    if spawned.is_err() {
        lock_worker().running = false;
    }
    status.attempts + 1
}

fn work() {
    let mut attempt = 0;
    loop {
        if let Attempt::Done = install_once() {
            lock_worker().running = false;
            return;
        }
        let worker = lock_worker();
        let mut worker = match RETRY_AFTER.get(attempt) {
            Some(&after) => {
                RETRY_WAKE
                    .wait_timeout_while(worker, after, |worker| !worker.retry_now)
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .0
            }
            None => worker,
        };
        if !worker.retry_now && attempt >= RETRY_AFTER.len() {
            worker.running = false;
            return;
        }
        worker.retry_now = false;
        attempt += 1;
    }
}

enum Attempt {
    Done,
    Retry,
}

fn install_once() -> Attempt {
    update(|s| {
        if s.state != State::Downloading {
            s.state = State::Checking;
        }
        s.attempts += 1;
    });
    let result = crate::annotate::helper::install_speech_model(|line| {
        update(|s| {
            s.engine = line.engine.clone().or(s.engine.take());
            s.locale = line.locale.clone().or(s.locale.take());
            s.state = match line.status.as_str() {
                "installed" => State::Ready,
                "unsupported" => State::Unavailable,
                "failed" => State::Failed,
                _ => State::Downloading,
            };
            s.progress = line.progress.unwrap_or(s.progress);
            s.error = line.error.clone();
        });
    });
    let status = current();
    if let Err(error) = result {
        update(|s| {
            s.state = State::Failed;
            s.error = Some(format!("{error:#}"));
        });
    }
    crate::annotate::capture::annotate_log(format!(
        "speech model: {:?} {} {}{}",
        current().state,
        status.engine.as_deref().unwrap_or("?"),
        status.locale.as_deref().unwrap_or("?"),
        current()
            .error
            .map(|e| format!(" ({e})"))
            .unwrap_or_default()
    ));
    match current().state {
        State::Ready => {
            if let Some(captures) = crate::annotate::handoff::default_captures_dir() {
                transcribe_pending(&captures);
            }
            Attempt::Done
        }
        State::Unavailable => Attempt::Done,
        _ => Attempt::Retry,
    }
}

pub fn missing(why: &str, locale: &str, error: Option<&str>) -> MissingTranscript {
    let model = format!("Apple's speech model ({locale})");
    match why {
        "downloading" => MissingTranscript {
            reason: format!(
                "{model} was still downloading. The audio is saved, and Bespoke adds the \
                 transcript to this bundle when the download finishes."
            ),
            retry: true,
        },
        "unsupported" => MissingTranscript {
            reason: format!("this Mac has no Apple speech model for {locale}."),
            retry: false,
        },
        _ => MissingTranscript {
            reason: format!(
                "{model} couldn't be installed{}. The audio is saved, and Bespoke adds the \
                 transcript to this bundle if a later try works.",
                error.map(|e| format!(" ({e})")).unwrap_or_default()
            ),
            retry: true,
        },
    }
}

pub fn mark_pending(bundle_dir: &Path) {
    let _ = std::fs::write(bundle_dir.join(PENDING), b"");
    if current().state == State::Ready {
        let dir = bundle_dir.to_path_buf();
        std::thread::spawn(move || transcribe_one(&dir));
    }
}

pub fn transcribe_pending(captures: &Path) {
    let Ok(entries) = std::fs::read_dir(captures) else {
        return;
    };
    let pending: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|dir| dir.join(PENDING).is_file())
        .collect();
    for dir in pending {
        transcribe_one(&dir);
    }
}

fn transcribe_one(dir: &Path) {
    match transcribe_again(dir) {
        Ok(done) => crate::annotate::capture::annotate_log(format!(
            "transcribed again {}: {}",
            dir.display(),
            if done { "done" } else { "still no model" }
        )),
        Err(error) => {
            crate::annotate::capture::annotate_log(format!(
                "transcribing {} again failed: {error:#}",
                dir.display()
            ));
            let _ = std::fs::remove_file(dir.join(PENDING));
        }
    }
}

pub fn transcribe_again(dir: &Path) -> Result<bool> {
    let (dir, mut bundle) = crate::annotate::handoff::load_bundle(dir)?;
    let wav = dir.join(bundle.audio.as_deref().unwrap_or("audio.wav"));
    anyhow::ensure!(wav.is_file(), "the bundle has no audio.wav any more");
    let transcript = crate::annotate::transcribe::run(&wav)?;
    if transcript
        .missing
        .as_ref()
        .is_some_and(|missing| missing.retry)
    {
        return Ok(false);
    }
    let times: Vec<u64> = bundle.keyframes.iter().map(|k| k.t).collect();
    let said = crate::annotate::keyframes::said_per_keyframe(&times, &[], &transcript);
    for (frame, said) in bundle.keyframes.iter_mut().zip(said) {
        frame.said = said;
    }
    bundle.transcript = transcript;
    std::fs::write(
        dir.join("scrawl.json"),
        serde_json::to_vec_pretty(&bundle).context("Could not encode scrawl.json")?,
    )?;
    std::fs::write(
        dir.join("summary.md"),
        crate::annotate::sketch::summary_markdown(&bundle, &dir),
    )?;
    let _ = std::fs::remove_file(dir.join(PENDING));
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_in_progress_will_be_retried_but_a_missing_language_will_not() {
        let downloading = missing("downloading", "en_US", None);
        assert!(downloading.retry);
        assert!(downloading.reason.contains("still downloading"));
        let unsupported = missing("unsupported", "xx_XX", None);
        assert!(!unsupported.retry);
        assert!(
            unsupported
                .reason
                .contains("no Apple speech model for xx_XX")
        );
        let failed = missing("failed", "en_US", Some("asset unavailable"));
        assert!(failed.retry && failed.reason.contains("(asset unavailable)"));
    }
}
