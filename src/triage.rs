//! Triage an Issue. Writes this Mac's facts (`context.md`) and the triage playbook (`PROMPT.md`)
//! to a fresh `triage/<time>` folder in the app data folder, then hands the folder to a coding
//! agent through its deep link, or copies a prompt that points at it. The agent reads the logs,
//! finds the cause and helps file an issue, following `.github/triage/PLAYBOOK.md`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::app_handle::AppHandle;

/// `.github/triage/PLAYBOOK.md`, built in so an old release still ships a playbook. The playbook
/// tells the agent to prefer the copy on `main` when the two differ.
pub const PLAYBOOK: &str = include_str!("../.github/triage/PLAYBOOK.md");

const REPO: &str = "https://github.com/andynaguyen/bespoke";
const PROMPT_FILE: &str = "PROMPT.md";
const CONTEXT_FILE: &str = "context.md";

/// Where a triage session goes once its folder is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handoff {
    /// Claude Code in the Claude desktop app (`claude://code/new`).
    Claude,
    /// Claude Code in a terminal window (`claude-cli://open`).
    ClaudeCode,
    /// The Codex app (`codex://threads/new`).
    Codex,
    /// The launch prompt on the clipboard, for any agent.
    Copy,
}

impl Handoff {
    pub const ALL: [Handoff; 4] = [
        Handoff::Claude,
        Handoff::ClaudeCode,
        Handoff::Codex,
        Handoff::Copy,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Handoff::Claude => "claude",
            Handoff::ClaudeCode => "claude-code",
            Handoff::Codex => "codex",
            Handoff::Copy => "copy",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|handoff| handoff.id() == id)
    }

    fn scheme(self) -> Option<&'static str> {
        match self {
            Handoff::Claude => Some("claude"),
            Handoff::ClaudeCode => Some("claude-cli"),
            Handoff::Codex => Some("codex"),
            Handoff::Copy => None,
        }
    }

    /// Whether an app on this Mac opens this handoff's links. Claude Code registers
    /// `claude-cli://` the first time someone sends a prompt in an interactive `claude`.
    pub fn available(self) -> bool {
        self.scheme().is_none_or(has_url_handler)
    }

    /// The deep link that opens the agent in `dir` with `prompt` in its composer. Every agent
    /// waits for the user to press send.
    pub fn url(self, prompt: &str, dir: &Path) -> Option<String> {
        let prompt = encode(prompt);
        let dir = encode(&dir.to_string_lossy());
        match self {
            Handoff::Claude => Some(format!("claude://code/new?q={prompt}&folder={dir}")),
            Handoff::ClaudeCode => Some(format!("claude-cli://open?cwd={dir}&q={prompt}")),
            Handoff::Codex => Some(format!("codex://threads/new?prompt={prompt}&path={dir}")),
            Handoff::Copy => None,
        }
    }
}

#[cfg(target_os = "macos")]
fn has_url_handler(scheme: &str) -> bool {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSString, NSURL};

    let Some(url) = NSURL::URLWithString(&NSString::from_str(&format!("{scheme}://"))) else {
        return false;
    };
    NSWorkspace::sharedWorkspace()
        .URLForApplicationToOpenURL(&url)
        .is_some()
}

#[cfg(not(target_os = "macos"))]
fn has_url_handler(_scheme: &str) -> bool {
    false
}

/// What wrote the session. Only the running app can read its own permissions and settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    App,
    Cli,
}

pub struct Session {
    pub dir: PathBuf,
    pub prompt: PathBuf,
    pub context: PathBuf,
}

/// The text a handoff puts in the agent's composer. The playbook has the agent ask what went wrong
/// unless the user adds it to this message.
pub fn launch_prompt(prompt_file: &Path) -> String {
    format!(
        "Read the file \"{}\" and follow it: it is your Bespoke triage playbook.",
        prompt_file.display()
    )
}

/// Writes a session folder from the running app, then hands it off. Shows a toast with the
/// outcome.
pub fn start(handoff: Handoff, cx: &mut gpui::App) {
    let app = AppHandle::global().clone();
    cx.spawn(async move |cx| {
        let written = crate::runtime::spawn_blocking(move || write_session(&app, Origin::App))
            .await
            .map_err(|e| e.to_string())
            .and_then(|written| written);
        let _ = cx.update(
            |cx| match written.and_then(|session| hand_off(handoff, &session)) {
                Ok(()) => {
                    let key = if handoff == Handoff::Copy {
                        "settings.triage.copied"
                    } else {
                        "settings.triage.opened"
                    };
                    crate::ui::toast::info(crate::i18n::t(key), cx);
                }
                Err(e) => {
                    log::error!("Triage failed: {e}");
                    crate::ui::toast::error_with(crate::i18n::t("settings.triage.failed"), e, cx);
                }
            },
        );
    })
    .detach();
}

