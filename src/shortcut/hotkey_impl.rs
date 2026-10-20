//! Global-hotkey shortcut implementation
//!
//! Registers shortcuts with the OS through the `global-hotkey` crate, the
//! library Tauri's global-shortcut plugin wrapped. The persisted setting value
//! for this backend is still `"tauri"`, so existing installs keep their choice.
//!
//! Registration runs on the main thread: Carbon (macOS) and `RegisterHotKey`
//! (Windows) both bind hotkeys to the main run loop.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use global_hotkey::hotkey::HotKey as Shortcut;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use log::{debug, error, warn};

use crate::app_handle::AppHandle;

#[cfg(not(target_os = "linux"))]
use crate::settings::get_settings;
use crate::settings::{self, ShortcutBinding};

use super::handler::handle_shortcut_event;

/// A registered hotkey and the binding it fires.
struct Registration {
    binding_id: String,
    shortcut: Shortcut,
}

fn registrations() -> &'static Mutex<HashMap<u32, Registration>> {
    static REGISTRATIONS: OnceLock<Mutex<HashMap<u32, Registration>>> = OnceLock::new();
    REGISTRATIONS.get_or_init(Default::default)
}

/// The OS hotkey manager. Created on the main thread by [`install`].
static MANAGER: OnceLock<GlobalHotKeyManager> = OnceLock::new();

/// Creates the hotkey manager and routes its events to the shortcut handler.
/// Must run on the main thread before any shortcut is registered.
pub fn install(app: &AppHandle) {
    match GlobalHotKeyManager::new() {
        Ok(manager) => {
            let _ = MANAGER.set(manager);
        }
        Err(e) => {
            error!("Failed to create the global hotkey manager: {}", e);
            return;
        }
    }

    let app = app.clone();
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        let Some((binding_id, shortcut)) = registrations()
            .lock()
            .unwrap()
            .get(&event.id())
            .map(|r| (r.binding_id.clone(), r.shortcut))
        else {
            return;
        };
        let shortcut_string = shortcut.into_string();
        let is_pressed = event.state() == HotKeyState::Pressed;
        // Mirrors the handy-keys event log line; the distinct prefix
        // makes it possible to tell which backend fired a shortcut
        // (e.g. when diagnosing the Secure Input fallback)
        debug!(
            "global-hotkey event: binding={}, shortcut={}, state={:?}",
            binding_id,
            shortcut_string,
            event.state()
        );
        handle_shortcut_event(&app, &binding_id, &shortcut_string, is_pressed);
    }));
}

fn with_manager<R: Send + 'static>(
    app: &AppHandle,
    f: impl FnOnce(&GlobalHotKeyManager) -> Result<R, String> + Send + 'static,
) -> Result<R, String> {
    app.run_on_main_thread_sync(move || match MANAGER.get() {
        Some(manager) => f(manager),
        None => Err("Global hotkey manager is not available".to_string()),
    })
    .map_err(|e| e.to_string())?
}

/// Initialize shortcuts using the global-hotkey backend
pub fn init_shortcuts(app: &AppHandle) {
    let default_bindings = settings::get_default_settings().bindings;
    let user_settings = settings::load_or_create_app_settings(app);

    // Register all default shortcuts, applying user customizations
    for (id, default_binding) in default_bindings {
        if id == "cancel" {
            continue; // Skip cancel shortcut, it will be registered dynamically
        }
        // Skip post-processing shortcut when the feature is disabled
        if id == "transcribe_with_post_process" && !user_settings.post_process_enabled {
            continue;
        }
        let binding = user_settings
            .bindings
            .get(&id)
            .cloned()
            .unwrap_or(default_binding);

        if let Err(e) = register_shortcut(app, binding) {
            error!("Failed to register shortcut {} during init: {}", id, e);
        }
    }
}

/// Validate a shortcut string for the global-hotkey implementation.
/// global-hotkey requires at least one non-modifier key and doesn't support the fn key.
pub fn validate_shortcut(raw: &str) -> Result<(), String> {
    if super::handy_keys::is_mouse_shortcut(raw) {
        return super::handy_keys::validate_shortcut(raw);
    }
    if raw.trim().is_empty() {
        return Err("Shortcut cannot be empty".into());
    }

    let modifiers = [
        "ctrl", "control", "shift", "alt", "option", "meta", "command", "cmd", "super", "win",
        "windows",
    ];

    // Check for fn key which global-hotkey doesn't support
    let parts: Vec<String> = raw.split('+').map(|p| p.trim().to_lowercase()).collect();
    for part in &parts {
        if part == "fn" || part == "function" {
            return Err("The 'fn' key is not supported by global shortcuts".into());
        }
    }

    // Check for at least one non-modifier key
    let has_non_modifier = parts.iter().any(|part| !modifiers.contains(&part.as_str()));

    if has_non_modifier {
        Ok(())
    } else {
        Err("Global shortcuts must include a main key (letter, number, F-key, etc.) in addition to modifiers".into())
    }
}

