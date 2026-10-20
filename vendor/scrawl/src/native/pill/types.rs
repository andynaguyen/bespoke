//! The native pill's values shared with the coordinator, on every platform.

use super::model::Pres;
use crate::ui::SelectionMode;

/// Capsule visual style (`pillStyle: "black" | "glass"`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PillStyle {
    #[default]
    Black,
    Glass,
}

#[allow(dead_code)]
impl PillStyle {
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "glass" | "liquid" | "liquid-glass" | "1" | "true" => Some(Self::Glass),
            "black" | "dark" | "metal" | "0" | "false" => Some(Self::Black),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Black => "black",
            Self::Glass => "glass",
        }
    }

    /// The style `pillStyle` in Annotate's `config.json` asks for (black unless it says glass).
    /// The capsule asks every frame, so the file is read at most once a second.
    pub fn current() -> Self {
        static CACHED: std::sync::Mutex<Option<(std::time::Instant, PillStyle)>> =
            std::sync::Mutex::new(None);
        let mut cached = CACHED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match *cached {
            Some((read_at, style)) if read_at.elapsed() < std::time::Duration::from_secs(1) => {
                style
            }
            _ => {
                let style = crate::handoff::pill_style();
                *cached = Some((std::time::Instant::now(), style));
                style
            }
        }
    }
}

/// User actions emitted by clicking or pressing keys on the native pill (macOS only, so
/// elsewhere nothing makes them).
#[derive(Clone, Copy, Debug)]
pub enum PillAction {
    SelectMode(SelectionMode),
    SwitchRecording(SelectionMode),
    RecordSelected,
    Undo,
    Cancel,
    Finish,
    Paste(Option<[f32; 2]>),
    Inspect,
    DismissCompleted,
}

/// Coordinator state snapshot synced to `NativePill`.
#[derive(Clone, Debug, PartialEq)]
pub struct PillSnapshot {
    pub pres: Pres,
    pub clock: String,
    pub reduced_motion: bool,
    pub style: PillStyle,
}
