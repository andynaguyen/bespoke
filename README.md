# Bespoke

Bespoke is a speech-to-text dictation app for macOS. It records while you hold or tap a shortcut, transcribes on your machine with a local model, and pastes the text into the app in front. It is written in Rust with [GPUI](https://www.gpui.rs/), Zed's UI framework.

## Build and run

```bash
cargo run                        # debug build
cargo run -- --start-hidden      # pass CLI flags after --
cargo test
```

If cmake fails on an old policy version, prefix the command with `CMAKE_POLICY_VERSION_MINIMUM=3.5`. GPUI uses the `runtime_shaders` feature, so the build does not need Xcode's Metal compiler.

Development builds retain full debug information for Bespoke and omit it for
dependencies to reduce compilation time. Dependency source stepping and local
variable inspection require removing the `profile.dev.package."*"` override in
`Cargo.toml`.

To measure a clean development build and a subsequent source-touch rebuild:

```bash
cargo fetch --locked
python3 scripts/benchmark-build.py --output .audit/build-times/my-run
```

Use a new output directory for each run. The script builds offline in an empty
target directory, verifies that Cargo compiled the binary, and runs `--help`.
It saves timings and compiler output, then removes its generated build artifacts.
Pass `--keep-target` to retain those artifacts for additional checks.

## Install

`scripts/install-local.sh` builds a release, bundles `Bespoke.app`, signs it, and installs it to `/Applications`. It signs with the "Bespoke Local Signing" identity. Set `BESPOKE_SIGNING_IDENTITY` to sign with another codesign identity. A stable identity keeps the Accessibility and Microphone grants across reinstalls.

Run `scripts/create-signing-identity.sh` once per Mac to add that self-signed identity to the login keychain. `--no-build` reinstalls the last build.

## Where things live

| Path | Contents |
| --- | --- |
| `src/main.rs`, `src/startup.rs` | CLI parsing, logging, and app setup |
| `src/app_handle.rs` | App state, events, data paths, and the settings store |
| `src/managers/` | Audio recording, model downloads, transcription, and history |
| `src/audio_toolkit/` | Device enumeration, recording, resampling, and voice activity detection |
| `src/shortcut/` | Global shortcuts (`handy-keys` and `global-hotkey` backends) |
| `src/overlay.rs`, `src/ui/overlay.rs` | The recording overlay: backend entry points, then the window and view |
| `src/tray.rs` | The menu bar icon and menu |
| `src/ui/shell/` | The main window: sidebar, footer, and banners |
| `src/ui/pages/` | Sidebar pages, with single-setting rows in `rows.rs` |
| `src/ui/components/` | Buttons, toggles, sliders, dropdowns, dialogs, and other controls |
| `src/ui/onboarding/` | First-run permission and model screens |
| `src/ui/stores/` | Shared UI state as GPUI entities |

Backend code emits events with `AppHandle::emit`. The UI receives them on the GPUI main thread through `ui::event_bus`, and views subscribe with `cx.subscribe(&event_bus(cx), ...)`. Async backend work runs on the tokio runtime in `runtime.rs`, and GPUI tasks await it with `runtime::run`.

## Translations

UI and tray strings live in `locales/<language>/translation.json`. `build.rs` embeds every locale. English is the source: add new keys to `locales/en/translation.json`.

## Release notes

What's New reads `release-notes/<version>.md`. Put images in `release-notes/images/` and reference them as `/release-notes/images/<file>`. `build.rs` embeds both folders, and either may be missing.

## Dev snapshots

Debug builds can render pages to PNG files without clicking through the app:

```bash
BESPOKE_DEV_SNAPSHOT=/tmp/shots \
BESPOKE_DEV_PAGES=history,settings:system,debug@900,onboarding:model,overlay:streaming \
BESPOKE_DEV_THEME=dark \
cargo run
```

Pages are sidebar sections, `settings:<tab>`, `customize:<tab>`, `onboarding:accessibility|model` and `overlay:<state>` (see `ui::overlay::dev_preview`). An `@<px>` suffix scrolls the page first. `#<n>` presses Tab n times with focus rings shown, and focus carries over between pages. `BESPOKE_DEV_LANGUAGE=ar` renders in another language, which checks right-to-left layout. Snapshots work on macOS only, and they keep working with the screen locked.

## GPUI layout notes

The UI sets a 15px root size, so GPUI's rem-based helpers (`p_4`, `gap_3`, `rounded_lg`) follow Tailwind's spacing scale at 15px per rem. GPUI differs from a browser in these ways:

- Block layout does not reliably stretch flex children. Lists and wrappers whose rows must span the full width use `.flex().flex_col()`.
- Text reports its unwrapped width as its minimum, so a text column in a flex row needs `.flex_1().min_w_0()` to wrap.
- `Window::refresh` does nothing while a frame draws. Layouts that depend on their own width use `components::width_tracker`, which re-renders after the frame.
- `svg()` paints nothing without an explicit color. Use `components::icon`, which paints with the inherited text color.
- A glyph drawn in a zero-alpha color comes out dark. Hide text by painting it in the background color.
- AppKit calls that can call back into GPUI (showing, hiding or activating windows, changing the appearance) run on the next main-thread turn through `cx.foreground_executor()`, or GPUI panics on a re-entrant borrow.

## Known limitations

- **Platforms.** Only the macOS build has been compiled and run. The Windows and Linux code paths have never been compiled.
- **Screen readers.** GPUI exposes no accessibility tree, so screen readers get no labels. Keyboard use works: Tab moves through controls and stays inside open dialogs, Enter and Space activate, arrow keys move sliders and heatmap days, and Escape closes menus.
- **Linux windows.** GPUI cannot hide a window and ends the app when the last window closes. Closing the main window therefore minimizes it, and starting hidden starts minimized.
- **Overlay placement.** On Wayland the compositor places the overlay. On Windows the overlay is centered on the display under the cursor, without DPI or text-scale adjustments.
- **Overlay window lifetime.** The overlay window opens on each show and closes 300ms after hiding. Its pop-in fades without a scale effect, because GPUI cannot scale an element.
- **Right-to-left languages.** Rows, margins and the toggle knob mirror. Text inputs keep left-aligned text, the slider and the recording seek bar stay left to right, and long wrapped Arabic or Hebrew lines can clip at the left edge (GPUI's bidi line wrapping).
- **Shortcut recorder on Windows and Linux.** Key names come from the logical key, so numpad keys and non-US layouts can store different names than the physical key.
- **Packaging.** There are no installers or release signing. `scripts/install-local.sh` builds a locally signed app for this machine.

## License

MIT. Bespoke started as a fork of Handy by CJ Pais; see LICENSE.
