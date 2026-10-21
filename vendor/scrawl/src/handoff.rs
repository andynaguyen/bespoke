use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::model::ScrawlBundle;

/// Where captures go: `captures/` in Bespoke's Annotate folder.
pub fn default_captures_dir() -> PathBuf {
    match crate::host::data_dir() {
        Some(dir) => dir.join("captures"),
        // Tests and a host that never called `host::configure`.
        None => std::env::temp_dir().join("scrawl-captures"),
    }
}

/// Opens a file, folder or URL with its default app (Finder, the browser).
pub fn open_path(target: &Path) -> Result<()> {
    let status = Command::new("/usr/bin/open").arg(target).status()?;
    ensure!(status.success(), "open {} failed", target.display());
    Ok(())
}

pub fn resolve_bundle_dir(path_or_id: &Path) -> Result<PathBuf> {
    if path_or_id == Path::new("latest") {
        return newest_bundle(&default_captures_dir());
    }
    let mut path = path_or_id.to_owned();
    if path.file_name().is_some_and(|name| name == "scrawl.json") {
        path = path
            .parent()
            .context("Missing bundle directory")?
            .to_owned();
        if path.as_os_str().is_empty() {
            path = PathBuf::from(".");
        }
    }
    if !path.exists() && path.components().count() == 1 {
        path = default_captures_dir().join(&path);
        if path.extension().is_none() {
            path.set_extension("scrawl");
        }
    }
    ensure!(
        path.is_dir(),
        "Bundle directory not found: {}",
        path.display()
    );
    let path = path
        .canonicalize()
        .with_context(|| format!("Could not resolve {}", path.display()))?;
    ensure!(
        path.extension().is_some_and(|ext| ext == "scrawl"),
        "Expected a .scrawl directory: {}",
        path.display()
    );
    Ok(path)
}

