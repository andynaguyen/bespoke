//! Keeps the captures folder from growing forever.
//!
//! A bundle's prompt (`summary.md`, `scrawl.json`, `contact-sheet.png`, `frames/`) is small; its
//! video, audio and full-size frames are nearly all of its size. After `mediaDays` the heavy
//! media goes to the Trash and the prompt stays; after `bundleDays` the whole bundle goes. Both
//! live under `"retention"` in Annotate's `config.json` (`null` keeps forever). A bundle younger
//! than a day or changed within the last day, one Scrawl is working on, and a pinned one
//! (`.pinned`) are never touched.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::capture::scrawl_log;

/// Marks a bundle to keep forever.
pub const PINNED: &str = ".pinned";
/// Written when the heavy media went to the Trash: when, and after how many days.
pub const MEDIA_CLEANED: &str = ".media-cleaned";

const DAY_MS: u64 = 24 * 60 * 60 * 1000;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    /// Days to keep video, audio and full-size frames; `None` keeps them.
    #[serde(default = "default_media_days")]
    pub media_days: Option<u32>,
    /// Days to keep the bundle itself; `None` keeps it.
    #[serde(default = "default_bundle_days")]
    pub bundle_days: Option<u32>,
}

fn default_media_days() -> Option<u32> {
    Some(7)
}

fn default_bundle_days() -> Option<u32> {
    Some(30)
}

impl Default for Policy {
    fn default() -> Self {
        Preset::Week.policy()
    }
}

/// The choices in the Annotate page's Keep Recordings popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// Video for 7 days, the rest for 30.
    Week,
    /// Video for 30 days, the rest for 90.
    Month,
    Forever,
}

impl Preset {
    pub const ALL: [Self; 3] = [Self::Week, Self::Month, Self::Forever];

    pub fn policy(self) -> Policy {
        let (media_days, bundle_days) = match self {
            Self::Week => (Some(7), Some(30)),
            Self::Month => (Some(30), Some(90)),
            Self::Forever => (None, None),
        };
        Policy {
            media_days,
            bundle_days,
        }
    }
}

impl Policy {
    pub fn preset(&self) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.policy() == *self)
    }

    pub fn load() -> Self {
        Self::load_from(&crate::handoff::default_config_path())
    }

    pub fn load_from(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|config| config.get("retention").cloned())
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    /// Writes `"retention"`, keeping the rest of the config (and its `0600` mode) as it is.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        crate::handoff::set_config_value_in(path, "retention", serde_json::to_value(self)?)
    }
}

// MARK: What Scrawl is busy with

static BUSY: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
static RECORDING: AtomicUsize = AtomicUsize::new(0);

/// Holds a bundle out of cleanup while Scrawl works on it.
pub struct Claim(PathBuf);

/// Claims `dir` unless something else already has.
pub fn claim(dir: &Path) -> Option<Claim> {
    let key = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let inserted = BUSY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key.clone());
    // Built after the lock is released: a refused `Claim` would unlock on drop.
    inserted.then(|| Claim(key))
}

fn is_claimed(dir: &Path) -> bool {
    let key = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    BUSY.lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&key)
}

