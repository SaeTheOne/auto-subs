//! Runtime ACL grant for the portable data folder.
//!
//! Transcript storage and the audio export folder used to sit in Tauri's
//! app-local-data dir, which the `fs` plugin could reach through the static
//! `$APPLOCALDATA/**` capability scope. They now live in
//! `<install dir>/AppData`, a path that depends on where the app was installed,
//! so no static capability can name it.
//!
//! Rather than widening the scope to `**`, this adds one capability at runtime
//! (Tauri's `dynamic-acl` feature is on by default) whose scope is exactly
//! `<install dir>/AppData`. The frontend keeps using the `fs` plugin unchanged;
//! the permissions listed here are the ones its file helpers actually call on
//! those folders.
//!
//! Scopes are additive: the static `default` capability keeps covering user
//! documents, videos and the legacy migration paths.

use serde_json::{json, Value};
use tauri::Manager;

/// `fs` permissions the frontend needs inside the portable data folder.
const FS_PERMISSIONS: &[&str] = &[
    "fs:allow-read-dir",
    "fs:allow-read-text-file",
    "fs:allow-write-text-file",
    "fs:allow-exists",
    "fs:allow-mkdir",
    "fs:allow-stat",
    "fs:allow-rename",
    "fs:allow-copy-file",
    "fs:allow-remove",
];

/// Add the portable-data capability to the running app. Never fatal: without it
/// transcript saving reports a denied path, but the app still starts.
pub fn grant_portable_data_access(app: &tauri::AppHandle) {
    let root = crate::app_paths::data_root();
    let root_display = root.to_string_lossy().to_string();
    let trimmed = root_display.trim_end_matches(['\\', '/']);

    // The capability scope entries are glob patterns, so literal characters
    // that glob treats specially are escaped. `**` then matches everything
    // below the folder.
    let base = escape_glob(trimmed);
    let recursive = format!("{base}{}**", std::path::MAIN_SEPARATOR);

    let permissions: Vec<Value> = FS_PERMISSIONS
        .iter()
        .map(|identifier| {
            json!({
                "identifier": identifier,
                "allow": [ { "path": base }, { "path": recursive } ],
            })
        })
        .collect();

    let capability = json!({
        "identifier": "portable-data",
        "description": "Files AutoSubs keeps in <install dir>/AppData",
        "windows": ["main"],
        "permissions": permissions,
    });

    match app.add_capability(capability.to_string()) {
        Ok(()) => tracing::debug!(path = %root.display(), "granted fs access to the portable data folder"),
        Err(error) => tracing::warn!(
            "could not grant fs access to {}: {error}; transcript saving will be blocked",
            root.display()
        ),
    }
}

/// Escape the glob metacharacters glob would otherwise interpret. Windows
/// paths cannot contain `*` or `?`, but a folder may contain brackets.
fn escape_glob(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len() + 8);
    for ch in path.chars() {
        match ch {
            '[' | ']' | '?' | '*' | '{' | '}' => {
                escaped.push('[');
                escaped.push(ch);
                escaped.push(']');
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_characters_are_escaped() {
        assert_eq!(escape_glob(r"C:\App [beta]"), r"C:\App [[]beta[]]");
        assert_eq!(escape_glob("/opt/autosubs"), "/opt/autosubs");
    }
}
