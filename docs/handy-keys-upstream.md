# Shortcut keys and upstream handy-keys

`src/shortcut/keys.rs` and `src/shortcut/keys/` are [handy-keys](https://github.com/handy-computer/handy-keys) 0.3.4 from crates.io (checksum `1a007b6c921d3273fd88aac45b516eee01e5a9dc2d76bb6f1a981500cc0d818a`), MIT licensed (`THIRD_PARTY_NOTICES.md`). They were vendored as a patched crate in `vendor/handy-keys` until they moved into `src/shortcut/`. `src/shortcut/handy_keys.rs` runs Bespoke's shortcuts on them. The setting value `handy_keys` still selects it.

## Where upstream files went

| Upstream | Bespoke |
| --- | --- |
| `src/lib.rs` | `src/shortcut/keys.rs` |
| `src/<path>` | `src/shortcut/keys/<path>` |

To port an upstream commit, apply its `src/` changes at the same paths and replace `crate::` with `crate::shortcut::keys::`.

## Changed from 0.3.4

- Mouse buttons 6 to 8: `Key::Mouse6` to `Key::Mouse8`, macOS button numbers 5 to 7.
- Rust 2024: `unsafe extern` blocks, `unsafe` blocks inside `unsafe fn` bodies, and no `&` or `ref` in patterns that already borrow.
- Removed because Bespoke never calls them: `HotkeyManager::new` (the manager always blocks its hotkeys), `get_hotkey`, `hotkey_count` and both `recv`s, `KeyboardListener::blocking_hotkeys`, `KeyEvent::changed_modifier`, `HotkeyId::as_u32`, `Hotkey::to_lowercase_string`, `open_accessibility_settings`, two error variants, and the serde derives.
- Added `Key::is_mouse_button`, which Bespoke's callers use.
- Dropped every listener and keycode table except `platform/macos`.
- Left out: the README, `TESTING.md`, the examples and the integration tests in `tests/`, which need Accessibility and send synthetic key presses. The unit tests run with `cargo test`.
