mod handler;
pub mod handy_keys;
pub mod hotkey_impl;
pub mod keys;

use crate::app_handle::AppHandle;
use log::{debug, error, info, warn};
use serde::Serialize;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use crate::settings::APPLE_INTELLIGENCE_DEFAULT_MODEL_ID;
use crate::settings::{
    self, APPLE_INTELLIGENCE_PROVIDER_ID, AutoSubmitKey, ClipboardHandling, KeyboardImplementation,
    LLMPrompt, OverlayPosition, OverlayStyle, PasteMethod, ShortcutActivation, ShortcutBinding,
    SoundTheme, Theme, VadBackend, get_settings,
};
use crate::tray;

pub fn init_shortcuts(app: &AppHandle) {
    let user_settings = settings::load_or_create_app_settings(app);

    match user_settings.keyboard_implementation {
        KeyboardImplementation::Tauri => {
            hotkey_impl::init_shortcuts(app);
        }
        KeyboardImplementation::HandyKeys => {
            if let Err(e) = handy_keys::init_shortcuts(app) {
                error!("Failed to initialize handy-keys shortcuts: {}", e);
                warn!(
                    "Falling back to Tauri global shortcut implementation and saving fallback to settings"
                );

                settings::update(app, |settings| {
                    settings.keyboard_implementation = KeyboardImplementation::Tauri;
                });

                hotkey_impl::init_shortcuts(app);
            }
        }
    }
}

pub fn register_cancel_shortcut(app: &AppHandle) {
    crate::secure_input::register_cancel_fallback(app);

    let settings = get_settings(app);
    match settings.keyboard_implementation {
        KeyboardImplementation::Tauri => hotkey_impl::register_cancel_shortcut(app),
        KeyboardImplementation::HandyKeys => handy_keys::register_cancel_shortcut(app),
    }
}

pub fn unregister_cancel_shortcut(app: &AppHandle) {
    crate::secure_input::unregister_cancel_fallback(app);

    let settings = get_settings(app);
    match settings.keyboard_implementation {
        KeyboardImplementation::Tauri => hotkey_impl::unregister_cancel_shortcut(app),
        KeyboardImplementation::HandyKeys => handy_keys::unregister_cancel_shortcut(app),
    }
}

pub fn registers_globally(id: &str) -> bool {
    id != "cancel" && id != crate::annotate::CANCEL_BINDING
}

pub fn register_shortcut(app: &AppHandle, binding: ShortcutBinding) -> Result<(), String> {
    let settings = get_settings(app);
    match settings.keyboard_implementation {
        KeyboardImplementation::Tauri => hotkey_impl::register_shortcut(app, binding),
        KeyboardImplementation::HandyKeys => handy_keys::register_shortcut(app, binding),
    }
}

pub fn unregister_shortcut(app: &AppHandle, binding: ShortcutBinding) -> Result<(), String> {
    let settings = get_settings(app);
    match settings.keyboard_implementation {
        KeyboardImplementation::Tauri => hotkey_impl::unregister_shortcut(app, binding),
        KeyboardImplementation::HandyKeys => handy_keys::unregister_shortcut(app, binding),
    }
}

#[derive(Serialize)]
pub struct BindingResponse {
    pub success: bool,
    pub error: Option<String>,
}

