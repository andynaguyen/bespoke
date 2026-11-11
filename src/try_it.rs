use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::audio_toolkit::Fix;

/// The event a dictation sends to the Dictionary page in place of pasting.
pub const TAKE_EVENT: &str = "dictionary-take";

static DICTIONARY_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn set_dictionary_active(active: bool) {
    DICTIONARY_ACTIVE.store(active, Ordering::Release);
}

/// Whether a dictation starting now belongs to Try it.
pub fn dictionary_active() -> bool {
    DICTIONARY_ACTIVE.load(Ordering::Acquire)
}

/// One dictation on the Try it panel: the text Bespoke would have pasted, and the dictionary's
/// changes to it, in order.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Take {
    pub text: String,
    pub fixes: Vec<Fix>,
    #[serde(default)]
    pub cut_by_voice: bool,
}
