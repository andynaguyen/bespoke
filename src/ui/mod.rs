//! The GPUI front end: the settings window, the recording overlay, and the
//! glue that keeps them in sync with the backend.

pub mod assets;
pub mod audio_player;
pub mod capsule;
pub mod components;
pub mod dates;
#[cfg(debug_assertions)]
pub mod dev_snapshot;
pub mod event_bus;
pub mod format;
pub mod glass;
pub mod keyboard;
pub mod languages;
pub mod model_card;
pub mod onboarding;
pub mod overlay;
pub mod pages;
pub mod platform;
pub mod shell;
pub mod stores;
pub mod theme;
pub mod toast;
pub mod whats_new;

use gpui::{
    actions, px, size, App, AppContext, Bounds, Global, KeyBinding, Menu, MenuItem, OsAction,
    TitlebarOptions, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind,
    WindowOptions,
};
use gpui_component::Root;

use crate::app_handle::{AppHandle, UiMessage};
use crate::cli::CliArgs;
use crate::settings::AppSettings;
use shell::MainWindow;

/// A shared callback that runs with the window, e.g. a toast action or the
/// end of an onboarding step.
pub type WindowCallback = std::rc::Rc<dyn Fn(&mut gpui::Window, &mut App)>;

actions!(
    bespoke,
    [
        Quit,
        HideApp,
        HideOtherApps,
        ShowAllApps,
        Minimize,
        CloseWindow,
        ToggleDebugMode,
        OpenSettings,
        DialogFocusNext,
        DialogFocusPrev
    ]
);

/// The settings window, while it exists.
#[derive(Default)]
struct MainWindowSlot {
    handle: Option<WindowHandle<Root>>,
    view: Option<gpui::Entity<MainWindow>>,
}

impl Global for MainWindowSlot {}

/// Sets up theme, strings, event delivery and app-level actions. Runs before
/// the backend managers start, so it touches no managed state.
pub fn init(
    _app: &AppHandle,
    ui_messages: async_channel::Receiver<UiMessage>,
    settings: &AppSettings,
    cx: &mut App,
) {
    gpui_component::init(cx);
    components::focus_init(cx);
    theme::init(settings.theme, cx);
    let language = if settings.app_language.is_empty() {
        sys_locale::get_locale().unwrap_or_else(|| "en".into())
    } else {
        settings.app_language.clone()
    };
    crate::i18n::set_language(&language);
    event_bus::init(ui_messages, cx);
    toast::init(cx);
    cx.set_global(MainWindowSlot::default());

    #[cfg(target_os = "macos")]
    platform::prevent_panels_from_taking_focus();

    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &HideApp, cx| cx.hide());
    cx.on_action(|_: &HideOtherApps, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAllApps, cx| cx.unhide_other_apps());
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", HideApp, None),
        KeyBinding::new("alt-cmd-h", HideOtherApps, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("secondary-shift-d", ToggleDebugMode, None),
        KeyBinding::new("secondary-,", OpenSettings, None),
        // Inside a dialog Tab cycles through its controls only. The dialog's
        // key context sits deeper than gpui-component's Root, so these win.
        KeyBinding::new("tab", DialogFocusNext, Some(components::DIALOG_CONTEXT)),
        KeyBinding::new(
            "shift-tab",
            DialogFocusPrev,
            Some(components::DIALOG_CONTEXT),
        ),
    ]);
    set_menus(cx);
}

/// The default macOS menu bar a Tauri app gets: app, Edit and Window menus.
fn set_menus(cx: &mut App) {
    use gpui_component::input;
    cx.set_menus(vec![
        Menu {
            name: "Bespoke".into(),
            items: vec![
                MenuItem::action("Hide Bespoke", HideApp),
                MenuItem::action("Hide Others", HideOtherApps),
                MenuItem::action("Show All", ShowAllApps),
                MenuItem::separator(),
                MenuItem::action("Quit Bespoke", Quit),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
                MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
                MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
            ],
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Close Window", CloseWindow),
            ],
        },
    ]);
}

/// Creates the stores that read backend state. Runs once the managers exist.
pub fn init_stores(cx: &mut App) {
    stores::SettingsStore::init(cx);
    stores::ModelStore::init(cx);
}

pub fn main_window_handle(cx: &App) -> Option<WindowHandle<Root>> {
    main_window(cx)
}

fn main_window(cx: &App) -> Option<WindowHandle<Root>> {
    cx.global::<MainWindowSlot>().handle
}