pub fn change_binding(
    app: AppHandle,
    id: String,
    binding: String,
) -> Result<BindingResponse, String> {
    if binding.trim().is_empty() {
        return Err("Binding cannot be empty".to_string());
    }

    let mut settings = settings::get_settings(&app);

    let Some(binding_to_modify) = settings.bindings.get(&id).cloned() else {
        let error_msg = format!("Binding with id '{}' not found", id);
        warn!("change_binding error: {}", error_msg);
        return Ok(BindingResponse {
            success: false,
            error: Some(error_msg),
        });
    };

    validate_shortcut_for_implementation(&binding, settings.keyboard_implementation)?;
    if handy_keys::is_mouse_shortcut(&binding) {
        let requested = binding.parse::<keys::Hotkey>().ok();
        if settings.bindings.iter().any(|(other_id, other)| {
            other_id != &id && other.current_binding.parse::<keys::Hotkey>().ok() == requested
        }) {
            return Err(format!("Shortcut '{}' is already in use", binding));
        }
    }

    if crate::annotate::clashes_with_cancel(&settings, &id, &binding) {
        return Err(format!("Shortcut '{}' is already in use", binding));
    }

    if id == crate::annotate::CANCEL_BINDING {
        if crate::annotate::capture::shortcut_from_binding(&binding).is_none() {
            return Err(
                "Annotate's cancel shortcut must be a key, with or without modifiers".into(),
            );
        }
        let mut b = binding_to_modify;
        b.current_binding = binding;
        settings.bindings.insert(id.clone(), b.clone());
        crate::annotate::sync_shortcuts(&settings);
        settings::update(&app, |settings| {
            settings.bindings.insert(id, b);
        });
        return Ok(BindingResponse {
            success: true,
            error: None,
        });
    }

    if id == "cancel"
        && let Some(mut b) = settings.bindings.get(&id).cloned()
    {
        b.current_binding = binding;
        settings::update(&app, |settings| {
            settings.bindings.insert(id.clone(), b);
        });
        crate::secure_input::reconcile_fallback(&app);
        return Ok(BindingResponse {
            success: true,
            error: None,
        });
    }

    if let Err(e) = unregister_shortcut(&app, binding_to_modify.clone()) {
        let error_msg = format!("Failed to unregister shortcut: {}", e);
        error!("change_binding error: {}", error_msg);
    }

    let mut updated_binding = binding_to_modify.clone();
    updated_binding.current_binding = binding;

    if let Err(e) = register_shortcut(&app, updated_binding.clone()) {
        let error_msg = format!("Failed to register shortcut: {}", e);
        error!("change_binding error: {}", error_msg);
        restore_registration(&app, &binding_to_modify);
        return Ok(BindingResponse {
            success: false,
            error: Some(error_msg),
        });
    }

    settings
        .bindings
        .insert(id.clone(), updated_binding.clone());

    if crate::annotate::is_annotate_binding(&id) {
        crate::annotate::sync_shortcuts(&settings);
    }
    settings::update(&app, |settings| {
        settings.bindings.insert(id.clone(), updated_binding);
    });
    crate::secure_input::reconcile_fallback(&app);

    Ok(BindingResponse {
        success: true,
        error: None,
    })
}

fn restore_registration(app: &AppHandle, binding: &ShortcutBinding) {
    if let Err(e) = register_shortcut(app, binding.clone()) {
        error!(
            "Failed to restore previous binding '{}' ({}): {}",
            binding.id, binding.current_binding, e
        );
    }
}

pub fn reset_binding(app: AppHandle, id: String) -> Result<BindingResponse, String> {
    let binding = settings::get_stored_binding(&settings::get_settings(&app), &id)?;
    change_binding(app, id, binding.default_binding)
}

fn startup_bindings(app: &AppHandle) -> Vec<(String, ShortcutBinding)> {
    let user_settings = settings::load_or_create_app_settings(app);
    settings::get_default_settings()
        .bindings
        .into_iter()
        .filter(|(id, _)| {
            registers_globally(id)
                && (id != "transcribe_with_post_process" || user_settings.post_process_enabled)
        })
        .map(|(id, default_binding)| {
            let binding = user_settings
                .bindings
                .get(&id)
                .cloned()
                .unwrap_or(default_binding);
            (id, binding)
        })
        .collect()
}

pub fn suspend_all_shortcuts(app: &AppHandle) {
    crate::annotate::capture::pause_shortcuts(true);
    for (id, binding) in settings::get_bindings(app) {
        if !registers_globally(&id) {
            continue;
        }
        if let Err(e) = unregister_shortcut(app, binding) {
            debug!(
                "suspend_all_shortcuts: could not unregister '{}': {}",
                id, e
            );
        }
    }
}

pub fn resume_all_shortcuts(app: &AppHandle) {
    crate::annotate::capture::pause_shortcuts(false);
    let settings = get_settings(app);
    for (id, binding) in &settings.bindings {
        if !registers_globally(id) {
            continue;
        }
        if id == "transcribe_with_post_process" && !settings.post_process_enabled {
            continue;
        }
        if let Err(e) = register_shortcut(app, binding.clone()) {
            debug!("resume_all_shortcuts: could not register '{}': {}", id, e);
        }
    }
}

