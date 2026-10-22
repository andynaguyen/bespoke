//! System tray icon and menu.
//!
//! The tray is driven by a single *desired state* snapshot ([`TrayDesired`])
//! that callers update through [`set_tray_state`], [`refresh_tray_icon`] and
//! [`update_tray_menu`]. Every such call just records intent and schedules a
//! single applier on the main thread, which diffs the desired snapshot against
//! what is currently displayed and touches the native tray only for the parts
//! that actually changed. Requests that arrive while an apply is pending are
//! coalesced into it, so bursts of state changes never queue up native work.
//!
//! Why: native tray updates are the lever we control for the macOS tray
//! disappearance bug (tauri-apps/tauri#12060, Handy #1948). Before this, every
//! recording cycle rebuilt the full menu 3-6 times from several threads, and
//! concurrent rebuilds could interleave and leave a stale menu behind.
//!
//! Exception: [`set_tray_visibility`] and [`recreate_tray_icon`] call the tray
//! directly. Visibility is a separate attribute that never participates in the
//! icon/menu diff, both are rare and user-initiated, and they run on the main
//! thread so they serialize with the applier anyway. Re-showing a hidden tray
//! relies on tray-icon recreating it from the last applied icon/menu/tooltip,
//! so those must only ever be set through the applier.
//!
//! The native `TrayIcon` is not `Send`, so it lives in a main-thread
//! thread-local and is only touched from closures queued with `with_ui`.

use crate::app_handle::AppHandle;
use crate::managers::history::{HistoryEntry, HistoryManager};
use crate::managers::model::ModelManager;
use crate::managers::transcription::TranscriptionManager;
use crate::settings;
use crate::tray_i18n::get_tray_translations;
use log::{debug, error, info, trace, warn};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;
use tray_icon::menu::{
    accelerator::Accelerator, CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// Decoded RGBA pixels for a tray icon.
#[derive(Clone)]
struct TrayImage {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl TrayImage {
    fn to_icon(&self) -> Result<Icon, String> {
        Icon::from_rgba(self.rgba.clone(), self.width, self.height).map_err(|e| e.to_string())
    }
}

thread_local! {
    /// The native tray. Only ever touched on the main thread.
    static TRAY: RefCell<Option<TrayIcon>> = const { RefCell::new(None) };
}

/// Set once the native tray exists, so background threads can check for it.
static TRAY_BUILT: AtomicBool = AtomicBool::new(false);

fn with_tray<R>(f: impl FnOnce(&TrayIcon) -> R) -> Option<R> {
    TRAY.with(|tray| tray.borrow().as_ref().map(f))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayIconState {
    Idle,
    Recording,
    Transcribing,
}

impl TrayIconState {
    /// Recording and Transcribing share the same menu ("Cancel" instead of the
    /// model submenu), so only the idle/busy distinction matters for the menu.
    fn is_busy(self) -> bool {
        self != TrayIconState::Idle
    }
}

/// Everything the tray *menu* (and tooltip) depends on. When two snapshots
/// compare equal the menu is not rebuilt.
#[derive(Clone, Debug, PartialEq, Eq)]
struct MenuInputs {
    busy: bool,
    warning: bool,
    model_loaded: bool,
    selected_model: String,
    /// `(id, name)` of downloaded models, sorted by name.
    downloaded_models: Vec<(String, String)>,
    locale: String,
}

/// Complete description of what the tray should look like.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TrayDesired {
    icon_path: &'static str,
    menu: MenuInputs,
}

struct TrayInner {
    /// Intent set by [`set_tray_state`].
    icon_state: TrayIconState,
    /// Latest computed snapshot, waiting to be (or just) applied.
    desired: Option<TrayDesired>,
    /// Icon the native tray currently shows. Only updated when `set_icon`
    /// succeeds, so a failed update is retried on the next sync.
    applied_icon: Option<&'static str>,
    /// Inputs the native menu was last successfully built from. The tooltip
    /// is derived from the same inputs and set best-effort alongside the menu;
    /// it is not tracked separately.
    applied_menu: Option<MenuInputs>,
    /// An apply is scheduled on the main thread.
    pending: bool,
    /// Decoded icons by resource path so the main thread never touches disk.
    icons: HashMap<&'static str, TrayImage>,
    /// Handed out to each sync request in trigger order, so a slow request
    /// can't overwrite the snapshot of one that was triggered after it.
    next_seq: u64,
    /// Sequence number of the request that produced `desired`.
    desired_seq: u64,
}

/// Managed state owning the tray's desired/applied snapshots.
pub struct TrayState(Mutex<TrayInner>);

impl TrayState {
    pub fn new() -> Self {
        Self(Mutex::new(TrayInner {
            icon_state: TrayIconState::Idle,
            desired: None,
            applied_icon: None,
            applied_menu: None,
            pending: false,
            icons: HashMap::new(),
            next_seq: 0,
            desired_seq: 0,
        }))
    }

    fn lock(&self) -> MutexGuard<'_, TrayInner> {
        self.0.lock().unwrap_or_else(|poisoned| {
            warn!("Tray state mutex was poisoned, recovering");
            poisoned.into_inner()
        })
    }
}

impl Default for TrayState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AppTheme {
    Dark,
    Light,
    Colored, // Pink/colored theme for Linux
}

/// Gets the current app theme, with Linux defaulting to Colored theme
pub fn get_current_theme(app: &AppHandle) -> AppTheme {
    if cfg!(target_os = "linux") {
        // On Linux, always use the colored theme
        AppTheme::Colored
    } else {
        // On Windows the tray icon sits on the taskbar, which follows the
        // *system* theme (SystemUsesLightTheme), not the app theme. With the
        // "Custom" personalization mode the two can differ (e.g. dark taskbar
        // + light apps), and the window theme would pick an icon that is
        // invisible against the taskbar.
        #[cfg(target_os = "windows")]
        if let Some(theme) = windows_taskbar_theme() {
            return theme;
        }

        // On other platforms, follow the app's effective appearance
        let _ = app;
        if crate::ui::theme::effective_is_dark() {
            AppTheme::Dark
        } else {
            AppTheme::Light
        }
    }
}

/// Reads the Windows taskbar theme from the registry.
///
/// Returns None if the value is missing (older Windows 10 builds default to a
/// dark taskbar there, but falling back to the window theme is safer than
/// guessing).
#[cfg(target_os = "windows")]
fn windows_taskbar_theme() -> Option<AppTheme> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let personalize = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
        .ok()?;
    let system_uses_light: u32 = personalize.get_value("SystemUsesLightTheme").ok()?;
    Some(if system_uses_light == 1 {
        AppTheme::Light
    } else {
        AppTheme::Dark
    })
}

