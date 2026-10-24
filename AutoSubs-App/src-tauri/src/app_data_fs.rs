//! File operations the webview performs on the app's own folders.
//!
//! The frontend used `@tauri-apps/plugin-fs` for these, authorised by a runtime
//! ACL capability scoped to `<install dir>/AppData`. That capability failed to
//! deserialise on Windows (`error deserializing scope: data did not match any
//! variant of untagged enum EntryRaw`), and a static capability cannot name an
//! install-dependent path at all, so the work moved here instead:
//!
//! * every path is resolved to an absolute path and checked against an explicit
//!   allow-list of roots (the portable data folder plus the user folders the
//!   save/open dialogs hand us);
//! * the commands cover exactly what `src/utils/file-utils.ts` and the document
//!   contexts need, so the fs plugin is no longer involved in app data.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Serialize;
use tauri::{AppHandle, Manager};

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

/// Folders the webview may touch through these commands.
fn allowed_roots(app: &AppHandle) -> Vec<PathBuf> {
    let mut roots = vec![crate::app_paths::data_root()];

    let resolver = app.path();
    for dir in [
        resolver.document_dir(),
        resolver.download_dir(),
        resolver.desktop_dir(),
        resolver.video_dir(),
        resolver.audio_dir(),
        resolver.picture_dir(),
    ]
    .into_iter()
    .flatten()
    {
        roots.push(dir);
    }

    roots
}

/// Canonicalise the longest existing prefix and re-append the rest, so a path
/// that does not exist yet still compares against the roots.
fn normalize(path: &Path) -> PathBuf {
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path.to_path_buf();

    loop {
        if let Ok(canonical) = fs::canonicalize(&current) {
            let mut out = canonical;
            for part in suffix.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match current.file_name() {
            Some(name) => {
                suffix.push(name.to_os_string());
                if !current.pop() {
                    break;
                }
            }
            None => break,
        }
    }

    path.to_path_buf()
}

fn check_path(app: &AppHandle, raw: &str) -> Result<PathBuf, String> {
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

    let normalized = normalize(path);
    let allowed = allowed_roots(app)
        .into_iter()
        .map(|root| normalize(&root))
        .any(|root| normalized.starts_with(&root));

    if allowed {
        Ok(path.to_path_buf())
    } else {
        Err(format!("path is outside the app's folders: {raw}"))
    }
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

#[cfg(test)]
mod tests {
    use super::normalize;
    use std::path::PathBuf;

    #[test]
    fn normalize_keeps_a_missing_tail() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(normalize(&path).exists(), "{:?}", normalize(&path));
    }
}