fn hand_off(handoff: Handoff, session: &Session) -> Result<(), String> {
    let prompt = launch_prompt(&session.prompt);
    match handoff.url(&prompt, &session.dir) {
        Some(url) => open::that_detached(&url).map_err(|e| format!("Failed to open {url}: {e}")),
        None => AppHandle::global().clipboard().write_text(prompt),
    }
}

/// `bespoke --triage`: writes a session without the running app and prints how to start it.
/// It still works when the app won't launch.
pub fn run_cli(app: &AppHandle) -> i32 {
    let session = match write_session(app, Origin::Cli) {
        Ok(session) => session,
        Err(e) => {
            eprintln!("error: could not write the triage session: {e}");
            return 1;
        }
    };
    let prompt = launch_prompt(&session.prompt);
    let mut out = format!(
        "Wrote the triage playbook and this Mac's facts to:\n  {}\n  {}\n\n\
         Paste this into a coding agent on this Mac:\n  {prompt}\n",
        session.prompt.display(),
        session.context.display(),
    );
    let links: Vec<_> = [Handoff::Claude, Handoff::ClaudeCode, Handoff::Codex]
        .into_iter()
        .filter(|handoff| handoff.available())
        .filter_map(|handoff| Some((handoff, handoff.url(&prompt, &session.dir)?)))
        .collect();
    if !links.is_empty() {
        out.push_str("\nOr open one of these links with `open \"<link>\"`:\n");
        for (handoff, url) in links {
            let _ = writeln!(out, "  {}: {url}", handoff.id());
        }
    }
    print!("{out}");
    0
}

/// Writes `context.md` and `PROMPT.md` into a fresh `triage/<time>` folder.
pub fn write_session(app: &AppHandle, origin: Origin) -> Result<Session, String> {
    let data_dir = crate::portable::app_data_dir(app).map_err(|e| e.to_string())?;
    let now = chrono::Local::now();
    let dir = data_dir
        .join("triage")
        .join(now.format("%Y-%m-%dT%H-%M-%S").to_string());
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create {}: {e}", dir.display()))?;

    let facts = Facts::collect(app, origin, &data_dir, &dir, now.to_rfc3339());
    let context = dir.join(CONTEXT_FILE);
    std::fs::write(&context, build_context(&facts))
        .map_err(|e| format!("Failed to write {}: {e}", context.display()))?;
    let prompt = dir.join(PROMPT_FILE);
    std::fs::write(&prompt, build_prompt(&context, origin))
        .map_err(|e| format!("Failed to write {}: {e}", prompt.display()))?;
    log::info!("Wrote a triage session to {}", dir.display());
    Ok(Session {
        dir,
        prompt,
        context,
    })
}

fn build_prompt(context: &Path, origin: Origin) -> String {
    let from = match origin {
        Origin::App => "from Bespoke's Triage an Issue",
        Origin::Cli => "with `bespoke --triage`",
    };
    format!(
        "A Bespoke user is having a problem and started this session {from}.\n\n\
         Machine facts (version, build, permissions, settings, paths) are in the triage \
         context file:\n\n    {}\n\n\
         Follow the playbook below. The message that sent you here may already describe what \
         went wrong. If it does not, start by asking.\n\n---\n\n{PLAYBOOK}",
        context.display()
    )
}

/// Everything `context.md` reports. `app` is `None` when the command line wrote the session.
struct Facts {
    generated_at: String,
    origin: Origin,
    pid: u32,
    version: &'static str,
    build: &'static str,
    executable: Option<PathBuf>,
    os: String,
    chip: Option<String>,
    app: Option<AppFacts>,
    paths: Vec<(&'static str, PathBuf)>,
}

/// Facts only the running app can read about itself.
struct AppFacts {
    permissions: Vec<String>,
    settings: Vec<String>,
}

impl Facts {
    fn collect(
        app: &AppHandle,
        origin: Origin,
        data_dir: &Path,
        session_dir: &Path,
        generated_at: String,
    ) -> Self {
        let executable = std::env::current_exe().ok();
        Self {
            generated_at,
            origin,
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION"),
            build: build_kind(executable.as_deref()),
            executable,
            os: os_version(),
            chip: chip(),
            app: (origin == Origin::App).then(|| AppFacts {
                permissions: permissions(),
                settings: settings(app),
            }),
            paths: paths(app, data_dir, session_dir),
        }
    }
}

fn build_kind(executable: Option<&Path>) -> &'static str {
    if cfg!(debug_assertions) {
        return "debug build (`cargo run` from a checkout)";
    }
    let sparkle = executable
        .and_then(Path::parent)
        .map(|macos| macos.join("../Frameworks/Sparkle.framework"))
        .is_some_and(|framework| framework.exists());
    if sparkle {
        "Sparkle release (from a GitHub release, updates itself)"
    } else {
        "local build (`scripts/install-local.sh`, no self-update)"
    }
}