/// Gets the appropriate icon path for the given theme and state.
///
/// `warning` overlays a badge on the idle icon while keyboard shortcuts are
/// blocked (macOS Secure Input); recording/transcribing states keep their
/// normal icons so in-flight activity stays recognizable.
pub fn get_icon_path(theme: AppTheme, state: TrayIconState, warning: bool) -> &'static str {
    if warning && state == TrayIconState::Idle {
        return match theme {
            AppTheme::Dark => "resources/tray_idle_warning.png",
            AppTheme::Light => "resources/tray_idle_warning_dark.png",
            // Linux never sets the warning flag (Secure Input is macOS-only),
            // but fall back to the normal icon just in case.
            AppTheme::Colored => "resources/bespoke.png",
        };
    }
    match (theme, state) {
        // Dark theme uses light icons
        (AppTheme::Dark, TrayIconState::Idle) => "resources/tray_idle.png",
        (AppTheme::Dark, TrayIconState::Recording) => "resources/tray_recording.png",
        (AppTheme::Dark, TrayIconState::Transcribing) => "resources/tray_transcribing.png",
        // Light theme uses dark icons
        (AppTheme::Light, TrayIconState::Idle) => "resources/tray_idle_dark.png",
        (AppTheme::Light, TrayIconState::Recording) => "resources/tray_recording_dark.png",
        (AppTheme::Light, TrayIconState::Transcribing) => "resources/tray_transcribing_dark.png",
        // Colored theme uses pink icons (for Linux)
        (AppTheme::Colored, TrayIconState::Idle) => "resources/bespoke.png",
        (AppTheme::Colored, TrayIconState::Recording) => "resources/recording.png",
        (AppTheme::Colored, TrayIconState::Transcribing) => "resources/transcribing.png",
    }
}

