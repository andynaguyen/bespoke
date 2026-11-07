//! Global keyboard and mouse shortcuts, from handy-keys 0.3.4 (MIT, `THIRD_PARTY_NOTICES.md`).
//! `HotkeyManager` matches registered hotkeys and blocks them from reaching other apps, and
//! `KeyboardListener` reports every key event, for recording a new shortcut. This copy adds
//! mouse buttons 6 to 8; `docs/handy-keys-upstream.md` lists what else changed.

mod error;
mod listener;
mod manager;
mod platform;
mod types;

pub use listener::KeyboardListener;
pub use manager::HotkeyManager;
pub use types::{Hotkey, HotkeyId, HotkeyState, Key, Modifiers};