fn newest_bundle(captures: &Path) -> Result<PathBuf> {
    let mut candidates = Vec::new();
    for entry in
        fs::read_dir(captures).with_context(|| format!("Could not read {}", captures.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "scrawl") && path.is_dir() {
            // Bundle creation time is stable even after later zoom-frame extraction.
            let created = fs::read(path.join("scrawl.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<ScrawlBundle>(&bytes).ok())
                .map(|bundle| bundle.created_at_ms)
                .unwrap_or_else(|| {
                    entry
                        .metadata()
                        .ok()
                        .and_then(|m| m.created().or_else(|_| m.modified()).ok())
                        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |time| time.as_millis() as u64)
                });
            candidates.push((created, path));
        }
    }
    candidates.sort_unstable();
    candidates
        .pop()
        .context("No .scrawl captures found")?
        .1
        .canonicalize()
        .map_err(Into::into)
}

pub fn load_bundle(bundle_dir: &Path) -> Result<(PathBuf, ScrawlBundle)> {
    let directory = resolve_bundle_dir(bundle_dir)?;
    let path = directory.join("scrawl.json");
    let bundle = serde_json::from_slice(
        &fs::read(&path).with_context(|| format!("Could not read {}", path.display()))?,
    )
    .with_context(|| format!("Invalid bundle manifest: {}", path.display()))?;
    Ok((directory, bundle))
}

/// A capture as the host's MCP server reads it for an agent, which may run on another machine.
pub struct Capture {
    dir: PathBuf,
    bundle: ScrawlBundle,
}

impl Capture {
    /// The capture `reference` names in `captures`: `latest`, an id like
    /// `scrawl-1792561826739-14c29-6`, or a path from a paste, which may be another machine's
    /// path to the same bundle or a file inside it. Only the `.scrawl` folder's name counts, so
    /// a reference can't reach outside `captures`.
    pub fn find(captures: &Path, reference: &str) -> Result<Self> {
        let reference = reference.trim();
        if reference.is_empty() || reference == "latest" {
            return Self::load(&newest_bundle(captures)?);
        }
        let names: Vec<_> = Path::new(reference)
            .components()
            .filter_map(|part| match part {
                Component::Normal(name) => Some(name),
                _ => None,
            })
            .collect();
        let name = match names
            .iter()
            .rev()
            .find(|name| Path::new(name).extension().is_some_and(|ext| ext == "scrawl"))
        {
            Some(name) => name.to_os_string(),
            None if names.len() == 1 => {
                let mut name = names[0].to_os_string();
                name.push(".scrawl");
                name
            }
            None => bail!("Not a capture: {reference}"),
        };
        let dir = captures.join(&name);
        ensure!(
            dir.join("scrawl.json").is_file(),
            "No capture named {}",
            Path::new(&name).with_extension("").display()
        );
        Self::load(&dir)
    }

    /// Up to `limit` captures, newest first. Skips bundles whose manifest doesn't load.
    pub fn recent(captures: &Path, limit: usize) -> Vec<Self> {
        let Ok(entries) = fs::read_dir(captures) else {
            return Vec::new();
        };
        let mut found: Vec<_> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "scrawl"))
            .filter_map(|path| Self::load(&path).ok())
            .collect();
        found.sort_by_key(|capture| std::cmp::Reverse(capture.bundle.created_at_ms));
        found.truncate(limit);
        found
    }

    fn load(dir: &Path) -> Result<Self> {
        let (dir, bundle) = load_bundle(dir)?;
        Ok(Self { dir, bundle })
    }

    /// The folder's name without `.scrawl`, like `scrawl-1792561826739-14c29-6`.
    pub fn id(&self) -> String {
        self.dir
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn created_at_ms(&self) -> u64 {
        self.bundle.created_at_ms
    }

    /// Like `0:16`.
    pub fn duration(&self) -> String {
        crate::model::format_clock(self.bundle.duration_ms)
    }

    /// The app and window recorded, like `Slack “Inbox”`, or `Display`.
    pub fn target(&self) -> String {
        crate::sketch::target_label(&self.bundle.target)
    }

    pub fn keyframe_count(&self) -> usize {
        self.bundle.keyframes.len()
    }

    pub fn note(&self) -> Option<&str> {
        self.bundle
            .note
            .as_deref()
            .map(str::trim)
            .filter(|note| !note.is_empty())
    }

    /// `summary.md`, with the id where the bundle path goes: the path is this Mac's, and the
    /// agent reading it may run elsewhere.
    pub fn summary(&self) -> String {
        crate::sketch::summary_markdown(&self.bundle, Path::new(&self.id()))
    }

    /// The contact sheet, uncropped when `full` and the capture was cropped.
    pub fn contact_sheet(&self, full: bool) -> Result<PathBuf> {
        match &self.bundle.autocrop {
            Some(crop) if full => self.full_size_file(&crop.full_contact_sheet),
            _ => self.file(&self.bundle.contact_sheet),
        }
    }

    /// Keyframe `#index` as a label like `#3 (0:10)` and its image, uncropped when `full` and
    /// the capture was cropped.
    pub fn keyframe(&self, index: usize, full: bool) -> Result<(String, PathBuf)> {
        let keyframes = &self.bundle.keyframes;
        let Some(keyframe) = keyframes.iter().find(|keyframe| keyframe.index == index) else {
            match (keyframes.first(), keyframes.last()) {
                (Some(first), Some(last)) => bail!(
                    "No keyframe #{index}. This capture has #{} to #{}.",
                    first.index,
                    last.index
                ),
                _ => bail!("This capture has no keyframes."),
            }
        };
        let label = format!(
            "#{index} ({})",
            crate::model::format_clock(keyframe.t)
        );
        let path = match &self.bundle.autocrop {
            Some(crop) if full => {
                let name = Path::new(&keyframe.file)
                    .file_name()
                    .context("Keyframe has no file name")?;
                self.full_size_file(&Path::new(&crop.full_frames).join(name).to_string_lossy())?
            }
            _ => self.file(&keyframe.file)?,
        };
        Ok((label, path))
    }

    /// `scrawl.json` as compact JSON, without `cursor`, `windows` and the strokes' `pts` unless
    /// `include` names them. They are most of the file and rarely help an agent.
    pub fn manifest(&self, include: &[&str]) -> Result<String> {
        let path = self.dir.join("scrawl.json");
        let mut manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(&path).with_context(|| format!("Could not read {}", path.display()))?,
        )?;
        let object = manifest
            .as_object_mut()
            .context("scrawl.json isn't an object")?;
        for key in ["cursor", "windows"] {
            if !include.contains(&key) {
                object.remove(key);
            }
        }
        if !include.contains(&"strokes")
            && let Some(strokes) = object
                .get_mut("strokes")
                .and_then(serde_json::Value::as_array_mut)
        {
            for stroke in strokes.iter_mut().filter_map(serde_json::Value::as_object_mut) {
                stroke.remove("pts");
            }
        }
        Ok(serde_json::to_string(&manifest)?)
    }

    /// A file the manifest names, which must stay inside the bundle.
    fn file(&self, relative: &str) -> Result<PathBuf> {
        let relative = Path::new(relative);
        ensure!(
            relative
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
            "Bundle file outside the bundle: {}",
            relative.display()
        );
        let path = self.dir.join(relative);
        ensure!(path.is_file(), "Missing from the capture: {}", relative.display());
        Ok(path)
    }

    /// Like `file`, for the uncropped copies retention trashes with the video.
    fn full_size_file(&self, relative: &str) -> Result<PathBuf> {
        self.file(relative).map_err(|_| {
            anyhow!(
                "The uncropped images are gone: Annotate deletes them with the video. Ask without full."
            )
        })
    }
}

