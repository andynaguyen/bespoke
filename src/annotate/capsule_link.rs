use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::annotate::coordinator::SelectionMode;

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

#[derive(Clone, Debug)]
pub enum CapsuleCommand {
    Sync(PillSnapshot),
    Place([f64; 2]),
}

#[derive(Debug)]
pub struct CapsuleState {
    pub hidden: AtomicBool,
    pub hovered: AtomicBool,
    pub window_number: AtomicU32,
}

pub struct CapsuleLink {
    pub commands: flume::Receiver<CapsuleCommand>,
    pub actions: flume::Sender<PillAction>,
    pub state: Arc<CapsuleState>,
    pub excluded_window_ids: Arc<Mutex<Vec<u32>>>,
}

type Host = Box<dyn Fn(CapsuleLink) + Send + Sync>;

static HOST: OnceLock<Host> = OnceLock::new();

pub fn set_host(host: impl Fn(CapsuleLink) + Send + Sync + 'static) {
    let _ = HOST.set(Box::new(host));
}

pub struct NativePill {
    commands: flume::Sender<CapsuleCommand>,
    actions: flume::Receiver<PillAction>,
    state: Arc<CapsuleState>,
    last: Mutex<Option<PillSnapshot>>,
}

impl NativePill {
    pub fn open(excluded_window_ids: Arc<Mutex<Vec<u32>>>) -> Result<Self, String> {
        let host = HOST.get().ok_or("no capsule host is registered")?;
        let (commands_tx, commands_rx) = flume::unbounded();
        let (actions_tx, actions_rx) = flume::unbounded();
        let state = Arc::new(CapsuleState {
            hidden: AtomicBool::new(true),
            hovered: AtomicBool::new(false),
            window_number: AtomicU32::new(0),
        });
        host(CapsuleLink {
            commands: commands_rx,
            actions: actions_tx,
            state: Arc::clone(&state),
            excluded_window_ids,
        });
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
