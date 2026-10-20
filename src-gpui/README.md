# Handy (GPUI)

A port of Handy from Tauri + React to [GPUI](https://www.gpui.rs/), Zed's Rust UI framework. It aims for the same features and the same UI as the Tauri app in `src-tauri/` and `src/`, which stay in the repo for now.

The backend (`managers/`, `audio_toolkit/`, `shortcut/`, `actions.rs`, ...) is a copy of `src-tauri/src` with Tauri removed. The UI under `src/ui/` is a rewrite of the React app, one module per React component or page.

## Build and run

```bash
cd src-gpui
cargo run                        # debug build
cargo run -- --start-hidden      # CLI flags match the Tauri app
cargo test
```

On macOS, if cmake fails on an old policy version, prefix the command with `CMAKE_POLICY_VERSION_MINIMUM=3.5`. GPUI uses the `runtime_shaders` feature, so the build does not need Xcode's Metal compiler.

`build.rs` embeds files from outside this crate. Edit them in their original place:

| Embedded data | Source |
| --- | --- |
| Translations | `../src/i18n/locales/*/translation.json` |
| Release notes and their images | `../src/content/release-notes/*.md`, `../public/release-notes/` |
| Icons and the logo | `assets/` (Lucide 0.542.0 SVGs in `assets/icons/`) |

## Where things live

| Tauri / React | GPUI |
| --- | --- |
| `tauri::AppHandle`, managed state, events, paths, `tauri-plugin-store` | `app_handle.rs` |
| `lib.rs` setup | `startup.rs`, `main.rs` |
| `tauri-plugin-single-instance` | `single_instance.rs` (Unix socket, loopback TCP on Windows) |
| `tauri-plugin-global-shortcut` | `shortcut/hotkey_impl.rs` (the setting value stays `"tauri"`) |
| `tauri-plugin-autostart`, clipboard, opener | `autostart.rs`, `system_clipboard.rs`, the `open` crate |
| `App.tsx`, sidebar, footer, banners | `ui/shell/` |
| `components/settings/*` pages | `ui/pages/` (`rows.rs` holds the single-setting rows) |
| `components/ui/*` | `ui/components/` |
| `components/onboarding/` | `ui/onboarding/`, `ui/model_card.rs` |
| `src/overlay/RecordingOverlay.tsx` + `overlay.rs` | `ui/overlay.rs` (window, view), `overlay.rs` (backend entry points) |
| Zustand stores | `ui/stores/` (GPUI entities) |
| `theme.css`, Tailwind | `ui/theme.rs`, `ui/components/mod.rs` (`Typography`) |

Backend code emits events with `AppHandle::emit`. The UI receives them on the GPUI main thread through `ui::event_bus`; views subscribe with `cx.subscribe(&event_bus(cx), ...)`. Backend async work runs on the tokio runtime in `runtime.rs`, and GPUI tasks await it with `runtime::run`.

## Matching the React layout

The UI sets a 15px root size, so GPUI's rem-based helpers (`p_4`, `gap_3`, `rounded_lg`) match the Tailwind classes they replace. These GPUI behaviors differ from the browser:

- Block layout does not reliably stretch flex children. Lists and wrappers whose rows must span the full width use `.flex().flex_col()`.
- Text reports its unwrapped width as its minimum, so a text column in a flex row needs `.flex_1().min_w_0()` to wrap.
- `Window::refresh` does nothing while a frame draws. Layouts that depend on their own width use `components::width_tracker`, which re-renders after the frame.
- `svg()` paints nothing without an explicit color. Use `components::icon`, which paints with the inherited text color.
- A glyph drawn in a zero-alpha color comes out dark. Hide text by painting it in the background color.
- AppKit calls that can call back into GPUI (showing, hiding or activating windows, changing the appearance) run on the next main-thread turn through `cx.foreground_executor()`, or GPUI panics on a re-entrant borrow.

## Dev snapshots

Debug builds can render pages to PNG files, which helps compare against the React app without clicking through it:

```bash
HANDY_DEV_SNAPSHOT=/tmp/shots \
HANDY_DEV_PAGES=history,settings:system,debug@900,onboarding:model,overlay:streaming \
HANDY_DEV_THEME=dark \
cargo run
```

Pages are sidebar sections, `settings:<tab>`, `customize:<tab>`, `onboarding:accessibility|model` and `overlay:<state>` (see `ui::overlay::dev_preview`). An `@<px>` suffix scrolls the page first, and `#<n>` presses Tab n times with focus rings shown (focus carries over between pages). `HANDY_DEV_LANGUAGE=ar` renders in another language, which checks right-to-left layout. macOS only. The capture keeps working with the screen locked.

## Known differences from the Tauri app

- **Platforms.** Only the macOS build has been compiled and run. The Windows and Linux code paths have never been compiled.
- **Screen readers.** GPUI exposes no accessibility tree, so the React `aria-*` labels have no equivalent. Keyboard use works: Tab moves through controls and stays inside open dialogs, Enter and Space activate, arrow keys move sliders and heatmap days, and Escape closes menus.
- **Linux windows.** GPUI cannot hide a window and ends the app when the last window closes. Closing the settings window therefore minimizes it, and starting hidden starts minimized.
- **Overlay placement.** On Wayland the compositor places the overlay, since there is no gtk-layer-shell. On Windows it is centered on the display under the cursor, without the original DPI and text-scale adjustments.
- **Overlay window lifetime.** The overlay window opens on each show and closes 300ms after hiding, instead of staying alive hidden. Its pop-in fades without the 0.92 scale, because GPUI cannot scale an element.
- **Right-to-left languages.** Rows, margins and the toggle knob mirror. Text inputs keep left-aligned text, the slider and the recording seek bar stay left to right, and long wrapped Arabic or Hebrew lines can clip at the left edge (GPUI's bidi line wrapping).
- **Shortcut recorder on Windows and Linux.** Key names come from the logical key, while React read the physical key code, so numpad keys and non-US layouts can store different names.
- **Packaging.** Installers and signing are not set up.
