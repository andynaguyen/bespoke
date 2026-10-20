// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
#[cfg(target_os = "macos")]
mod annotate;
mod app_handle;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod apple_intelligence;
mod audio_feedback;
pub mod audio_toolkit;
mod autostart;
mod catalog;
pub mod cli;
mod clipboard;
mod commands;
mod headless;
mod helpers;
mod i18n;
mod input;
mod llm_client;
mod logging;
mod managers;
mod memory;
mod overlay;
mod paste_tx;
pub mod portable;
mod runtime;
mod secure_input;
mod settings;
mod shortcut;
mod signal_handle;
mod single_instance;
mod startup;
mod system_clipboard;
mod transcription_coordinator;
mod tray;
mod tray_i18n;
mod ui;
mod utils;
mod voice_submit;
mod writing_style;

use clap::Parser;

pub use cli::CliArgs;
pub use logging::{FILE_LOG_LEVEL, LOG_STREAMING};
pub use transcription_coordinator::TranscriptionCoordinator;
pub use utils::env_flag_enabled;

use app_handle::AppHandle;
use single_instance::Instance;

fn main() {
    let cli_args = CliArgs::parse();

    #[cfg(target_os = "windows")]
    {
        // Avoid overlay/capture layer crashes (#2049). Set before backend
        // initialization, preserving user overrides.
        if std::env::var_os("VK_LOADER_LAYERS_DISABLE").is_none()
            && !env_flag_enabled("BESPOKE_KEEP_VULKAN_IMPLICIT_LAYERS")
        {
            std::env::set_var("VK_LOADER_LAYERS_DISABLE", "~implicit~");
        }
    }

    // Avoid ggml-metal residency-set teardown assertions when a native engine
    // outlives shutdown (#1902). This must happen before transcribe-cpp
    // initializes its Metal device. Advanced users can restore upstream
    // residency behavior with BESPOKE_METAL_RESIDENCY=1.
    #[cfg(target_os = "macos")]
    if std::env::var("BESPOKE_METAL_RESIDENCY").as_deref() == Ok("1") {
        // ggml treats GGML_METAL_NO_RESIDENCY as presence-based, so remove an
        // inherited value as well when explicitly opting back in.
        std::env::remove_var("GGML_METAL_NO_RESIDENCY");
    } else {
        std::env::set_var("GGML_METAL_NO_RESIDENCY", "1");
    }

    // Pin glibc's dynamic mmap threshold before the first large allocation,
    // so per-dictation transient buffers are returned to the OS on free
    // instead of accumulating in malloc arenas (#1792). No-op off Linux/glibc.
    memory::init_allocator();

    // Detect portable mode before anything else
    portable::init();

    // Before logging starts, which would create the new log folder.
    let migration_notes = if portable::is_portable() {
        Vec::new()
    } else {
        app_handle::migrate_legacy_dirs()
    };

    // The headless path must run as its own instance, not forward to an
    // already-running app.
    let headless_mode =
        cli_args.transcribe_file.is_some() || cli_args.list_devices || cli_args.list_models;

    let (app, ui_messages) = AppHandle::new();
    match portable::app_log_dir(&app) {
        Ok(log_dir) => logging::init(log_dir, headless_mode),
        Err(e) => eprintln!("No log directory: {e}"),
    }
    for note in migration_notes {
        log::info!("{note}");
    }

    if headless_mode {
        let code = headless::run(&app, &cli_args);
        // process::exit runs no destructors or buffer flushes.
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        std::process::exit(code);
    }

    // A second launch forwards its arguments to the running instance and
    // exits; remote-control flags (--toggle-transcription, ...) rely on it.
    let args: Vec<String> = std::env::args().collect();
    let listener = match single_instance::acquire(&args) {
        Instance::Forwarded => return,
        Instance::Primary(listener) => listener,
    };

    app.manage(cli_args);

    let application = gpui::Application::new().with_assets(ui::assets::Assets);
    #[cfg(target_os = "macos")]
    {
        #[cfg(debug_assertions)]
        let snapshot = ui::dev_snapshot::requested();
        #[cfg(not(debug_assertions))]
        let snapshot = false;
        if startup::starts_hidden(&app) && !snapshot {
            log::info!(
                "Starting hidden with tray available: launching as Accessory (no Dock icon)"
            );
            ui::platform::launch_as_accessory();
        }
    }
    // Fired when the running bundle is launched again from Spotlight/Finder
    // or its Dock icon is clicked.
    application.on_reopen(startup::on_reopen);
    application.run(move |cx| startup::run(app, ui_messages, listener, cx));
}