pub fn copy_string_to_clipboard(content: &str) -> Result<()> {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    use objc2_foundation::{NSArray, NSString};

    let text = NSString::from_str(content);
    let pasteboard = NSPasteboard::generalPasteboard();
    unsafe {
        let types = NSArray::from_slice(&[NSPasteboardTypeString]);
        pasteboard.clearContents();
        pasteboard.declareTypes_owner(&types, None);
        ensure!(
            pasteboard.setString_forType(&text, NSPasteboardTypeString),
            "Could not copy text to clipboard"
        );
    }
    Ok(())
}

pub fn copy_png_to_clipboard(png_bytes: &[u8]) -> Result<()> {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypePNG};
    use objc2_foundation::{NSArray, NSData};

    let data = NSData::with_bytes(png_bytes);
    let pasteboard = NSPasteboard::generalPasteboard();
    unsafe {
        let types = NSArray::from_slice(&[NSPasteboardTypePNG]);
        pasteboard.clearContents();
        pasteboard.declareTypes_owner(&types, None);
        ensure!(
            pasteboard.setData_forType(Some(&data), NSPasteboardTypePNG),
            "Could not copy PNG image to clipboard"
        );
    }
    Ok(())
}

pub fn copy_bundle_to_clipboard(bundle_dir: &Path) -> Result<()> {
    let (directory, bundle) = load_bundle(bundle_dir)?;
    let summary = crate::sketch::summary_markdown(&bundle, &directory);
    copy_string_to_clipboard(&summary)
}

/// Pastes both the contact-sheet image and the transcript/keyframe text into the active app
/// as a two-step sequence (`⌘V` image, then `⌘V` text). If `click_target` is
/// provided (from dragging the Paste chip onto a window/input), clicks at that screen coordinate
/// first to focus it.
pub fn paste_bundle_at_target(bundle_dir: &Path, click_target: Option<[f32; 2]>) -> Result<()> {
    if let Some([x, y]) = click_target {
        crate::capture::post_synthetic_click(f64::from(x), f64::from(y));
        std::thread::sleep(std::time::Duration::from_millis(65));
    } else {
        crate::capture::ensure_external_app_active();
    }
    paste_bundle_image_and_text(bundle_dir)
}

