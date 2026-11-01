use crate::ui::SelectionMode;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pres {
    Hidden,
    Recording { chips: bool, stroke_count: usize },
    Selecting { mode: SelectionMode, hold: bool },
    Processing,
    Completed { label: String },
}

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

#[derive(Clone, Debug, PartialEq)]
pub struct PillSnapshot {
    pub pres: Pres,
    pub clock: String,
    pub reduced_motion: bool,
    pub shortcut: String,
}