/// Register a shortcut with the global-hotkey backend
pub fn register_shortcut(app: &AppHandle, binding: ShortcutBinding) -> Result<(), String> {
    if super::handy_keys::is_mouse_shortcut(&binding.current_binding) {
        return super::handy_keys::register_shortcut(app, binding);
    }
    // Validate for global-hotkey requirements
    if let Err(e) = validate_shortcut(&binding.current_binding) {
        warn!(
            "register_hotkey_shortcut validation error for binding '{}': {}",
            binding.current_binding, e
        );
        return Err(e);
    }

    // Parse shortcut and return error if it fails
    let shortcut = match binding.current_binding.parse::<Shortcut>() {
        Ok(s) => s,
        Err(e) => {
            let error_msg = format!(
                "Failed to parse shortcut '{}': {}",
                binding.current_binding, e
            );
            error!("register_hotkey_shortcut parse error: {}", error_msg);
            return Err(error_msg);
        }
    };

    // Prevent duplicate registrations that would silently shadow one another
    if registrations().lock().unwrap().contains_key(&shortcut.id()) {
        let error_msg = format!("Shortcut '{}' is already in use", binding.current_binding);
        warn!("register_hotkey_shortcut duplicate error: {}", error_msg);
        return Err(error_msg);
    }

    let current_binding = binding.current_binding.clone();
    with_manager(app, move |manager| {
        manager.register(shortcut).map_err(|e| {
            let error_msg = format!("Couldn't register shortcut '{}': {}", current_binding, e);
            error!("register_hotkey_shortcut registration error: {}", error_msg);
            error_msg
        })
    })?;

    registrations().lock().unwrap().insert(
        shortcut.id(),
        Registration {
            binding_id: binding.id,
            shortcut,
        },
    );
    Ok(())
}

/// Unregister a shortcut from the global-hotkey backend
pub fn unregister_shortcut(app: &AppHandle, binding: ShortcutBinding) -> Result<(), String> {
    if super::handy_keys::is_mouse_shortcut(&binding.current_binding) {
        return super::handy_keys::unregister_shortcut(app, binding);
    }
    let shortcut = match binding.current_binding.parse::<Shortcut>() {
        Ok(s) => s,
        Err(e) => {
            let error_msg = format!(
                "Failed to parse shortcut '{}' for unregistration: {}",
                binding.current_binding, e
            );
            error!("unregister_hotkey_shortcut parse error: {}", error_msg);
            return Err(error_msg);
        }
    };

    registrations().lock().unwrap().remove(&shortcut.id());
    let current_binding = binding.current_binding.clone();
    with_manager(app, move |manager| {
        manager.unregister(shortcut).map_err(|e| {
            let error_msg = format!("Failed to unregister shortcut '{}': {}", current_binding, e);
            error!("unregister_hotkey_shortcut error: {}", error_msg);
            error_msg
        })
    })
}

/// Register the cancel shortcut (called when recording starts)
pub fn register_cancel_shortcut(app: &AppHandle) {
    // Cancel shortcut is disabled on Linux due to instability with dynamic shortcut registration
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        return;
    }

    #[cfg(not(target_os = "linux"))]
    {
        let app_clone = app.clone();
        crate::runtime::spawn(async move {
            if let Some(cancel_binding) = get_settings(&app_clone).bindings.get("cancel").cloned() {
                if let Err(e) = register_shortcut(&app_clone, cancel_binding) {
                    error!("Failed to register cancel shortcut: {}", e);
                }
            }
        });
    }
}

/// Unregister the cancel shortcut (called when recording stops)
pub fn unregister_cancel_shortcut(app: &AppHandle) {
    // Cancel shortcut is disabled on Linux due to instability with dynamic shortcut registration
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        return;
    }

    #[cfg(not(target_os = "linux"))]
    {
        let app_clone = app.clone();
        crate::runtime::spawn(async move {
            if let Some(cancel_binding) = get_settings(&app_clone).bindings.get("cancel").cloned() {
                // We ignore errors here as it might already be unregistered
                let _ = unregister_shortcut(&app_clone, cancel_binding);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::validate_shortcut;

    #[test]
    fn mouse_bindings_use_handy_keys_validation() {
        for binding in [
            "MouseMiddle",
            "MouseX1",
            "MouseX2",
            "ctrl+MouseLeft",
            "fn+MouseRight",
        ] {
            assert!(validate_shortcut(binding).is_ok(), "{binding}");
        }
        for binding in ["MouseLeft", "MouseRight", "lmb", "mouse2"] {
            assert_eq!(
                validate_shortcut(binding),
                Err("Left and right mouse buttons require a modifier key".into()),
                "{binding}"
            );
        }
    }

    #[test]
    fn keyboard_bindings_keep_hotkey_requirements() {
        assert!(validate_shortcut("ctrl+Space").is_ok());
        assert!(validate_shortcut("ctrl+shift").is_err());
        assert!(validate_shortcut("fn+Space").is_err());
        assert!(validate_shortcut("").is_err());
    }
}
