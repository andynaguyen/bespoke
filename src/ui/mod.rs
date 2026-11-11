pub mod assets;
pub mod audio_player;
pub mod capsule;
pub mod clay;
pub mod components;
pub mod dates;
#[cfg(debug_assertions)]
pub mod dev_snapshot;
pub mod event_bus;
pub mod format;
pub mod glass;
pub mod hover_card;
pub mod keyboard;
pub mod languages;
pub mod model_card;
pub mod onboarding;
pub mod overlay;
pub mod pages;
pub mod platform;
pub mod popup;
pub mod shell;
pub mod stores;
pub mod theme;
pub mod toast;
pub mod whats_new;

use gpui::{
    App, AppContext, Bounds, Global, KeyBinding, Menu, MenuItem, OsAction, TitlebarOptions,
    WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions, actions, px,
    size,
};
use gpui_component::Root;

use crate::app_handle::{AppHandle, UiMessage};
use crate::cli::CliArgs;
use crate::settings::AppSettings;
use shell::MainWindow;

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

#[derive(Default)]
struct MainWindowSlot {
    handle: Option<WindowHandle<Root>>,
    view: Option<gpui::Entity<MainWindow>>,
}

impl Global for MainWindowSlot {}

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
    pages::triage::init(cx);
    cx.set_global(MainWindowSlot::default());

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

pub fn apply_language(code: &str, cx: &mut App) {
    crate::i18n::set_language(code);
    set_menus(cx);
    cx.refresh_windows();
}

fn set_menus(cx: &mut App) {
    use crate::i18n::t;
    use gpui_component::input;
    cx.set_menus(vec![
        Menu {
            name: "Bespoke".into(),
            items: vec![
                MenuItem::action(t("menu.hideApp"), HideApp),
                MenuItem::action(t("menu.hideOthers"), HideOtherApps),
                MenuItem::action(t("menu.showAll"), ShowAllApps),
                MenuItem::separator(),
                MenuItem::action(t("menu.quitApp"), Quit),
            ],
        },
        Menu {
            name: t("menu.edit"),
            items: vec![
                MenuItem::os_action(t("menu.undo"), input::Undo, OsAction::Undo),
                MenuItem::os_action(t("menu.redo"), input::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action(t("menu.cut"), input::Cut, OsAction::Cut),
                MenuItem::os_action(t("menu.copy"), input::Copy, OsAction::Copy),
                MenuItem::os_action(t("menu.paste"), input::Paste, OsAction::Paste),
                MenuItem::os_action(t("menu.selectAll"), input::SelectAll, OsAction::SelectAll),
            ],
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action(t("menu.minimize"), Minimize),
                MenuItem::action(t("menu.closeWindow"), CloseWindow),
            ],
        },
    ]);
    // GPUI finds the Window menu by this English name, so it gets its title afterwards.
    platform::set_window_menu_title(&t("menu.window"));
}

pub fn init_stores(cx: &mut App) {
    stores::SettingsStore::init(cx);
    stores::ModelStore::init(cx);
}

pub fn main_window(cx: &App) -> Option<WindowHandle<Root>> {
    cx.global::<MainWindowSlot>().handle
}

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
            appears_transparent: true,
            traffic_light_position: Some(gpui::point(px(19.), px(19.))),
        }),
        focus: show,
        show,
        kind: WindowKind::Normal,
        is_movable: true,
        is_resizable: true,
        is_minimizable: true,
        window_min_size: Some(size(px(920.), px(570.))),
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
            if show {
                show_main_window(cx);
            }
        }
        Err(e) => log::error!("Failed to open the main window: {e}"),
    }
}

pub fn hide_main_window(window: &mut gpui::Window, cx: &mut App) -> bool {
    hover_card::hide(cx);
    let hidden = platform::hide_window(window, cx);
    if !hidden {
        let slot = cx.global_mut::<MainWindowSlot>();
        slot.handle = None;
        slot.view = None;
    }

    {
        let app = AppHandle::global();
        let settings = crate::settings::get_settings(app);
        let tray_visible = settings.show_tray_icon && !app.state::<CliArgs>().no_tray;
        if tray_visible {
            cx.foreground_executor()
                .spawn(async { platform::set_activation_policy_regular(false) })
                .detach();
        }
    }

    !hidden
}

pub fn show_main_window(cx: &mut App) {
    let Some(handle) = main_window(cx) else {
        open_main_window(true, cx);
        return;
    };
    cx.foreground_executor()
        .spawn(async { platform::set_activation_policy_regular(true) })
        .detach();
    let result = handle.update(cx, |_, window, cx| platform::show_window(window, cx));
    if result.is_err() {
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

/// Shows `section` in the main window. A dialog over the window, like Settings, closes first,
/// or it would cover the section it was asked for.
pub fn show_section(section: shell::Section, cx: &mut App) {
    show_main_window(cx);
    let (Some(main), Some(handle)) = (main_view(cx), main_window(cx)) else {
        return;
    };
    let _ = handle.update(cx, |_, window, cx| {
        main.update(cx, |main, cx| {
            if main.has_dialog() {
                main.close_dialog(window, cx);
            }
            main.select_section(section, window, cx);
        });
    });
}

/// Shows the main window with the Settings dialog open at `section`.
pub fn show_settings(section: pages::settings::SettingsSection, cx: &mut App) {
    show_main_window(cx);
    // The window comes forward on the next main-thread turn, and the dialog attaches to it.
    cx.defer(move |cx| {
        let Some(handle) = main_window(cx) else {
            return;
        };
        let _ = handle.update(cx, |_, window, cx| {
            pages::settings::open(Some(section), window, cx)
        });
    });
}

/// Brings the main window forward and opens Triage an Issue over it.
pub fn show_triage(cx: &mut App) {
    show_main_window(cx);
    cx.defer(|cx| {
        let Some(handle) = main_window(cx) else {
            return;
        };
        let _ = handle.update(cx, |_, window, cx| pages::triage::open(window, cx));
    });
}

pub fn main_view(cx: &App) -> Option<gpui::Entity<MainWindow>> {
    cx.global::<MainWindowSlot>().view.clone()
}
