//! Process-wide handle shared by the backend managers and the GPUI front end.
//!
//! It replaces the Tauri `AppHandle` the managers were written against and
//! keeps the same method names (`state`, `manage`, `emit`, `path`, `store`),
//! so the backend reads the same as before. The pieces are:
//!
//! - managed state: one `Arc` per type, looked up by `TypeId`;
//! - events: `emit` serializes the payload to JSON and queues it for the UI
//!   thread, which forwards it to every view subscribed to the event bus;
//! - UI calls: `with_ui` queues a closure that runs on the GPUI main thread
//!   with `&mut gpui::App`, used for window, tray and quit requests;
//! - paths and the JSON settings store, matching Tauri's on-disk layout so an
//!   existing install keeps its settings, models and history.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use parking_lot::{Mutex, RwLock};
use serde::Serialize;

pub const APP_IDENTIFIER: &str = "com.andynguyen.bespoke";
pub const APP_NAME: &str = "Bespoke";

/// The identifier and name of Handy, the app Bespoke started from. Bespoke
/// moves folders left under them on first launch.
pub const LEGACY_IDENTIFIER: &str = "com.pais.handy";
pub const LEGACY_APP_NAME: &str = "Handy";

pub type Error = anyhow::Error;

/// A message for the UI thread, drained by `ui::event_bus`.
pub enum UiMessage {
    Event(AppEvent),
    Call(Box<dyn FnOnce(&mut gpui::App) + Send>),
}

/// An event the backend sent to the UI, with its JSON payload.
#[derive(Clone, Debug)]
pub struct AppEvent {
    pub name: String,
    pub payload: serde_json::Value,
}

impl AppEvent {
    /// Deserializes the payload, logging and returning `None` on a mismatch.
    pub fn parse<T: serde::de::DeserializeOwned>(&self) -> Option<T> {
        match serde_json::from_value(self.payload.clone()) {
            Ok(value) => Some(value),
            Err(e) => {
                log::warn!("Malformed payload for event '{}': {}", self.name, e);
                None
            }
        }
    }
}

type Listener = Arc<dyn Fn(&AppEvent) + Send + Sync>;

struct Inner {
    state: RwLock<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
    listeners: Mutex<HashMap<String, Vec<Listener>>>,
    ui: async_channel::Sender<UiMessage>,
    stores: Mutex<HashMap<PathBuf, Arc<Store>>>,
}

#[derive(Clone)]
pub struct AppHandle {
    inner: Arc<Inner>,
}

static GLOBAL: OnceLock<AppHandle> = OnceLock::new();
static MAIN_THREAD: OnceLock<std::thread::ThreadId> = OnceLock::new();

/// True on the thread that created the handle, which is the GPUI main thread.
pub fn is_main_thread() -> bool {
    MAIN_THREAD.get() == Some(&std::thread::current().id())
}

impl AppHandle {
    /// Creates the handle and the receiving end the UI thread drains.
    pub fn new() -> (Self, async_channel::Receiver<UiMessage>) {
        let (tx, rx) = async_channel::unbounded();
        let handle = Self {
            inner: Arc::new(Inner {
                state: RwLock::new(HashMap::new()),
                listeners: Mutex::new(HashMap::new()),
                ui: tx,
                stores: Mutex::new(HashMap::new()),
            }),
        };
        let _ = GLOBAL.set(handle.clone());
        let _ = MAIN_THREAD.set(std::thread::current().id());
        (handle, rx)
    }

