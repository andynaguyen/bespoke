use crate::app_handle::AppHandle;
use crate::input::{self, EnigoState};
use crate::settings::{AutoSubmitKey, ClipboardHandling, PasteMethod, get_settings};
use crate::system_clipboard::ClipboardSnapshot;
use enigo::{Direction, Enigo, Key, Keyboard};
use log::info;
use std::time::Duration;

fn with_enigo<T>(
    app_handle: &AppHandle,
    f: impl FnOnce(&mut Enigo) -> Result<T, String>,
) -> Result<T, String> {
    let enigo_state = app_handle
        .try_state::<EnigoState>()
        .ok_or("Enigo state not initialized")?;
    let mut enigo = enigo_state
        .0
        .lock()
        .map_err(|e| format!("Failed to lock Enigo: {}", e))?;
    f(&mut enigo)
}

fn write_text_to_clipboard(app_handle: &AppHandle, text: &str) -> Result<(), String> {
    app_handle
        .clipboard()
        .write_text(text)
        .map_err(|e| format!("Failed to write to clipboard: {}", e))
}

fn finish_clipboard_paste(
    paste_result: Result<(), String>,
    paste_delay_after_ms: u64,
    restore_clipboard: impl FnOnce(),
) -> Result<(), String> {
    std::thread::sleep(Duration::from_millis(paste_delay_after_ms));
    restore_clipboard();
    paste_result
}

fn paste_via_clipboard(
    text: &str,
    app_handle: &AppHandle,
    paste_delay_ms: u64,
    paste_delay_after_ms: u64,
) -> Result<(), String> {
    let clipboard = app_handle.clipboard();
    let saved = clipboard.snapshot();

    write_text_to_clipboard(app_handle, text)?;

    std::thread::sleep(Duration::from_millis(paste_delay_ms));

    let paste_result = with_enigo(app_handle, |enigo| input::send_paste_ctrl_v(enigo, 100));

    finish_clipboard_paste(paste_result, paste_delay_after_ms, || match &saved {
        ClipboardSnapshot::Text(text) => {
            let _ = write_text_to_clipboard(app_handle, text);
        }
        other => clipboard.restore(other),
    })
}

fn paste_direct(text: &str, app_handle: &AppHandle) -> Result<(), String> {
    with_enigo(app_handle, |enigo| input::paste_text_direct(enigo, text))
}

pub(crate) fn send_return_key(enigo: &mut Enigo, key_type: AutoSubmitKey) -> Result<(), String> {
    match key_type {
        AutoSubmitKey::Enter => {
            enigo
                .key(Key::Return, Direction::Press)
                .map_err(|e| format!("Failed to press Return key: {}", e))?;
            enigo
                .key(Key::Return, Direction::Release)
                .map_err(|e| format!("Failed to release Return key: {}", e))?;
        }
        AutoSubmitKey::CtrlEnter => {
            enigo
                .key(Key::Control, Direction::Press)
                .map_err(|e| format!("Failed to press Control key: {}", e))?;
            enigo
                .key(Key::Return, Direction::Press)
                .map_err(|e| format!("Failed to press Return key: {}", e))?;
            enigo
                .key(Key::Return, Direction::Release)
                .map_err(|e| format!("Failed to release Return key: {}", e))?;
            enigo
                .key(Key::Control, Direction::Release)
                .map_err(|e| format!("Failed to release Control key: {}", e))?;
        }
        AutoSubmitKey::CmdEnter => {
            enigo
                .key(Key::Meta, Direction::Press)
                .map_err(|e| format!("Failed to press Meta/Cmd key: {}", e))?;
            enigo
                .key(Key::Return, Direction::Press)
                .map_err(|e| format!("Failed to press Return key: {}", e))?;
            enigo
                .key(Key::Return, Direction::Release)
                .map_err(|e| format!("Failed to release Return key: {}", e))?;
            enigo
                .key(Key::Meta, Direction::Release)
                .map_err(|e| format!("Failed to release Meta/Cmd key: {}", e))?;
        }
    }

    Ok(())
}