#[cfg(target_os = "macos")]
fn os_version() -> String {
    let version = objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersionString();
    format!("macOS {version}")
}

#[cfg(not(target_os = "macos"))]
fn os_version() -> String {
    std::env::consts::OS.to_string()
}

fn chip() -> Option<String> {
    let output = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .ok()?;
    let chip = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!chip.is_empty()).then_some(chip)
}

/// How long one permission check may take. Some checks ask macOS's privacy service, which
/// can stall, and a stuck check must not hold up the rest of the context.
const CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Runs `check` on its own thread and gives up after `CHECK_TIMEOUT`. A check that never
/// returns leaves its thread behind, which is cheaper than a triage that never starts.
fn bounded<T: Send + 'static>(check: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("triage-check".into())
        .spawn(move || {
            let _ = tx.send(check());
        })
        .ok()?;
    rx.recv_timeout(CHECK_TIMEOUT).ok()
}

const NO_ANSWER: &str = "no answer within 3 seconds";

#[cfg(target_os = "macos")]
fn permissions() -> Vec<String> {
    use crate::ui::platform;
    use scrawl::host::{Permission, PermissionStatus};

    let yes_no = |on: Option<bool>| match on {
        Some(true) => "granted",
        Some(false) => "not granted",
        None => NO_ANSWER,
    };
    let mut lines = vec![
        format!(
            "Accessibility (this process): {}",
            yes_no(bounded(platform::accessibility_granted))
        ),
        format!(
            "Microphone (this process): {}",
            match bounded(platform::microphone_authorization) {
                Some(0) => "not determined",
                Some(1) => "restricted",
                Some(2) => "denied",
                Some(3) => "granted",
                Some(_) => "unknown",
                None => NO_ANSWER,
            }
        ),
        format!(
            "Screen Recording (this process): {}",
            yes_no(bounded(scrawl::host::screen_capture_granted))
        ),
    ];
    match bounded(scrawl::host::read_grants) {
        Some(Some(grants)) => {
            let status = |permission| match grants.status(permission) {
                PermissionStatus::Granted => "granted",
                PermissionStatus::Denied => "denied",
                PermissionStatus::NotDetermined => "not determined",
            };
            lines.push(format!(
                "Scrawl helper (a fresh process): Screen Recording {}, Microphone {}, Accessibility {}",
                status(Permission::ScreenRecording),
                status(Permission::Microphone),
                status(Permission::Accessibility),
            ));
            lines.push(format!(
                "Screen Recording needs a relaunch to apply: {}",
                if grants.screen_needs_relaunch {
                    "yes"
                } else {
                    "no"
                }
            ));
        }
        Some(None) => lines
            .push("Scrawl helper (a fresh process): could not run, see annotate.log".to_string()),
        None => lines.push(format!("Scrawl helper (a fresh process): {NO_ANSWER}")),
    }
    lines.push(format!(
        "Secure Input on now: {}",
        match bounded(crate::secure_input::is_enabled_now) {
            Some(true) => "yes (global shortcuts may not fire)",
            Some(false) => "no",
            None => NO_ANSWER,
        }
    ));
    lines
}

#[cfg(not(target_os = "macos"))]
fn permissions() -> Vec<String> {
    Vec::new()
}

