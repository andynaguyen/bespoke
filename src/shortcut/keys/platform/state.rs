use std::collections::HashSet;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use crate::shortcut::keys::types::Hotkey;
use crate::shortcut::keys::types::Key;
use crate::shortcut::keys::types::{KeyEvent, Modifiers};

/// Hotkeys that should be blocked when triggered
pub type BlockingHotkeys = Arc<Mutex<HashSet<Hotkey>>>;

pub struct ListenerState {
    pub event_sender: Sender<KeyEvent>,
    pub current_modifiers: Modifiers,
    pub blocking_hotkeys: Option<BlockingHotkeys>,
}

impl ListenerState {
    pub fn new(event_sender: Sender<KeyEvent>, blocking_hotkeys: Option<BlockingHotkeys>) -> Self {
        Self {
            event_sender,
            current_modifiers: Modifiers::empty(),
            blocking_hotkeys,
        }
    }

    pub fn should_block(&self, modifiers: Modifiers, key: Option<Key>) -> bool {
        let Some(Ok(set)) = self.blocking_hotkeys.as_ref().map(|h| h.lock()) else {
            return false;
        };
        set.iter()
            .any(|h| h.modifiers.matches(modifiers) && h.key == key)
    }
}