impl Drop for Claim {
    fn drop(&mut self) {
        BUSY.lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// Held by a running capture; cleanup and re-encoding wait until none is.
pub struct Recording(());

impl Recording {
    pub fn start() -> Self {
        RECORDING.fetch_add(1, Ordering::AcqRel);
        Self(())
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        RECORDING.fetch_sub(1, Ordering::AcqRel);
    }
}

fn recording() -> bool {
    RECORDING.load(Ordering::Acquire) > 0
}

// MARK: Planning

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    #[serde(default)]
    created_at_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Move these heavy files out of the bundle and into the Trash.
    TrashMedia {
        bundle: PathBuf,
        items: Vec<PathBuf>,
    },
    /// Move the whole bundle to the Trash.
    TrashBundle(PathBuf),
}

fn modified_ago(path: &Path, now: SystemTime) -> Option<Duration> {
    let modified = path.metadata().ok()?.modified().ok()?;
    Some(now.duration_since(modified).unwrap_or_default())
}

/// When anything last changed in the folder itself: an entry added, removed or renamed (a copy
/// arriving, a zoom, a re-encode). Uses the change time, which `tar` can't backdate.
#[cfg(unix)]
fn changed_ago(path: &Path, now: SystemTime) -> Option<Duration> {
    use std::os::unix::fs::MetadataExt;
    let meta = path.metadata().ok()?;
    let changed = UNIX_EPOCH + Duration::new(meta.ctime().max(0) as u64, meta.ctime_nsec() as u32);
    Some(now.duration_since(changed).unwrap_or_default())
}

fn created_ms(dir: &Path, manifest: &Manifest) -> Option<u64> {
    manifest.created_at_ms.or_else(|| {
        // `scrawl-<ms>-…`
        let name = dir.file_stem()?.to_str()?;
        name.strip_prefix("scrawl-")?
            .split('-')
            .next()?
            .parse()
            .ok()
    })
}

/// What one cleanup pass would do to the bundles in `captures`, given `now`.
pub fn plan(captures: &Path, policy: &Policy, now: SystemTime) -> Vec<Action> {
    let now_ms = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let Ok(entries) = std::fs::read_dir(captures) else {
        return Vec::new();
    };
    let mut bundles: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "scrawl") && p.is_dir())
        .collect();
    bundles.sort();
    let mut actions = Vec::new();
    for dir in bundles {
        if dir.join(PINNED).exists() || is_claimed(&dir) {
            continue;
        }
        let manifest: Manifest = std::fs::read(dir.join("scrawl.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let Some(created) = created_ms(&dir, &manifest).or_else(|| {
            let ago = modified_ago(&dir, now)?;
            Some(now_ms.saturating_sub(ago.as_millis() as u64))
        }) else {
            continue;
        };
        let age_ms = now_ms.saturating_sub(created);
        let unchanged_ms = changed_ago(&dir, now).map_or(0, |ago| ago.as_millis() as u64);
        let settled = |ms: u64| age_ms >= ms && unchanged_ms >= ms;
        if !settled(DAY_MS) {
            continue;
        }
        let due = |days: Option<u32>| days.is_some_and(|d| age_ms >= u64::from(d.max(1)) * DAY_MS);
        if due(policy.bundle_days) {
            actions.push(Action::TrashBundle(dir));
            continue;
        }
        if due(policy.media_days) {
            let items: Vec<PathBuf> = crate::handoff::HEAVY_MEDIA
                .iter()
                .map(|rel| dir.join(rel))
                .filter(|p| p.exists())
                .collect();
            if !items.is_empty() {
                actions.push(Action::TrashMedia { bundle: dir, items });
            }
        }
    }
    actions
}

// MARK: Doing it

/// Moves `path` to the Trash.
pub type Trasher<'a> = &'a dyn Fn(&Path) -> Result<()>;

pub fn trash(path: &Path) -> Result<()> {
    let output = Command::new("/usr/bin/trash")
        .arg(path)
        .output()
        .context("Could not run /usr/bin/trash")?;
    ensure!(
        output.status.success() && !path.exists(),
        "Could not move {} to the Trash: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

fn size_of(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_dir() {
        std::fs::read_dir(path)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|e| size_of(&e.path()))
                    .sum()
            })
            .unwrap_or(0)
    } else {
        meta.len()
    }
}

/// Everything in the captures folder, in bytes.
pub fn captures_size(captures: &Path) -> u64 {
    size_of(captures)
}

