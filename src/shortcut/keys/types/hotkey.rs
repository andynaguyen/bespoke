use std::fmt;
use std::str::FromStr;

use crate::shortcut::keys::error::{Error, Result};

use super::key::Key;
use super::modifiers::Modifiers;

/// A unique identifier for a registered hotkey
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HotkeyId(pub(crate) u32);

/// A hotkey definition - either a key with modifiers, or modifiers only
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub modifiers: Modifiers,
    pub key: Option<Key>,
}

impl Hotkey {
    /// Create a hotkey with modifiers and/or a key
    ///
    /// At least one of modifiers or key must be provided.
    /// Returns an error if both are empty/None.
    pub fn new(modifiers: Modifiers, key: impl Into<Option<Key>>) -> Result<Self> {
        let key = key.into();
        if modifiers.is_empty() && key.is_none() {
            return Err(Error::EmptyHotkey);
        }
        Ok(Self { modifiers, key })
    }

    /// Format hotkey using Handy-compatible key names (lowercase with full modifier names):
    /// "command", "option", "ctrl", "shift".
    ///
    /// Side-specific modifiers use `_left`/`_right` suffixes.
    pub fn to_handy_string(self) -> String {
        fn mod_names(
            mods: Modifiers,
            left: Modifiers,
            right: Modifiers,
            compound: Modifiers,
            name: &str,
        ) -> Option<String> {
            if mods.contains(compound) {
                Some(name.to_string())
            } else if mods.contains(left) {
                Some(format!("{}_left", name))
            } else if mods.contains(right) {
                Some(format!("{}_right", name))
            } else {
                None
            }
        }

        let mut parts = Vec::new();

        if let Some(s) = mod_names(
            self.modifiers,
            Modifiers::CTRL_LEFT,
            Modifiers::CTRL_RIGHT,
            Modifiers::CTRL,
            "ctrl",
        ) {
            parts.push(s);
        }

        let opt_name = "option";
        if let Some(s) = mod_names(
            self.modifiers,
            Modifiers::OPT_LEFT,
            Modifiers::OPT_RIGHT,
            Modifiers::OPT,
            opt_name,
        ) {
            parts.push(s);
        }

        if let Some(s) = mod_names(
            self.modifiers,
            Modifiers::SHIFT_LEFT,
            Modifiers::SHIFT_RIGHT,
            Modifiers::SHIFT,
            "shift",
        ) {
            parts.push(s);
        }

        let cmd_name = "command";
        if let Some(s) = mod_names(
            self.modifiers,
            Modifiers::CMD_LEFT,
            Modifiers::CMD_RIGHT,
            Modifiers::CMD,
            cmd_name,
        ) {
            parts.push(s);
        }

        if self.modifiers.contains(Modifiers::FN) {
            parts.push("fn".to_string());
        }

        if let Some(key) = &self.key {
            let key_str = key.to_string().to_lowercase();
            let mut result = parts.join("+");
            if !result.is_empty() {
                result.push('+');
            }
            result.push_str(&key_str);
            result
        } else {
            parts.join("+")
        }
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.modifiers.is_empty() {
            if let Some(key) = &self.key {
                write!(f, "{}", key)
            } else {
                write!(f, "(none)")
            }
        } else if let Some(key) = &self.key {
            write!(f, "{}+{}", self.modifiers, key)
        } else {
            write!(f, "{}", self.modifiers)
        }
    }
}

impl FromStr for Hotkey {
    type Err = Error;

    /// Parse a hotkey from a string like "Cmd+Shift+K" or "Ctrl+Space"
    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        if s.is_empty() {
            return Err(Error::EmptyHotkey);
        }

        let parts: Vec<&str> = s.split('+').map(|p| p.trim()).collect();

        let mut modifiers = Modifiers::empty();
        let mut key: Option<Key> = None;

        for part in parts {
            if part.is_empty() {
                continue;
            }

            if let Some(m) = Modifiers::parse_single(part) {
                modifiers |= m;
            } else {
                if key.is_some() {
                    return Err(Error::InvalidHotkeyFormat(format!(
                        "Multiple keys specified: already have a key, found '{}'",
                        part
                    )));
                }
                key = Some(Key::from_str(part)?);
            }
        }

        Hotkey::new(modifiers, key)
    }
}

/// The state of a hotkey (pressed or released)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HotkeyState {
    /// The hotkey was just pressed
    Pressed,
    /// The hotkey was just released
    Released,
}

/// Event emitted when a hotkey is pressed or released
#[derive(Debug, Clone, Copy)]
pub struct HotkeyEvent {
    pub id: HotkeyId,
    pub state: HotkeyState,
}

/// Event emitted during key recording
#[derive(Debug, Clone, Copy)]
pub struct KeyEvent {
    pub modifiers: Modifiers,
    pub key: Option<Key>,
    pub is_key_down: bool,
}

