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
- `annotate.rs` - Annotate (macOS only): starts the Scrawl capture vendored in `vendor/scrawl` and forwards its two shortcut bindings. `annotate/capsule.rs` draws Scrawl's capsule with `ui::capsule`, and `ui/pages/captures.rs` lists captures and opens one in place of Scrawl's Inspect window. Read `vendor/scrawl/BESPOKE.md` before changing the vendored code
- `annotate/mcp.rs` - Agent Access: an MCP server on `127.0.0.1:47790` that serves captures to coding agents. Claude Code and Codex on a remote machine reach it through an SSH `RemoteForward`, or through this Mac's Tailscale address when Agent Access is set to Tailnet. The tools are in `annotate/mcp/tools.rs`, and the guide sent to agents is `annotate/mcp/instructions.md`. Its Off, This Mac and Tailnet control and the setup to copy are on the MCP page, `ui/pages/mcp.rs`
- `shortcut/` - Global shortcuts. `handy_keys.rs` wraps the patched `handy-keys` crate in `vendor/`, and `hotkey_impl.rs` wraps `global-hotkey`
- `settings.rs` - Settings schema, defaults, and migrations
- `clipboard.rs`, `input.rs`, `paste_tx/` - Pasting text into the front app
- `overlay.rs` - Recording overlay entry points
- `tray.rs`, `tray_i18n.rs` - Menu bar icon and menu
- `single_instance.rs` - A second launch forwards its arguments to the running instance over a local socket, then exits
- `signal_handle.rs` - `send_transcription_input()`, shared by Unix signal handlers and CLI flags
- `updater.rs` - Self-updates through Sparkle (macOS). Only builds from `scripts/release.sh` embed `Sparkle.framework`, so `cargo run` and `install-local.sh` builds hide the update UI. A found update shows as a card in the sidebar, a row in Settings > System and an item in the tray menu. `release.sh --publish` posts each release, with its `appcast.xml`, to this repo's GitHub releases, which `SUFeedURL` in `Info.plist` reads. The repo must stay public, or installed copies can't reach the feed
- `headless.rs` - `--transcribe-file`, `--list-devices`, and `--list-models` without a window
- `utils.rs` - Platform helpers

### UI (`src/ui/`)

- `shell/` - Main window: sidebar, toolbar, banners, inspector. The window sits on AppKit's window material tinted to the content surface, and the sidebar floats on it as a glass panel inset 8pt. A page's inspector (`Page::inspector`, built with `shell::inspector`) is a glass panel flush with the window's trailing edge at full height, with the inspected item's actions in its top row
- `pages/` - Sidebar pages. Each implements `pages::Page`, which puts its tabs and actions in the toolbar and picks a layout (`Padded` or `Fill`). `rows.rs` holds the single-setting rows
- `components/` - Shared controls (buttons, toggles, sliders, dropdowns, dialogs)
- `onboarding/` - First-run permission and model screens
- `stores/` - Shared UI state as GPUI entities
- `overlay.rs` - Recording overlay window and view
- `capsule.rs` - The capsule dictation and Annotate share: its glass surface, live dot, spinner, chips and colors
- `clay.rs` - Clay for small solid objects (the live dot, the done check, keycaps, the toggle knob): `.clay(color, size, radius, dark)` layers gradients, rims, a specular spot and shadows over a solid fill, and `keycap()` builds a shortcut hint from it. `.muted_clay(...)` drops the spot and softens the rest, for the segmented thumb and the shortcut chips that sit among text. Glass stays on cards and panels
- `glass.rs` - Liquid Glass behind GPUI elements: `glass(id, style, radius)` keeps an `NSGlassEffectView` under an element's bounds, below GPUI's Metal view. Anything GPUI paints over that area covers the glass, so leave it unpainted
- `popup.rs` - Dropdown menus, toasts, tooltips and dialogs on Liquid Glass. Each gets a borderless child window of the main window, because glass can't show through GPUI's drawing in the same window. The main window paints an `anchor` for each menu, toast or tooltip every frame, and a frame without one hides it. These popups never take keyboard focus, so their click handlers reach the main window through `in_host`. A dialog is a key window over the whole main window (`DialogWindow`)
- `hover_card.rs` - A hover card on Liquid Glass, like a heatmap day's. Glass can't float over GPUI's drawing in the same window, so the card is one shared transparent window that `show` moves over the anchor
- `theme.rs` - Palette for light and dark mode, and the corner radii (`radius`)

Backend code emits events with `AppHandle::emit`. The UI receives them on the GPUI main thread through `ui::event_bus`, and views subscribe with `cx.subscribe(&event_bus(cx), ...)`. Async backend work runs on the tokio runtime in `runtime.rs`, and GPUI tasks await it with `runtime::run`.

### Pipeline

Annotate has its own pipeline in `vendor/scrawl`: the shortcut starts a screen, voice and ink recording, and finishing it writes a `.scrawl` bundle (keyframes, a contact sheet and `summary.md`) and copies it to the clipboard.

Dictation: shortcut press, then audio recording with VAD, then transcription (`transcribe-cpp` for GGML/GGUF models, `transcribe-rs` for ONNX models), then optional post-processing, then paste into the front app.

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

The remote-control flags work through `single_instance.rs`: the second instance sends its arguments to the running one and exits.

## Debug mode

`Cmd+Shift+D` (macOS) or `Ctrl+Shift+D` (Windows and Linux) toggles debug mode.

## Platform notes

- **macOS** is the only platform that has been built and run. It uses Metal acceleration, and global shortcuts need the Accessibility permission.
- **Windows** code paths exist but have never been compiled. `main.rs` sets `VK_LOADER_LAYERS_DISABLE=~implicit~` to disable implicit Vulkan layers (overlays, capture hooks). Opt out with `BESPOKE_KEEP_VULKAN_IMPLICIT_LAYERS=1` or by setting `VK_LOADER_LAYERS_DISABLE` yourself.
- **Linux** code paths exist but have never been compiled.

## Data location

Settings, history, recordings, and downloaded models live in `~/Library/Application Support/com.andynguyen.bespoke`. Annotate keeps its captures, `config.json` and the Agent Access token (`mcp-token`) in its `annotate/` folder and logs to `annotate.log`. Logs go to `~/Library/Logs/com.andynguyen.bespoke`. On first launch, Bespoke moves data from the old `com.pais.handy` folders.

## Commits

Use conventional commit prefixes (`feat:`, `fix:`, `docs:`, `refactor:`, `chore:`). Focus the message on why, not what.
