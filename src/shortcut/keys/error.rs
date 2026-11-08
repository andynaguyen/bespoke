use crate::shortcut::keys::types::HotkeyId;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[cfg(target_os = "macos")]
    #[error(
        "Accessibility permission not granted. Please enable it in System Settings > Privacy & Security > Accessibility"
    )]
    AccessibilityNotGranted,

    #[cfg(target_os = "macos")]
    #[error("Failed to create event tap: {0}")]
    EventTapCreationFailed(String),

    #[error("Hotkey with id {0:?} not found")]
    HotkeyNotFound(HotkeyId),

    #[error("Hotkey already registered: {0}")]
    HotkeyAlreadyRegistered(String),

    #[error("Event loop not running")]
    EventLoopNotRunning,

    #[error("Operation timed out")]
    Timeout,

    #[cfg(target_os = "linux")]
    #[error("Platform error: {0}")]
    Platform(String),

    #[error("Hotkey cannot be empty (must have at least a key or modifiers)")]
    EmptyHotkey,

    #[error("Invalid hotkey format: {0}")]
    InvalidHotkeyFormat(String),

    #[error("Unknown key: {0}")]
    UnknownKey(String),

    #[error("Unknown modifier: {0}")]
    UnknownModifier(String),

    #[error("Internal error: Mutex poisoned")]
    MutexPoisoned,
}

pub type Result<T> = std::result::Result<T, Error>;