pub fn format_bytes(bytes: u64) -> String {
    const MB: f64 = 1_000_000.0;
    let mb = bytes as f64 / MB;
    if mb >= 1000.0 {
        format!("{:.1} GB", mb / 1000.0)
    } else {
        format!("{mb:.0} MB")
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub media_trashed: usize,
    pub bundles_trashed: usize,
    pub freed: u64,
    pub errors: usize,
}

/// Puts a bundle's heavy media in the Trash as one folder named after the bundle, so it reads
/// (and restores) as a unit, then notes the cleanup in the bundle.
fn trash_media(bundle: &Path, items: &[PathBuf], days: u32, trasher: Trasher) -> Result<u64> {
    let name = bundle
        .file_stem()
        .context("Bundle has no name")?
        .to_string_lossy();
    let staging_root = bundle
        .parent()
        .context("Bundle has no parent")?
        .join(".trash-staging");
    let staging = staging_root.join(format!("{name} media"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let mut freed = 0;
    let mut moved = Vec::new();
    for item in items {
        let Some(file) = item.file_name() else {
            continue;
        };
        freed += size_of(item);
        std::fs::rename(item, staging.join(file))?;
        moved.push(file.to_string_lossy().into_owned());
    }
    let note = serde_json::json!({
        "cleanedAtMs": crate::now_epoch_ms(),
        "afterDays": days,
        "trashed": moved,
    });
    std::fs::write(
        bundle.join(MEDIA_CLEANED),
        serde_json::to_vec_pretty(&note)?,
    )?;
    trasher(&staging)?;
    let _ = std::fs::remove_dir(&staging_root);
    Ok(freed)
}

/// Runs a plan. Each step is checked again just before it runs, in case a capture or a copy
/// started meanwhile.
pub fn apply(actions: &[Action], policy: &Policy, trasher: Trasher) -> Report {
    let mut report = Report::default();
    for action in actions {
        let bundle = match action {
            Action::TrashMedia { bundle, .. } | Action::TrashBundle(bundle) => bundle,
        };
        let Some(_claim) = claim(bundle) else {
            continue;
        };
        if bundle.join(PINNED).exists() {
            continue;
        }
        let result = match action {
            Action::TrashMedia { bundle, items } => {
                trash_media(bundle, items, policy.media_days.unwrap_or(0), trasher).map(|freed| {
                    report.media_trashed += 1;
                    report.freed += freed;
                })
            }
            Action::TrashBundle(bundle) => {
                let size = size_of(bundle);
                trasher(bundle).map(|()| {
                    report.bundles_trashed += 1;
                    report.freed += size;
                })
            }
        };
        if let Err(error) = result {
            report.errors += 1;
            scrawl_log(format!("retention: {}: {error:#}", bundle.display()));
        }
    }
    report
}

/// Removes what a crash mid-cleanup left beside the bundles.
fn sweep_leftovers(captures: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(captures) else {
        return;
    };
    for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let leftover = name == ".trash-staging";
        if leftover
            && modified_ago(&path, now).is_some_and(|ago| ago > Duration::from_secs(6 * 3600))
        {
            let _ = std::fs::remove_dir_all(&path).or_else(|_| std::fs::remove_file(&path));
        }
    }
}

/// One cleanup pass over `captures`, planned as if `ahead` from now (for trying it out).
pub fn run_pass(
    captures: &Path,
    policy: &Policy,
    dry_run: bool,
    ahead: Duration,
) -> (Vec<Action>, Report) {
    let now = SystemTime::now();
    let actions = plan(captures, policy, now + ahead);
    if dry_run {
        return (actions, Report::default());
    }
    sweep_leftovers(captures, now);
    let report = apply(&actions, policy, &trash);
    if report != Report::default() {
        scrawl_log(format!(
            "retention: media of {} bundle(s) and {} whole bundle(s) to the Trash, {} freed, {} error(s)",
            report.media_trashed,
            report.bundles_trashed,
            format_bytes(report.freed),
            report.errors
        ));
    }
    (actions, report)
}

// MARK: Background worker

static WORKER: std::sync::OnceLock<flume::Sender<()>> = std::sync::OnceLock::new();

fn lower_priority() {
    // SAFETY: only changes the calling thread's scheduling class.
    #[cfg(target_os = "macos")]
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
}

/// Starts the app's cleanup: a pass shortly after launch, then daily.
pub fn start() {
    if WORKER.get().is_some() {
        return;
    }
    let (sender, receiver) = flume::unbounded();
    if WORKER.set(sender).is_err() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("scrawl-retention".into())
        .spawn(move || worker(&receiver));
}

/// Runs a pass soon, after the policy changed.
pub fn nudge() {
    if let Some(worker) = WORKER.get() {
        let _ = worker.send(());
    }
}

fn worker(jobs: &flume::Receiver<()>) {
    lower_priority();
    let captures = crate::handoff::default_captures_dir();
    let mut next_pass = Instant::now() + Duration::from_secs(90);
    let mut last_pass: Option<SystemTime> = None;
    loop {
        let wait = next_pass
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(3600));
        match jobs.recv_timeout(wait) {
            Ok(()) => next_pass = Instant::now() + Duration::from_secs(5),
            Err(flume::RecvTimeoutError::Timeout) => {}
            Err(flume::RecvTimeoutError::Disconnected) => return,
        }
        // The Mac sleeps through `Instant`s, so the daily pass also goes by the wall clock.
        let day_passed = last_pass.is_none_or(|at| {
            SystemTime::now().duration_since(at).unwrap_or_default()
                >= Duration::from_secs(24 * 3600)
        });
        if Instant::now() >= next_pass || (last_pass.is_some() && day_passed) {
            if recording() {
                next_pass = Instant::now() + Duration::from_secs(60);
                continue;
            }
            let policy = Policy::load();
            let (actions, report) = run_pass(&captures, &policy, false, Duration::ZERO);
            scrawl_log(format!(
                "retention: pass over {} ({} action(s), policy {:?}/{:?} days), {} now",
                captures.display(),
                actions.len(),
                policy.media_days,
                policy.bundle_days,
                format_bytes(captures_size(&captures)),
            ));
            let _ = report;
            last_pass = Some(SystemTime::now());
            next_pass = Instant::now() + Duration::from_secs(24 * 3600);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "scrawl-retention-{tag}-{}-{}",
                std::process::id(),
                crate::make_bundle_id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Plans run a little over a day ahead of the real clock, so folders made by the test
    /// count as unchanged for a day (their change time can't be backdated).
    fn later() -> SystemTime {
        SystemTime::now() + Duration::from_secs(25 * 3600)
    }

    fn ms(time: SystemTime) -> u64 {
        time.duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
    }

    /// A bundle created `days` before `later()`.
    fn bundle(captures: &Path, name: &str, days: f64) -> PathBuf {
        let dir = captures.join(format!("{name}.scrawl"));
        std::fs::create_dir_all(dir.join("frames")).unwrap();
        let created = ms(later()) - (days * DAY_MS as f64) as u64;
        let manifest = serde_json::json!({
            "version": 1,
            "id": name,
            "createdAtMs": created,
            "durationMs": 10_000,
            "video": "recording.mp4",
        });
        std::fs::write(dir.join("scrawl.json"), manifest.to_string()).unwrap();
        std::fs::write(dir.join("summary.md"), "summary").unwrap();
        std::fs::write(dir.join("contact-sheet.png"), "png").unwrap();
        std::fs::write(dir.join("frames/frame-01.png"), "png").unwrap();
        std::fs::write(dir.join("recording.mp4"), vec![0u8; 25_000_000]).unwrap();
        std::fs::write(dir.join("audio.wav"), vec![0u8; 100]).unwrap();
        dir
    }

    fn planned(captures: &Path, policy: &Policy) -> Vec<Action> {
        plan(captures, policy, later())
    }

    #[test]
    fn policy_reads_presets_nulls_and_keeps_other_keys() {
        let tmp = TempDir::new("config");
        let path = tmp.0.join("config.json");
        assert_eq!(Policy::load_from(&path), Preset::Week.policy());
        std::fs::write(
            &path,
            r#"{"ingestToken":"abc","agentHost":"studio","retention":{"mediaDays":null}}"#,
        )
        .unwrap();
        let loaded = Policy::load_from(&path);
        assert_eq!(
            loaded,
            Policy {
                media_days: None,
                bundle_days: Some(30)
            }
        );
        assert_eq!(loaded.preset(), None);
        Preset::Forever.policy().save_to(&path).unwrap();
        assert_eq!(Policy::load_from(&path).preset(), Some(Preset::Forever));
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(config["ingestToken"], "abc");
        assert_eq!(config["agentHost"], "studio");
        assert_eq!(config["retention"]["mediaDays"], serde_json::Value::Null);
        Preset::Month.policy().save_to(&path).unwrap();
        assert_eq!(
            Policy::load_from(&path),
            Policy {
                media_days: Some(30),
                bundle_days: Some(90)
            }
        );
    }

    #[test]
    fn old_media_and_older_bundles_are_planned_for_the_trash() {
        let tmp = TempDir::new("plan");
        bundle(&tmp.0, "scrawl-fresh", 0.5);
        bundle(&tmp.0, "scrawl-week", 3.0);
        let old = bundle(&tmp.0, "scrawl-old", 8.0);
        std::fs::create_dir_all(old.join("frames-full")).unwrap();
        let ancient = bundle(&tmp.0, "scrawl-ancient", 31.0);
        assert_eq!(
            planned(&tmp.0, &Policy::default()),
            vec![
                Action::TrashBundle(ancient),
                Action::TrashMedia {
                    bundle: old.clone(),
                    items: vec![
                        old.join("recording.mp4"),
                        old.join("audio.wav"),
                        old.join("frames-full")
                    ],
                },
            ]
        );
        assert!(planned(&tmp.0, &Preset::Forever.policy()).is_empty());
        // However short the policy, a day-old bundle is kept.
        let short = Policy {
            media_days: Some(0),
            bundle_days: None,
        };
        assert_eq!(planned(&tmp.0, &short).len(), 3);
    }

    #[test]
    fn pinned_and_busy_bundles_are_left_alone() {
        let tmp = TempDir::new("guards");
        let pinned = bundle(&tmp.0, "scrawl-pinned", 40.0);
        std::fs::write(pinned.join(PINNED), "").unwrap();
        let busy = bundle(&tmp.0, "scrawl-busy", 40.0);
        let held = claim(&busy).unwrap();
        assert!(claim(&busy).is_none(), "one claim at a time");
        assert!(planned(&tmp.0, &Policy::default()).is_empty());
        drop(held);
        assert_eq!(
            planned(&tmp.0, &Policy::default()),
            vec![Action::TrashBundle(busy)]
        );
    }

    #[test]
    fn recently_changed_folders_wait_a_day() {
        let tmp = TempDir::new("changed");
        // Just copied from another Mac: made long ago, but the folder changed now.
        let arrived = bundle(&tmp.0, "scrawl-arrived", 40.0);
        let today = later() - Duration::from_secs(24 * 3600);
        assert!(plan(&tmp.0, &Policy::default(), today).is_empty());
        assert_eq!(
            planned(&tmp.0, &Policy::default()),
            vec![Action::TrashBundle(arrived)]
        );
    }

    #[test]
    fn applying_moves_media_as_one_folder_and_keeps_the_prompt() {
        let tmp = TempDir::new("apply");
        let captures = tmp.0.join("captures");
        let trash_dir = tmp.0.join("Trash");
        std::fs::create_dir_all(&trash_dir).unwrap();
        let old = bundle(&captures, "scrawl-old", 8.0);
        let ancient = bundle(&captures, "scrawl-ancient", 31.0);
        let fake_trash = |path: &Path| -> Result<()> {
            std::fs::rename(path, trash_dir.join(path.file_name().unwrap()))?;
            Ok(())
        };
        let actions = planned(&captures, &Policy::default());
        assert_eq!(actions.len(), 2);
        let report = apply(&actions, &Policy::default(), &fake_trash);
        assert_eq!(
            (report.media_trashed, report.bundles_trashed, report.errors),
            (1, 1, 0)
        );
        assert!(report.freed > 25_000_000);
        assert!(!ancient.exists());
        assert!(
            trash_dir
                .join("scrawl-ancient.scrawl/scrawl.json")
                .is_file()
        );
        let media = trash_dir.join("scrawl-old media");
        assert!(media.join("recording.mp4").is_file() && media.join("audio.wav").is_file());
        for kept in [
            "scrawl.json",
            "summary.md",
            "contact-sheet.png",
            "frames/frame-01.png",
        ] {
            assert!(old.join(kept).is_file(), "{kept} kept");
        }
        assert!(!old.join("recording.mp4").exists());
        assert!(!captures.join(".trash-staging").exists());
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(old.join("scrawl.json")).unwrap()).unwrap();
        assert_eq!(
            manifest["video"], "recording.mp4",
            "the manifest is left as it was"
        );
        // Nothing left to clean, and the bundle changed today besides.
        let next_week = later() + Duration::from_secs(2 * 24 * 3600);
        assert!(plan(&captures, &Policy::default(), next_week).is_empty());
    }

    #[test]
    fn sizes_read_plainly() {
        assert_eq!(format_bytes(72_247_194), "72 MB");
        assert_eq!(format_bytes(2_512_000_000), "2.5 GB");
    }
}
