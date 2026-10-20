# Scrawl in Bespoke

This folder holds the parts of [Scrawl](https://github.com/kitlangton/scrawl) at commit `10531a67a9d7677cb276f1c0aaaf0ce91bd025fc` that Bespoke's Annotate feature runs. It is a library, not Scrawl's app. Bespoke starts it with `scrawl::start` and reaches it through `host.rs`.

Upstream `src/` and `helper/` hold about 69,000 lines. This copy holds about 27,000.

Bundles keep Scrawl's format (`.scrawl` folders with `scrawl.json` and `summary.md`), so Scrawl's agent skill and `scrawl` command read them unchanged.

## Kept

- Recording: `capture.rs` and `capture/macos.rs` (ScreenCaptureKit video, the key tap, the mouse monitor), `audio.rs` and `mic.rs` (the microphone), `sketch.rs` and `laser_overlay.rs` (ink).
- The capsule and picker: `native/pill/` (the Metal capsule), `ui.rs` and `ui/macos.rs` (the coordinator, the picker windows, the review window), `hold.rs` (tap or hold), `windows.rs` and `screens.rs` (window and screen targets).
- Building a capture: `keyframes.rs`, `autocrop.rs`, `render.rs` (the contact sheet), `transcribe.rs` and `speech_model.rs` (Apple Speech through the helper), `model.rs` and `lib.rs` (the bundle and `summary.md`).
- After a capture: `handoff.rs` (clipboard, paste, `config.json`) and `retention.rs` (deleting old captures).
- `helper/`: the Swift `scrawl-helper` with the `transcribe`, `speech-model`, `frames`, `permissions` and `permission-panel` commands.

## Left out

- The CLI (`main.rs`, `command_line.rs`), Sparkle, the menu bar item, onboarding, and the Rust and SwiftUI settings windows. Bespoke's Annotate page and its tray submenu replace them.
- The iPhone mirror (`phone.rs`, `Mirror.swift`, ScrawlKit in `Sources/`). The hold switcher steps between Screen and Window only.
- Sending captures to agents: OpenCode routing and delivery (`routing/`, `delivery.rs`, `inbox.rs`, `web.rs`, `page.rs`, `sdk.js`), Hex, and the accessibility context (`agent_context.rs`, `ax.rs`). Captures go to the clipboard only.
- Experiments and tools: live transcription, the pill lab, the GPUI capsule fallback, tune and trace output, video export (`export/`, `Export.swift`), `zoom.rs`, and the autocrop playground.
- All Windows code.

## Changes

- `host.rs` (new) is the seam to Bespoke: data and log paths, menu actions, shortcut presses, and the reads and writes behind the Annotate page.
- `permissions.rs` (new) reads the Screen Recording, Microphone and Accessibility grants. Upstream read them in its onboarding code.
- Shortcuts: Bespoke owns the global shortcuts and forwards presses to `host::press`. The Carbon hotkeys are gone. The key tap still matches the record combo, and `emit_capture_shortcut` drops the second report of one press.
- `ui.rs`: `start` replaces `run_app` and runs inside Bespoke's GPUI app. Every "Settings" button opens Bespoke's Annotate page through `host::open_settings`.
- `ui/macos.rs`: the picker panel orders front without becoming key, and `focus_picker_soon` makes it key once Bespoke is active. Making a nonactivating panel key while the app is inactive sends Bespoke's main window a stray `windowDidBecomeKey`, and GPUI deadlocks re-entering its own window lock. For the same reason, Bespoke's swizzle in `src/ui/platform.rs` refuses key status only to its recording overlay. `hand_back_keys` reactivates another app only when Annotate took the keys from it.
- `handoff.rs`, `capture.rs`: captures and `config.json` live in Bespoke's `annotate/` data folder, and the log is `annotate.log` in Bespoke's log folder.
- `mic.rs`, `audio.rs`: cpal 0.16 instead of 0.18, because Bespoke's rodio pins it and two cpal versions both link `alsa-sys`. Device ids stay CoreAudio UIDs, read from CoreAudio and matched to cpal's devices by name.
- `laser_overlay.rs`, `native/pill/mod.rs`: the Metal shaders compile from source at launch, like GPUI's `runtime_shaders`, so the build needs no Metal toolchain.
- `native/pill/types.rs`: `PillStyle::current` reads `config.json` once a second instead of every frame.
- `helper/`: the bundle id is `com.andynguyen.bespoke.scrawl-helper`, and the permission panel names Bespoke.
- `build.rs` builds the helper into `target/<profile>/`. `scripts/install-local.sh` copies it into `Bespoke.app` and signs it.

To see every change, diff `src/` and `helper/` against upstream at that commit.

Run its tests from Bespoke's root with `cargo test -p scrawl`, so they use Bespoke's `Cargo.lock`.
