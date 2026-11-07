# Annotate and upstream Scrawl

Annotate's capture code in `src/annotate/` and its Swift helper in `swift/annotate-helper/` started as [Scrawl](https://github.com/kitlangton/scrawl) at commit `10531a67a9d7677cb276f1c0aaaf0ce91bd025fc`, by Kit Langton under the MIT license (`THIRD_PARTY_NOTICES.md`). It was vendored as a separate crate until it moved into `src/annotate/`. This file maps it back to upstream, to port later upstream commits.

Bundles keep Scrawl's format (`.scrawl` folders with `scrawl.json` and `summary.md`), so Scrawl's agent skill and `scrawl` command read them unchanged. The helper keeps the file name `scrawl-helper` and the bundle id `com.andynguyen.bespoke.scrawl-helper`, so the macOS permissions it holds carry over.

## Where upstream files went

| Upstream | Bespoke |
| --- | --- |
| `src/lib.rs` | `src/annotate/bundle.rs` |
| `src/ui.rs`, `src/ui/macos.rs` | `src/annotate/coordinator.rs`, `src/annotate/coordinator/macos.rs` |
| `src/native/pill/` | `src/annotate/capsule_link.rs` |
| `src/transcribe.rs` | `transcribe` and `run` in `src/annotate/transcribe.rs` |
| `src/capture/macos.rs` | `src/annotate/capture/macos.rs` |
| other `src/<name>.rs` | `src/annotate/<name>.rs` |
| `helper/` | `swift/annotate-helper/` |
| `build.rs` (the helper build) | `build_annotate_helper` in Bespoke's `build.rs` |

## Kept

