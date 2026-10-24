//! Portable data root: `<install dir>/AppData`.
//!
//! Every file the app owns used to be scattered across the user profile:
//!
//! | data | old location |
//! | --- | --- |
//! | `autosubs-store.json`, `autosubs-presets.json`, `.window-state.json` | `%APPDATA%\com.autosubs` (`app_config_dir`) |
//! | models, `normalized_audio.wav`, caption previews | `%LOCALAPPDATA%\com.autosubs` / platform cache dirs |
//! | logs | `app_log_dir` |
//! | Resolve mailbox | `%LOCALAPPDATA%\com.autosubs\resolve-bridge` |
//! | WebView2 profile (`EBWebView`) | `app_local_data_dir` |
//!
//! They now all live under the install directory, so a copy of the folder is
//! the whole application state and nothing is left behind in the user profile:
//!
//! ```text
//! <install dir>/
//!   AutoSubs.exe
//!   config.json            <- translation provider (see crate::app_config)
//!   AppData/
//!     autosubs-store.json
//!     autosubs-presets.json
//!     window-state.json
//!     Audio/
//!     Transcripts/          <- saved subtitle documents
//!     caption-previews/
//!     logs/
//!     models/
//!     resolve-bridge/       <- Resolve file mailbox
//!     webview/              <- WebView2 user data folder
//! ```
//!
//! The one case that cannot work is an install directory the user may not
//! write to (a machine-wide install under `Program Files`, or a read-only
//! package path on Linux). There we fall back to the per-user app data folder
//! instead of failing to start; `data_root()` logs which one is in use.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Folder created next to the executable.
pub const DATA_DIR_NAME: &str = "AppData";

/// File the translation provider is configured in (install directory, not
/// inside `AppData`, so it is easy to find and hand-edit).
pub const CONFIG_FILE_NAME: &str = "config.json";

/// Saved subtitle documents (written by the frontend through the fs plugin).
pub const TRANSCRIPTS_DIR: &str = "Transcripts";

/// Audio renders/exports, mostly produced by Resolve and Premiere bridges.
pub const AUDIO_DIR: &str = "Audio";

/// Directory containing the running executable.
///
/// On macOS this is the `Contents` directory inside the `.app` bundle: the
/// bundle's parent (`/Applications`) is not user-writable, while `Contents`
/// belongs to whoever installed the app.
pub fn install_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;

    #[cfg(target_os = "macos")]
    {
        let macos = exe.parent()?;
        let contents = macos.parent()?;
        if macos.file_name().and_then(|s| s.to_str()) == Some("MacOS")
            && contents.file_name().and_then(|s| s.to_str()) == Some("Contents")
        {
            return Some(contents.to_path_buf());
        }
    }

    exe.parent().map(Path::to_path_buf)
}

static DATA_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Root of every file the app writes, created on first use.
pub fn data_root() -> PathBuf {
    DATA_ROOT.get_or_init(resolve_data_root).clone()
}

fn resolve_data_root() -> PathBuf {
    if let Some(root) = install_dir().map(|dir| dir.join(DATA_DIR_NAME)) {
        match fs::create_dir_all(&root) {
            Ok(()) => {
                migrate_legacy(&root);
                return root;
            }
            Err(error) => {
                tracing::warn!(
                    "cannot use the portable data folder {}: {error}; falling back to the per-user app data folder",
                    root.display()
                );
            }
        }
    }

    let fallback = legacy_data_root().unwrap_or_else(std::env::temp_dir);
    let _ = fs::create_dir_all(&fallback);
    tracing::info!("using fallback data folder {}", fallback.display());
    fallback
}

/// The per-user folder that used to hold everything, and that the Resolve
/// Lua bridge still falls back to when the launcher could not bake a path in.
///
/// `data_local_dir` is deliberate: it matches the Lua side's
/// `%LOCALAPPDATA%` / `$XDG_DATA_HOME` / `~/Library/Application Support`
/// mailbox fallback, so app and bridge agree even on the fallback path.
fn legacy_data_root() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join("com.autosubs"))
}

/// Directory inside the data root, created if needed.
pub fn subdir(name: &str) -> PathBuf {
    let path = data_root().join(name);
    if let Err(error) = fs::create_dir_all(&path) {
        tracing::warn!("could not create {}: {error}", path.display());
    }
    path
}

/// File inside the data root (parent directories are created).
pub fn file(name: &str) -> PathBuf {
    let path = data_root().join(name);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    path
}

/// `<install dir>/config.json`.
pub fn config_path() -> PathBuf {
    install_dir()
        .unwrap_or_else(data_root)
        .join(CONFIG_FILE_NAME)
}

