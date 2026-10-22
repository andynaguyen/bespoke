//! The capsule: the floating control for picking, recording and handing off a capture.
//!
//! The host draws it. Bespoke registers a host with `host::on_capsule`, which receives a
//! [`CapsuleLink`] for each [`NativePill`] the coordinator opens. The pill sends snapshots and
//! placement over the link and reads clicks back, so the coordinator keeps the same calls it
//! made to the Metal capsule this replaced.

mod types;

pub use self::types::{PillAction, PillSnapshot, Pres};

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// What the coordinator sends the capsule. The link closes when the pill drops.
#[derive(Clone, Debug)]
pub enum CapsuleCommand {
    Sync(PillSnapshot),
    /// Move to the display holding this point (global points, top-left origin).
    Place([f64; 2]),
}

/// State the host writes and the coordinator reads.
#[derive(Debug)]
pub struct CapsuleState {
    /// True once the capsule has faded out after `Pres::Hidden`.
    pub hidden: AtomicBool,
    /// Whether the pointer is over the capsule.
    pub hovered: AtomicBool,
    /// The capsule window's number, 0 until it opens.
    pub window_number: AtomicU32,
}

/// One capsule's connection to the host.
pub struct CapsuleLink {
    pub commands: flume::Receiver<CapsuleCommand>,
    pub actions: flume::Sender<PillAction>,
    pub state: Arc<CapsuleState>,
    /// Windows screen capture leaves out. The host adds the capsule's window once it opens.
    pub excluded_window_ids: Arc<Mutex<Vec<u32>>>,
}

type Host = Box<dyn Fn(CapsuleLink) + Send + Sync>;

static HOST: OnceLock<Host> = OnceLock::new();

/// Registers the host that draws capsules. Only the first call counts.
pub fn set_host(host: impl Fn(CapsuleLink) + Send + Sync + 'static) {
    let _ = HOST.set(Box::new(host));
}

pub struct NativePill {
    commands: flume::Sender<CapsuleCommand>,
    actions: flume::Receiver<PillAction>,
    state: Arc<CapsuleState>,
    /// The last snapshot sent; the coordinator syncs every tick, but most ticks change nothing.
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

    /// Moves the capsule to the display holding `point` (global points, top-left origin).
    pub fn place_on(&self, point: [f64; 2]) {
        let _ = self.commands.send(CapsuleCommand::Place(point));
    }

    pub fn sync(&self, snapshot: PillSnapshot) {
        let mut last = self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if last.as_ref() == Some(&snapshot) {
            return;
        }
        // Visible from now on, so the coordinator never mistakes the moment before the host
        // shows the capsule for the end of its fade.
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