- Recording: `capture.rs` and `capture/macos.rs` (ScreenCaptureKit video, the key tap, the mouse monitor), `audio.rs` (the microphone), `sketch.rs` and `laser_overlay.rs` (ink).
- The capsule and picker: `capsule_link.rs` (the capsule's link to `capsule.rs`, which draws it), `coordinator.rs` and `coordinator/macos.rs` (the coordinator and the picker windows), `hold.rs` (tap or hold), `windows.rs` and `screens.rs` (window and screen targets).
- Building a capture: `keyframes.rs`, `autocrop.rs`, `render.rs` (the contact sheet), `transcribe.rs` and `speech_model.rs` (Apple Speech through the helper), `model.rs` and `bundle.rs` (the bundle and `summary.md`).
- After a capture: `handoff.rs` (clipboard, paste, `config.json`) and `retention.rs` (deleting old captures).
- `helper/`: the Swift `scrawl-helper` with the `transcribe`, `speech-model`, `frames`, `permissions` and `permission-panel` commands.

## Left out

- The CLI (`main.rs`, `command_line.rs`), Sparkle, the menu bar item, onboarding, and the Rust and SwiftUI settings windows. Bespoke's Annotate page and its tray submenu replace them.
- The iPhone mirror (`phone.rs`, `Mirror.swift`, ScrawlKit in `Sources/`). The hold switcher steps between Screen and Window only.
- Sending captures to agents: OpenCode routing and delivery (`routing/`, `delivery.rs`, `inbox.rs`, `web.rs`, `page.rs`, `sdk.js`), Hex, and the accessibility context (`agent_context.rs`, `ax.rs`). Captures go to the clipboard, and Bespoke's own MCP server (`src/annotate/mcp.rs`) serves them to agents.
- Experiments and tools: live transcription, the pill lab, the GPUI capsule fallback, tune and trace output, video export (`export/`, `Export.swift`), `zoom.rs`, and the autocrop playground.
- All Windows and Linux code.
- Scrawl's review window (`BundleReviewWindow`) and its own microphone choice (`mic.rs`, `microphone` in `config.json`).

## Changes

- Bespoke calls the capture modules directly, and they call back into `src/annotate.rs`. `finish_recording` in `coordinator.rs` passes each capture's folder, length, marks and app (`handoff::Finished`) to `annotate::capture_finished` once its bundle is written, for Bespoke's Stats page and Triage dialog. `handoff::finished_captures` lists the ones already on disk, and `handoff::newest_capture` finds the newest by the time in its folder name and reads only that manifest. `annotate::capture_cancelled` reports a recording or picker that ends without one: `cancel_recording`, or a bundle that fails to write. Bespoke's Triage dialog uses the pair to reopen after its Record button.
- `permissions.rs` (new) reads the Screen Recording, Microphone and Accessibility grants. The key tap and paste check Accessibility with Bespoke's `ui::platform::accessibility_granted`. Upstream read them in its onboarding code. `capture::screen_capture_granted` reports this process's own Screen Recording answer for Bespoke's triage context. `read_grants` logs the grants to `annotate.log` each time they change. `request_screen_capture` asks macOS for Screen Recording and logs the answer, and `open_permission_settings` opens the helper's permission panel or System Settings. Bespoke calls it on the first click of the Screen Recording row, so Bespoke appears in the Screen Recording list.
- Shortcuts: Bespoke owns the global shortcuts and forwards presses to `capture::press`. The Carbon hotkeys are gone. `coordinator::start` takes the first record and paste combos, `capture::set_shortcuts` takes later changes, and `capture::shortcut_from_binding` reads Bespoke's binding strings. The key tap still matches the record combo, and `emit_capture_shortcut` drops the second report of one press.
- `coordinator.rs`: `start` replaces `run_app` and runs inside Bespoke's GPUI app. Every "Settings" button opens Bespoke's settings through `annotate::open_settings`. `coordinator::menu` queues the actions from Bespoke's tray, Annotate page and Triage dialog. When a missing permission holds back a recording, `tick_gate` passes the missing permissions along, and Bespoke opens Settings at System, where Annotate's permission rows are, with a toast that names them.
- `coordinator/macos.rs`: the picker panel orders front without becoming key, and `focus_picker_soon` makes it key once Bespoke is active. Making a nonactivating panel key while the app is inactive sends Bespoke's main window a stray `windowDidBecomeKey`, and GPUI deadlocks re-entering its own window lock. For the same reason, Bespoke's swizzle in `src/ui/platform.rs` refuses key status only to its recording overlay. `hand_back_keys` reactivates another app only when Annotate took the keys from it. The picker is found by its own window number. Upstream took the first large window in the app, which in Bespoke can be the hidden main window.
- `handoff.rs`, `capture.rs`: captures and `config.json` live in Bespoke's `annotate/` data folder, and the log is `annotate.log` in Bespoke's log folder. Both paths come from `AppHandle::try_global`. Without a running Bespoke, as in tests, there is no captures folder and `annotate_log` writes to stderr only.
- `bundle.rs`: bundle ids start with `bespoke-` instead of `scrawl-`. `retention.rs` reads the creation time from either prefix, so older bundles still expire.
- `sketch.rs`: the first line of `summary.md` and of a copied capture starts with `[Bespoke annotation` instead of `[scrawl`.
- `coordinator.rs`: the capsule's Inspect and Open Latest open the capture on Bespoke's Annotate page with `ui::pages::captures::inspect`. `handoff.rs`: `Capture` also gives its folder, mark count and `keyframe_rows` (time, action, speech and image of each keyframe), and `recent_captures` and `show_captures` list and open them for that page.
- `handoff.rs`: `Capture` (new) reads a bundle for Bespoke's MCP server. `Capture::find` takes an id, `latest`, or another machine's path to the bundle, and looks only inside the captures folder. `sketch.rs`: `target_label` moved out of `summary_markdown` so `Capture` can use it.
- `audio.rs`: cpal 0.16 instead of 0.18, because Bespoke's rodio pins it and two cpal versions both link `alsa-sys`.
- `audio.rs`: each recording opens the microphone on Bespoke's Sound page (`managers::audio::preferred_microphone`), clamshell rule included, matched to cpal's devices by name. It falls back to the system default when that device isn't connected or won't open.
- `laser_overlay.rs`: the Metal shader compiles from source at launch, like GPUI's `runtime_shaders`, so the build needs no Metal toolchain. While recording, the target's border turns into the red ring Bespoke's capsule uses, and the picker's size badge takes the capsule's translucent style.
- `capsule_link.rs`: Bespoke draws the capsule. Upstream's Metal capsule (`mod.rs`, `model.rs`, `atlas.rs`, `pill.metal`) is gone. `NativePill` keeps its calls (`sync`, `place_on`, `drain_actions`, `is_fully_hidden`, `pointer_on_pill`) and sends them over a `CapsuleLink` to `capsule::open` in `src/annotate/capsule.rs`, which draws it with the capsule dictation uses. `Pres` moved to `types.rs`, `PillSnapshot` gained the record shortcut, and `pillStyle` in `config.json` is gone. `capsule_origin` in `capsule.rs` places the capsule at the top (upstream's `pill_origin`) or above the Dock.
- `helper/`: the bundle id is `com.andynguyen.bespoke.scrawl-helper`, and the permission panel names Bespoke.
- Transcription: `transcribe::run` transcribes a capture with the model picked on Bespoke's Models page, or with Apple Speech without a running Bespoke, as in tests. `speech_model::transcribe_again` goes through `run` too. Bespoke's `apple_speech.rs` runs Apple Speech through `helper::transcribe` and lists it as a model. The helper's `transcribe` takes `--contextual-strings` (a JSON array, Bespoke's dictionary) and passes them to `SpeechAnalyzer` in an `AnalysisContext`. `speech_model` reports each change in Apple's speech model to `apple_speech::status_changed`, and Bespoke's Models page starts its install with `speech_model::ensure`. `speech_model::ensure` returns the number of the attempt that answers the call. A call while the worker waits to retry starts the next attempt at once, where upstream slept through it.
- Bespoke's `build.rs` builds the helper into `target/<profile>/scrawl-helper`. `scripts/install-local.sh` copies it into `Bespoke.app` and signs it.