    /// The handle created at startup. Used by UI code, which has no handle
    /// threaded through it.
    pub fn global() -> &'static AppHandle {
        GLOBAL
            .get()
            .expect("AppHandle::new must run before AppHandle::global")
    }

    pub fn try_global() -> Option<&'static AppHandle> {
        GLOBAL.get()
    }

    /// Stores `value` as managed state. Returns `false` when a value of this
    /// type was already managed; the existing value is kept, as in Tauri.
    pub fn manage<T: Send + Sync + 'static>(&self, value: T) -> bool {
        let mut state = self.inner.state.write();
        if state.contains_key(&TypeId::of::<T>()) {
            return false;
        }
        state.insert(TypeId::of::<T>(), Arc::new(value));
        true
    }

    /// Returns managed state of type `T`. Panics when it was never managed,
    /// which is a startup-order bug.
    pub fn state<T: Send + Sync + 'static>(&self) -> State<T> {
        self.try_state::<T>().unwrap_or_else(|| {
            panic!(
                "state() called before manage() for {}",
                std::any::type_name::<T>()
            )
        })
    }

    pub fn try_state<T: Send + Sync + 'static>(&self) -> Option<State<T>> {
        let value = self.inner.state.read().get(&TypeId::of::<T>())?.clone();
        value.downcast::<T>().ok().map(State)
    }

    /// Sends an event to the UI and to backend listeners registered with
    /// [`AppHandle::listen`].
    pub fn emit<S: Serialize>(&self, event: &str, payload: S) -> Result<(), Error> {
        let event = AppEvent {
            name: event.to_string(),
            payload: serde_json::to_value(payload)?,
        };
        let listeners = self
            .inner
            .listeners
            .lock()
            .get(&event.name)
            .cloned()
            .unwrap_or_default();
        for listener in listeners {
            listener(&event);
        }
        self.inner
            .ui
            .try_send(UiMessage::Event(event))
            .map_err(|e| anyhow::anyhow!("UI channel closed: {e}"))
    }

    /// Runs `handler` on the emitting thread for every `event`.
    pub fn listen(&self, event: &str, handler: impl Fn(&AppEvent) + Send + Sync + 'static) {
        self.inner
            .listeners
            .lock()
            .entry(event.to_string())
            .or_default()
            .push(Arc::new(handler));
    }

    /// Queues `f` to run on the GPUI main thread.
    pub fn with_ui(&self, f: impl FnOnce(&mut gpui::App) + Send + 'static) {
        if self
            .inner
            .ui
            .try_send(UiMessage::Call(Box::new(f)))
            .is_err()
        {
            log::warn!("UI channel closed; dropping main-thread call");
        }
    }

    /// Tauri-compatible form of [`AppHandle::with_ui`] for closures that only
    /// need to be on the main thread (AppKit calls, for example).
    pub fn run_on_main_thread(&self, f: impl FnOnce() + Send + 'static) -> Result<(), Error> {
        self.with_ui(move |_| f());
        Ok(())
    }

    /// Runs `f` on the main thread and waits for its result. Runs inline when
    /// already on the main thread. Platform APIs that bind to the main run
    /// loop (global hotkeys, AppKit) go through here.
    pub fn run_on_main_thread_sync<R: Send + 'static>(
        &self,
        f: impl FnOnce() -> R + Send + 'static,
    ) -> Result<R, Error> {
        if is_main_thread() {
            return Ok(f());
        }
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.with_ui(move |_| {
            let _ = tx.send(f());
        });
        rx.recv()
            .map_err(|_| anyhow::anyhow!("main thread dropped the call"))
    }

    /// Quits the app. `code` is ignored, as in Tauri's `exit` for code 0.
    pub fn exit(&self, code: i32) {
        if code != 0 {
            log::warn!("exit({code}) requested; GPUI quits with status 0");
        }
        self.with_ui(|cx| cx.quit());
    }

    /// Relaunches the app from the same executable.
    pub fn restart(&self) {
        self.with_ui(|cx| cx.restart());
    }

    pub fn path(&self) -> PathResolver {
        PathResolver
    }

    pub fn clipboard(&self) -> crate::system_clipboard::Clipboard {
        crate::system_clipboard::Clipboard
    }

    /// Opens (or returns the already-open) JSON store at `path`. A relative
    /// path resolves against the app data directory, like tauri-plugin-store.
    pub fn store(&self, path: impl AsRef<Path>) -> Result<Arc<Store>, Error> {
        let path = path.as_ref();
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.path().app_data_dir()?.join(path)
        };
        let mut stores = self.inner.stores.lock();
        if let Some(store) = stores.get(&path) {
            return Ok(store.clone());
        }
        let store = Arc::new(Store::load(path.clone()));
        stores.insert(path, store.clone());
        Ok(store)
    }
}

/// Managed state returned by [`AppHandle::state`].
pub struct State<T>(Arc<T>);

impl<T> State<T> {
    pub fn inner(&self) -> &T {
        &self.0
    }
}

impl<T> Deref for State<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

/// Where Bespoke keeps its files, in folders named after [`APP_IDENTIFIER`].
pub struct PathResolver;

impl PathResolver {
    /// macOS: `~/Library/Application Support/com.andynguyen.bespoke`,
    /// Windows: `%APPDATA%\com.andynguyen.bespoke`,
    /// Linux: `~/.local/share/com.andynguyen.bespoke`.
    pub fn app_data_dir(&self) -> Result<PathBuf, Error> {
        dirs::data_dir()
            .map(|dir| dir.join(APP_IDENTIFIER))
            .ok_or_else(|| anyhow::anyhow!("no data directory for this platform"))
    }

    /// macOS: `~/Library/Logs/com.andynguyen.bespoke`, elsewhere
    /// `<local data>/com.andynguyen.bespoke/logs`.
    pub fn app_log_dir(&self) -> Result<PathBuf, Error> {
        #[cfg(target_os = "macos")]
        let dir = dirs::home_dir().map(|home| home.join("Library/Logs").join(APP_IDENTIFIER));
        #[cfg(not(target_os = "macos"))]
        let dir = dirs::data_local_dir().map(|dir| dir.join(APP_IDENTIFIER).join("logs"));
        dir.ok_or_else(|| anyhow::anyhow!("no log directory for this platform"))
    }

    pub fn app_config_dir(&self) -> Result<PathBuf, Error> {
        dirs::config_dir()
            .map(|dir| dir.join(APP_IDENTIFIER))
            .ok_or_else(|| anyhow::anyhow!("no config directory for this platform"))
    }

