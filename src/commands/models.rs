use crate::app_handle::{AppHandle, State};
use crate::managers::model::ModelManager;
use crate::managers::transcription::{ModelStateEvent, TranscriptionManager};
use crate::settings::{self, get_settings};
use log::error;
use std::sync::Arc;

pub async fn download_model(
    app_handle: AppHandle,
    model_manager: State<Arc<ModelManager>>,
    model_id: String,
) -> Result<(), String> {
    let result = model_manager
        .download_model(&model_id)
        .await
        .map_err(|e| e.to_string());

    if let Err(ref error) = result {
        error!("Model download failed for {}: {}", model_id, error);
        let _ = app_handle.emit(
            "model-download-failed",
            serde_json::json!({ "model_id": &model_id, "error": error }),
        );
    }

    result
}

pub async fn delete_model(
    app_handle: AppHandle,
    model_manager: State<Arc<ModelManager>>,
    transcription_manager: State<Arc<TranscriptionManager>>,
    model_id: String,
) -> Result<(), String> {
    let settings = get_settings(&app_handle);
    if settings.selected_model == model_id {
        transcription_manager.unload_model();

        settings::update(&app_handle, |settings| {
            settings.selected_model = String::new()
        });
    }

    model_manager
        .delete_model(&model_id)
        .map_err(|e| e.to_string())
}

pub fn switch_active_model(app: &AppHandle, model_id: &str) -> Result<(), String> {
    let model_manager = app.state::<Arc<ModelManager>>();

    let model_info = model_manager
        .get_model_info(model_id)
        .ok_or_else(|| format!("Model not found: {}", model_id))?;

    if !model_info.is_downloaded {
        return Err(format!("Model not downloaded: {}", model_id));
    }

    settings::update(app, |settings| {
        settings.selected_model = model_id.to_string();
        settings.onboarding_completed = true;
    });

    let _ = app.emit(
        "model-state-changed",
        ModelStateEvent {
            event_type: "selection_changed".to_string(),
            model_id: Some(model_id.to_string()),
            model_name: Some(model_info.name.clone()),
            error: None,
        },
    );
    log::info!("Model selection changed to {}", model_id);

    Ok(())
}