#[derive(Serialize)]
pub struct ImplementationChangeResult {
    pub reset_bindings: Vec<String>,
}

pub fn change_keyboard_implementation_setting(
    app: AppHandle,
    implementation: String,
) -> Result<ImplementationChangeResult, String> {
    let current_settings = settings::get_settings(&app);
    let current_impl = current_settings.keyboard_implementation;
    let new_impl = parse_keyboard_implementation(&implementation);

    if current_impl == new_impl {
        return Ok(ImplementationChangeResult {
            reset_bindings: vec![],
        });
    }

    info!(
        "Switching keyboard implementation from {:?} to {:?}",
        current_impl, new_impl
    );

    unregister_all_shortcuts(&app, current_impl);

    settings::update(&app, |settings| settings.keyboard_implementation = new_impl);

    if new_impl == KeyboardImplementation::Tauri {
        crate::secure_input::reconcile_fallback(&app);
    }

    if new_impl == KeyboardImplementation::HandyKeys && initialize_handy_keys_with_rollback(&app)? {
        crate::secure_input::reconcile_fallback(&app);
        return Ok(ImplementationChangeResult {
            reset_bindings: vec![],
        });
    }

    let reset_bindings = register_all_shortcuts_for_implementation(&app, new_impl);
    crate::secure_input::reconcile_fallback(&app);

    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "keyboard_implementation",
            "value": implementation,
            "reset_bindings": reset_bindings
        }),
    );

    info!("Keyboard implementation switched to {:?}", new_impl);

    Ok(ImplementationChangeResult { reset_bindings })
}

fn validate_shortcut_for_implementation(
    raw: &str,
    implementation: KeyboardImplementation,
) -> Result<(), String> {
    match implementation {
        KeyboardImplementation::Tauri => hotkey_impl::validate_shortcut(raw),
        KeyboardImplementation::HandyKeys => handy_keys::validate_shortcut(raw),
    }
}

fn parse_keyboard_implementation(s: &str) -> KeyboardImplementation {
    match s {
        "tauri" => KeyboardImplementation::Tauri,
        "handy_keys" => KeyboardImplementation::HandyKeys,
        other => {
            warn!(
                "Invalid keyboard implementation '{}', defaulting to tauri",
                other
            );
            KeyboardImplementation::Tauri
        }
    }
}

fn unregister_all_shortcuts(app: &AppHandle, implementation: KeyboardImplementation) {
    let bindings = settings::get_bindings(app);

    for (id, binding) in bindings {
        if !registers_globally(&id) {
            continue;
        }

        let result = match implementation {
            KeyboardImplementation::Tauri => hotkey_impl::unregister_shortcut(app, binding),
            KeyboardImplementation::HandyKeys => handy_keys::unregister_shortcut(app, binding),
        };

        if let Err(e) = result {
            warn!(
                "Failed to unregister shortcut '{}' during switch: {}",
                id, e
            );
        }
    }
}

fn register_all_shortcuts_for_implementation(
    app: &AppHandle,
    implementation: KeyboardImplementation,
) -> Vec<String> {
    let mut reset_bindings = Vec::new();
    let default_bindings = settings::get_default_settings().bindings;
    let mut current_settings = settings::get_settings(app);

    for (id, default_binding) in &default_bindings {
        if !registers_globally(id) {
            continue;
        }

        if id == "transcribe_with_post_process" && !current_settings.post_process_enabled {
            continue;
        }

        let mut binding = current_settings
            .bindings
            .get(id)
            .cloned()
            .unwrap_or_else(|| default_binding.clone());

        if let Err(e) =
            validate_shortcut_for_implementation(&binding.current_binding, implementation)
        {
            info!(
                "Shortcut '{}' ({}) is invalid for {:?}: {}. Resetting to default.",
                id, binding.current_binding, implementation, e
            );

            binding.current_binding = default_binding.current_binding.clone();
            current_settings
                .bindings
                .insert(id.clone(), binding.clone());
            reset_bindings.push(id.clone());
        }

        let result = match implementation {
            KeyboardImplementation::Tauri => hotkey_impl::register_shortcut(app, binding),
            KeyboardImplementation::HandyKeys => handy_keys::register_shortcut(app, binding),
        };

        if let Err(e) = result {
            error!(
                "Failed to register shortcut '{}' for {:?}: {}",
                id, implementation, e
            );
        }
    }

    if !reset_bindings.is_empty() {
        settings::update(app, |settings| {
            for id in &reset_bindings {
                if let Some(binding) = current_settings.bindings.remove(id) {
                    settings.bindings.insert(id.clone(), binding);
                }
            }
        });
    }

    reset_bindings
}

