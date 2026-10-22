//! Transcription models and their download state, ported from `modelStore.ts`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use gpui::{App, AppContext, Context, Entity, Global, SharedString};
use serde::Deserialize;

use crate::app_handle::{AppEvent, AppHandle};
use crate::managers::model::{DownloadProgress, ModelInfo, ModelManager};
use crate::ui::event_bus::event_bus;
use crate::ui::toast;

/// Download speed bookkeeping, smoothed like the React store.
#[derive(Clone, Copy)]
pub struct DownloadStats {
    last_update: Instant,
    total_downloaded: u64,
    /// MiB/s.
    pub speed: f64,
}

#[derive(Deserialize)]
struct ModelError {
    model_id: String,
    error: String,
}

pub struct ModelStore {
    app: AppHandle,
    pub models: Vec<ModelInfo>,
    pub current_model: String,
    pub downloading: HashSet<String>,
    pub verifying: HashSet<String>,
    pub extracting: HashSet<String>,
    pub progress: HashMap<String, DownloadProgress>,
    pub stats: HashMap<String, DownloadStats>,
    /// True until the first model list loads.
    pub loading: bool,
    pub error: Option<SharedString>,
    pub is_rescanning: bool,
}

struct GlobalModelStore(Entity<ModelStore>);

impl Global for GlobalModelStore {}

impl ModelStore {
    pub fn init(cx: &mut App) {
        let store = cx.new(Self::new);
        cx.set_global(GlobalModelStore(store));
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalModelStore>().0.clone()
    }

    fn new(cx: &mut Context<Self>) -> Self {
        cx.subscribe(&event_bus(cx), |this, _, event: &AppEvent, cx| {
            this.handle_event(event, cx)
        })
        .detach();

        let mut store = Self {
            app: AppHandle::global().clone(),
            models: Vec::new(),
            current_model: String::new(),
            downloading: HashSet::new(),
            verifying: HashSet::new(),
            extracting: HashSet::new(),
            progress: HashMap::new(),
            stats: HashMap::new(),
            loading: true,
            error: None,
            is_rescanning: false,
        };
        store.load_models(cx);
        store.load_current_model(cx);
        store
    }

    fn manager(&self) -> Arc<ModelManager> {
        self.app.state::<Arc<ModelManager>>().inner().clone()
    }

    pub fn model(&self, id: &str) -> Option<&ModelInfo> {
        self.models.iter().find(|m| m.id == id)
    }

    pub fn current_model_info(&self) -> Option<&ModelInfo> {
        self.model(&self.current_model)
    }

    pub fn load_models(&mut self, cx: &mut Context<Self>) {
        if self.app.try_state::<Arc<ModelManager>>().is_none() {
            return;
        }
        self.models = self.manager().get_available_models();
        self.error = None;
        // Merge the backend's downloading flags; drop entries the backend
        // finished and that have no progress left (completed or cancelled).
        let backend: HashSet<String> = self
            .models
            .iter()
            .filter(|m| m.is_downloading)
            .map(|m| m.id.clone())
            .collect();
        self.downloading.extend(backend.iter().cloned());
        let progress = &self.progress;
        self.downloading
            .retain(|id| backend.contains(id) || progress.contains_key(id));
        self.loading = false;
        cx.notify();
    }

    pub fn load_current_model(&mut self, cx: &mut Context<Self>) {
        self.current_model = crate::settings::get_settings(&self.app).selected_model;
        cx.notify();
    }