fn send_trailing_space(app_handle: &AppHandle) {
    if let Err(error) = with_enigo(app_handle, |enigo| {
        enigo
            .key(Key::Space, Direction::Click)
            .map_err(|e| format!("Failed to type trailing space: {}", e))
    }) {
        log::warn!("Paste succeeded, but the trailing space failed: {error}");
    }
}

fn should_send_auto_submit(auto_submit: bool, paste_method: PasteMethod) -> bool {
    auto_submit && paste_method != PasteMethod::None
}

pub enum PasteOutcome {
    Pasted,
    PasteOff,
}

pub fn paste(
    text: String,
    app_handle: AppHandle,
    force_submit: bool,
) -> Result<PasteOutcome, String> {
    let settings = get_settings(&app_handle);
    let auto_submit = settings.auto_submit || force_submit;
    let paste_method = settings.paste_method;
    let paste_delay_ms = settings.paste_delay_ms;
    let paste_delay_after_ms = settings.paste_delay_after_ms;

    // Rich-text editors (Slack, Gmail) trim trailing whitespace from pasted
    // text, so clipboard pastes type the space as a keystroke afterwards.
    let clipboard_paste = paste_method == PasteMethod::CtrlV;
    let type_trailing_space = settings.append_trailing_space && clipboard_paste;
    let text = if settings.append_trailing_space && !type_trailing_space {
        format!("{} ", text)
    } else {
        text
    };

    info!(
        "Using paste method: {:?}, delay before: {}ms, delay after: {}ms",
        paste_method, paste_delay_ms, paste_delay_after_ms
    );

    match paste_method {
        PasteMethod::None => {
            info!("PasteMethod::None selected - skipping paste action");
        }
        PasteMethod::Direct => {
            paste_direct(&text, &app_handle)?;
        }
        PasteMethod::CtrlV => {
            if settings.reliable_paste {
                let reliable_result = with_enigo(&app_handle, |enigo| {
                    crate::paste_tx::try_reliable_paste(
                        &text,
                        &app_handle,
                        &paste_method,
                        enigo,
                        auto_submit,
                        settings.auto_submit_key,
                        settings.clipboard_handling,
                    )
                });
                match reliable_result {
                    Ok(false) => return Err("Couldn't send the paste shortcut".to_string()),
                    Ok(true) => {
                        if type_trailing_space {
                            send_trailing_space(&app_handle);
                        }
                        return Ok(PasteOutcome::Pasted);
                    }
                    Err(e) => {
                        log::warn!("Reliable paste unavailable ({e}); falling back to legacy paste")
                    }
                }
            }
            paste_via_clipboard(&text, &app_handle, paste_delay_ms, paste_delay_after_ms)?;
            if type_trailing_space {
                send_trailing_space(&app_handle);
            }
        }
    }

    if should_send_auto_submit(auto_submit, paste_method) {
        std::thread::sleep(Duration::from_millis(50));
        if let Err(error) = with_enigo(&app_handle, |enigo| {
            send_return_key(enigo, settings.auto_submit_key)
        }) {
            log::warn!("Paste succeeded, but auto-submit failed: {error}");
        }
    }

    if settings.clipboard_handling == ClipboardHandling::CopyToClipboard {
        write_text_to_clipboard(&app_handle, &text)?;
    }

    Ok(if paste_method == PasteMethod::None {
        PasteOutcome::PasteOff
    } else {
        PasteOutcome::Pasted
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn auto_submit_requires_setting_enabled() {
        assert!(!should_send_auto_submit(false, PasteMethod::CtrlV));
        assert!(!should_send_auto_submit(false, PasteMethod::Direct));
    }

    #[test]
    fn auto_submit_skips_none_paste_method() {
        assert!(!should_send_auto_submit(true, PasteMethod::None));
    }

    #[test]
    fn auto_submit_runs_for_active_paste_methods() {
        assert!(should_send_auto_submit(true, PasteMethod::CtrlV));
        assert!(should_send_auto_submit(true, PasteMethod::Direct));
    }

    #[test]
    fn clipboard_is_restored_before_key_injection_error_is_returned() {
        let restored = Cell::new(false);
        let result = finish_clipboard_paste(Err("input failed".into()), 0, || {
            restored.set(true);
        });

        assert_eq!(result.unwrap_err(), "input failed");
        assert!(restored.get());
    }
}
