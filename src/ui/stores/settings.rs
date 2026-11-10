use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;

use gpui::{App, AppContext, Context, Entity, Global, SharedString};

use crate::shortcut::BindingResponse;

use crate::app_handle::{AppEvent, AppHandle};
use crate::commands::audio::{AudioDevice, CustomSounds};
use crate::settings::{self, AppSettings};
use crate::ui::event_bus::event_bus;

pub struct SettingsStore {
    app: AppHandle,
    pub settings: AppSettings,
    pub defaults: AppSettings,
    /// Keys with work in flight that is not a settings write, such as a model list fetch.
    /// Writes are tracked by `writes`.
    updating: HashSet<SharedString>,
    writes: WriteQueue<Write>,
    pub audio_devices: Vec<AudioDevice>,
    pub output_devices: Vec<AudioDevice>,
    pub custom_sounds: CustomSounds,
    pub post_process_models: HashMap<String, Vec<String>>,
}

/// A settings command that waits its turn in the `WriteQueue`, and what to run on the main
/// thread once it ends.
struct Write {
    job: Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
    on_done: Option<OnDone>,
}

type OnDone = Box<dyn FnOnce(Result<(), String>, &mut App)>;

/// Orders the writes for each setting. One write per key runs at a time. A write that
/// arrives while another runs waits its turn, so writes for one key land in the order
/// they were made.
struct WriteQueue<J> {
    running: HashSet<SharedString>,
    waiting: HashMap<SharedString, VecDeque<J>>,
}

impl<J> Default for WriteQueue<J> {
    fn default() -> Self {
        Self {
            running: HashSet::new(),
            waiting: HashMap::new(),
        }
    }
}

impl<J> WriteQueue<J> {
    /// Hands `job` back when it can start now. Otherwise it waits behind the running one
    /// and any that arrived before it.
    fn submit(&mut self, key: &SharedString, job: J) -> Option<J> {
        if self.running.contains(key) {
            self.waiting.entry(key.clone()).or_default().push_back(job);
            None
        } else {
            self.running.insert(key.clone());
            Some(job)
        }
    }

    /// Call when the running job for `key` ends. Returns the oldest job that waited, which
    /// the caller must start, and which now counts as running.
    fn finished(&mut self, key: &SharedString) -> Option<J> {
        let next = self.waiting.get_mut(key).and_then(VecDeque::pop_front);
        if self.waiting.get(key).is_some_and(VecDeque::is_empty) {
            self.waiting.remove(key);
        }
        if next.is_none() {
            self.running.remove(key);
        }
        next
    }

    /// Whether a write for `key` runs or waits.
    fn is_busy(&self, key: &str) -> bool {
        self.running.contains(key)
    }

    /// Whether nothing runs and nothing waits.
    fn is_idle(&self) -> bool {
        self.running.is_empty() && self.waiting.is_empty()
    }
}

struct GlobalSettingsStore(Entity<SettingsStore>);

impl Global for GlobalSettingsStore {}

fn default_device() -> AudioDevice {
    AudioDevice {
        index: "default".into(),
        name: "Default".into(),
        is_default: true,
    }
}

fn with_default_device(devices: Vec<AudioDevice>) -> Vec<AudioDevice> {
    std::iter::once(default_device())
        .chain(
            devices
                .into_iter()
                .filter(|d| !d.name.eq_ignore_ascii_case("default")),
        )
        .collect()
}

impl SettingsStore {
    pub fn init(cx: &mut App) {
        let store = cx.new(Self::new);
        cx.set_global(GlobalSettingsStore(store));
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalSettingsStore>().0.clone()
    }

    pub fn get(cx: &App) -> &AppSettings {
        &cx.global::<GlobalSettingsStore>().0.read(cx).settings
    }

    fn new(cx: &mut Context<Self>) -> Self {
        let app = AppHandle::global().clone();
        cx.subscribe(
            &event_bus(cx),
            |this, _, event: &AppEvent, cx| match event.name.as_str() {
                "model-state-changed" => this.refresh(cx),
                "settings-changed" => this.refresh(cx),
                _ => {}
            },
        )
        .detach();

        Self {
            settings: settings::get_settings(&app),
            defaults: settings::get_default_settings(),
            updating: HashSet::new(),
            writes: WriteQueue::default(),
            audio_devices: Vec::new(),
            output_devices: Vec::new(),
            custom_sounds: crate::commands::audio::check_custom_sounds(app.clone()),
            post_process_models: HashMap::new(),
            app,
        }
    }