pub fn paste_bundle_image_and_text(bundle_dir: &Path) -> Result<()> {
    let (directory, bundle) = load_bundle(bundle_dir)?;
    let sheet_path = directory.join(&bundle.contact_sheet);
    let image = fs::read(&sheet_path).context("Could not read contact sheet")?;
    let summary = crate::sketch::summary_markdown(&bundle, &directory);

    crate::capture::wait_for_modifiers_released(350);

    if crate::capture::is_frontmost_terminal() {
        // In terminals (Ghostty / OpenCode TUI / Claude Code), pasting the image path first
        // resolves and attaches `[Image 1]`, and the second paste inserts the text summary.
        copy_string_to_clipboard(&sheet_path.display().to_string())?;
    } else {
        // In GUI apps (OpenCode Desktop, Slack, Linear, browsers), PNG-only attaches the image.
        copy_png_to_clipboard(&image)?;
    }
    std::thread::sleep(std::time::Duration::from_millis(20));
    crate::capture::post_synthetic_cmd_v();

    std::thread::sleep(std::time::Duration::from_millis(160));
    copy_string_to_clipboard(&summary)?;
    std::thread::sleep(std::time::Duration::from_millis(20));
    crate::capture::post_synthetic_cmd_v();
    Ok(())
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// `false` turns Screen-capture auto-crop off; an object overrides its settings.
    #[serde(default)]
    pub autocrop: Option<serde_json::Value>,
    /// Visual style of the native recording pill: `"black"` (default) or `"glass"` (`NSGlassEffectView`).
    #[serde(default)]
    pub pill_style: Option<String>,
    /// The microphone Mac captures record from (`mic`); absent follows the macOS default.
    #[serde(default)]
    pub microphone: Option<crate::mic::Choice>,
}

/// Config problems are logged once per process, not on every read.
fn log_config_once(message: String) {
    static SEEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let mut seen = SEEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !seen.contains(&message) {
        crate::capture::scrawl_log(&message);
        seen.push(message);
    }
}

pub fn default_config_path() -> PathBuf {
    match crate::host::data_dir() {
        Some(dir) => dir.join("config.json"),
        None => std::env::temp_dir().join("scrawl-config.json"),
    }
}

pub fn load_config_from(path: &Path) -> Config {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .map(config_from_value)
        .unwrap_or_default()
}

/// The config in `value`. A key whose value doesn't fit (`"autoSend": "maybe"`) is dropped and
/// logged on its own, so one bad setting never resets the others to their defaults.
fn config_from_value(value: serde_json::Value) -> Config {
    let serde_json::Value::Object(map) = value else {
        return Config::default();
    };
    if let Ok(config) = serde_json::from_value(serde_json::Value::Object(map.clone())) {
        return config;
    }
    // Add keys one at a time, keeping each only if everything so far still reads.
    let mut kept = serde_json::Map::new();
    for (key, value) in map {
        kept.insert(key.clone(), value.clone());
        if let Err(error) =
            serde_json::from_value::<Config>(serde_json::Value::Object(kept.clone()))
        {
            kept.remove(&key);
            log_config_once(format!("config.json: ignoring \"{key}\": {error}"));
        }
    }
    serde_json::from_value(serde_json::Value::Object(kept)).unwrap_or_default()
}

fn config() -> Config {
    load_config_from(&default_config_path())
}

/// Configured capsule style (`pillStyle` in `config.json`, default `"black"`).
pub fn pill_style() -> crate::native::pill::PillStyle {
    config()
        .pill_style
        .as_deref()
        .and_then(crate::native::pill::PillStyle::from_str_opt)
        .unwrap_or(crate::native::pill::PillStyle::Black)
}