/// The settings behind most reports. Never the post-processing API keys.
fn settings(app: &AppHandle) -> Vec<String> {
    use std::sync::Arc;

    let s = crate::settings::get_settings(app);
    let downloaded = app
        .try_state::<Arc<crate::managers::model::ModelManager>>()
        .and_then(|models| {
            models
                .get_available_models()
                .into_iter()
                .find(|model| model.id == s.selected_model)
                .map(|model| model.is_downloaded)
        });
    let model = match downloaded {
        _ if s.selected_model.is_empty() => "none selected".to_string(),
        Some(true) => format!("{} (downloaded)", s.selected_model),
        Some(false) => format!("{} (not downloaded)", s.selected_model),
        None => format!("{} (not in the catalog)", s.selected_model),
    };
    let mut bindings: Vec<_> = s
        .bindings
        .values()
        .map(|b| format!("{} = `{}`", b.id, b.current_binding))
        .collect();
    bindings.sort();
    vec![
        format!("Selected model: {model}"),
        format!("Language: {}", s.selected_language),
        format!("Shortcuts: {}", bindings.join(", ")),
        format!("Shortcut activation: {:?}", s.shortcut_activation),
        format!("Keyboard implementation: {:?}", s.keyboard_implementation),
        format!(
            "Microphone: {}{}",
            s.selected_microphone.as_deref().unwrap_or("system default"),
            if s.always_on_microphone {
                ", always on"
            } else {
                ""
            }
        ),
        format!("Paste method: {:?}", s.paste_method),
        format!("Typing tool: {:?}", s.typing_tool),
        format!("Overlay: {:?}", s.overlay_style),
        format!(
            "Post-processing: {}",
            if s.post_process_enabled {
                format!("on, provider {}", s.post_process_provider_id)
            } else {
                "off".to_string()
            }
        ),
        format!("Agent Access (MCP): {:?}", s.annotate_mcp_reach),
        format!(
            "Debug mode: {}, log level {:?}",
            if s.debug_mode { "on" } else { "off" },
            s.log_level
        ),
        format!(
            "Onboarding completed: {}",
            if s.onboarding_completed { "yes" } else { "no" }
        ),
    ]
}

fn paths(app: &AppHandle, data_dir: &Path, session_dir: &Path) -> Vec<(&'static str, PathBuf)> {
    let mut paths = vec![
        ("App data", data_dir.to_path_buf()),
        (
            "Settings (JSON; holds API keys under `post_process_api_keys`, never quote them)",
            data_dir.join(crate::settings::SETTINGS_STORE_PATH),
        ),
        (
            "History database (SQLite, open in Bespoke; read with `sqlite3 -readonly`, write only with the user's explicit permission)",
            data_dir.join("history.db"),
        ),
        ("Recordings", data_dir.join("recordings")),
        ("Models", data_dir.join("models")),
    ];
    if cfg!(target_os = "macos") {
        let annotate = data_dir.join("annotate");
        paths.extend([
            (
                "Annotate captures (`.scrawl` folders)",
                annotate.join("captures"),
            ),
            ("Annotate config", annotate.join("config.json")),
            (
                "Agent Access token (NEVER read this)",
                annotate.join("mcp-token"),
            ),
        ]);
    }
    if let Ok(logs) = crate::portable::app_log_dir(app) {
        paths.extend([
            (
                "Main log (starts over past 500 KB)",
                logs.join("bespoke.log"),
            ),
            ("Annotate log", logs.join("annotate.log")),
            ("Logs", logs),
        ]);
    }
    if let Some(home) = dirs::home_dir() {
        paths.push(("Crash reports", home.join("Library/Logs/DiagnosticReports")));
    }
    paths.extend([
        ("This triage session", session_dir.to_path_buf()),
        (
            "Source cache (clone the repo here)",
            data_dir.join("triage").join("source"),
        ),
    ]);
    paths
}

fn build_context(facts: &Facts) -> String {
    let mut out = String::from("# Bespoke triage context\n\n");
    let (writer, started) = match facts.origin {
        Origin::App => (
            "the Bespoke app",
            format!("the running Bespoke app (pid {})", facts.pid),
        ),
        Origin::Cli => (
            "`bespoke --triage`",
            format!(
                "the command line (`bespoke --triage`, pid {}). This process is not the running app, so it did not record permissions or settings",
                facts.pid
            ),
        ),
    };
    let _ = writeln!(out, "Generated by {writer} at {}.\n", facts.generated_at);
    let _ = writeln!(out, "- Started from: {started}");
    let _ = writeln!(
        out,
        "- Bespoke version: {} (release tag v{})",
        facts.version, facts.version
    );
    let _ = writeln!(out, "- Build: {}", facts.build);
    if let Some(executable) = &facts.executable {
        let _ = writeln!(out, "- Executable: {}", executable.display());
    }
    let _ = writeln!(out, "- OS: {} ({})", facts.os, std::env::consts::ARCH);
    if let Some(chip) = &facts.chip {
        let _ = writeln!(out, "- Chip: {chip}");
    }
    let _ = writeln!(out, "- Repo: {REPO}");

    if let Some(app) = &facts.app {
        if !app.permissions.is_empty() {
            out.push_str(
                "\n## Permissions\n\n\
                 As the running app sees them. macOS gives a running app its Screen Recording \
                 answer at its first check, so a grant made later reaches the app only after it \
                 relaunches. The Scrawl helper starts fresh each time, so it sees the current \
                 grant.\n\n",
            );
            for line in &app.permissions {
                let _ = writeln!(out, "- {line}");
            }
        }
        out.push_str("\n## Settings\n\n");
        for line in &app.settings {
            let _ = writeln!(out, "- {line}");
        }
    }

    out.push_str("\n## Paths\n\n");
    for (label, path) in &facts.paths {
        let _ = writeln!(out, "- {label}: {}", path.display());
    }
    out
}