impl KeyEvent {
    /// Convert this key event to a hotkey definition
    pub fn as_hotkey(&self) -> Result<Hotkey> {
        Hotkey::new(self.modifiers, self.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_modifier_plus_key() {
        let hotkey: Hotkey = "Cmd+K".parse().unwrap();
        assert_eq!(hotkey.modifiers, Modifiers::CMD);
        assert_eq!(hotkey.key, Some(Key::K));
    }

    #[test]
    fn parse_multiple_modifiers_plus_key() {
        let hotkey: Hotkey = "Cmd+Shift+K".parse().unwrap();
        assert_eq!(hotkey.modifiers, Modifiers::CMD | Modifiers::SHIFT);
        assert_eq!(hotkey.key, Some(Key::K));

        let hotkey: Hotkey = "Ctrl+Alt+Delete".parse().unwrap();
        assert_eq!(hotkey.modifiers, Modifiers::CTRL | Modifiers::OPT);
        assert_eq!(hotkey.key, Some(Key::Delete));
    }

    #[test]
    fn parse_key_only() {
        let hotkey: Hotkey = "F1".parse().unwrap();
        assert_eq!(hotkey.modifiers, Modifiers::empty());
        assert_eq!(hotkey.key, Some(Key::F1));

        let hotkey: Hotkey = "Space".parse().unwrap();
        assert_eq!(hotkey.modifiers, Modifiers::empty());
        assert_eq!(hotkey.key, Some(Key::Space));
    }

    #[test]
    fn parse_modifiers_only() {
        let hotkey: Hotkey = "Cmd+Shift".parse().unwrap();
        assert_eq!(hotkey.modifiers, Modifiers::CMD | Modifiers::SHIFT);
        assert_eq!(hotkey.key, None);
    }

    #[test]
    fn parse_side_specific_hotkey() {
        let hotkey: Hotkey = "CtrlRight+Space".parse().unwrap();
        assert_eq!(hotkey.modifiers, Modifiers::CTRL_RIGHT);
        assert_eq!(hotkey.key, Some(Key::Space));

        let hotkey: Hotkey = "CmdLeft+ShiftRight+K".parse().unwrap();
        assert_eq!(
            hotkey.modifiers,
            Modifiers::CMD_LEFT | Modifiers::SHIFT_RIGHT
        );
        assert_eq!(hotkey.key, Some(Key::K));
    }

    #[test]
    fn parse_empty_fails() {
        assert!("".parse::<Hotkey>().is_err());
    }

    #[test]
    fn parse_multiple_keys_fails() {
        assert!("A+B".parse::<Hotkey>().is_err());
        assert!("Cmd+A+B".parse::<Hotkey>().is_err());
    }

    #[test]
    fn parse_case_insensitive() {
        let h1: Hotkey = "CMD+SHIFT+K".parse().unwrap();
        let h2: Hotkey = "cmd+shift+k".parse().unwrap();
        let h3: Hotkey = "Cmd+Shift+K".parse().unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h2, h3);
    }

    #[test]
    fn hotkey_display() {
        let hotkey = Hotkey::new(Modifiers::CMD | Modifiers::SHIFT, Key::K).unwrap();
        let displayed = format!("{}", hotkey);
        assert!(displayed.contains("Cmd"));
        assert!(displayed.contains("Shift"));
        assert!(displayed.contains("K"));
    }

    #[test]
    fn hotkey_display_roundtrip_keypad() {
        let keypad_keys = [
            Key::KeypadPlus,
            Key::KeypadMinus,
            Key::KeypadMultiply,
            Key::KeypadDivide,
            Key::KeypadDecimal,
            Key::KeypadEquals,
            Key::KeypadEnter,
            Key::KeypadClear,
        ];
        for key in keypad_keys {
            let hotkey = Hotkey::new(Modifiers::empty(), key).unwrap();
            let displayed = format!("{}", hotkey);
            let parsed: Hotkey = displayed.parse().unwrap_or_else(|e| {
                panic!("Failed to parse '{}' (from {:?}): {}", displayed, key, e)
            });
            assert_eq!(parsed, hotkey, "Key-only roundtrip failed for {:?}", key);

            let hotkey = Hotkey::new(Modifiers::CMD, key).unwrap();
            let displayed = format!("{}", hotkey);
            let parsed: Hotkey = displayed.parse().unwrap_or_else(|e| {
                panic!(
                    "Failed to parse '{}' (from Cmd+{:?}): {}",
                    displayed, key, e
                )
            });
            assert_eq!(parsed, hotkey, "Cmd+{:?} roundtrip failed", key);
        }
    }

    #[test]
    fn hotkey_new_validates() {
        assert!(Hotkey::new(Modifiers::CMD, Key::K).is_ok());
        assert!(Hotkey::new(Modifiers::CMD | Modifiers::SHIFT, None).is_ok());
        assert!(Hotkey::new(Modifiers::empty(), Key::F1).is_ok());

        assert!(Hotkey::new(Modifiers::empty(), None).is_err());
    }
}
