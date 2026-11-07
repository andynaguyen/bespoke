# Contributing to Bespoke

Bug reports, fixes and features are welcome. Open an issue first for a large change, so we can agree on the approach before you write it.

Coding agents should also read [AGENTS.md](AGENTS.md), which maps the code.

## Set up

You need a Mac with Apple Silicon and:

- Rust stable, from [rustup](https://rustup.rs)
- cmake: `brew install cmake`
- Xcode 26 or its Command Line Tools. The build compiles Annotate's Swift helper against the macOS 26 SDK.

```bash
git clone https://github.com/andynaguyen/bespoke.git
cd bespoke
cargo run
```

If cmake fails on an old policy version, prefix the command with `CMAKE_POLICY_VERSION_MINIMUM=3.5`. GPUI uses the `runtime_shaders` feature, so the build does not need Xcode's Metal compiler.

## Build and run

```bash
cargo run                        # debug build
cargo run -- --start-hidden      # pass CLI flags after --
cargo test
cargo clippy
cargo fmt
scripts/install-local.sh         # release build, signed, in /Applications
scripts/install-release.sh       # back to the latest published release
```

`scripts/install-local.sh` builds a release, bundles `Bespoke.app`, signs it, and replaces `/Applications/Bespoke.app`. It signs with the "Bespoke Local Signing" identity. Run `scripts/create-signing-identity.sh` once per Mac to add that self-signed identity to the login keychain, or set `BESPOKE_SIGNING_IDENTITY` to sign with another codesign identity. A stable identity keeps the Accessibility and Microphone grants across reinstalls. `--no-build` reinstalls the last build.

`scripts/install-release.sh` puts a published release back in `/Applications/Bespoke.app`, with Sparkle, so it updates itself again. It downloads the zip from GitHub releases with `gh`, checks it against the release's SHA-256 digest, quits Bespoke and swaps the app. Pass a tag such as `v1.0.4` for an older release. Releases sign with the same identity, so the grants carry over.

Development builds keep file and line numbers in Bespoke backtraces. Other dependencies build without debug information, which shortens clean builds. Run `CARGO_PROFILE_DEV_DEBUG=2 cargo build` to inspect Bespoke's local variables in a debugger. Dependency backtraces have no file or line numbers. To step through dependency source or inspect its local variables, remove the `[profile.dev.package."*"]` section from `Cargo.toml`.

## Release CI

The release workflow runs on version tags and manual requests. It uses one standard `macos-15` build runner with Xcode 26.3 and the macOS 26.2 SDK. CI retains Rust optimization level 3, disables LTO and uses 256 code generation units. Local release builds keep the existing Cargo.toml profile and compile the native transcription library from source.

CI downloads the upstream transcribe.cpp 0.2.3 ARM64 Metal binary. `scripts/ci/prepare-transcribe.sh` verifies its SHA-256, version and backend contract. A dependency upgrade must update that pin. The app bundle includes the signed native libraries and their licenses.

The workflow extracts the signed ZIP to another directory, verifies its signatures and checks Metal device enumeration. Release tags then publish the verified ZIP and appcast with `scripts/release.sh --publish-only`. Manual runs upload the files in `dist/` as an artifact.

## Send a pull request

1. Branch from `main`.
2. Run `cargo fmt`, `cargo clippy` and `cargo test`. Fix every clippy warning your change adds.
3. For a UI change, attach before and after screenshots. [Dev snapshots](#dev-snapshots) render them without clicking through the app.
4. Write commit messages with a conventional prefix (`feat:`, `fix:`, `docs:`, `refactor:`, `chore:`), and say why the change is needed, not what the diff does.

Handle errors explicitly. Production code avoids `unwrap`. Give public APIs doc comments.

## Where things live

| Path | Contents |
| --- | --- |
| `src/main.rs`, `src/startup.rs` | CLI parsing, logging, and app setup |
| `src/app_handle.rs` | App state, events, data paths, and the settings store |
| `src/managers/` | Audio recording, model downloads, transcription, and history |
| `src/audio_toolkit/` | Device enumeration, recording, resampling, and voice activity detection |
| `src/shortcut/` | Global shortcuts: the event-tap listener in `keys/`, from `handy-keys`, and the `global-hotkey` backend |
| `src/annotate.rs`, `src/annotate/`, `swift/annotate-helper/` | Annotate: screen, voice and ink capture, started from Scrawl. `docs/annotate-upstream.md` maps it to upstream |
| `src/overlay.rs`, `src/ui/overlay.rs` | The recording overlay: backend entry points, then the window and view |
| `src/tray.rs` | The menu bar icon and menu |
| `src/updater.rs` | Self-updates through Sparkle |
| `src/ui/shell/` | The main window: sidebar, toolbar, banners, and inspector |
| `src/ui/glass.rs`, `src/ui/capsule.rs` | Liquid Glass under GPUI elements, and the capsule dictation and Annotate share |
| `src/ui/pages/` | Sidebar pages, with single-setting rows in `rows.rs` |
| `src/ui/components/` | Buttons, toggles, sliders, dropdowns, dialogs, and other controls |
| `src/ui/onboarding/` | First-run permission and model screens |
| `src/ui/stores/` | Shared UI state as GPUI entities |

Backend code emits events with `AppHandle::emit`. The UI receives them on the GPUI main thread through `ui::event_bus`, and views subscribe with `cx.subscribe(&event_bus(cx), ...)`. Async backend work runs on the tokio runtime in `runtime.rs`, and GPUI tasks await it with `runtime::run`.

`src/shortcut/keys/` started as the `handy-keys` crate. `docs/handy-keys-upstream.md` maps it to upstream.

## Translations

UI and tray strings live in `locales/<language>/translation.json`. `build.rs` embeds every locale. English is the source: add new keys to `locales/en/translation.json` and read them with `crate::i18n::t("key.path")`, or `t_with` for `{{placeholders}}`. A key missing from another language falls back to English.

## Dev snapshots

Debug builds can render pages to PNG files without clicking through the app:

```bash
BESPOKE_DEV_SNAPSHOT=/tmp/shots \
BESPOKE_DEV_PAGES=dictate:history,settings:system,debug@900,onboarding:model,overlay:streaming \
BESPOKE_DEV_THEME=dark \
cargo run
```

Pages are sidebar sections, `settings:<page>` for a page of the Settings dialog (the ids in `SettingsSection::id`, like `general` or `dictate-history`), `dictate:<tab>`, `stats:<tab>`, `onboarding:accessibility|model`, `overlay:<state>` (see `ui::overlay::dev_preview`), `capsule:<form>` for Annotate's capsule (see `annotate::capsule::dev_preview`), and `annotate:latest`, which opens the newest capture. `scrawl:choose` opens Annotate's picker and captures the whole main display, which needs the Screen Recording permission. An `@<px>` suffix scrolls the page first. `#<n>` presses Tab n times with focus rings shown, and focus carries over between pages. `!<key>` then presses a key, so `settings:appearance#12!down` opens the language menu. `toast:error|info|warning` shows a sample toast over the current page, `dialog:entry` opens the dictionary's Add replacement dialog, `dialog:word` its Add word dialog, and `dialog:triage` the Triage an Issue dialog. `BESPOKE_DEV_TAKE=<text>` fills the Dictionary page's Try it panel as if a model heard that text, and `BESPOKE_DEV_TAKE_PICK=<n>` picks its nth word to teach. Captures include the main window's popups and dialog windows. `BESPOKE_DEV_LANGUAGE=ar` renders in another language, which checks right-to-left layout. A window capture has nothing behind its glass, so glass looks flat in it. `BESPOKE_DEV_BACKDROP=<png>` fills each window behind its glass with an image, and `BESPOKE_DEV_DESKTOP=1` keeps what is on screen behind the window in the capture. `BESPOKE_DEV_SLIDE=0.5` holds every segmented control's thumb halfway through its slide from the first option to the selected one. `BESPOKE_DEV_UPDATE=1.1.0` shows the sidebar's update card for that version. Snapshots work on macOS only, and they keep working with the screen locked.

Snapshots read your real settings, history and captures. Check them for private text before you attach them to a pull request.

The README's screenshots in `docs/screenshots/` use sample data instead. Bespoke finds its data folder through `$HOME`, so point `HOME` at a folder whose `Library/Application Support/com.andynguyen.bespoke` holds a sample `settings_store.json` and `history.db`, and symlink `.cache/huggingface` there so downloaded models still show. Quit any running Bespoke first, or the snapshot run forwards its launch to it and exits. Each screenshot has a `-light` and a `-dark` copy, rendered with `BESPOKE_DEV_THEME` and a `BESPOKE_DEV_BACKDROP` gradient, and resized to 1600px wide.

## GPUI layout notes

The UI sets a 15px root size, so GPUI's rem-based helpers (`p_4`, `gap_3`, `rounded_lg`) follow Tailwind's spacing scale at 15px per rem. GPUI differs from a browser in these ways:

- Block layout does not reliably stretch flex children. Lists and wrappers whose rows must span the full width use `.flex().flex_col()`.
- Text reports its unwrapped width as its minimum, so a text column in a flex row needs `.flex_1().min_w_0()` to wrap.
- `Window::refresh` does nothing while a frame draws. Layouts that depend on their own width use `components::width_tracker`, which re-renders after the frame.
- `svg()` paints nothing without an explicit color. Use `components::icon`, which paints with the inherited text color.
- A glyph drawn in a zero-alpha color comes out dark. Hide text by painting it in the background color.
- AppKit calls that can call back into GPUI (showing, hiding or activating windows, changing the appearance) run on the next main-thread turn through `cx.foreground_executor()`, or GPUI panics on a re-entrant borrow.

## Releases

Maintainers publish releases. Bump `version` in `Cargo.toml`, add `release-notes/<version>.md`, push, then run:

```bash
npm run build-release -- --publish
```

`scripts/release.sh` builds a release with Sparkle embedded, zips `Bespoke.app` into `dist/<version>/`, signs the zip with the Sparkle key in the login keychain, and writes `appcast.xml`. `--publish` creates release `v<version>` on GitHub with both files. Installed copies read the appcast from the latest release, so this repo must stay public.

What's New reads `release-notes/<version>.md`. Put images in `release-notes/images/` and reference them as `/release-notes/images/<file>`. `build.rs` embeds both folders, and either may be missing.
