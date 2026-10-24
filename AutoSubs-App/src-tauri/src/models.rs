use tauri::{AppHandle, Runtime};

/// Cache directory for whisper/diarization models.
///
/// `<install dir>/AppData/models` — the model downloads are the largest thing
/// the app stores, so they move with the install folder instead of sitting in
/// the per-user cache.
pub fn get_cache_dir<R: Runtime>(_app: AppHandle<R>) -> Result<std::path::PathBuf, String> {
    let model_dir = crate::app_paths::subdir("models");

    std::fs::create_dir_all(&model_dir)
        .map_err(|e| format!("Failed to create model cache directory: {}", e))?;

    Ok(model_dir)
}

/// Lists all downloaded Whisper models from the portable model cache
#[tauri::command]
pub fn get_downloaded_models<R: Runtime>(app: AppHandle<R>) -> Result<Vec<String>, String> {
    let model_dir = get_cache_dir(app)?;

    let models = transcription_engine::list_cached_models(&model_dir)
    .map_err(|e| format!("Failed to list cached models: {}", e))?;

    Ok(models)
}

/// Deletes a specific Whisper model from the portable model cache
#[tauri::command]
pub fn delete_model<R: Runtime>(model: &str, app: AppHandle<R>) -> Result<(), String> {
    let model_dir = get_cache_dir(app)?;

    let deleted = transcription_engine::delete_cached_model(&model_dir, model);
    if deleted {
        Ok(())
    } else {
        Err(format!("Failed to delete model '{}' (model may not exist)", model))
    }
}