/// Percent-encodes `text` for a URL query value, like JavaScript's `encodeURIComponent`.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(byte as char),
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(origin: Origin) -> Facts {
        Facts {
            generated_at: "2026-11-01T09:00:00-08:00".into(),
            origin,
            pid: 42,
            version: "1.0.2",
            build: "Sparkle release",
            executable: None,
            os: "macOS Version 26.1".into(),
            chip: None,
            app: (origin == Origin::App).then(|| AppFacts {
                permissions: vec!["Screen Recording (this process): not granted".into()],
                settings: vec!["Selected model: parakeet (downloaded)".into()],
            }),
            paths: vec![(
                "Agent Access token (NEVER read this)",
                PathBuf::from("/data/annotate/mcp-token"),
            )],
        }
    }

    #[test]
    fn encodes_like_encode_uri_component() {
        assert_eq!(encode("a b/c?d=e&f"), "a%20b%2Fc%3Fd%3De%26f");
        assert_eq!(encode("\"x\": é"), "%22x%22%3A%20%C3%A9");
        assert_eq!(encode("A-z_0.9!~*'()"), "A-z_0.9!~*'()");
    }

    #[test]
    fn each_agent_link_carries_the_prompt_and_the_session_folder() {
        let dir = Path::new("/data/triage/now");
        let prompt = "Read it";
        assert_eq!(
            Handoff::Claude.url(prompt, dir).as_deref(),
            Some("claude://code/new?q=Read%20it&folder=%2Fdata%2Ftriage%2Fnow")
        );
        assert_eq!(
            Handoff::ClaudeCode.url(prompt, dir).as_deref(),
            Some("claude-cli://open?cwd=%2Fdata%2Ftriage%2Fnow&q=Read%20it")
        );
        assert_eq!(
            Handoff::Codex.url(prompt, dir).as_deref(),
            Some("codex://threads/new?prompt=Read%20it&path=%2Fdata%2Ftriage%2Fnow")
        );
        assert_eq!(Handoff::Copy.url(prompt, dir), None);
    }

    #[test]
    fn handoff_ids_round_trip() {
        for handoff in Handoff::ALL {
            assert_eq!(Handoff::from_id(handoff.id()), Some(handoff));
        }
    }

    #[test]
    fn the_launch_prompt_fits_claude_codes_link_limit() {
        let prompt = launch_prompt(Path::new(
            "/Users/someone/Library/Application Support/com.andynguyen.bespoke/triage/2026-11-01T09-00-00/PROMPT.md",
        ));
        assert!(prompt.len() < 5_000);
    }

    #[test]
    fn the_app_context_records_permissions_and_settings() {
        let context = build_context(&facts(Origin::App));
        assert!(context.contains("the running Bespoke app (pid 42)"));
        assert!(context.contains("## Permissions"));
        assert!(context.contains("- Screen Recording (this process): not granted"));
        assert!(context.contains("## Settings"));
        assert!(context.contains("(release tag v1.0.2)"));
        assert!(context.contains("Agent Access token (NEVER read this): /data/annotate/mcp-token"));
    }

    #[test]
    fn the_command_line_context_says_it_has_no_permissions() {
        let context = build_context(&facts(Origin::Cli));
        assert!(context.contains("did not record permissions or settings"));
        assert!(!context.contains("## Permissions"));
        assert!(!context.contains("## Settings"));
    }

    #[test]
    fn the_prompt_points_at_the_context_and_carries_the_playbook() {
        let prompt = build_prompt(Path::new("/data/triage/now/context.md"), Origin::App);
        assert!(prompt.contains("    /data/triage/now/context.md"));
        assert!(prompt.ends_with(PLAYBOOK));
        assert!(PLAYBOOK.starts_with("# Bespoke triage playbook"));
    }
}
