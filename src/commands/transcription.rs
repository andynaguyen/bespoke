use crate::app_handle::AppHandle;
use crate::settings::{get_settings, write_settings, ModelUnloadTimeout};

pub fn set_model_unload_timeout(app: AppHandle, timeout: ModelUnloadTimeout) {
    let mut settings = get_settings(&app);
    settings.model_unload_timeout = timeout;
    write_settings(&app, settings);
}
