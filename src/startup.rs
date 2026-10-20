//! App startup, ported from the Tauri `setup` hook and `initialize_core_logic`.
//! Runs on the GPUI main thread once the platform app has launched.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use gpui::App;

use crate::app_handle::{AppHandle, UiMessage};
use crate::cli::CliArgs;
use crate::managers::audio::AudioRecordingManager;
use crate::managers::history::HistoryManager;
use crate::managers::model::ModelManager;
use crate::managers::transcription::TranscriptionManager;
use crate::settings::{self, get_settings};
use crate::single_instance;
use crate::{
    autostart, managers, overlay, secure_input, shortcut, signal_handle, tray, ui, utils,
    voice_submit, TranscriptionCoordinator, FILE_LOG_LEVEL, LOG_STREAMING,
};

pub fn run(
    app: AppHandle,
    ui_messages: async_channel::Receiver<UiMessage>,
    listener: single_instance::Listener,
    cx: &mut App,
) {
    #[cfg(target_os = "windows")]
    log::info!(
        "Vulkan layer policy: VK_LOADER_LAYERS_DISABLE={:?}, BESPOKE_KEEP_VULKAN_IMPLICIT_LAYERS={}",
        std::env::var_os("VK_LOADER_LAYERS_DISABLE"),
        utils::env_flag_enabled("BESPOKE_KEEP_VULKAN_IMPLICIT_LAYERS"),
    );

    let cli_args = app.state::<CliArgs>().inner().clone();
    let mut settings = get_settings(&app);

    // CLI --debug overrides debug_mode and log level (runtime-only, not persisted)
    if cli_args.debug {
        settings.debug_mode = true;
        settings.log_level = settings::LogLevel::Trace;
    }

    let file_log_level: log::LevelFilter = settings.log_level.into();
    FILE_LOG_LEVEL.store(file_log_level as u8, Ordering::Relaxed);
    // Only forward logs to the UI while debug mode is on (the live log viewer
    // is the sole consumer and only exists in debug mode). This also honors
    // the runtime `--debug` override applied to `settings` above.
    LOG_STREAMING.store(settings.debug_mode, Ordering::Relaxed);

    ui::init(&app, ui_messages, &settings, cx);

    app.manage(TranscriptionCoordinator::new(app.clone()));
    initialize_core_logic(&app);
    ui::init_stores(cx);

    // Secure Input monitor (macOS): detects stuck secure input that silently
    // blocks keyed shortcuts, warns the user, and activates the Carbon
    // fallback. See secure_input.rs and issue #1578.
    secure_input::init(&app);

    // Populate the overlay-enabled cache from initial settings so the audio
    // path (overlay::emit_levels, called ~24 Hz during recording) can do a
    // single atomic load instead of reading the settings store.
    overlay::update_overlay_enabled_cache(settings.overlay_style != settings::OverlayStyle::None);

    // Pre-warm GPU/accelerator enumeration on a background thread. The first
    // get_available_accelerators call enumerates ORT execution providers and
    // transcribe-cpp compute devices, which can take a moment; without this
    // the cost is paid when the user first opens the acceleration setting.
    std::thread::spawn(|| {
        let _ = crate::managers::transcription::get_available_accelerators();
    });

    if cli_args.no_tray {
        tray::set_tray_visibility(&app, false);
    }

    // Show the main window unless starting hidden. Permission onboarding
    // always shows it.
    let show = should_force_show_permissions_window(&app) || !starts_hidden(&app);
    #[cfg(debug_assertions)]
    let show = show || ui::dev_snapshot::requested();
    ui::open_main_window(show, cx);
    #[cfg(debug_assertions)]
    ui::dev_snapshot::start(cx);

    let forward_app = app.clone();
    listener.serve(move |args| handle_second_instance(&forward_app, &args));

    // Teardown transcribe.cpp before exit
    let quit_app = app.clone();
    cx.on_app_quit(move |_| {
        if let Some(tm) = quit_app.try_state::<Arc<TranscriptionManager>>() {
            let _ = tm.unload_model();
        }
        async {}
    })
    .detach();
}

/// Fired when the running bundle is launched again from Spotlight/Finder or
/// the Dock icon is clicked. If the settings window is hidden, the user is
/// likely looking for a tray icon that vanished (#1948): recreate it. When
/// the window is already visible this is just a focus request.
/// Whether the app starts with its window hidden: start hidden is on (setting
/// or `--start-hidden`) and a tray icon is available. Without a tray icon the
/// window is the only way back in (#903). `main` reads this before launch to
/// pick the macOS activation policy, so both decisions stay in sync.
pub fn starts_hidden(app: &AppHandle) -> bool {
    let cli_args = app.state::<CliArgs>().inner().clone();
    let settings = get_settings(app);
    let should_hide = settings.start_hidden || cli_args.start_hidden;
    let tray_available = settings.show_tray_icon && !cli_args.no_tray;
    should_hide && tray_available
}