fn initialize_handy_keys_with_rollback(app: &AppHandle) -> Result<bool, String> {
    if app.try_state::<handy_keys::HandyKeysState>().is_some() {
        return Ok(false);
    }

    if let Err(e) = handy_keys::init_shortcuts(app) {
        error!("Failed to initialize HandyKeys: {}", e);
        settings::update(app, |settings| {
            settings.keyboard_implementation = KeyboardImplementation::Tauri;
        });
        crate::secure_input::reconcile_fallback(app);
        hotkey_impl::init_shortcuts(app);
        return Err(format!(
            "Failed to initialize HandyKeys: {}. Reverted to Tauri.",
            e
        ));
    }

    Ok(true)
}

pub fn change_shortcut_activation_setting(
    app: AppHandle,
    activation: ShortcutActivation,
) -> Result<(), String> {
    settings::update(&app, |settings| settings.shortcut_activation = activation);
    Ok(())
}

pub fn change_annotate_shortcut_activation_setting(
    app: AppHandle,
    activation: ShortcutActivation,
) -> Result<(), String> {
    settings::update(&app, |settings| {
        settings.annotate_shortcut_activation = activation;
    });
    Ok(())
}

pub fn change_hold_threshold_ms_setting(app: AppHandle, ms: u64) -> Result<(), String> {
    settings::update(&app, |settings| settings.hold_threshold_ms = ms);
    Ok(())
}

pub fn change_audio_feedback_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::update(&app, |settings| settings.audio_feedback = enabled);
    Ok(())
}

pub fn change_audio_feedback_volume_setting(app: AppHandle, volume: f32) -> Result<(), String> {
    settings::update(&app, |settings| settings.audio_feedback_volume = volume);
    Ok(())
}

pub fn change_sound_theme_setting(app: AppHandle, theme: String) -> Result<(), String> {
    let parsed = match theme.as_str() {
        "marimba" => SoundTheme::Marimba,
        "pop" => SoundTheme::Pop,
        "custom" => SoundTheme::Custom,
        other => {
            warn!("Invalid sound theme '{}', defaulting to marimba", other);
            SoundTheme::Marimba
        }
    };
    settings::update(&app, |settings| settings.sound_theme = parsed);
    Ok(())
}

pub fn change_theme_setting(app: AppHandle, theme: String) -> Result<(), String> {
    let parsed = match theme.as_str() {
        "system" => Theme::System,
        "light" => Theme::Light,
        "dark" => Theme::Dark,
        other => {
            warn!("Invalid theme '{}', defaulting to system", other);
            Theme::System
        }
    };
    settings::update(&app, |settings| settings.theme = parsed);
    apply_window_theme(&app, parsed);
    Ok(())
}

pub fn apply_window_theme(app: &AppHandle, theme: Theme) {
    app.with_ui(move |cx| crate::ui::theme::apply_theme_setting(theme, cx));
}

pub fn change_translate_to_english_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::update(&app, |settings| settings.translate_to_english = enabled);
    Ok(())
}

pub fn change_selected_language_setting(app: AppHandle, language: String) -> Result<(), String> {
    settings::update(&app, |settings| settings.selected_language = language);
    Ok(())
}

