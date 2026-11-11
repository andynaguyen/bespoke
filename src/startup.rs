use std::sync::Arc;
use std::sync::atomic::Ordering;

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
    FILE_LOG_LEVEL, LOG_STREAMING, TranscriptionCoordinator, autostart, managers, overlay,
    secure_input, shortcut, signal_handle, tray, ui, updater, utils, voice_submit,
};

pub fn run(
    app: AppHandle,
    ui_messages: async_channel::Receiver<UiMessage>,
    listener: single_instance::Listener,
    cx: &mut App,
) {
    let cli_args = app.state::<CliArgs>().inner().clone();
    let mut settings = get_settings(&app);

    if cli_args.debug {
        settings.debug_mode = true;
        settings.log_level = settings::LogLevel::Trace;
    }

    let file_log_level: log::LevelFilter = settings.log_level.into();
    FILE_LOG_LEVEL.store(file_log_level as u8, Ordering::Relaxed);
    LOG_STREAMING.store(settings.debug_mode, Ordering::Relaxed);

    ui::init(ui_messages, &settings, cx);

    app.manage(TranscriptionCoordinator::new(app.clone()));
    initialize_core_logic(&app);
    ui::init_stores(cx);
    updater::start();

    // Secure Input monitor (macOS): detects stuck secure input that silently
    // blocks keyed shortcuts, warns the user, and activates the Carbon
    // fallback. See secure_input.rs and issue #1578.
    secure_input::init(&app);

    overlay::update_overlay_enabled_cache(settings.overlay_style != settings::OverlayStyle::None);

    if cli_args.no_tray {
        tray::set_tray_visibility(&app, false);
    }

    let tour_in_progress = settings.onboarding_tour != settings::TourProgress::Done;
    let show = !starts_hidden(&app) || tour_in_progress;
    #[cfg(debug_assertions)]
    let show = show || ui::dev_snapshot::requested();
    ui::open_main_window(show, cx);
    #[cfg(debug_assertions)]
    ui::dev_snapshot::start(cx);

    crate::annotate::start(&app, cx);

    let forward_app = app.clone();
    listener.serve(move |args| handle_second_instance(&forward_app, &args));

    let quit_app = app.clone();
    cx.on_app_quit(move |_| {
        crate::system_output::restore_and_stop_lowering();
        if let Some(tm) = quit_app.try_state::<Arc<TranscriptionManager>>() {
            tm.unload_model();
        }
        async {}
    })
    .detach();
}

pub fn starts_hidden(app: &AppHandle) -> bool {
    let cli_args = app.state::<CliArgs>().inner().clone();
    let settings = get_settings(app);
    let should_hide = settings.start_hidden || cli_args.start_hidden;
    let tray_available = settings.show_tray_icon && !cli_args.no_tray;
    should_hide && tray_available
}

/// Fired when the running bundle is launched again from Spotlight/Finder or
/// the Dock icon is clicked. If the settings window is hidden, the user is
/// likely looking for a tray icon that vanished (#1948): recreate it. When
/// the window is already visible this is just a focus request.
pub fn on_reopen(cx: &mut App) {
    if !ui::is_main_window_visible(cx) {
        tray::recreate_tray_icon(AppHandle::global());
    }
    ui::show_main_window(cx);
}

fn handle_second_instance(app: &AppHandle, args: &[String]) {
    if args.iter().any(|a| a == "--toggle-transcription") {
        signal_handle::send_transcription_input(app, "transcribe");
    } else if args.iter().any(|a| a == "--toggle-post-process") {
        signal_handle::send_transcription_input(app, "transcribe_with_post_process");
    } else if args.iter().any(|a| a == "--cancel") {
        utils::cancel_current_operation(app);
    } else {
        // Launched without remote-control flags (e.g. the binary run from a
        // shell): raise the window and recreate a possibly vanished tray
        // icon (#1948).
        tray::recreate_tray_icon(app);
        app.with_ui(ui::show_main_window);
    }
}

fn initialize_core_logic(app_handle: &AppHandle) {
    let model_manager =
        Arc::new(ModelManager::new(app_handle).expect("Failed to initialize model manager"));
    let transcription_manager =
        Arc::new(TranscriptionManager::new(app_handle, model_manager.clone()));
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

    managers::transcription::apply_accelerator_settings(app_handle);

    app_handle.manage(recording_manager);
    app_handle.manage(model_manager);
    app_handle.manage(transcription_manager);
    app_handle.manage(history_manager);
    app_handle.manage(voice_submit);
    app_handle.manage(tray::TrayState::new());

    signal_handle::setup_signal_handler(app_handle.clone());

    // The global hotkey manager binds to the main run loop, so it is created
    // here, before the UI initializes shortcuts.
    shortcut::hotkey_impl::install(app_handle);

    tray::create_tray(app_handle);
    tray::sync_tray(app_handle);

    let settings = get_settings(app_handle);
    if !settings.show_tray_icon {
        tray::set_tray_visibility(app_handle, false);
    }

    for event in ["model-state-changed", updater::STATE_CHANGED] {
        let app_handle_for_listener = app_handle.clone();
        app_handle.listen(event, move |_| {
            tray::sync_tray(&app_handle_for_listener);
        });
    }

    autostart::apply_autostart(settings.autostart_enabled);
}
