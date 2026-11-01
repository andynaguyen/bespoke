use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use parking_lot::{Mutex, RwLock};
use serde::Serialize;

pub const APP_IDENTIFIER: &str = "com.andynguyen.bespoke";
pub const APP_NAME: &str = "Bespoke";

pub const LEGACY_IDENTIFIER: &str = "com.pais.handy";
pub const LEGACY_APP_NAME: &str = "Handy";

pub type Error = anyhow::Error;

pub enum UiMessage {
    Event(AppEvent),
    Call(Box<dyn FnOnce(&mut gpui::App) + Send>),
}

#[derive(Clone, Debug)]
pub struct AppEvent {
    pub name: String,
    pub payload: serde_json::Value,
}

impl AppEvent {
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

pub fn is_main_thread() -> bool {
    MAIN_THREAD.get() == Some(&std::thread::current().id())
}

impl AppHandle {
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

    pub fn global() -> &'static AppHandle {
        GLOBAL
            .get()
            .expect("AppHandle::new must run before AppHandle::global")
    }

    pub fn try_global() -> Option<&'static AppHandle> {
        GLOBAL.get()
    }

    pub fn manage<T: Send + Sync + 'static>(&self, value: T) -> bool {
        let mut state = self.inner.state.write();
        if state.contains_key(&TypeId::of::<T>()) {
            return false;
        }
        state.insert(TypeId::of::<T>(), Arc::new(value));
        true
    }

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

    pub fn listen(&self, event: &str, handler: impl Fn(&AppEvent) + Send + Sync + 'static) {
        self.inner
            .listeners
            .lock()
            .entry(event.to_string())
            .or_default()
            .push(Arc::new(handler));
    }

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

    pub fn restart(&self) {
        self.with_ui(|cx| cx.restart());
    }

    pub fn path(&self) -> PathResolver {
        PathResolver
    }

    pub fn clipboard(&self) -> crate::system_clipboard::Clipboard {
        crate::system_clipboard::Clipboard
    }

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

pub struct PathResolver;

impl PathResolver {
    pub fn app_data_dir(&self) -> Result<PathBuf, Error> {
        dirs::data_dir()
            .map(|dir| dir.join(APP_IDENTIFIER))
            .ok_or_else(|| anyhow::anyhow!("no data directory for this platform"))
    }

    pub fn app_log_dir(&self) -> Result<PathBuf, Error> {
        #[cfg(target_os = "macos")]
        let dir = dirs::home_dir().map(|home| home.join("Library/Logs").join(APP_IDENTIFIER));
        #[cfg(not(target_os = "macos"))]
        let dir = dirs::data_local_dir().map(|dir| dir.join(APP_IDENTIFIER).join("logs"));
        dir.ok_or_else(|| anyhow::anyhow!("no log directory for this platform"))
    }

    pub fn resource_dir(&self) -> Result<PathBuf, Error> {
        Ok(resource_dir().to_path_buf())
    }

    pub fn resolve(&self, path: impl AsRef<Path>, base: BaseDirectory) -> Result<PathBuf, Error> {
        let base = match base {
            BaseDirectory::Resource => self.resource_dir()?,
            BaseDirectory::AppData => self.app_data_dir()?,
        };
        Ok(base.join(path))
    }
}

#[derive(Clone, Copy, Debug)]
pub enum BaseDirectory {
    Resource,
    AppData,
}

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

    pub fn set(&self, key: impl Into<String>, value: serde_json::Value) {
        let mut values = self.values.lock();
        values.insert(key.into(), value);
        self.write(&values);
    }

    fn write(&self, values: &serde_json::Map<String, serde_json::Value>) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let result = serde_json::to_vec_pretty(values)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| {
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
