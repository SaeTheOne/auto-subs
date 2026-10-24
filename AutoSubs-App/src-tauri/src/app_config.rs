//! `<install dir>/config.json` — the user-editable configuration file.
//!
//! Today it only carries the translation provider:
//!
//! * `google` (default) needs no credentials.
//! * `microsoft` talks to the Azure AI Translator v3 API and needs a
//!   subscription key plus the resource region.
//!
//! The file is created with defaults on first launch so there is something to
//! edit, and it is re-read (by modification time) before every translation
//! batch, so switching provider does not require restarting the app. Editing
//! the file while the app runs is the expected workflow; the Settings-adjacent
//! commands below expose the resolved path and provider for diagnostics.

use std::fs;
use std::path::PathBuf;

use transcription_engine::translation_config::{self, Status};

use crate::app_paths;

/// Written once, when the file does not exist yet.
const DEFAULT_CONFIG: &str = r#"{
  "_readme": "AutoSubs settings. Set translation.provider to \"google\" or \"microsoft\". For Microsoft fill translation.microsoft.key (Azure Translator key) and translation.microsoft.region, then set provider to \"microsoft\". Changes apply to the next translation, no restart needed.",
  "translation": {
    "provider": "google",
    "fallback": null,
    "concurrency": 2,
    "google": {
      "endpoint": "https://translate.googleapis.com/translate_a/single"
    },
    "microsoft": {
      "endpoint": "https://api.cognitive.microsofttranslator.com",
      "key": "",
      "region": ""
    }
  }
}
"#;

/// Absolute path of the configuration file.
pub fn path() -> PathBuf {
    app_paths::config_path()
}

/// Create the file with defaults if it is missing, then point the translation
/// engine at it.
pub fn init() {
    let path = path();

    if !path.exists() {
        match fs::write(&path, DEFAULT_CONFIG) {
            Ok(()) => tracing::info!("created default configuration at {}", path.display()),
            Err(error) => tracing::warn!(
                "could not create {}: {error}; translation falls back to defaults",
                path.display()
            ),
        }
    }

    translation_config::set_config_path(path);

    match translation_config::status() {
        Ok(status) => tracing::info!(
            provider = %status.provider,
            fallback = status.fallback.as_deref().unwrap_or("none"),
            microsoft = status.microsoft_configured,
            path = %status.config_path,
            "translation provider resolved"
        ),
        Err(error) => tracing::warn!("could not read the translation configuration: {error}"),
    }
}

/// Which translation service is active, for diagnostics and the UI.
#[tauri::command]
pub fn get_translation_status() -> Result<Status, String> {
    translation_config::status()
}

/// Re-read `config.json` (the loader already notices edits by mtime; this is
/// for an explicit refresh from the UI).
#[tauri::command]
pub fn reload_translation_config() -> Result<Status, String> {
    translation_config::reload()
}