/// Sets the recording state shown by the tray (icon + Cancel/model menu).
pub fn set_tray_state(app: &AppHandle, state: TrayIconState) {
    sync_tray_with(app, |inner| inner.icon_state = state);
}

/// Re-syncs the tray after something other than the recording state changed
/// (theme, Secure Input warning). The recording state itself is preserved.
pub fn refresh_tray_icon(app: &AppHandle) {
    sync_tray(app);
}

/// Re-syncs the tray after something the menu depends on changed (model
/// list/selection/loaded state, language, settings).
pub fn update_tray_menu(app: &AppHandle) {
    sync_tray(app);
}

/// Records the current desired tray state and schedules one apply on the main
/// thread (or lets an already-pending apply pick it up). Never blocks on the
/// main thread.
///
/// The snapshot (settings, model list, loaded state) is computed on the
/// *calling* thread on purpose: the main-thread applier must not take manager
/// locks that a worker may hold across slow work (see #1716).
pub fn sync_tray(app: &AppHandle) {
    sync_tray_with(app, |_| {});
}

fn sync_tray_with(app: &AppHandle, update: impl FnOnce(&mut TrayInner)) {
    let Some(state) = app.try_state::<TrayState>() else {
        return;
    };

    // Record intent and claim a sequence number in one critical section, so
    // sequence order == the order in which state changes were requested.
    let (seq, icon_state) = {
        let mut inner = state.lock();
        update(&mut inner);
        inner.next_seq += 1;
        (inner.next_seq, inner.icon_state)
    };

    // Tray not built yet (early secure-input monitor callbacks). The intent
    // is kept and picked up by the first sync after the tray exists.
    if !TRAY_BUILT.load(Ordering::Acquire) {
        return;
    }

    let desired = compute_desired(app, icon_state);

    // Decode the icon off the main thread, once per path, outside the lock.
    let needs_icon = !state.lock().icons.contains_key(desired.icon_path);
    let loaded_icon = if needs_icon {
        match load_tray_icon(app.path().resolve(
            desired.icon_path,
            crate::app_handle::BaseDirectory::Resource,
        )) {
            Ok(image) => Some(image),
            Err(err) => {
                error!("Failed to load tray icon '{}': {err}", desired.icon_path);
                None
            }
        }
    } else {
        None
    };

    let schedule = {
        let mut inner = state.lock();
        if let Some(image) = loaded_icon {
            inner.icons.insert(desired.icon_path, image);
        }
        if seq < inner.desired_seq {
            // A request triggered after this one already stored its snapshot
            // (and scheduled an apply). Ours is stale; drop it.
            trace!(
                "tray sync: request {seq} superseded by {}",
                inner.desired_seq
            );
            return;
        }
        inner.desired = Some(desired);
        inner.desired_seq = seq;
        // If an apply is already pending it will read the snapshot we just
        // stored; otherwise schedule one.
        !std::mem::replace(&mut inner.pending, true)
    };

    if schedule {
        post_apply(app);
    } else {
        trace!("tray sync: apply already pending");
    }
}

fn compute_desired(app: &AppHandle, icon_state: TrayIconState) -> TrayDesired {
    let settings = settings::get_settings(app);
    let theme = get_current_theme(app);
    let warning = crate::secure_input::tray_warning_active(app);
    let model_loaded = app.state::<Arc<TranscriptionManager>>().is_model_loaded();

    let mut downloaded_models: Vec<(String, String)> = app
        .state::<Arc<ModelManager>>()
        .get_available_models()
        .into_iter()
        .filter(|m| m.is_downloaded)
        .map(|m| (m.id, m.name))
        .collect();
    downloaded_models.sort_by(|a, b| a.1.cmp(&b.1));

    TrayDesired {
        icon_path: get_icon_path(theme, icon_state, warning),
        menu: MenuInputs {
            busy: icon_state.is_busy(),
            warning,
            model_loaded,
            selected_model: settings.selected_model,
            downloaded_models,
            locale: settings.app_language,
        },
    }
}

fn post_apply(app: &AppHandle) {
    let handle = app.clone();
    app.with_ui(move |_| apply_on_main(&handle));
}

