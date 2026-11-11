# AGENTS.md

This file provides guidance to AI coding assistants working with code in this repository.

Bespoke is a speech-to-text dictation app for macOS, written in Rust with [GPUI](https://www.gpui.rs/). Read [CONTRIBUTING.md](CONTRIBUTING.md) for dev snapshots and GPUI layout notes before changing UI code.

## Development commands

```bash
cargo run                    # debug build
cargo run -- --start-hidden  # pass CLI flags after --
cargo test
cargo clippy
cargo fmt
scripts/install-local.sh     # release build, signed Bespoke.app in /Applications
scripts/install-release.sh   # replace it with the latest GitHub release (or pass a tag)
npm run build-release        # the zip you share, with Sparkle, in dist/<version>/ (scripts/release.sh)
```

If cmake fails on an old policy version, prefix the command with `CMAKE_POLICY_VERSION_MINIMUM=3.5`.

The Silero VAD model (`resources/models/silero_vad_v4.onnx`) is checked in, so a fresh clone needs no model download to build. Transcription models download at runtime from the Models page.

## Architecture

### Backend (`src/`)

- `main.rs`, `startup.rs` - CLI parsing, logging, manager setup
- `app_handle.rs` - App state, events, data paths, and the JSON settings store (`settings_store.json`)
- `managers/` - Core logic:
  - `audio.rs` - Recording and device management
  - `model.rs`, `model/` - Model catalog, downloads, and the Hugging Face cache
  - `transcription.rs` - Speech-to-text pipeline
  - `history.rs` - Transcription history (SQLite)
- `audio_toolkit/` - Low-level audio:
  - `audio/` - Device enumeration, recording, resampling
  - `vad/` - Voice activity detection (Silero, Earshot)
- `commands/` - Functions the UI calls into the backend (audio, history, models, transcription)
- `actions.rs`, `transcription_coordinator.rs` - Start, stop, and cancel a dictation
- `annotate.rs`, `annotate/` - Annotate (macOS only): screen, voice and ink capture, started from Scrawl (`docs/annotate-upstream.md` maps each file to upstream, for porting later upstream commits). `annotate.rs` starts it and forwards its two shortcut bindings. `annotate/coordinator.rs` runs a recording: the picker, the capsule's state and finishing a capture. `annotate/capture.rs` and `capture/macos.rs` record the screen (ScreenCaptureKit), the key tap and mouse monitor; `audio.rs` the microphone, with dictation's `audio_toolkit::AudioRecorder`; `sketch.rs` and `laser_overlay.rs` the ink and laser. `bundle.rs` builds a capture from `keyframes.rs`, `autocrop.rs` and `render.rs` (the contact sheet), and `model.rs` is its format. `handoff.rs` copies and pastes captures and reads `config.json`, and `retention.rs` deletes old ones. The rest of Bespoke calls these modules directly, and they call back into `annotate.rs` when a capture finishes or is cancelled and to open Settings. `annotate/capsule.rs` draws the capsule with `ui::capsule` through `capsule_link.rs`, and `ui/pages/captures.rs` lists captures. `helper.rs` runs the Swift helper in `swift/annotate-helper/` (Apple Speech, frames from the recording, the permission panel), which `build.rs` builds
- `annotate/transcribe.rs` - Transcribes each capture with the model picked on the Models page, in runs of at most 28 seconds, then corrects the words with the dictionary: its words, then its replacements, as dictation does. Apple Speech runs when no model is picked or the picked one fails. Whisper reports segment times only, so its words get times spread evenly over their segment
- `apple_speech.rs` - Apple Speech as a model (macOS 26). It runs `SpeechTranscriber` through Scrawl's helper, takes the dictionary as contextual strings, and has no live text. macOS downloads the speech model, and Annotate's start asks it to
- `annotate/mcp.rs` - Agent Access: an MCP server on `127.0.0.1:47790` that serves captures to coding agents. Claude Code and Codex on a remote machine reach it through an SSH `RemoteForward`, or through this Mac's Tailscale address when Agent Access is set to Tailnet. The tools are in `annotate/mcp/tools.rs`, and the guide sent to agents is `annotate/mcp/instructions.md`. Its Off, This Mac and Tailnet control and the setup to copy are on the MCP page, `ui/pages/mcp.rs`
- `shortcut/` - Global shortcuts. `keys.rs` and `keys/` are the event-tap listener and hotkey matcher, from the `handy-keys` crate (`docs/handy-keys-upstream.md`). `handy_keys.rs` runs Bespoke's shortcuts on them, and `hotkey_impl.rs` on `global-hotkey`
- `settings.rs` - Settings schema, defaults, and migrations
- `system_output.rs` - Turns the system volume down while Dictate or Annotate records, to the level on the Sound page, and back up when the last recording ends
- `clipboard.rs`, `input.rs`, `paste_tx/` - Pasting text into the front app
- `overlay.rs` - Recording overlay entry points
- `tray.rs`, `tray_i18n.rs` - Menu bar icon and menu
- `single_instance.rs` - A second launch forwards its arguments to the running instance over a local socket, then exits
- `signal_handle.rs` - `send_transcription_input()`, shared by Unix signal handlers and CLI flags
- `updater.rs` - Self-updates through Sparkle (macOS). Only builds from `scripts/release.sh` embed `Sparkle.framework`, so `cargo run` and `install-local.sh` builds hide the update UI. A found update shows as a card in the sidebar, a row in Settings > General and an item in the tray menu. `release.sh --publish` posts each release, with its `appcast.xml`, to this repo's GitHub releases, which `SUFeedURL` in `Info.plist` reads. The repo must stay public, or installed copies can't reach the feed. Pushing a `v<version>` tag runs `.github/workflows/release.yml`, which does the same on a macOS runner with the signing identity and Sparkle key from repo secrets
- `headless.rs` - `--transcribe-file`, `--list-devices`, and `--list-models` without a window
- `triage.rs` - Triage an Issue, behind the bug button at the foot of the sidebar and in the tray menu. Its dialog, `ui/pages/triage.rs`, walks through recording the problem with Annotate (optional), picking an agent, and starting. Record closes the dialog so the whole screen is free, and the finished capture opens it again. A capture finished in the 10 minutes before the dialog opened, or while it is open, goes to the agent in the launch prompt. It writes `context.md` (version, build, permissions as the running app sees them, key settings, paths), `PROMPT.md` (the playbook) and `settings.json` (a copy of the settings with the API keys removed) to `triage/<time>` in the app data folder. Then it opens Claude, Claude Code or Codex through its deep link, or copies the prompt. `.github/triage/PLAYBOOK.md` is the playbook the agent follows, built into the binary with `include_str!`. Issues it files use `.github/ISSUE_TEMPLATE/via-triage.yml`. When the playbook changes what the agent should find, change `context.md` to match
- `try_it.rs` - Try it, on the Dictionary page. A dictation that starts while that page shows in the active main window skips paste, history and stats, and goes to the page as a `Take`: the text plus each `Fix` the dictionary made (`audio_toolkit::text`). The page lights up the entries that fired and lets you pick a misheard word to add as a word or replacement
- `utils.rs` - Platform helpers

### UI (`src/ui/`)

- `shell/` - Main window: sidebar, toolbar, banners, inspector. The window sits on AppKit's window material tinted to the content surface, and the sidebar floats on it as a glass panel inset 8pt. A page's inspector (`Page::inspector`, built with `shell::inspector`) is a glass panel flush with the window's trailing edge at full height, with the inspected item's actions in its top row
- `pages/` - Sidebar pages. Each implements `pages::Page`, which puts its tabs and actions in the toolbar and picks a layout (`Padded` or `Fill`). `rows.rs` holds the single-setting rows
- `pages/settings.rs` - Settings, a dialog over the main window rather than a page. Its sidebar lists `SettingsSection`s: General, Appearance and Sound, then Dictate's and Annotate's pages, then System. The gear at the foot of the sidebar, `Cmd+,`, the tray and `ui::show_settings` open it. `DictationSettings` (`dictation_settings.rs`) and `AnnotateSettings` (`annotate.rs`) hold each feature's settings and return one page of groups at a time
- `components/` - Shared controls (buttons, toggles, sliders, dropdowns, dialogs)
- `onboarding/` - First-run screens. `coach.rs` is Keycap Coach, a new user's guided tour: microphone and Accessibility, a dictation practice (the takes come through Try it, so nothing pastes), Screen Recording with a relaunch, then an Annotate practice. `tour.rs` makes its decisions as pure functions, `coach.rs` carries them out, `coach/view.rs` draws each step, and `settings.onboarding_tour` saves its place so a quit or the relaunch resumes it. The tour downloads `ModelStore::starter_model` from its first screen. `models.rs`, the picker, opens if that fails or the user asks. `permissions.rs` is for a returning user who lost a permission
- `stores/` - Shared UI state as GPUI entities
- `overlay.rs` - Recording overlay window and view
- `capsule.rs` - The capsule dictation and Annotate share: its glass surface, live dot, mic meter, spinner, chips and colors. `Fold` shows a capsule's buttons for the first 3.5 seconds of a recording and while the pointer is on it, then folds them away; Esc and the shortcuts work either way. After a paste, Dictate's capsule shows a check for 0.6 seconds, or "Couldn't paste" for 25 seconds or until dismissed. Annotate's done capsule stays 8 seconds, longer while the pointer is on it
- `clay.rs` - Clay for small solid objects (the live dot, the done check, keycaps, the toggle knob): `.clay(color, size, radius, dark)` layers gradients, rims, a specular spot and shadows over a solid fill, and `keycap()` builds a shortcut hint from it. `.muted_clay(...)` drops the spot and softens the rest, for the segmented thumb and the shortcut chips that sit among text. Glass stays on cards and panels
- `glass.rs` - Liquid Glass behind GPUI elements: `glass(id, style, radius)` keeps an `NSGlassEffectView` under an element's bounds, below GPUI's Metal view. Anything GPUI paints over that area covers the glass, so leave it unpainted
- `popup.rs` - Dropdown menus, toasts, tooltips and dialogs on Liquid Glass. Each gets a borderless child window of the main window, because glass can't show through GPUI's drawing in the same window. The main window paints an `anchor` for each menu, toast or tooltip every frame, and a frame without one hides it. These popups never take keyboard focus, so their click handlers reach the main window through `in_host`. A dialog is a key window over the whole main window (`DialogWindow`)
- `hover_card.rs` - A hover card on Liquid Glass, like a heatmap day's. Glass can't float over GPUI's drawing in the same window, so the card is one shared transparent window that `show` moves over the anchor
- `theme.rs` - Palette for light and dark mode, and the corner radii (`radius`)

Backend code emits events with `AppHandle::emit`. The UI receives them on the GPUI main thread through `ui::event_bus`, and views subscribe with `cx.subscribe(&event_bus(cx), ...)`. Async backend work runs on the tokio runtime in `runtime.rs`, and GPUI tasks await it with `runtime::run`.

### Pipeline

Annotate has its own pipeline in `src/annotate/`: the shortcut starts a screen, voice and ink recording, and finishing it transcribes the narration with dictation's model and dictionary (`annotate/transcribe.rs`), writes a `.scrawl` bundle (keyframes, a contact sheet and `summary.md`) and copies it to the clipboard.

Dictation: shortcut press, then audio recording with VAD, then transcription (`transcribe-cpp` for GGML/GGUF models, `transcribe-rs` for ONNX models, Scrawl's helper for Apple Speech), then optional post-processing, then paste into the front app.

## Internationalization

All user-facing strings use translations. Add the key to `locales/en/translation.json`, then read it with `crate::i18n::t("key.path")` (or `t_with` for `{{placeholders}}`). `build.rs` embeds every file under `locales/`.

## Code style

- Run `cargo fmt` and `cargo clippy` before committing
- Handle errors explicitly (avoid `unwrap` in production code)
- Use descriptive names and add doc comments for public APIs

## CLI parameters

Flags are defined in `cli.rs`. They are runtime overrides and never change persisted settings.

| Flag | Description |
| --- | --- |
| `--toggle-transcription` | Toggle recording on a running instance |
| `--toggle-post-process` | Toggle recording with post-processing on a running instance |
| `--cancel` | Cancel the current operation on a running instance |
| `--start-hidden` | Launch without showing the main window |
| `--no-tray` | Launch without the menu bar icon |
| `--debug` | Enable debug mode with verbose logging |
| `-f`, `--transcribe-file <WAV>` | Transcribe a 16 kHz mono WAV and exit |
| `--model <ID>` | Model for `--transcribe-file` (default: the selected model) |
| `--device-index <N>` | Compute device for `--transcribe-file` (see `--list-devices`) |
| `--list-devices` | List transcribe-cpp compute devices and exit |
| `--list-models` | List model ids and exit |
| `--repeat <N>` | Repeat `--transcribe-file` N times |
| `--json` | Print `--transcribe-file` and `--list-models` results as JSON |
| `--triage` | Write a triage session and print the prompt and agent links, then exit. Records no permissions or settings, because a terminal's process doesn't hold Bespoke's grants |

The remote-control flags work through `single_instance.rs`: the second instance sends its arguments to the running one and exits.

## Debug mode

`Cmd+Shift+D` toggles debug mode.

## Platform notes

Bespoke supports macOS only, and a build for another OS fails at compile time.

## Data location

Settings, history, recordings, and downloaded models live in `~/Library/Application Support/com.andynguyen.bespoke`. Annotate keeps its captures, `config.json` and the Agent Access token (`mcp-token`) in its `annotate/` folder and logs to `annotate.log`. Logs go to `~/Library/Logs/com.andynguyen.bespoke`. On first launch, Bespoke moves data from the old `com.pais.handy` folders.

## Commits

Use conventional commit prefixes (`feat:`, `fix:`, `docs:`, `refactor:`, `chore:`). Focus the message on why, not what.
