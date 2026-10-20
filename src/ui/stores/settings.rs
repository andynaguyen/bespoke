//! App settings as the UI sees them, ported from `settingsStore.ts`.
//!
//! Writes are optimistic: [`SettingsStore::update`] applies the change to the
//! local copy at once, runs the backend command on a worker thread, then
//! reloads the persisted settings. A failed write therefore rolls back to the
//! stored value instead of lingering until the next refresh.

use std::collections::{HashMap, HashSet};
use std::future::Future;

use gpui::{App, AppContext, Context, Entity, Global, SharedString};

use crate::app_handle::{AppEvent, AppHandle};
use crate::commands::audio::{AudioDevice, CustomSounds};
use crate::settings::{self, AppSettings};
use crate::ui::event_bus::event_bus;

pub struct SettingsStore {
    app: AppHandle,
    pub settings: AppSettings,
    pub defaults: AppSettings,
    updating: HashSet<SharedString>,
    /// Inputs, with "Default" first. Empty until first refreshed.
    pub audio_devices: Vec<AudioDevice>,
    /// Outputs, with "Default" first. Empty until first refreshed.
    pub output_devices: Vec<AudioDevice>,
    pub custom_sounds: CustomSounds,
    /// Cached `/models` results per post-processing provider.
    pub post_process_models: HashMap<String, Vec<String>>,
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

/// Prepends "Default" and drops any backend entry with that name.
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

    /// The current settings snapshot.
    pub fn get(cx: &App) -> &AppSettings {
        &cx.global::<GlobalSettingsStore>().0.read(cx).settings
    }

    fn new(cx: &mut Context<Self>) -> Self {
        let app = AppHandle::global().clone();
        cx.subscribe(
            &event_bus(cx),
            |this, _, event: &AppEvent, cx| match event.name.as_str() {
                "model-state-changed" => this.refresh(cx),
                "settings-changed" => {
                    this.refresh(cx);
                    let setting = event.payload.get("setting").and_then(|s| s.as_str());
                    if setting == Some("selected_microphone") {
                        this.refresh_audio_devices(cx);
                    }
                }
                _ => {}
            },
        )
        .detach();

        Self {
            settings: settings::get_settings(&app),
            defaults: settings::get_default_settings(),
            updating: HashSet::new(),
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

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.settings = settings::get_settings(&self.app);
        cx.notify();
    }

    pub fn is_updating(&self, key: &str) -> bool {
        self.updating.contains(key)
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

    /// Applies `apply` to the local settings now, runs `command` on a worker
    /// thread, then reloads the stored settings.
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

    /// Like [`update`](Self::update) for commands that are `async` (they may
    /// use tokio internally, so they run on the tokio runtime).
    pub fn update_async(
        &mut self,
        key: &str,
        apply: impl FnOnce(&mut AppSettings),
        command: impl Future<Output = Result<(), String>> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let key = SharedString::from(key.to_string());
        apply(&mut self.settings);
        self.set_updating(key.clone(), true, cx);
        cx.spawn(async move |this, cx| {
            let result = crate::runtime::run(command).await;
            let _ = this.update(cx, |this, cx| {
                if let Err(e) = &result {
                    log::error!("Failed to update setting {}: {}", key, e);
                }
                this.updating.remove(&key);
                this.refresh(cx);
            });
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
                this.audio_devices = with_default_device(devices);
                cx.notify();
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

    /// Writes a shortcut binding. On failure the old binding is restored and
    /// `on_done` receives the error.
    pub fn update_binding(
        &mut self,
        id: String,
        binding: String,
        on_done: impl FnOnce(Result<(), String>, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        let key = format!("binding_{id}");
        if let Some(entry) = self.settings.bindings.get_mut(&id) {
            entry.current_binding = binding.clone();
        }
        self.set_updating(key.clone(), true, cx);
        let app = self.app.clone();
        cx.spawn(async move |this, cx| {
            let result = crate::runtime::spawn_blocking(move || {
                crate::shortcut::change_binding(app, id, binding)
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r)
            .and_then(|response| {
                if response.success {
                    Ok(())
                } else {
                    Err(response
                        .error
                        .unwrap_or_else(|| "Failed to update binding".to_string()))
                }
            });
            let _ = this.update(cx, |this, cx| {
                this.updating.remove(key.as_str());
                this.refresh(cx);
                on_done(result, cx);
            });
        })
        .detach();
    }

    pub fn reset_binding(&mut self, id: String, cx: &mut Context<Self>) {
        let key = format!("binding_{id}");
        self.set_updating(key.clone(), true, cx);
        let app = self.app.clone();
        cx.spawn(async move |this, cx| {
            let result =
                crate::runtime::spawn_blocking(move || crate::shortcut::reset_binding(app, id))
                    .await;
            if let Ok(Err(e)) = result {
                log::error!("Failed to reset binding: {}", e);
            }
            let _ = this.update(cx, |this, cx| {
                this.updating.remove(key.as_str());
                this.refresh(cx);
            });
        })
        .detach();
    }

    /// Fetches the model list for a post-processing provider and caches it.
    /// Errors are logged and leave the cache untouched.
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