/// The single writer to the native tray. Runs on the main thread.
fn apply_on_main(app: &AppHandle) {
    let Some(state) = app.try_state::<TrayState>() else {
        return;
    };
    if with_tray(|_| ()).is_none() {
        return;
    }

    let started = Instant::now();
    let (desired, icon, icon_changed, menu_changed) = {
        let mut inner = state.lock();
        inner.pending = false;
        let Some(desired) = inner.desired.clone() else {
            return;
        };
        let icon_changed = inner.applied_icon != Some(desired.icon_path);
        let menu_changed = inner.applied_menu.as_ref() != Some(&desired.menu);
        if !icon_changed && !menu_changed {
            trace!("tray apply: nothing changed");
            return;
        }
        let icon = inner.icons.get(desired.icon_path).cloned();
        (desired, icon, icon_changed, menu_changed)
    };

    // Each part is recorded as applied only if its native call succeeded, so a
    // transient failure is retried on the next sync instead of being
    // remembered as displayed.
    let mut icon_ok = false;
    if icon_changed {
        match icon.map(|image| image.to_icon()) {
            Some(Ok(icon)) => match with_tray(|tray| set_tray_icon(tray, icon)).unwrap_or(Ok(())) {
                Ok(()) => icon_ok = true,
                Err(err) => {
                    error!("Failed to update tray icon '{}': {err}", desired.icon_path)
                }
            },
            Some(Err(err)) => error!("Tray icon '{}' is invalid: {err}", desired.icon_path),
            None => error!("Tray icon '{}' is not loaded", desired.icon_path),
        }
    }

    let mut menu_ok = false;
    if menu_changed {
        match build_menu(&desired.menu) {
            Ok((menu, tooltip)) => {
                with_tray(|tray| {
                    tray.set_menu(Some(Box::new(menu)));
                    // Best-effort: logged, not retried. The tooltip is cosmetic
                    // and can only fail on Windows, where a failing
                    // Shell_NotifyIcon call means the icon is failing too.
                    // Gating `menu_ok` on it would re-run the full menu
                    // rebuild on every sync for the cheapest mutation.
                    if let Err(err) = tray.set_tooltip(Some(tooltip)) {
                        error!("Failed to set tray tooltip: {err}");
                    }
                });
                menu_ok = true;
            }
            Err(err) => error!("Failed to build tray menu: {err}"),
        }
    }

    {
        let mut inner = state.lock();
        if icon_ok {
            inner.applied_icon = Some(desired.icon_path);
        }
        if menu_ok {
            inner.applied_menu = Some(desired.menu.clone());
        }
    }

    debug!(
        "tray apply: icon={} menu={} busy={} took={:?}",
        if icon_changed {
            desired.icon_path
        } else {
            "unchanged"
        },
        if menu_changed { "rebuilt" } else { "unchanged" },
        desired.menu.busy,
        started.elapsed()
    );
}

#[cfg(target_os = "macos")]
fn set_tray_icon(tray: &TrayIcon, icon: Icon) -> Result<(), String> {
    tray.set_icon_with_as_template(Some(icon), true)
        .map_err(|e| e.to_string())
}

#[cfg(not(target_os = "macos"))]
fn set_tray_icon(tray: &TrayIcon, icon: Icon) -> Result<(), String> {
    tray.set_icon(Some(icon)).map_err(|e| e.to_string())
}

fn load_tray_icon(
    resolved_icon_path: Result<PathBuf, crate::app_handle::Error>,
) -> Result<TrayImage, String> {
    let resolved_icon_path = resolved_icon_path.map_err(|e| e.to_string())?;
    let image = image::open(&resolved_icon_path)
        .map_err(|e| format!("{}: {e}", resolved_icon_path.display()))?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Ok(TrayImage {
        rgba: image.into_raw(),
        width,
        height,
    })
}

pub fn tray_tooltip() -> String {
    title_label()
}

fn title_label() -> String {
    if cfg!(debug_assertions) {
        "Bespoke (Dev)".to_string()
    } else {
        "Bespoke".to_string()
    }
}