    pub fn app(&self) -> &AppHandle {
        &self.app
    }

    /// Reloads the settings from disk. While a write runs or waits, the reload waits for the
    /// last write to finish, so it cannot replace a value the UI shows but has not saved yet.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.writes.is_idle() {
            self.settings = settings::get_settings(&self.app);
        }
        cx.notify();
    }

    pub fn is_updating(&self, key: &str) -> bool {
        self.updating.contains(key) || self.writes.is_busy(key)
    }

    pub fn set_updating(
        &mut self,
        key: impl Into<SharedString>,
        updating: bool,
        cx: &mut Context<Self>,
    ) {
        let key = key.into();
        if updating {
            self.updating.insert(key);
        } else {
            self.updating.remove(&key);
        }
        cx.notify();
    }

    pub fn update(
        &mut self,
        key: &str,
        apply: impl FnOnce(&mut AppSettings),
        command: impl FnOnce(AppHandle) -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let app = self.app.clone();
        self.update_async(key, apply, async move { command(app) }, cx);
    }

    pub fn update_async(
        &mut self,
        key: &str,
        apply: impl FnOnce(&mut AppSettings),
        command: impl Future<Output = Result<(), String>> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.write(key, apply, command, None, cx);
    }

    /// Like `update_async`, then runs `on_done` with the command's result once it ends.
    pub fn update_async_then(
        &mut self,
        key: &str,
        apply: impl FnOnce(&mut AppSettings),
        command: impl Future<Output = Result<(), String>> + Send + 'static,
        on_done: impl FnOnce(Result<(), String>, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.write(key, apply, command, Some(Box::new(on_done)), cx);
    }

    /// Shows `apply` at once and queues `command` behind any write for `key` that runs or
    /// waits.
    fn write(
        &mut self,
        key: &str,
        apply: impl FnOnce(&mut AppSettings),
        command: impl Future<Output = Result<(), String>> + Send + 'static,
        on_done: Option<OnDone>,
        cx: &mut Context<Self>,
    ) {
        let key = SharedString::from(key.to_string());
        apply(&mut self.settings);
        let write = Write {
            job: Box::pin(command),
            on_done,
        };
        if let Some(write) = self.writes.submit(&key, write) {
            self.start_write(key, write, cx);
        }
        cx.notify();
    }

    /// Runs one write for `key`, then each write that waited for it, in order. The store
    /// reloads from disk once no write runs or waits.
    fn start_write(&mut self, key: SharedString, write: Write, cx: &mut Context<Self>) {
        let Write { job, on_done } = write;
        cx.spawn(async move |this, cx| {
            let result = crate::runtime::run(job).await;
            if let Err(e) = &result {
                log::error!("Failed to update setting {}: {}", key, e);
            }
            let _ = this.update(cx, |this, cx| {
                if let Some(next) = this.writes.finished(&key) {
                    this.start_write(key, next, cx);
                }
                this.refresh(cx);
            });
            // Outside the store's update, so `on_done` can update the store itself.
            if let Some(on_done) = on_done {
                let _ = cx.update(|cx| on_done(result, cx));
            }
        })
        .detach();
    }

    pub fn refresh_audio_devices(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let devices = crate::runtime::run(crate::commands::audio::get_available_microphones())
                .await
                .unwrap_or_else(|e| {
                    log::error!("Failed to load audio devices: {}", e);
                    Vec::new()
                });
            let _ = this.update(cx, |this, cx| {
                let devices = with_default_device(devices);
                if this.audio_devices != devices {
                    this.audio_devices = devices;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn refresh_output_devices(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let devices =
                crate::runtime::run(crate::commands::audio::get_available_output_devices())
                    .await
                    .unwrap_or_else(|e| {
                        log::error!("Failed to load output devices: {}", e);
                        Vec::new()
                    });
            let _ = this.update(cx, |this, cx| {
                this.output_devices = with_default_device(devices);
                cx.notify();
            });
        })
        .detach();
    }

    pub fn update_binding(
        &mut self,
        id: String,
        binding: String,
        on_done: impl FnOnce(Result<(), String>, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        let key = format!("binding_{id}");
        let app = self.app.clone();
        let (shown_id, shown) = (id.clone(), binding.clone());
        self.update_async_then(
            &key,
            move |settings| {
                if let Some(entry) = settings.bindings.get_mut(&shown_id) {
                    entry.current_binding = shown;
                }
            },
            async move {
                binding_outcome(
                    crate::runtime::spawn_blocking(move || {
                        crate::shortcut::change_binding(app, id, binding)
                    })
                    .await,
                )
            },
            on_done,
            cx,
        );
    }

    pub fn reset_binding(&mut self, id: String, cx: &mut Context<Self>) {
        let key = format!("binding_{id}");
        let app = self.app.clone();
        self.update_async(
            &key,
            |_| {},
            async move {
                binding_outcome(
                    crate::runtime::spawn_blocking(move || crate::shortcut::reset_binding(app, id))
                        .await,
                )
            },
            cx,
        );
    }

    pub fn fetch_post_process_models(&mut self, provider_id: String, cx: &mut Context<Self>) {
        let key = format!("post_process_models_fetch:{provider_id}");
        self.set_updating(key.clone(), true, cx);
        let app = self.app.clone();
        cx.spawn(async move |this, cx| {
            let id = provider_id.clone();
            let result =
                crate::runtime::run(crate::shortcut::fetch_post_process_models(app, id)).await;
            let _ = this.update(cx, |this, cx| {
                this.updating.remove(key.as_str());
                match result {
                    Ok(models) => {
                        this.post_process_models.insert(provider_id, models);
                    }
                    Err(e) => log::error!("Failed to fetch models: {}", e),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// A binding change's result, with a refused change as an error.
fn binding_outcome(
    result: Result<Result<BindingResponse, String>, tokio::task::JoinError>,
) -> Result<(), String> {
    let response = result.map_err(|e| e.to_string())??;
    if response.success {
        Ok(())
    } else {
        Err(response
            .error
            .unwrap_or_else(|| "Failed to update binding".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::WriteQueue;
    use gpui::SharedString;

    fn key(name: &'static str) -> SharedString {
        SharedString::from(name)
    }

    #[test]
    fn the_first_write_for_a_setting_starts_at_once() {
        let mut queue = WriteQueue::<u32>::default();
        let k = key("volume");
        assert_eq!(queue.submit(&k, 1), Some(1));
        assert!(!queue.is_idle());
    }

    #[test]
    fn a_write_waits_for_the_one_that_is_running() {
        let mut queue = WriteQueue::<u32>::default();
        let k = key("volume");
        assert_eq!(queue.submit(&k, 1), Some(1));
        assert_eq!(queue.submit(&k, 2), None);
        assert_eq!(queue.finished(&k), Some(2));
        assert_eq!(queue.finished(&k), None);
        assert!(queue.is_idle());
    }

    #[test]
    fn waiting_writes_run_in_the_order_they_arrived() {
        let mut queue = WriteQueue::<u32>::default();
        let k = key("volume");
        assert_eq!(queue.submit(&k, 1), Some(1));
        assert_eq!(queue.submit(&k, 2), None);
        assert_eq!(queue.submit(&k, 3), None);
        assert_eq!(queue.finished(&k), Some(2));
        assert_eq!(queue.finished(&k), Some(3));
        assert_eq!(queue.finished(&k), None);
        assert!(queue.is_idle());
    }

    #[test]
    fn the_queue_is_busy_while_a_write_waits() {
        let mut queue = WriteQueue::<u32>::default();
        let k = key("volume");
        assert_eq!(queue.submit(&k, 1), Some(1));
        assert_eq!(queue.submit(&k, 2), None);
        assert_eq!(queue.finished(&k), Some(2));
        assert!(!queue.is_idle());
        assert_eq!(queue.finished(&k), None);
        assert!(queue.is_idle());
    }

    #[test]
    fn different_settings_do_not_wait_for_each_other() {
        let mut queue = WriteQueue::<u32>::default();
        assert_eq!(queue.submit(&key("a"), 1), Some(1));
        assert_eq!(queue.submit(&key("b"), 2), Some(2));
    }

    #[test]
    fn the_queue_is_busy_until_the_last_write_ends() {
        let mut queue = WriteQueue::<u32>::default();
        let (a, b) = (key("a"), key("b"));
        assert_eq!(queue.submit(&a, 1), Some(1));
        assert_eq!(queue.submit(&b, 2), Some(2));
        assert_eq!(queue.finished(&a), None);
        assert!(!queue.is_idle());
        assert_eq!(queue.finished(&b), None);
        assert!(queue.is_idle());
    }
}