pub fn change_overlay_position_setting(app: AppHandle, position: String) -> Result<(), String> {
    let parsed = match position.as_str() {
        "none" | "bottom" => OverlayPosition::Bottom,
        "top" => OverlayPosition::Top,
        other => {
            warn!("Invalid overlay position '{}', defaulting to bottom", other);
            OverlayPosition::Bottom
        }
    };
    settings::update(&app, |settings| settings.overlay_position = parsed);

    crate::utils::update_overlay_position(&app);

    Ok(())
}

pub fn change_overlay_style_setting(app: AppHandle, style: String) -> Result<(), String> {
    let parsed = match style.as_str() {
        "none" => OverlayStyle::None,
        "minimal" => OverlayStyle::Minimal,
        "live" => OverlayStyle::Live,
        other => {
            warn!("Invalid overlay style '{}', defaulting to minimal", other);
            OverlayStyle::Minimal
        }
    };
    settings::update(&app, |settings| settings.overlay_style = parsed);

    crate::overlay::update_overlay_enabled_cache(parsed != OverlayStyle::None);

    crate::utils::update_overlay_position(&app);

    Ok(())
}

pub fn change_debug_mode_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::update(&app, |settings| settings.debug_mode = enabled);

    crate::LOG_STREAMING.store(enabled, std::sync::atomic::Ordering::Relaxed);

    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "debug_mode",
            "value": enabled
        }),
    );

    Ok(())
}

pub fn change_start_hidden_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::update(&app, |settings| settings.start_hidden = enabled);

    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "start_hidden",
            "value": enabled
        }),
    );

    Ok(())
}

pub fn change_autostart_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::update(&app, |settings| settings.autostart_enabled = enabled);

    crate::autostart::apply_autostart(&app, enabled);

    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "autostart_enabled",
            "value": enabled
        }),
    );

    Ok(())
}

pub fn change_show_whats_new_on_update_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    settings::update(&app, |settings| settings.show_whats_new_on_update = enabled);

    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "show_whats_new_on_update",
            "value": enabled
        }),
    );

    Ok(())
}

pub fn change_whats_new_last_seen_version_setting(
    app: AppHandle,
    version: String,
) -> Result<(), String> {
    let version = version.trim().to_string();
    settings::update(&app, |settings| {
        settings.whats_new_last_seen_version = version.clone()
    });

    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "whats_new_last_seen_version",
            "value": version
        }),
    );

    Ok(())
}

pub fn update_custom_words(app: AppHandle, words: Vec<String>) -> Result<(), String> {
    settings::update(&app, |settings| settings.custom_words = words);
    Ok(())
}

pub fn update_snippets(app: AppHandle, snippets: Vec<settings::Snippet>) -> Result<(), String> {
    settings::update(&app, |settings| settings.snippets = snippets);
    Ok(())
}

pub fn update_app_styles(app: AppHandle, rules: Vec<settings::AppStyleRule>) -> Result<(), String> {
    settings::update(&app, |settings| settings.app_styles = rules);
    Ok(())
}

pub fn update_default_style(app: AppHandle, style: settings::WritingStyle) -> Result<(), String> {
    settings::update(&app, |settings| settings.default_style = style);
    Ok(())
}

pub fn change_word_correction_threshold_setting(
    app: AppHandle,
    threshold: f64,
) -> Result<(), String> {
    settings::update(&app, |settings| {
        settings.word_correction_threshold = threshold
    });
    Ok(())
}

pub fn change_extra_recording_buffer_setting(app: AppHandle, ms: u64) -> Result<(), String> {
    settings::update(&app, |settings| settings.extra_recording_buffer_ms = ms);
    Ok(())
}

pub fn change_paste_delay_ms_setting(app: AppHandle, ms: u64) -> Result<(), String> {
    settings::update(&app, |settings| settings.paste_delay_ms = ms);
    Ok(())
}

pub fn change_paste_delay_after_ms_setting(app: AppHandle, ms: u64) -> Result<(), String> {
    settings::update(&app, |settings| settings.paste_delay_after_ms = ms);
    Ok(())
}

pub fn change_reliable_paste_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::update(&app, |settings| settings.reliable_paste = enabled);
    Ok(())
}