pub fn on_reopen(cx: &mut App) {
    #[cfg(target_os = "macos")]
    if !ui::is_main_window_visible(cx) {
        tray::recreate_tray_icon(AppHandle::global());
    }
    ui::show_main_window(cx);
}

/// A second process was launched; it forwarded its command line here.
fn handle_second_instance(app: &AppHandle, args: &[String]) {
    if args.iter().any(|a| a == "--toggle-transcription") {
        signal_handle::send_transcription_input(app, "transcribe", "CLI");
    } else if args.iter().any(|a| a == "--toggle-post-process") {
        signal_handle::send_transcription_input(app, "transcribe_with_post_process", "CLI");
    } else if args.iter().any(|a| a == "--cancel") {
        utils::cancel_current_operation(app);
    } else {
        // Launched without remote-control flags (e.g. the binary run from a
        // shell): raise the window and recreate a possibly vanished tray
        // icon (#1948).
        #[cfg(target_os = "macos")]
        tray::recreate_tray_icon(app);
        app.with_ui(ui::show_main_window);
    }
}

#[allow(unused_variables)]
fn should_force_show_permissions_window(app: &AppHandle) -> bool {
    #[cfg(target_os = "windows")]
    {
        let model_manager = app.state::<Arc<ModelManager>>();
        let has_downloaded_models = model_manager
            .get_available_models()
            .iter()
            .any(|model| model.is_downloaded);

        if !has_downloaded_models {
            return false;
        }

        let status = crate::commands::audio::get_windows_microphone_permission_status();
        if status.supported
            && status.overall_access == crate::commands::audio::PermissionAccess::Denied
        {
            log::info!(
                "Windows microphone permissions are denied; forcing main window visible for onboarding"
            );
            return true;
        }
    }

    false
}

fn initialize_core_logic(app_handle: &AppHandle) {
    // Note: Enigo (keyboard/mouse simulation) and shortcuts are NOT
    // initialized here. The UI initializes them after onboarding completes,
    // so macOS permission dialogs don't appear before the user is ready.

    // The audio recorder receives the streaming router explicitly, so
    // always-on microphone startup can wire live-preview frames before the
    // managers are in managed state.
    let model_manager =
        Arc::new(ModelManager::new(app_handle).expect("Failed to initialize model manager"));
    let transcription_manager = Arc::new(
        TranscriptionManager::new(app_handle, model_manager.clone())
            .expect("Failed to initialize transcription manager"),
    );
    let voice_submit = voice_submit::VoiceSubmit::new(app_handle);
    let recording_manager = Arc::new(
        AudioRecordingManager::new(
            app_handle,
            transcription_manager.stream_router(),
            voice_submit.clone(),
        )
        .expect("Failed to initialize recording manager"),
    );
    let history_manager =
        Arc::new(HistoryManager::new(app_handle).expect("Failed to initialize history manager"));

    // Initialize the transcribe-cpp native backend (logging + backend module
    // registration) once, before any whisper model is loaded.
    managers::transcription::init_transcribe_backend();

    // Apply accelerator preferences before any model loads
    managers::transcription::apply_accelerator_settings(app_handle);

    app_handle.manage(recording_manager);
    app_handle.manage(model_manager);
    app_handle.manage(transcription_manager);
    app_handle.manage(history_manager);
    app_handle.manage(voice_submit);
    app_handle.manage(tray::TrayState::new());

    // Set up signal handlers for toggling transcription. On Linux, SIGUSR1 is
    // deliberately not handled — see signal_handle.rs.
    #[cfg(unix)]
    signal_handle::setup_signal_handler(app_handle.clone());

    // The global hotkey manager binds to the main run loop, so it is created
    // here, before the UI initializes shortcuts.
    shortcut::hotkey_impl::install(app_handle);

    tray::create_tray(app_handle);
    // Initialize tray menu with idle state
    tray::update_tray_menu(app_handle);

    let settings = get_settings(app_handle);
    if !settings.show_tray_icon {
        tray::set_tray_visibility(app_handle, false);
    }

    // Refresh tray menu when model state changes
    let app_handle_for_listener = app_handle.clone();
    app_handle.listen("model-state-changed", move |_| {
        tray::update_tray_menu(&app_handle_for_listener);
    });

    // Apply the autostart preference (SMAppService login item on macOS 13+,
    // the auto-launch crate elsewhere)
    autostart::apply_autostart(app_handle, settings.autostart_enabled);

    utils::create_recording_overlay(app_handle);
}