    pub fn app_cache_dir(&self) -> Result<PathBuf, Error> {
        dirs::cache_dir()
            .map(|dir| dir.join(APP_IDENTIFIER))
            .ok_or_else(|| anyhow::anyhow!("no cache directory for this platform"))
    }

    /// The directory bundled resources live in. Relative resource paths in
    /// the code start with `resources/`, as in the Tauri bundle.
    pub fn resource_dir(&self) -> Result<PathBuf, Error> {
        Ok(resource_dir().to_path_buf())
    }

    pub fn resolve(&self, path: impl AsRef<Path>, base: BaseDirectory) -> Result<PathBuf, Error> {
        let base = match base {
            BaseDirectory::Resource => self.resource_dir()?,
            BaseDirectory::AppData => self.app_data_dir()?,
            BaseDirectory::AppLog => self.app_log_dir()?,
            BaseDirectory::AppConfig => self.app_config_dir()?,
        };
        Ok(base.join(path))
    }
}

#[derive(Clone, Copy, Debug)]
pub enum BaseDirectory {
    Resource,
    AppData,
    AppLog,
    AppConfig,
}

/// Renames each folder named [`LEGACY_IDENTIFIER`] (data, config, cache and,
/// on macOS, logs) to [`APP_IDENTIFIER`], so settings, history, recordings
/// and downloaded models carry over from Handy. A folder moves only while
/// Bespoke's does not exist yet, so later launches do nothing. Runs before
/// logging starts, so it returns what happened for the caller to log.
pub fn migrate_legacy_dirs() -> Vec<String> {
    let mut bases = vec![
        dirs::data_dir(),
        dirs::config_dir(),
        dirs::cache_dir(),
        dirs::data_local_dir(),
    ];
    #[cfg(target_os = "macos")]
    bases.push(dirs::home_dir().map(|home| home.join("Library/Logs")));

    let mut notes = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for base in bases.into_iter().flatten() {
        if !seen.insert(base.clone()) {
            continue;
        }
        let legacy = base.join(LEGACY_IDENTIFIER);
        let current = base.join(APP_IDENTIFIER);
        if !legacy.is_dir() || current.exists() {
            continue;
        }
        notes.push(match std::fs::rename(&legacy, &current) {
            Ok(()) => format!("Moved {} to {}", legacy.display(), current.display()),
            Err(e) => format!("Failed to move {}: {e}", legacy.display()),
        });
    }
    notes
}

/// Finds the directory that contains `resources/`: next to the executable on
/// Windows and Linux, `Contents/Resources` in a macOS bundle, and the crate
/// root during `cargo run`.
fn resource_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        let mut candidates = Vec::new();
        if let Some(exe_dir) = &exe_dir {
            candidates.push(exe_dir.join("../Resources"));
            candidates.push(exe_dir.to_path_buf());
            candidates.push(exe_dir.join("../lib/Bespoke"));
        }
        candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        candidates
            .into_iter()
            .find(|dir| dir.join("resources").is_dir())
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
    })
}

/// A JSON key-value file compatible with tauri-plugin-store: a pretty-printed
/// object of top-level keys. Every `set` writes the file.
pub struct Store {
    path: PathBuf,
    values: Mutex<serde_json::Map<String, serde_json::Value>>,
}

impl Store {
    fn load(path: PathBuf) -> Self {
        let values = std::fs::read(&path)
            .ok()
            .and_then(|bytes| match serde_json::from_slice(&bytes) {
                Ok(serde_json::Value::Object(map)) => Some(map),
                Ok(_) => None,
                Err(e) => {
                    log::error!("Failed to parse store {}: {}", path.display(), e);
                    None
                }
            })
            .unwrap_or_default();
        Self {
            path,
            values: Mutex::new(values),
        }
    }

    pub fn get(&self, key: &str) -> Option<serde_json::Value> {
        self.values.lock().get(key).cloned()
    }

    pub fn has(&self, key: &str) -> bool {
        self.values.lock().contains_key(key)
    }

    pub fn set(&self, key: impl Into<String>, value: serde_json::Value) {
        let mut values = self.values.lock();
        values.insert(key.into(), value);
        self.write(&values);
    }

    pub fn delete(&self, key: &str) -> bool {
        let mut values = self.values.lock();
        let removed = values.remove(key).is_some();
        if removed {
            self.write(&values);
        }
        removed
    }

    /// Kept for call sites written against tauri-plugin-store; `set` already
    /// persists.
    pub fn save(&self) -> Result<(), Error> {
        Ok(())
    }

    fn write(&self, values: &serde_json::Map<String, serde_json::Value>) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let result = serde_json::to_vec_pretty(values)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| {
                // Write to a sibling file and rename, so a crash mid-write can
                // never leave a truncated settings file behind.
                let tmp = self.path.with_extension("json.tmp");
                std::fs::write(&tmp, bytes)?;
                std::fs::rename(&tmp, &self.path)?;
                Ok(())
            });
        if let Err(e) = result {
            log::error!("Failed to write store {}: {}", self.path.display(), e);
        }
    }
}
