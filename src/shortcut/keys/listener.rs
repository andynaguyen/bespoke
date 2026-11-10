//!
//! # Platform Notes
//!
//! - **macOS**: Uses CGEventTap. Requires accessibility permissions.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::shortcut::keys::error::{Error, Result};
use crate::shortcut::keys::types::KeyEvent;

pub use crate::shortcut::keys::platform::state::BlockingHotkeys;

/// Platform-agnostic Keyboard Listener
///
/// Streams all keyboard events. Can optionally block events that match
/// registered hotkeys.
pub struct KeyboardListener {
    event_receiver: Receiver<KeyEvent>,
    _thread_handle: Option<JoinHandle<()>>,
    running: Arc<AtomicBool>,
    /// Backend-specific wakeup invoked on Drop after `running` is cleared.
    /// macOS parks its listener thread in a CFRunLoop with no periodic
    /// wakeups (see run_event_tap), so Drop must stop that loop explicitly.
    stop_wakeup: Box<dyn Fn() + Send>,
}

impl KeyboardListener {
    /// Create a new KeyboardListener (non-blocking mode)
    ///
    /// Events are observed but not blocked. Use this for "record hotkey" UI flows.
    ///
    /// On macOS, this will check for accessibility permissions and fail if not granted.
    pub fn new() -> Result<Self> {
        Self::new_internal(None)
    }

    /// Create a new KeyboardListener with blocking support
    ///
    /// Events matching hotkeys in the provided set will be blocked from reaching
    /// other applications. The set can be modified after creation to add/remove
    /// hotkeys dynamically.
    pub fn new_with_blocking(blocking_hotkeys: BlockingHotkeys) -> Result<Self> {
        Self::new_internal(Some(blocking_hotkeys))
    }

    fn new_internal(blocking_hotkeys: Option<BlockingHotkeys>) -> Result<Self> {
        use crate::shortcut::keys::platform::macos::listener;
        let state = listener::spawn(blocking_hotkeys)?;
        let run_loop = state.run_loop;
        Ok(KeyboardListener {
            event_receiver: state.event_receiver,
            _thread_handle: state.thread_handle,
            running: state.running,
            stop_wakeup: Box::new(move || run_loop.stop()),
        })
    }

    /// Blocking receive with timeout
    ///
    /// Blocks until a key event is received, the timeout expires, or the listener stops.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<KeyEvent> {
        self.event_receiver
            .recv_timeout(timeout)
            .map_err(|e| match e {
                std::sync::mpsc::RecvTimeoutError::Timeout => Error::Timeout,
                std::sync::mpsc::RecvTimeoutError::Disconnected => Error::EventLoopNotRunning,
            })
    }

    /// Non-blocking receive for key events
    ///
    /// Returns `Some(event)` if an event is available, `None` otherwise.
    pub fn try_recv(&self) -> Option<KeyEvent> {
        match self.event_receiver.try_recv() {
            Ok(event) => Some(event),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => None,
        }
    }
}

impl Drop for KeyboardListener {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);

        (self.stop_wakeup)();
        if let Some(handle) = self._thread_handle.take() {
            let _ = handle.join();
        }
    }
}