/// Builds the tray menu and tooltip for the given inputs. Pure with respect
/// to app state: everything it depends on is in `inputs`.
fn build_menu(inputs: &MenuInputs) -> Result<(Menu, String), tray_icon::menu::Error> {
    let strings = get_tray_translations(Some(inputs.locale.clone()));

    // Secure Input warning entry (macOS): clicking opens the settings window
    // where the full warning banner explains the situation. Locales that
    // haven't translated the key yet get the English string rather than a
    // blank menu item.
    let secure_input_warning = if inputs.warning {
        let label = if strings.secure_input_warning.is_empty() {
            get_tray_translations(Some("en".to_string())).secure_input_warning
        } else {
            strings.secure_input_warning.clone()
        };
        Some(MenuItem::with_id(
            "secure_input_warning",
            &label,
            true,
            None,
        ))
    } else {
        None
    };

    // Platform-specific accelerators
    #[cfg(target_os = "macos")]
    let (settings_accelerator, quit_accelerator) = ("Cmd+,", "Cmd+Q");
    #[cfg(not(target_os = "macos"))]
    let (settings_accelerator, quit_accelerator) = ("Ctrl+,", "Ctrl+Q");
    let accelerator = |s: &str| Accelerator::from_str(s).ok();

    // Create common menu items
    let title_label = title_label();
    let title_i = MenuItem::with_id("title", &title_label, false, None);
    let settings_i = MenuItem::with_id(
        "settings",
        &strings.settings,
        true,
        accelerator(settings_accelerator),
    );
    let copy_last_transcript_i = MenuItem::with_id(
        "copy_last_transcript",
        &strings.copy_last_transcript,
        true,
        None,
    );
    let quit_i = MenuItem::with_id("quit", &strings.quit, true, accelerator(quit_accelerator));
    let separator = PredefinedMenuItem::separator;

    // Where Annotate's submenu goes: just above Settings.
    let annotate_at = if inputs.busy { 6 } else { 7 };
    let menu = if inputs.busy {
        let cancel_i = MenuItem::with_id("cancel", &strings.cancel, true, None);
        Menu::with_items(&[
            &title_i,
            &separator(),
            &cancel_i,
            &separator(),
            &copy_last_transcript_i,
            &separator(),
            &settings_i,
            &separator(),
            &quit_i,
        ])?
    } else {
        // Build model submenu — label is the active model name
        let submenu_label = inputs
            .downloaded_models
            .iter()
            .find(|(id, _)| *id == inputs.selected_model)
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| strings.model.clone());

        let model_submenu = Submenu::with_id("model_submenu", &submenu_label, true);
        for (id, name) in &inputs.downloaded_models {
            let is_active = *id == inputs.selected_model;
            let item_id = format!("model_select:{}", id);
            let item = CheckMenuItem::with_id(item_id, name, true, is_active, None);
            model_submenu.append(&item)?;
        }

        let unload_model_i = MenuItem::with_id(
            "unload_model",
            &strings.unload_model,
            inputs.model_loaded,
            None,
        );

        Menu::with_items(&[
            &title_i,
            &separator(),
            &copy_last_transcript_i,
            &separator(),
            &model_submenu,
            &unload_model_i,
            &separator(),
            &settings_i,
            &separator(),
            &quit_i,
        ])?
    };

    // Annotate's items, which Scrawl kept in its own menu bar item.
    #[cfg(target_os = "macos")]
    {
        let annotate = Submenu::with_id("annotate_submenu", &strings.annotate, true);
        annotate.append_items(&[
            &MenuItem::with_id("annotate_record", &strings.annotate_record, true, None),
            &MenuItem::with_id("annotate_choose", &strings.annotate_choose, true, None),
            &separator(),
            &MenuItem::with_id(
                "annotate_open_latest",
                &strings.annotate_open_latest,
                true,
                None,
            ),
        ])?;
        menu.insert(&annotate, annotate_at)?;
        menu.insert(&separator(), annotate_at + 1)?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = annotate_at;

    // Both layouts start with [title, separator, ...]; slot the warning in
    // right below the title line so it's the first actionable thing seen.
    let mut tooltip = title_label;
    if let Some(warning_item) = secure_input_warning {
        menu.insert(&warning_item, 2)?;
        menu.insert(&separator(), 3)?;
        tooltip = format!("{} — {}", tooltip, warning_item.text());
    }

    Ok((menu, tooltip))
}

