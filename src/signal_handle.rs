use crate::TranscriptionCoordinator;
use crate::app_handle::AppHandle;
use log::debug;
use log::warn;

use signal_hook::consts::{SIGUSR1, SIGUSR2};
use signal_hook::iterator::Signals;
use std::thread;

pub fn send_transcription_input(app: &AppHandle, binding_id: &str, source: &str) {
    if let Some(c) = app.try_state::<TranscriptionCoordinator>() {
        c.send_external_input(binding_id, source);
    } else {
        warn!("TranscriptionCoordinator not initialized");
    }
}

pub fn setup_signal_handler(app_handle: AppHandle) {
    let mut signals =
        Signals::new([SIGUSR1, SIGUSR2]).expect("failed to register transcription signal handlers");
    debug!("Signal handlers registered (SIGUSR1, SIGUSR2)");
    thread::spawn(move || {
        for sig in signals.forever() {
            let (binding_id, signal_name) = match sig {
                SIGUSR1 => ("transcribe_with_post_process", "SIGUSR1"),
                SIGUSR2 => ("transcribe", "SIGUSR2"),
                _ => continue,
            };
            debug!("Received {signal_name}");
            send_transcription_input(&app_handle, binding_id, signal_name);
        }
    });
}