/// Opens the settings window, visible or hidden.
pub fn open_main_window(show: bool, cx: &mut App) {
    if main_window(cx).is_some() {
        if show {
            show_main_window(cx);
        }
        return;
    }
    let bounds = Bounds::centered(None, size(px(1024.), px(800.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        // On macOS the title bar is hidden and the traffic lights sit inside
        // the sidebar panel, clear of its rounded corner, the way macOS 26
        // places them in a sidebar. The title stays set so Mission Control
        // and the Window menu still show it.
        titlebar: Some(TitlebarOptions {
            title: Some("Bespoke".into()),
            appears_transparent: cfg!(target_os = "macos"),
            traffic_light_position: Some(gpui::point(px(19.), px(19.))),
        }),
        focus: show,
        show,
        kind: WindowKind::Normal,
        is_movable: true,
        is_resizable: true,
        is_minimizable: true,
        window_min_size: Some(size(px(920.), px(570.))),
        // The window draws its own glass (`ui::glass`) under GPUI's layer.
        window_background: if glass::native_available() {
            WindowBackgroundAppearance::Transparent
        } else {
            WindowBackgroundAppearance::Opaque
        },
        app_id: Some(crate::app_handle::APP_IDENTIFIER.into()),
        ..Default::default()
    };

    let handle = cx.open_window(options, |window, cx| {
        window.on_window_should_close(cx, hide_main_window);
        platform::prepare_main_window(window);
        let view = cx.new(|cx| MainWindow::new(window, cx));
        cx.global_mut::<MainWindowSlot>().view = Some(view.clone());
        cx.new(|cx| Root::new(view, window, cx))
    });
    match handle {
        Ok(handle) => {
            cx.global_mut::<MainWindowSlot>().handle = Some(handle);
            theme::sync_title_bar(cx);
            if show {
                show_main_window(cx);
            } else if cfg!(target_os = "linux") {
                // Linux windows always map on open; minimize stands in for
                // starting hidden (see `platform::hide_window`).
                let _ = handle.update(cx, |_, window, _| window.minimize_window());
            }
        }
        Err(e) => log::error!("Failed to open the main window: {e}"),
    }
}

/// Hides the window instead of closing it; the app keeps running in the
/// tray. Returns whether the platform should close the window anyway.
pub fn hide_main_window(window: &mut gpui::Window, cx: &mut App) -> bool {
    let hidden = platform::hide_window(window, cx);
    if !hidden {
        let slot = cx.global_mut::<MainWindowSlot>();
        slot.handle = None;
        slot.view = None;
    }

    #[cfg(target_os = "macos")]
    {
        let app = AppHandle::global();
        let settings = crate::settings::get_settings(app);
        let tray_visible = settings.show_tray_icon && !app.state::<CliArgs>().no_tray;
        if tray_visible {
            // Tray is available: hide the dock icon, the app lives in the tray
            cx.foreground_executor()
                .spawn(async { platform::set_activation_policy_regular(false) })
                .detach();
        }
        // No tray: keep the dock icon visible so the user can reopen
    }
    #[cfg(not(target_os = "macos"))]
    let _ = CliArgs::default;

    !hidden
}

/// Shows, unminimizes and focuses the settings window.
pub fn show_main_window(cx: &mut App) {
    let Some(handle) = main_window(cx) else {
        open_main_window(true, cx);
        return;
    };
    #[cfg(target_os = "macos")]
    cx.foreground_executor()
        .spawn(async { platform::set_activation_policy_regular(true) })
        .detach();
    let result = handle.update(cx, |_, window, cx| platform::show_window(window, cx));
    if result.is_err() {
        // The window is gone (closed on a platform that cannot hide).
        let slot = cx.global_mut::<MainWindowSlot>();
        slot.handle = None;
        slot.view = None;
        open_main_window(true, cx);
    }
}

pub fn is_main_window_visible(cx: &mut App) -> bool {
    main_window(cx)
        .and_then(|handle| {
            handle
                .update(cx, |_, window, _| platform::is_window_visible(window))
                .ok()
        })
        .unwrap_or(false)
}

/// Shows the main window on `section` (Annotate's missing permissions bring it forward).
pub fn show_section(section: shell::Section, cx: &mut App) {
    show_main_window(cx);
    let (Some(main), Some(handle)) = (main_view(cx), main_window(cx)) else {
        return;
    };
    let _ = handle.update(cx, |_, window, cx| {
        main.update(cx, |main, cx| main.select_section(section, window, cx));
    });
}

/// The settings view, for code that needs to open dialogs or switch pages.
pub fn main_view(cx: &App) -> Option<gpui::Entity<MainWindow>> {
    cx.global::<MainWindowSlot>().view.clone()
}