## Ported from later upstream commits

- `sketch.rs`: one-stroke arrows and flat or tailed circles count as marks (`6ea38e37098cb1a0014fdf8c6b23e1fc829a30e9`, `2a6428009ec5398d7a524deee613d51ee1c844c6`).
- Clicks: a ⇧-click pin shows live as a 350 ms ring pulse (`sketch::pin_pulse`) instead of a disc that stays for the whole recording, and keyframes mark clicks with a thin ring and a small dot (`8a8d4044a7f7a4ab51a874a81014b07441758281`, the `sketch.rs` part of `cb3567c54986bf46eca3ce6f60f6b5f4074707b4`).
- `render.rs`: the keyframe badge is the number alone in a small white chip (`c6651edaf12c0a41ed86954985524aa2cd3141ec`). The numbered discs on marks stay, because Bespoke has no `scrawl zoom --mark`.
- `laser_overlay.rs`: one pipeline per pair of `has_ink` and `skip_clear` function constants, so a frame runs only the shader work it needs (`da5694c7da63c4a41d6ba6f44d8e13c9612db587`), and a frame identical to the one on screen isn't drawn (`1e3e34859a1f3a96cdce11a5a10d32b3b664f24a`, tests from `9d6e806`). The library still compiles from source at launch.

`capture/macos.rs` (not from upstream, which still stops the stream first): `finish_sck_recording` detaches the recording output, waits up to 30 s for its delegate to report the MP4 finished or failed, then stops the stream within 10 s. A no-op frame handler keeps the stream running while the output finishes. A failed output drops the video. `discard_recording` in `coordinator.rs` runs the stop on its own thread, since the bundle is deleted anyway. `recording_ends_with_a_complete_mp4` (ignored) checks it on a real recording and needs Screen Recording for the process that runs it.

To see every change, diff the files in the table against upstream at that commit.
