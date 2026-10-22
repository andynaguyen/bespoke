//! The capsule's values shared between the coordinator and the host that draws it.

use crate::ui::SelectionMode;

/// What the capsule shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pres {
    Hidden,
    /// `chips`: the Area, Window and Screen chips replace Undo, Cancel and Finish, to switch
    /// what is recorded.
    Recording { chips: bool, stroke_count: usize },
    /// The picker is open on `mode`. `hold`: it is the held switcher, which records on release.
    Selecting { mode: SelectionMode, hold: bool },
    Processing,
    Completed { label: String },
}

/// User actions from clicking the capsule.
#[derive(Clone, Copy, Debug)]
pub enum PillAction {
    SelectMode(SelectionMode),
    SwitchRecording(SelectionMode),
    RecordSelected,
    Undo,
    Cancel,
    Finish,
    /// `Some`: the Paste chip was dragged to this point (global points, top-left origin), and
    /// the capture pastes into whatever is there.
    Paste(Option<[f32; 2]>),
    Inspect,
    DismissCompleted,
}

/// Coordinator state sent to the capsule.
#[derive(Clone, Debug, PartialEq)]
pub struct PillSnapshot {
    pub pres: Pres,
    /// The recording's length so far, or the last recording's once it ends.
    pub clock: String,
    pub reduced_motion: bool,
    /// The record shortcut, like `⌥⇧S`.
    pub shortcut: String,
}
