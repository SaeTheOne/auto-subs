//! Caption preview images: rendered inside Resolve, then shown in the preset
//! list.
//!
//! The images live in the portable data folder (`<install dir>/AppData/
//! caption-previews`) rather than the per-user local app data dir, so the file
//! moves happen here in Rust instead of through the fs plugin: the plugin's
//! capability scope is static (`$APPLOCALDATA/**`) and cannot name a folder
//! that depends on where the app was installed.
//!
//! The folder is also added to the asset-protocol scope at runtime so
//! `convertFileSrc` can serve the images to the webview.

use std::fs;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

const PREVIEW_DIR: &str = "caption-previews";

fn preview_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = crate::app_paths::subdir(PREVIEW_DIR);
    fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;

    if let Err(error) = app.asset_protocol_scope().allow_directory(&dir, true) {
        tracing::warn!(
            "could not extend the asset protocol scope to {}: {error}",
            dir.display()
        );
    }

    Ok(dir)
}

/// A preview file name is derived from the preset id, which is a UUID or a
/// `preset-…` string; anything else is refused rather than joined blindly.
fn preview_file_name(preset_id: &str) -> Result<String, String> {
    let valid = !preset_id.is_empty()
        && preset_id.len() <= 128
        && preset_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');

    if !valid {
        return Err(format!("invalid preset id for a preview file: {preset_id}"));
    }
    Ok(format!("{preset_id}.png"))
}

/// Ensure the preview folder exists and return its absolute path.
#[tauri::command]
pub fn ensure_caption_preview_dir(app: AppHandle) -> Result<String, String> {
    preview_dir(&app).map(|dir| dir.to_string_lossy().into_owned())
}

/// Move a freshly rendered preview into the preview folder, returning its file
/// name (which the presets store keeps).
#[tauri::command]
pub fn store_caption_preview(
    app: AppHandle,
    preset_id: String,
    source_path: String,
) -> Result<String, String> {
    let file_name = preview_file_name(&preset_id)?;
    let dir = preview_dir(&app)?;
    let target = dir.join(&file_name);
    let source = Path::new(&source_path);

    if !source.is_file() {
        return Err(format!("rendered preview not found: {source_path}"));
    }

    // Replace atomically-ish: drop the old image first so a failed rename never
    // leaves the previous preset image behind.
    let _ = fs::remove_file(&target);

    match fs::rename(source, &target) {
        Ok(()) => {}
        Err(_) => {
            // Cross-device move (e.g. a temp dir on another volume).
            fs::copy(source, &target)
                .map_err(|e| format!("copy {} -> {}: {e}", source.display(), target.display()))?;
            let _ = fs::remove_file(source);
        }
    }

    Ok(file_name)
}

/// Delete a preview image. Best effort: a missing file is not an error, the
/// frontend uses this for cleanup.
#[tauri::command]
pub fn delete_caption_preview(
    app: AppHandle,
    file_name: String,
) -> Result<(), String> {
    if !file_name.ends_with(".png")
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
    {
        return Err(format!("invalid preview file name: {file_name}"));
    }

    let path = preview_dir(&app)?.join(file_name);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_ids_are_sanitised() {
        assert!(preview_file_name("preset-123_abc").is_ok());
        assert!(preview_file_name("../x").is_err());
        assert!(preview_file_name("").is_err());
        assert!(preview_file_name("a/b").is_err());
    }
}