/// Sets one top-level key in `path` (`null` removes it), keeping the rest.
pub fn set_config_value_in(path: &Path, key: &str, value: serde_json::Value) -> Result<()> {
    let mut config = fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    match (value, config.as_object_mut()) {
        (serde_json::Value::Null, Some(map)) => {
            map.remove(key);
        }
        (value, _) => config[key] = value,
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut body = serde_json::to_vec_pretty(&config)?;
    body.push(b'\n');
    fs::write(&tmp, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&tmp, path).with_context(|| format!("Could not write {}", path.display()))
}

/// What retention trashes first: the video, audio and full-size frames.
pub const HEAVY_MEDIA: [&str; 4] = [
    "recording.mp4",
    "audio.wav",
    "frames-full",
    "contact-sheet-full.png",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "scrawl-handoff-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn bundle(&self, name: &str, created: u64) -> PathBuf {
            let path = self.0.join(format!("{name}.scrawl"));
            fs::create_dir_all(path.join("frames")).unwrap();
            let keyframes: Vec<_> = [0, 1_900, 2_500, 4_000, 5_100, 6_000].into_iter().enumerate().map(|(index, t)| {
                let file = format!("frames/frame-{index:03}.png");
                fs::write(path.join(&file), b"fixture").unwrap();
                serde_json::json!({"index": index, "t": t, "reason": "initial", "file": file, "event": ""})
            }).collect();
            let bundle = serde_json::json!({
                "version": 1, "id": name, "createdAtMs": created, "durationMs": 7_000,
                "target": {"mode": "display", "displayId": 1,
                    "frame": {"x": 0.0, "y": 0.0, "width": 100.0, "height": 100.0, "scale": 1.0}},
                "transcript": {"source": "none", "text": "", "segments": []},
                "strokes": [], "marks": [], "events": [], "keyframes": keyframes,
                "contactSheet": "contact-sheet.png"
            });
            fs::write(
                path.join("scrawl.json"),
                serde_json::to_vec(&bundle).unwrap(),
            )
            .unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn resolves_directory_manifest_and_newest_capture() {
        let fixture = Fixture::new();
        let older = fixture.bundle("old", 100);
        let newer = fixture.bundle("new", 200);
        assert_eq!(
            resolve_bundle_dir(&older).unwrap(),
            (older.canonicalize().unwrap())
        );
        assert_eq!(
            resolve_bundle_dir(&older.join("scrawl.json")).unwrap(),
            (older.canonicalize().unwrap())
        );
        assert_eq!(
            newest_bundle(&fixture.0).unwrap(),
            (newer.canonicalize().unwrap())
        );
        assert!(resolve_bundle_dir(&fixture.0).is_err());
        assert!(resolve_bundle_dir(&fixture.0.join("missing.scrawl")).is_err());
    }

    #[test]
    fn loads_manifest_and_reports_invalid_json() {
        let fixture = Fixture::new();
        let path = fixture.bundle("example", 123);
        let (resolved, bundle) = load_bundle(&path.join("scrawl.json")).unwrap();
        assert_eq!(resolved, (path.canonicalize().unwrap()));
        assert_eq!(bundle.id, "example");
        assert_eq!(bundle.created_at_ms, 123);
        fs::write(path.join("scrawl.json"), b"not json").unwrap();
        assert!(load_bundle(&path).is_err());
    }

    #[test]
    fn finds_a_capture_by_id_path_or_latest_and_never_outside_the_folder() {
        let fixture = Fixture::new();
        fixture.bundle("scrawl-1-a", 100);
        fixture.bundle("scrawl-2-b", 200);
        let id = |reference: &str| Capture::find(&fixture.0, reference).map(|c| c.id());

        assert_eq!(id("latest").unwrap(), "scrawl-2-b");
        assert_eq!(id("").unwrap(), "scrawl-2-b");
        assert_eq!(id("scrawl-1-a").unwrap(), "scrawl-1-a");
        assert_eq!(id("scrawl-1-a.scrawl").unwrap(), "scrawl-1-a");
        // Another machine's path to the same bundle, or a file inside it.
        assert_eq!(
            id("/Users/someone/Library/Application Support/x/captures/scrawl-1-a.scrawl").unwrap(),
            "scrawl-1-a"
        );
        assert_eq!(
            id("/elsewhere/scrawl-1-a.scrawl/contact-sheet.png").unwrap(),
            "scrawl-1-a"
        );

        assert!(id("scrawl-9-z").is_err());
        assert!(id("..").is_err());
        assert!(id("../scrawl-1-a/../../etc").is_err());
        // A bundle outside the folder resolves to the folder's copy, which doesn't exist.
        let outside = Fixture::new();
        let elsewhere = outside.bundle("scrawl-3-c", 300);
        assert!(id(&elsewhere.display().to_string()).is_err());
    }

    #[test]
    fn lists_recent_captures_newest_first() {
        let fixture = Fixture::new();
        fixture.bundle("scrawl-1-a", 100);
        fixture.bundle("scrawl-3-c", 300);
        fixture.bundle("scrawl-2-b", 200);
        fs::create_dir_all(fixture.0.join("broken.scrawl")).unwrap();

        let ids: Vec<_> = Capture::recent(&fixture.0, 2)
            .iter()
            .map(Capture::id)
            .collect();
        assert_eq!(ids, ["scrawl-3-c", "scrawl-2-b"]);
        assert!(Capture::recent(&fixture.0.join("missing"), 10).is_empty());
    }

    #[test]
    fn reads_keyframes_sheets_and_a_trimmed_manifest() {
        let fixture = Fixture::new();
        let dir = fixture.bundle("scrawl-1-a", 100);
        fs::write(dir.join("contact-sheet.png"), b"sheet").unwrap();
        let capture = Capture::find(&fixture.0, "scrawl-1-a").unwrap();

        assert!(capture.summary().starts_with("[scrawl 0:07 · Display · scrawl-1-a]"));
        assert!(capture.contact_sheet(false).unwrap().ends_with("contact-sheet.png"));
        // Without autocrop the frames are already full size.
        assert!(capture.contact_sheet(true).unwrap().ends_with("contact-sheet.png"));
        let (label, path) = capture.keyframe(3, true).unwrap();
        assert_eq!(label, "#3 (0:04)");
        assert!(path.ends_with("frames/frame-003.png"));
        assert!(capture.keyframe(9, false).unwrap_err().to_string().contains("#0 to #5"));

        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("scrawl.json")).unwrap()).unwrap();
        manifest["cursor"] = serde_json::json!([[0, 0.5, 0.5]]);
        manifest["strokes"] = serde_json::json!([{"t0": 0, "t1": 1, "fw": 1, "fh": 1, "pts": [1, 2]}]);
        manifest["autocrop"] = serde_json::json!({
            "label": "Slack", "fullFrames": "frames-full",
            "fullContactSheet": "contact-sheet-full.png", "crops": []
        });
        fs::write(dir.join("scrawl.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
        let capture = Capture::find(&fixture.0, "scrawl-1-a").unwrap();

        let trimmed: serde_json::Value =
            serde_json::from_str(&capture.manifest(&[]).unwrap()).unwrap();
        assert!(trimmed.get("cursor").is_none());
        assert_eq!(trimmed["strokes"], serde_json::json!([{"t0": 0, "t1": 1, "fw": 1, "fh": 1}]));
        let all: serde_json::Value =
            serde_json::from_str(&capture.manifest(&["cursor", "strokes"]).unwrap()).unwrap();
        assert_eq!(all["cursor"], manifest["cursor"]);
        assert_eq!(all["strokes"], manifest["strokes"]);

        // Retention trashed the uncropped copies.
        assert!(capture.contact_sheet(true).unwrap_err().to_string().contains("gone"));
        assert!(capture.keyframe(3, true).is_err());
        fs::create_dir_all(dir.join("frames-full")).unwrap();
        fs::write(dir.join("frames-full/frame-003.png"), b"full").unwrap();
        assert!(capture.keyframe(3, true).unwrap().1.ends_with("frames-full/frame-003.png"));
    }

    #[test]
    fn config_defaults_when_missing() {
        let fixture = Fixture::new();
        let cfg = load_config_from(&fixture.0.join("missing.json"));
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn one_bad_setting_never_resets_the_others() {
        let fixture = Fixture::new();
        let path = fixture.0.join("config.json");
        fs::write(
            &path,
            r#"{"pillStyle": 3, "autocrop": false, "unknown": {"x": 1}}"#,
        )
        .unwrap();
        let cfg = load_config_from(&path);
        assert_eq!(cfg.autocrop, Some(serde_json::Value::Bool(false)));
        assert_eq!(cfg.pill_style, None);
    }

    #[test]
    fn setting_a_value_keeps_the_other_keys() {
        let fixture = Fixture::new();
        let path = fixture.0.join("config.json");
        fs::write(&path, r#"{"pillStyle": "glass"}"#).unwrap();
        set_config_value_in(&path, "autocrop", serde_json::Value::Bool(false)).unwrap();
        let cfg = load_config_from(&path);
        assert_eq!(cfg.autocrop, Some(serde_json::Value::Bool(false)));
        assert_eq!(cfg.pill_style.as_deref(), Some("glass"));
        set_config_value_in(&path, "autocrop", serde_json::Value::Null).unwrap();
        assert_eq!(load_config_from(&path).autocrop, None);
    }
}