    pub fn rescan(&mut self, cx: &mut Context<Self>) {
        self.is_rescanning = true;
        cx.notify();
        let manager = self.manager();
        cx.spawn(async move |this, cx| {
            let result =
                crate::runtime::spawn_blocking(move || manager.rescan_local_models()).await;
            let _ = this.update(cx, |this, cx| {
                this.is_rescanning = false;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => this.error = Some(format!("Failed to rescan models: {e}").into()),
                    Err(e) => this.error = Some(format!("Failed to rescan models: {e}").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Switches the active model. `on_done` gets whether it worked.
    pub fn select_model(
        &mut self,
        model_id: String,
        on_done: impl FnOnce(bool, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        let app = self.app.clone();
        cx.spawn(async move |this, cx| {
            let id = model_id.clone();
            let result = crate::runtime::spawn_blocking(move || {
                crate::commands::models::switch_active_model(&app, &id)
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                let ok = match result {
                    Ok(()) => {
                        this.current_model = model_id;
                        true
                    }
                    Err(e) => {
                        this.error = Some(format!("Failed to switch to model: {e}").into());
                        false
                    }
                };
                cx.notify();
                on_done(ok, cx);
            });
        })
        .detach();
    }

    /// Starts a download. `on_done` gets whether the whole download worked.
    pub fn download_model(
        &mut self,
        model_id: String,
        on_done: impl FnOnce(bool, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.downloading.insert(model_id.clone());
        self.progress.insert(
            model_id.clone(),
            DownloadProgress {
                model_id: model_id.clone(),
                downloaded: 0,
                total: 0,
                percentage: 0.,
            },
        );
        cx.notify();

        let app = self.app.clone();
        cx.spawn(async move |this, cx| {
            let id = model_id.clone();
            let result = crate::runtime::run(async move {
                let manager = app.state::<Arc<ModelManager>>();
                crate::commands::models::download_model(app.clone(), manager, id).await
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                let ok = result.is_ok();
                if let Err(e) = result {
                    this.error = Some(format!("Failed to download model: {e}").into());
                    this.clear_download(&model_id);
                }
                cx.notify();
                on_done(ok, cx);
            });
        })
        .detach();
    }

    /// Cancels a download. `on_done` gets whether the cancel worked.
    pub fn cancel_download(
        &mut self,
        model_id: String,
        on_done: impl FnOnce(bool, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        let manager = self.manager();
        cx.spawn(async move |this, cx| {
            let id = model_id.clone();
            let result = crate::runtime::spawn_blocking(move || manager.cancel_download(&id)).await;
            let _ = this.update(cx, |this, cx| {
                let ok = match result {
                    Ok(Ok(())) => {
                        this.clear_download(&model_id);
                        this.load_models(cx);
                        true
                    }
                    Ok(Err(e)) => {
                        this.error = Some(format!("Failed to cancel download: {e}").into());
                        false
                    }
                    Err(e) => {
                        this.error = Some(format!("Failed to cancel download: {e}").into());
                        false
                    }
                };
                cx.notify();
                on_done(ok, cx);
            });
        })
        .detach();
    }

    pub fn delete_model(&mut self, model_id: String, cx: &mut Context<Self>) {
        let app = self.app.clone();
        cx.spawn(async move |this, cx| {
            let result = crate::runtime::run(async move {
                let manager = app.state::<Arc<ModelManager>>();
                let tm = app.state::<Arc<crate::managers::transcription::TranscriptionManager>>();
                crate::commands::models::delete_model(app.clone(), manager, tm, model_id).await
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                if let Err(e) = result {
                    this.error = Some(format!("Failed to delete model: {e}").into());
                }
                this.load_models(cx);
                this.load_current_model(cx);
            });
        })
        .detach();
    }

    fn clear_download(&mut self, model_id: &str) {
        self.downloading.remove(model_id);
        self.verifying.remove(model_id);
        self.progress.remove(model_id);
        self.stats.remove(model_id);
    }

    /// Switches to a model once its download (and any extraction)
    /// finishes, unless the user is recording.
    fn select_when_idle(&mut self, model_id: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;
            if crate::commands::audio::is_recording(AppHandle::global().clone()) {
                return;
            }
            let _ = this.update(cx, |this, cx| this.select_model(model_id, |_, _| {}, cx));
        })
        .detach();
    }

    fn handle_event(&mut self, event: &AppEvent, cx: &mut Context<Self>) {
        let id = || event.payload.as_str().map(str::to_string);
        match event.name.as_str() {
            "model-download-progress" => {
                let Some(progress) = event.parse::<DownloadProgress>() else {
                    return;
                };
                self.record_progress(&progress);
                self.progress.insert(progress.model_id.clone(), progress);
                cx.notify();
            }
            "model-download-complete" | "model-download-cancelled" => {
                if let Some(id) = id() {
                    self.clear_download(&id);
                    cx.notify();
                    if event.name == "model-download-complete" {
                        self.load_models(cx);
                        self.select_when_idle(id, cx);
                    }
                }
            }
            "model-download-failed" => {
                if let Some(failure) = event.parse::<ModelError>() {
                    self.clear_download(&failure.model_id);
                    self.error = Some(failure.error.clone().into());
                    toast::error(failure.error, cx);
                    cx.notify();
                }
            }
            "model-verification-started" => {
                if let Some(id) = id() {
                    self.verifying.insert(id);
                    cx.notify();
                }
            }
            "model-verification-completed" => {
                if let Some(id) = id() {
                    self.verifying.remove(&id);
                    cx.notify();
                }
            }
            "model-extraction-started" => {
                if let Some(id) = id() {
                    self.extracting.insert(id);
                    cx.notify();
                }
            }
            "model-extraction-completed" => {
                if let Some(id) = id() {
                    self.extracting.remove(&id);
                    self.load_models(cx);
                }
            }
            "model-extraction-failed" => {
                if let Some(failure) = event.parse::<ModelError>() {
                    self.extracting.remove(&failure.model_id);
                    self.error = Some(format!("Failed to extract model: {}", failure.error).into());
                    cx.notify();
                }
            }
            "model-deleted" | "model-state-changed" | "models-updated" => {
                self.load_models(cx);
                self.load_current_model(cx);
            }
            _ => {}
        }
    }

    /// Speed in MiB/s, updated at most every half second and smoothed
    /// (80% previous, 20% current).
    fn record_progress(&mut self, progress: &DownloadProgress) {
        let now = Instant::now();
        match self.stats.get_mut(&progress.model_id) {
            None => {
                self.stats.insert(
                    progress.model_id.clone(),
                    DownloadStats {
                        last_update: now,
                        total_downloaded: progress.downloaded,
                        speed: 0.,
                    },
                );
            }
            Some(stats) => {
                let elapsed = now.duration_since(stats.last_update).as_secs_f64();
                if elapsed > 0.5 {
                    let bytes = progress.downloaded.saturating_sub(stats.total_downloaded);
                    let current = (bytes as f64 / (1024. * 1024.) / elapsed).max(0.);
                    stats.speed = if stats.speed > 0. {
                        stats.speed * 0.8 + current * 0.2
                    } else {
                        current
                    };
                    stats.last_update = now;
                    stats.total_downloaded = progress.downloaded;
                }
            }
        }
    }
}
