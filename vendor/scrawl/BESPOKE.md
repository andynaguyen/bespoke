# Scrawl in Bespoke

This folder holds the parts of [Scrawl](https://github.com/kitlangton/scrawl) at commit `10531a67a9d7677cb276f1c0aaaf0ce91bd025fc` that Bespoke's Annotate feature runs. It is a library, not Scrawl's app. Bespoke starts it with `scrawl::start` and reaches it through `host.rs`.

Upstream `src/` and `helper/` hold about 69,000 lines. This copy holds about 27,000.

Bundles keep Scrawl's format (`.scrawl` folders with `scrawl.json` and `summary.md`), so Scrawl's agent skill and `scrawl` command read them unchanged.

## Kept

- Recording: `capture.rs` and `capture/macos.rs` (ScreenCaptureKit video, the key tap, the mouse monitor), `audio.rs` and `mic.rs` (the microphone), `sketch.rs` and `laser_overlay.rs` (ink).
- The capsule and picker: `native/pill/` (the capsule's link to the host that draws it), `ui.rs` and `ui/macos.rs` (the coordinator, the picker windows, the review window), `hold.rs` (tap or hold), `windows.rs` and `screens.rs` (window and screen targets).
- Building a capture: `keyframes.rs`, `autocrop.rs`, `render.rs` (the contact sheet), `transcribe.rs` and `speech_model.rs` (Apple Speech through the helper), `model.rs` and `lib.rs` (the bundle and `summary.md`).
- After a capture: `handoff.rs` (clipboard, paste, `config.json`) and `retention.rs` (deleting old captures).
- `helper/`: the Swift `scrawl-helper` with the `transcribe`, `speech-model`, `frames`, `permissions` and `permission-panel` commands.

## Left out

- The CLI (`main.rs`, `command_line.rs`), Sparkle, the menu bar item, onboarding, and the Rust and SwiftUI settings windows. Bespoke's Annotate page and its tray submenu replace them.
- The iPhone mirror (`phone.rs`, `Mirror.swift`, ScrawlKit in `Sources/`). The hold switcher steps between Screen and Window only.
- Sending captures to agents: OpenCode routing and delivery (`routing/`, `delivery.rs`, `inbox.rs`, `web.rs`, `page.rs`, `sdk.js`), Hex, and the accessibility context (`agent_context.rs`, `ax.rs`). Captures go to the clipboard, and Bespoke's own MCP server (`src/annotate/mcp.rs`) serves them to agents.
- Experiments and tools: live transcription, the pill lab, the GPUI capsule fallback, tune and trace output, video export (`export/`, `Export.swift`), `zoom.rs`, and the autocrop playground.
- All Windows code.

## Changes

- `host.rs` (new) is the seam to Bespoke: data and log paths, menu actions, shortcut presses, the reads and writes behind the Annotate page, and the length, marks and app of each finished capture for Bespoke's Stats page (`on_capture`, and `finished_captures` for the ones already on disk). `finish_recording` in `ui.rs` reports each capture once its bundle is written.
- `permissions.rs` (new) reads the Screen Recording, Microphone and Accessibility grants. Upstream read them in its onboarding code.
- Shortcuts: Bespoke owns the global shortcuts and forwards presses to `host::press`. The Carbon hotkeys are gone. The key tap still matches the record combo, and `emit_capture_shortcut` drops the second report of one press.
- `ui.rs`: `start` replaces `run_app` and runs inside Bespoke's GPUI app. Every "Settings" button opens Bespoke's Annotate page through `host::open_settings`.
- `ui/macos.rs`: the picker panel orders front without becoming key, and `focus_picker_soon` makes it key once Bespoke is active. Making a nonactivating panel key while the app is inactive sends Bespoke's main window a stray `windowDidBecomeKey`, and GPUI deadlocks re-entering its own window lock. For the same reason, Bespoke's swizzle in `src/ui/platform.rs` refuses key status only to its recording overlay. `hand_back_keys` reactivates another app only when Annotate took the keys from it. The picker is found by its own window number. Upstream took the first large window in the app, which in Bespoke can be the hidden main window.
- `handoff.rs`, `capture.rs`: captures and `config.json` live in Bespoke's `annotate/` data folder, and the log is `annotate.log` in Bespoke's log folder.
- `lib.rs`: bundle ids start with `bespoke-` instead of `scrawl-`. `retention.rs` reads the creation time from either prefix, so older bundles still expire.
- `sketch.rs`: the first line of `summary.md` and of a copied capture starts with `[Bespoke annotation` instead of `[scrawl`.
- `ui.rs`: `open_review_window` hands the capture to `host::on_inspect` when Bespoke registered it, so the capsule's Inspect and Open Latest open the capture on Bespoke's Annotate page. `BundleReviewWindow` only opens when no host is registered. `handoff.rs`: `Capture` also gives its folder, mark count and `keyframe_rows` (time, action, speech and image of each keyframe), and `host.rs` adds `recent_captures`, `copy_capture` and `reveal_capture` for that page.
- `handoff.rs`: `Capture` (new, re-exported by `host.rs`) reads a bundle for Bespoke's MCP server. `Capture::find` takes an id, `latest`, or another machine's path to the bundle, and looks only inside the captures folder. `sketch.rs`: `target_label` moved out of `summary_markdown` so `Capture` can use it.
- `mic.rs`, `audio.rs`: cpal 0.16 instead of 0.18, because Bespoke's rodio pins it and two cpal versions both link `alsa-sys`. Device ids stay CoreAudio UIDs, read from CoreAudio and matched to cpal's devices by name.
- `laser_overlay.rs`: the Metal shader compiles from source at launch, like GPUI's `runtime_shaders`, so the build needs no Metal toolchain. While recording, the target's border turns into the red ring Bespoke's capsule uses, and the picker's size badge takes the capsule's translucent style.
- `native/pill/`: Bespoke draws the capsule. Upstream's Metal capsule (`mod.rs`, `model.rs`, `atlas.rs`, `pill.metal`) is gone. `NativePill` keeps its calls (`sync`, `place_on`, `drain_actions`, `is_fully_hidden`, `pointer_on_pill`) and sends them over a `CapsuleLink` to the host registered with `host::on_capsule`. Bespoke's `src/annotate/capsule.rs` draws it with the capsule dictation uses. `Pres` moved to `types.rs`, `PillSnapshot` gained the record shortcut, and `pillStyle` in `config.json` is read but ignored. `host::capsule_origin` places the capsule at the top (upstream's `pill_origin`) or above the Dock.
- `helper/`: the bundle id is `com.andynguyen.bespoke.scrawl-helper`, and the permission panel names Bespoke.
- `build.rs` builds the helper into `target/<profile>/`. `scripts/install-local.sh` copies it into `Bespoke.app` and signs it.

To see every change, diff `src/` and `helper/` against upstream at that commit.

Run its tests from Bespoke's root with `cargo test -p scrawl`, so they use Bespoke's `Cargo.lock`.
