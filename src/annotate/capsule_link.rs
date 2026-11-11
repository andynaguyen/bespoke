use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::annotate::coordinator::SelectionMode;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pres {
    Hidden,
    Recording { chips: bool, stroke_count: usize },
    Selecting { mode: SelectionMode, hold: bool },
    Processing,
    Completed { copied: bool },
    Failed,
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
    pub shortcut: String,
    pub cancel: String,
}

#[derive(Clone, Debug)]
pub enum CapsuleCommand {
    Sync(PillSnapshot),
    Place([f64; 2]),
}

#[derive(Debug)]
pub struct CapsuleState {
    pub hidden: AtomicBool,
    pub hovered: AtomicBool,
}

pub struct CapsuleLink {
    pub commands: flume::Receiver<CapsuleCommand>,
    pub actions: flume::Sender<PillAction>,
    pub state: Arc<CapsuleState>,
    pub excluded_window_ids: Arc<Mutex<Vec<u32>>>,
}

pub struct NativePill {
    commands: flume::Sender<CapsuleCommand>,
    actions: flume::Receiver<PillAction>,
    state: Arc<CapsuleState>,
    last: Mutex<Option<PillSnapshot>>,
}

impl NativePill {
    pub fn open(excluded_window_ids: Arc<Mutex<Vec<u32>>>) -> Result<Self, String> {
        let app = crate::app_handle::AppHandle::try_global().ok_or("Bespoke isn't running")?;
        let (commands_tx, commands_rx) = flume::unbounded();
        let (actions_tx, actions_rx) = flume::unbounded();
        let state = Arc::new(CapsuleState {
            hidden: AtomicBool::new(true),
            hovered: AtomicBool::new(false),
        });
        crate::annotate::capsule::open(
            app,
            CapsuleLink {
                commands: commands_rx,
                actions: actions_tx,
                state: Arc::clone(&state),
                excluded_window_ids,
            },
        );
        Ok(Self {
            commands: commands_tx,
            actions: actions_rx,
            state,
            last: Mutex::new(None),
        })
    }

    pub fn place_on(&self, point: [f64; 2]) {
        let _ = self.commands.send(CapsuleCommand::Place(point));
    }

    pub fn sync(&self, snapshot: PillSnapshot) {
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if last.as_ref() == Some(&snapshot) {
            return;
        }
        if snapshot.pres != Pres::Hidden {
            self.state.hidden.store(false, Ordering::SeqCst);
        }
        *last = Some(snapshot.clone());
        let _ = self.commands.send(CapsuleCommand::Sync(snapshot));
    }

    pub fn drain_actions(&self) -> Vec<PillAction> {
        self.actions.try_iter().collect()
    }

    pub fn is_fully_hidden(&self) -> bool {
        self.state.hidden.load(Ordering::SeqCst)
    }

    pub fn pointer_on_pill(&self) -> bool {
        self.state.hovered.load(Ordering::SeqCst)
    }
}

/// Shows a capsule with `snapshot` at `anchor`, for dev snapshots. Opens it on first use.
pub fn preview_capsule(snapshot: PillSnapshot, anchor: [f64; 2]) {
    static PREVIEW: Mutex<Option<NativePill>> = Mutex::new(None);
    let mut preview = PREVIEW
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if preview.is_none() {
        *preview = NativePill::open(Arc::new(Mutex::new(Vec::new()))).ok();
    }
    if let Some(pill) = preview.as_ref() {
        pill.place_on(anchor);
        pill.sync(snapshot);
    }
}
