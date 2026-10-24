//! File operations the webview performs on the app's own folders, plus the
//! export targets the save dialog returns.
//!
//! The frontend used `@tauri-apps/plugin-fs` for these, authorised by a runtime
//! ACL capability scoped to `<install dir>/AppData`. That capability failed to
//! deserialise on Windows (`error deserializing scope: data did not match any
//! variant of untagged enum EntryRaw`), and a static capability cannot name an
//! install-dependent path at all, so the work moved here instead.
//!
//! Paths are accepted as given: the save/open dialogs decide what the user may
//! touch, and exports routinely land next to the imported media on another
//! volume. Only "absolute, no `..`" is enforced.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Serialize;
use tauri::AppHandle;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntryInfo {
    pub name: String,
    pub is_file: bool,
    pub is_directory: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileStat {
    pub size: u64,
    pub is_file: bool,
    pub is_directory: bool,
    pub modified_ms: Option<u64>,
}

/// Validate a path handed over by the webview.
///
/// The save/open dialogs already gate what the user picks, and transcripts,
/// exports and media can live on any volume (`F:\迅雷下载\...` is a normal case),
/// so there is no folder allow-list: the checks are "absolute, and no `..`
/// traversal". Anything else (undoing a user's own choice of folder) just breaks
/// exporting to their media folder.
fn check_path(_app: &AppHandle, raw: &str) -> Result<PathBuf, String> {
    if raw.trim().is_empty() {
        return Err("empty path".to_string());
    }

    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(format!("path must be absolute: {raw}"));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!("path must not contain '..': {raw}"));
    }

    Ok(path.to_path_buf())
}

#[tauri::command]
pub fn app_fs_exists(app: AppHandle, path: String) -> Result<bool, String> {
    Ok(check_path(&app, &path)?.exists())
}

#[tauri::command]
pub fn app_fs_mkdir(app: AppHandle, path: String) -> Result<(), String> {
    let path = check_path(&app, &path)?;
    fs::create_dir_all(&path).map_err(|e| format!("create {}: {e}", path.display()))
}

#[tauri::command]
pub fn app_fs_read_text(app: AppHandle, path: String) -> Result<String, String> {
    let path = check_path(&app, &path)?;
    fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))
}

#[tauri::command]
pub fn app_fs_write_text(app: AppHandle, path: String, contents: String) -> Result<(), String> {
    let path = check_path(&app, &path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    fs::write(&path, contents).map_err(|e| format!("write {}: {e}", path.display()))
}

#[tauri::command]
pub fn app_fs_read_dir(app: AppHandle, path: String) -> Result<Vec<DirEntryInfo>, String> {
    let path = check_path(&app, &path)?;
    let entries = fs::read_dir(&path).map_err(|e| format!("read dir {}: {e}", path.display()))?;

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let file_type = entry.file_type().ok();
        out.push(DirEntryInfo {
            name: entry.file_name().to_string_lossy().into_owned(),
            is_file: file_type.map(|t| t.is_file()).unwrap_or(false),
            is_directory: file_type.map(|t| t.is_dir()).unwrap_or(false),
        });
    }

    Ok(out)
}

#[tauri::command]
pub fn app_fs_stat(app: AppHandle, path: String) -> Result<FileStat, String> {
    let path = check_path(&app, &path)?;
    let metadata = fs::metadata(&path).map_err(|e| format!("stat {}: {e}", path.display()))?;
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64);

    Ok(FileStat {
        size: metadata.len(),
        is_file: metadata.is_file(),
        is_directory: metadata.is_dir(),
        modified_ms,
    })
}

#[tauri::command]
pub fn app_fs_remove(app: AppHandle, path: String, recursive: bool) -> Result<(), String> {
    let path = check_path(&app, &path)?;
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("stat {}: {error}", path.display())),
    };

    if metadata.is_dir() {
        if recursive {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_dir(&path)
        }
    } else {
        fs::remove_file(&path)
    }
    .map_err(|e| format!("remove {}: {e}", path.display()))
}

#[tauri::command]
pub fn app_fs_rename(app: AppHandle, from: String, to: String) -> Result<(), String> {
    let from = check_path(&app, &from)?;
    let to = check_path(&app, &to)?;
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    fs::rename(&from, &to).map_err(|e| format!("rename {} -> {}: {e}", from.display(), to.display()))
}

#[tauri::command]
pub fn app_fs_copy(app: AppHandle, from: String, to: String) -> Result<(), String> {
    let from = check_path(&app, &from)?;
    let to = check_path(&app, &to)?;
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    fs::copy(&from, &to).map_err(|e| format!("copy {} -> {}: {e}", from.display(), to.display()))?;
    Ok(())
}