pub fn change_paste_method_setting(app: AppHandle, method: String) -> Result<(), String> {
    let parsed = match method.as_str() {
        "ctrl_v" => PasteMethod::CtrlV,
        "direct" => PasteMethod::Direct,
        "none" => PasteMethod::None,
        other => {
            warn!("Invalid paste method '{}', defaulting to ctrl_v", other);
            PasteMethod::CtrlV
        }
    };
    settings::update(&app, |settings| settings.paste_method = parsed);
    Ok(())
}

pub fn change_clipboard_handling_setting(app: AppHandle, handling: String) -> Result<(), String> {
    let parsed = match handling.as_str() {
        "dont_modify" => ClipboardHandling::DontModify,
        "copy_to_clipboard" => ClipboardHandling::CopyToClipboard,
        other => {
            warn!(
                "Invalid clipboard handling '{}', defaulting to dont_modify",
                other
            );
            ClipboardHandling::DontModify
        }
    };
    settings::update(&app, |settings| settings.clipboard_handling = parsed);
    Ok(())
}

pub fn change_auto_submit_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::update(&app, |settings| settings.auto_submit = enabled);
    Ok(())
}

pub fn change_voice_submit_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::update(&app, |settings| settings.voice_submit_enabled = enabled);
    Ok(())
}

pub fn normalize_keyword(raw: &str) -> Option<String> {
    let words: Vec<&str> = raw.split_whitespace().collect();
    let valid = !words.is_empty()
        && words
            .iter()
            .all(|word| word.chars().any(char::is_alphanumeric));
    valid.then(|| words.join(" "))
}

pub fn change_voice_submit_keyword_setting(app: AppHandle, keyword: String) -> Result<(), String> {
    let keyword =
        normalize_keyword(&keyword).ok_or("Voice submit keyword must contain letters or digits")?;
    settings::update(&app, |settings| settings.voice_submit_keyword = keyword);
    Ok(())
}

pub fn change_scratch_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::update(&app, |settings| settings.scratch_enabled = enabled);
    Ok(())
}

pub fn change_scratch_keyword_setting(app: AppHandle, keyword: String) -> Result<(), String> {
    let keyword =
        normalize_keyword(&keyword).ok_or("Scratch keyword must contain letters or digits")?;
    settings::update(&app, |settings| settings.scratch_keyword = keyword);
    Ok(())
}

pub fn change_scrub_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::update(&app, |settings| settings.scrub_enabled = enabled);
    Ok(())
}

pub fn change_scrub_keyword_setting(app: AppHandle, keyword: String) -> Result<(), String> {
    let keyword =
        normalize_keyword(&keyword).ok_or("Scrub keyword must contain letters or digits")?;
    settings::update(&app, |settings| settings.scrub_keyword = keyword);
    Ok(())
}

pub fn change_auto_submit_key_setting(app: AppHandle, key: String) -> Result<(), String> {
    let parsed = match key.as_str() {
        "enter" => AutoSubmitKey::Enter,
        "ctrl_enter" => AutoSubmitKey::CtrlEnter,
        "cmd_enter" => AutoSubmitKey::CmdEnter,
        other => {
            warn!("Invalid auto submit key '{}', defaulting to enter", other);
            AutoSubmitKey::Enter
        }
    };
    settings::update(&app, |settings| settings.auto_submit_key = parsed);
    Ok(())
}

pub fn change_post_process_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let binding = settings::update(&app, |settings| {
        settings.post_process_enabled = enabled;
        settings
            .bindings
            .get("transcribe_with_post_process")
            .cloned()
    });

    if let Some(binding) = binding {
        if enabled {
            let _ = register_shortcut(&app, binding);
        } else {
            let _ = unregister_shortcut(&app, binding);
        }
    }

    crate::secure_input::reconcile_fallback(&app);
    Ok(())
}

pub fn change_experimental_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::update(&app, |settings| settings.experimental_enabled = enabled);
    Ok(())
}