fn last_transcript_text(entry: &HistoryEntry) -> &str {
    entry
        .post_processed_text
        .as_deref()
        .unwrap_or(&entry.transcription_text)
}

pub fn set_tray_visibility(app: &AppHandle, visible: bool) {
    app.with_ui(move |_| match with_tray(|tray| tray.set_visible(visible)) {
        Some(Err(e)) => error!("Failed to set tray visibility: {}", e),
        Some(Ok(())) => info!("Tray visibility set to: {}", visible),
        None => {}
    });
}

/// Recovery for the macOS tray-disappearance bug (#1948, tauri-apps/tauri#12060):
/// the `NSStatusItem` can silently vanish with no error surfaced to the app.
/// Hiding and re-showing the tray recreates it with its current icon, menu and
/// tooltip. Called when the user "relaunches" Bespoke while it is already running
/// (the app's reopen event for Spotlight/Finder/Dock, the single-instance
/// callback for a second process) — the natural "where did my icon go?"
/// moment — so a relaunch brings the icon back without a full quit.
#[cfg(target_os = "macos")]
pub fn recreate_tray_icon(app: &AppHandle) {
    let no_tray = app
        .try_state::<crate::cli::CliArgs>()
        .map(|args| args.no_tray)
        .unwrap_or(false);
    if no_tray || !settings::get_settings(app).show_tray_icon {
        return;
    }
    app.with_ui(|_| {
        let result = with_tray(|tray| {
            info!("Recreating tray icon on relaunch");
            tray.set_visible(false).and_then(|_| tray.set_visible(true))
        });
        if let Some(Err(e)) = result {
            error!("Failed to recreate tray icon: {}", e);
        }
    });
}

/// Builds the native tray on the main thread and routes its menu events.
/// Mirrors the Tauri setup: menu on left click everywhere except Windows,
/// where left click opens the settings window and right click shows the menu.
pub fn create_tray(app: &AppHandle) {
    let initial_icon_path = get_icon_path(get_current_theme(app), TrayIconState::Idle, false);
    let icon = load_tray_icon(app.path().resolve(
        initial_icon_path,
        crate::app_handle::BaseDirectory::Resource,
    ))
    .and_then(|image| image.to_icon());
    let icon = match icon {
        Ok(icon) => icon,
        Err(err) => {
            error!("Failed to load the initial tray icon: {err}");
            return;
        }
    };

    let builder = TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip(tray_tooltip())
        .with_icon_as_template(true)
        .with_menu_on_left_click(!cfg!(target_os = "windows"));
    let tray = match builder.build() {
        Ok(tray) => tray,
        Err(err) => {
            error!("Failed to create the tray icon: {err}");
            return;
        }
    };
    TRAY.with(|slot| *slot.borrow_mut() = Some(tray));
    TRAY_BUILT.store(true, Ordering::Release);

    let menu_app = app.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let app = menu_app.clone();
        let id = event.id.0.clone();
        // Menu callbacks fire inside AppKit's menu tracking; defer the work
        // so window and tray changes happen after the menu closes.
        menu_app.with_ui(move |cx| handle_menu_event(&app, &id, cx));
    }));

    #[cfg(target_os = "windows")]
    {
        use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
        let click_app = app.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            let opens_window = matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            );
            if opens_window {
                click_app.with_ui(crate::ui::show_main_window);
            }
        }));
    }
}