/// Carry the previous per-user data over on first launch, so updating to the
/// portable build does not look like a fresh install.
fn migrate_legacy(target: &Path) {
    let mut legacy_dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = dirs::data_dir() {
        legacy_dirs.push(dir.join("com.autosubs"));
    }
    if let Some(dir) = legacy_data_root() {
        if !legacy_dirs.contains(&dir) {
            legacy_dirs.push(dir);
        }
    }

    // The small JSON stores are the part users notice losing.
    for name in ["autosubs-store.json", "autosubs-presets.json"] {
        let destination = target.join(name);
        if destination.exists() {
            continue;
        }
        if let Some(source) = legacy_dirs.iter().map(|dir| dir.join(name)).find(|p| p.is_file()) {
            match fs::copy(&source, &destination) {
                Ok(_) => tracing::info!("migrated {} into the portable data folder", name),
                Err(error) => {
                    tracing::warn!("could not migrate {}: {error}", source.display())
                }
            }
        }
    }

    // Model cache: a rename keeps multi-GB downloads instead of refetching.
    let models = target.join("models");
    if !models.exists() {
        for legacy in &legacy_dirs {
            let source = legacy.join("models");
            if source.is_dir() {
                if let Err(error) = fs::rename(&source, &models) {
                    tracing::warn!("could not move {}: {error}", source.display());
                }
                break;
            }
        }
    }

    // Window geometry is re-saved in the new location on exit; the old file
    // was written by the window-state plugin and has no other use.
    for legacy in &legacy_dirs {
        let _ = fs::remove_file(legacy.join(".window-state.json"));
    }

    // Saved transcripts are user data: move them across instead of leaving the
    // library behind in the old app-local-data folder. Audio exports are
    // temporary renders, so the folder is only created.
    let transcripts = target.join(TRANSCRIPTS_DIR);
    let _ = fs::create_dir_all(&transcripts);
    let _ = fs::create_dir_all(target.join(AUDIO_DIR));

    if let Some(legacy) = legacy_dirs
        .iter()
        .map(|dir| dir.join(TRANSCRIPTS_DIR))
        .find(|path| path.is_dir())
    {
        migrate_transcripts(&legacy, &transcripts);
    }
}

/// Move every file from the old transcripts directory into the new one. A
/// destination file always wins, so a half-finished earlier run cannot clobber
/// anything. Best effort by design: failing to migrate is logged, not fatal.
fn migrate_transcripts(legacy: &Path, target: &Path) {
    let Ok(entries) = fs::read_dir(legacy) else {
        return;
    };

    let mut moved = 0usize;
    for entry in entries.flatten() {
        let source = entry.path();
        if !source.is_file() {
            continue;
        }
        let Some(name) = source.file_name() else {
            continue;
        };
        let destination = target.join(name);
        if destination.exists() {
            continue;
        }
        match fs::rename(&source, &destination) {
            Ok(()) => moved += 1,
            Err(_) => match fs::copy(&source, &destination) {
                Ok(_) => {
                    let _ = fs::remove_file(&source);
                    moved += 1;
                }
                Err(error) => tracing::warn!(
                    "could not migrate transcript {}: {error}",
                    source.display()
                ),
            },
        }
    }

    if moved > 0 {
        tracing::info!("migrated {moved} transcript(s) into the portable data folder");
    }

    // Drop the old directory only when it is empty; leftovers stay visible
    // rather than being deleted behind the user's back.
    let _ = fs::remove_dir(legacy);
}

/// Resolve a path relative to [`data_root`], refusing escapes.
fn resolve_relative(relative: &str) -> Result<PathBuf, String> {
    let normalized = relative.trim().replace('\\', "/");
    let normalized = normalized.trim_start_matches('/');

    if normalized.is_empty()
        || normalized.split('/').any(|part| part.is_empty() || part == "." || part == "..")
        || normalized.contains(':')
    {
        return Err(format!("invalid data path: {relative}"));
    }

    let path = data_root().join(normalized);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    Ok(path)
}

// ─── Commands ──────────────────────────────────────────────────────────────

/// Absolute path of the portable data root (`<install dir>/AppData`).
#[tauri::command]
pub fn get_app_data_dir() -> String {
    data_root().to_string_lossy().into_owned()
}

/// Absolute path of a file inside the data root, e.g. `autosubs-store.json`.
///
/// The frontend needs this to hand the store plugin an absolute path: a
/// relative path is resolved by Tauri against the per-user app data dir.
#[tauri::command]
pub fn get_data_file(relative_path: String) -> Result<String, String> {
    resolve_relative(&relative_path).map(|path| path.to_string_lossy().into_owned())
}

/// Absolute path of `<install dir>/config.json`.
#[tauri::command]
pub fn get_app_config_path() -> String {
    config_path().to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_cannot_escape_the_data_root() {
        assert!(resolve_relative("../etc/passwd").is_err());
        assert!(resolve_relative("logs/../../x").is_err());
        assert!(resolve_relative("").is_err());
        assert!(resolve_relative("C:\\Windows").is_err());
        assert!(resolve_relative("logs/autosubs.log").is_ok());
    }
}