pub fn change_post_process_base_url_setting(
    app: AppHandle,
    provider_id: String,
    base_url: String,
) -> Result<(), String> {
    settings::update(&app, |settings| {
        let label = settings
            .post_process_provider(&provider_id)
            .map(|provider| provider.label.clone())
            .ok_or_else(|| format!("Provider '{}' not found", provider_id))?;

        let provider = settings
            .post_process_provider_mut(&provider_id)
            .expect("Provider looked up above must exist");

        if provider.id != "custom" {
            return Err(format!(
                "Provider '{}' does not allow editing the base URL",
                label
            ));
        }

        provider.base_url = base_url;
        Ok(())
    })
}

fn validate_provider_exists(
    settings: &settings::AppSettings,
    provider_id: &str,
) -> Result<(), String> {
    if !settings
        .post_process_providers
        .iter()
        .any(|provider| provider.id == provider_id)
    {
        return Err(format!("Provider '{}' not found", provider_id));
    }
    Ok(())
}

pub fn change_post_process_api_key_setting(
    app: AppHandle,
    provider_id: String,
    api_key: String,
) -> Result<(), String> {
    settings::update(&app, |settings| {
        validate_provider_exists(settings, &provider_id)?;
        settings.post_process_api_keys.insert(provider_id, api_key);
        Ok(())
    })
}

pub fn change_post_process_model_setting(
    app: AppHandle,
    provider_id: String,
    model: String,
) -> Result<(), String> {
    settings::update(&app, |settings| {
        validate_provider_exists(settings, &provider_id)?;
        settings.post_process_models.insert(provider_id, model);
        Ok(())
    })
}

pub fn set_post_process_provider(app: AppHandle, provider_id: String) -> Result<(), String> {
    settings::update(&app, |settings| {
        validate_provider_exists(settings, &provider_id)?;
        settings.post_process_provider_id = provider_id;
        Ok(())
    })
}

pub fn add_post_process_prompt(
    app: AppHandle,
    name: String,
    prompt: String,
) -> Result<LLMPrompt, String> {
    let id = format!("prompt_{}", chrono::Utc::now().timestamp_millis());

    let new_prompt = LLMPrompt {
        id: id.clone(),
        name,
        prompt,
    };

    settings::update(&app, |settings| {
        settings.post_process_prompts.push(new_prompt.clone());
    });

    Ok(new_prompt)
}

pub fn update_post_process_prompt(
    app: AppHandle,
    id: String,
    name: String,
    prompt: String,
) -> Result<(), String> {
    settings::update(&app, |settings| {
        let existing_prompt = settings
            .post_process_prompts
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("Prompt with id '{}' not found", id))?;
        existing_prompt.name = name;
        existing_prompt.prompt = prompt;
        Ok(())
    })
}

pub fn delete_post_process_prompt(app: AppHandle, id: String) -> Result<(), String> {
    settings::update(&app, |settings| {
        if settings.post_process_prompts.len() <= 1 {
            return Err("Cannot delete the last prompt".to_string());
        }

        let original_len = settings.post_process_prompts.len();
        settings.post_process_prompts.retain(|p| p.id != id);

        if settings.post_process_prompts.len() == original_len {
            return Err(format!("Prompt with id '{}' not found", id));
        }

        if settings.post_process_selected_prompt_id.as_ref() == Some(&id) {
            settings.post_process_selected_prompt_id =
                settings.post_process_prompts.first().map(|p| p.id.clone());
        }

        Ok(())
    })
}

pub async fn fetch_post_process_models(
    app: AppHandle,
    provider_id: String,
) -> Result<Vec<String>, String> {
    let settings = settings::get_settings(&app);

    let provider = settings
        .post_process_providers
        .iter()
        .find(|p| p.id == provider_id)
        .ok_or_else(|| format!("Provider '{}' not found", provider_id))?;

    if provider.id == APPLE_INTELLIGENCE_PROVIDER_ID {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            return Ok(vec![APPLE_INTELLIGENCE_DEFAULT_MODEL_ID.to_string()]);
        }

        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            return Err("Apple Intelligence is only available on Apple silicon Macs running macOS 15 or later.".to_string());
        }
    }

    let api_key = settings
        .post_process_api_keys
        .get(&provider_id)
        .cloned()
        .unwrap_or_default();

    if api_key.trim().is_empty() && provider.id != "custom" {
        return Err(format!(
            "API key is required for {}. Please add an API key to list available models.",
            provider.label
        ));
    }

    crate::llm_client::fetch_models(provider, api_key).await
}