fn handle_menu_event(app: &AppHandle, id: &str, cx: &mut gpui::App) {
    match id {
        "settings" | "secure_input_warning" => {
            // The secure input entry opens the window, where the full
            // explanation lives in the settings banner.
            crate::ui::show_main_window(cx);
        }
        "copy_last_transcript" => copy_last_transcript(app),
        #[cfg(target_os = "macos")]
        "annotate_record" => scrawl::host::menu(scrawl::MenuAction::ToggleCapture),
        #[cfg(target_os = "macos")]
        "annotate_choose" => scrawl::host::menu(scrawl::MenuAction::ChooseTarget),
        #[cfg(target_os = "macos")]
        "annotate_open_latest" => scrawl::host::menu(scrawl::MenuAction::OpenLatest),
        "unload_model" => {
            let transcription_manager = app.state::<Arc<TranscriptionManager>>();
            if !transcription_manager.is_model_loaded() {
                warn!("No model is currently loaded.");
                return;
            }
            match transcription_manager.unload_model() {
                Ok(()) => info!("Model unloaded via tray."),
                Err(e) => error!("Failed to unload model via tray: {}", e),
            }
        }
        "cancel" => crate::utils::cancel_current_operation(app),
        "quit" => cx.quit(),
        id if id.starts_with("model_select:") => {
            let model_id = id.trim_start_matches("model_select:").to_string();
            if model_id == settings::get_settings(app).selected_model {
                return;
            }
            let app = app.clone();
            std::thread::spawn(move || {
                match crate::commands::models::switch_active_model(&app, &model_id) {
                    Ok(()) => info!("Model switched to {} via tray.", model_id),
                    Err(e) => error!("Failed to switch model via tray: {}", e),
                }
                update_tray_menu(&app);
            });
        }
        _ => {}
    }
}

pub fn copy_last_transcript(app: &AppHandle) {
    let history_manager = app.state::<Arc<HistoryManager>>();
    let entry = match history_manager.get_latest_completed_entry() {
        Ok(Some(entry)) => entry,
        Ok(None) => {
            warn!("No completed transcription history entries available for tray copy.");
            return;
        }
        Err(err) => {
            error!(
                "Failed to fetch last completed transcription entry: {}",
                err
            );
            return;
        }
    };

    let text = last_transcript_text(&entry);
    if text.trim().is_empty() {
        warn!("Last completed transcription is empty; skipping tray copy.");
        return;
    }

    if let Err(err) = app.clipboard().write_text(text) {
        error!("Failed to copy last transcript to clipboard: {}", err);
        return;
    }

    info!("Copied last transcript to clipboard via tray.");
}

#[cfg(test)]
mod tests {
    use super::{last_transcript_text, load_tray_icon, MenuInputs, TrayDesired, TrayIconState};
    use crate::managers::history::HistoryEntry;

    fn build_entry(transcription: &str, post_processed: Option<&str>) -> HistoryEntry {
        HistoryEntry {
            id: 1,
            file_name: "bespoke-1.wav".to_string(),
            timestamp: 0,
            saved: false,
            title: "Recording".to_string(),
            transcription_text: transcription.to_string(),
            post_processed_text: post_processed.map(|text| text.to_string()),
            post_process_prompt: None,
            post_process_requested: false,
        }
    }

    fn inputs(busy: bool) -> MenuInputs {
        MenuInputs {
            busy,
            warning: false,
            model_loaded: true,
            selected_model: "small".to_string(),
            downloaded_models: vec![("small".to_string(), "Small".to_string())],
            locale: "en".to_string(),
        }
    }

    #[test]
    fn uses_post_processed_text_when_available() {
        let entry = build_entry("raw", Some("processed"));
        assert_eq!(last_transcript_text(&entry), "processed");
    }

    #[test]
    fn falls_back_to_raw_transcription() {
        let entry = build_entry("raw", None);
        assert_eq!(last_transcript_text(&entry), "raw");
    }

    #[test]
    fn tray_icon_resolution_failure_is_returned_instead_of_panicking() {
        assert!(load_tray_icon(Err(anyhow::anyhow!("unknown path"))).is_err());
    }

    #[test]
    fn tray_icon_returns_err_when_file_does_not_exist() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let missing = dir.path().join("does_not_exist.png");
        assert!(load_tray_icon(Ok(missing)).is_err());
    }

    #[test]
    fn recording_and_transcribing_share_a_menu() {
        // The icon differs but the menu inputs are identical, so a
        // Recording -> Transcribing transition must not rebuild the menu.
        let recording = TrayDesired {
            icon_path: "resources/tray_recording.png",
            menu: inputs(TrayIconState::Recording.is_busy()),
        };
        let transcribing = TrayDesired {
            icon_path: "resources/tray_transcribing.png",
            menu: inputs(TrayIconState::Transcribing.is_busy()),
        };
        assert_ne!(recording.icon_path, transcribing.icon_path);
        assert_eq!(recording.menu, transcribing.menu);
    }

    #[test]
    fn idle_and_busy_menus_differ() {
        assert_ne!(inputs(false), inputs(true));
    }
}