pub fn set_post_process_selected_prompt(app: AppHandle, id: String) -> Result<(), String> {
    settings::update(&app, |settings| {
        if !settings.post_process_prompts.iter().any(|p| p.id == id) {
            return Err(format!("Prompt with id '{}' not found", id));
        }

        settings.post_process_selected_prompt_id = Some(id);
        Ok(())
    })
}

pub fn change_recording_output_volume_setting(app: AppHandle, volume: f32) -> Result<(), String> {
    settings::update(&app, |settings| settings.recording_output_volume = volume);
    Ok(())
}

pub fn change_append_trailing_space_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::update(&app, |settings| settings.append_trailing_space = enabled);
    Ok(())
}

pub fn change_lazy_stream_close_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::update(&app, |settings| settings.lazy_stream_close = enabled);
    Ok(())
}

pub fn change_vad_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::update(&app, |settings| settings.vad_enabled = enabled);
    Ok(())
}

pub async fn change_vad_backend_setting(app: AppHandle, backend: VadBackend) -> Result<(), String> {
    if settings::get_settings(&app).vad_backend == backend {
        return Ok(());
    }

    let manager = app
        .state::<std::sync::Arc<crate::managers::audio::AudioRecordingManager>>()
        .inner()
        .clone();
    tokio::task::spawn_blocking(move || manager.update_vad_backend(backend))
        .await
        .map_err(|e| format!("audio task join failed: {e}"))?
        .map_err(|e| format!("Failed to update VAD backend: {e}"))?;

    settings::update(&app, |settings| settings.vad_backend = backend);
    Ok(())
}

pub fn change_filler_word_removal_enabled_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    settings::update(&app, |settings| {
        settings.filler_word_removal_enabled = enabled
    });
    Ok(())
}

pub fn change_app_language_setting(app: AppHandle, language: String) -> Result<(), String> {
    settings::update(&app, |settings| settings.app_language = language.clone());

    tray::update_tray_menu(&app);

    Ok(())
}

pub fn change_show_tray_icon_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::update(&app, |settings| settings.show_tray_icon = enabled);

    tray::set_tray_visibility(&app, enabled);

    Ok(())
}

pub fn change_transcribe_accelerator_setting(
    app: AppHandle,
    accelerator: settings::TranscribeAcceleratorSetting,
    device: Option<String>,
) -> Result<(), String> {
    settings::update(&app, |settings| {
        settings.transcribe_accelerator = accelerator;
        settings.transcribe_gpu_device = device;
    });
    Ok(())
}

pub fn change_ort_accelerator_setting(
    app: AppHandle,
    accelerator: settings::OrtAcceleratorSetting,
) -> Result<(), String> {
    settings::update(&app, |settings| settings.ort_accelerator = accelerator);
    Ok(())
}

pub async fn get_available_accelerators() -> crate::managers::transcription::AvailableAccelerators {
    crate::runtime::spawn_blocking(crate::managers::transcription::get_available_accelerators)
        .await
        .expect("get_available_accelerators panicked")
}

#[cfg(test)]
mod tests {
    #[test]
    fn keyword_needs_a_letter_or_digit_per_word() {
        use super::normalize_keyword;
        assert_eq!(
            normalize_keyword("scrub  that ").as_deref(),
            Some("scrub that")
        );
        assert_eq!(normalize_keyword("go 2").as_deref(), Some("go 2"));
        assert_eq!(normalize_keyword("   "), None);
        assert_eq!(normalize_keyword("scrub !!"), None);
    }

    use super::keys::Hotkey;
    use global_hotkey::hotkey::HotKey as Shortcut;

    #[test]
    fn compound_shortcut_keys_parse_on_both_backends() {
        for key in [
            "scrolllock",
            "capslock",
            "numlock",
            "pageup",
            "pagedown",
            "printscreen",
        ] {
            assert!(
                key.parse::<Shortcut>().is_ok(),
                "global-hotkey rejected {key}"
            );
            assert!(key.parse::<Hotkey>().is_ok(), "HandyKeys rejected {key}");
        }
    }
}
