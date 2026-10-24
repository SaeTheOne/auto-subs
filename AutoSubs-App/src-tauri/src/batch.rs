//! Folder scanning for the batch panel.
//!
//! The batch view only needs one thing from the backend: "every media file under
//! this folder", recursively. Doing it here keeps the frontend from walking a
//! tree one `read_dir` at a time and gives one place to bound the work.

use std::fs;
use std::path::Path;

/// Container/codec extensions the transcription pipeline can read through
/// ffmpeg. Kept in sync with the picker's filter list.
const MEDIA_EXTENSIONS: &[&str] = &[
    "aac", "aif", "aiff", "alac", "avi", "flac", "m2ts", "m4a", "m4v", "mkv", "mov", "mp3", "mp4",
    "mpeg", "mpg", "mts", "ogg", "opus", "ts", "wav", "webm", "wma", "wmv", "3gp",
];

/// Never descend into these: Explorer/system bookkeeping, not the user's media.
const SKIPPED_DIRECTORIES: &[&str] = &[
    "$recycle.bin",
    "system volume information",
    "node_modules",
    ".git",
];

const MAX_FILES: usize = 5000;
const MAX_DEPTH: usize = 16;

fn is_media_file(path: &Path) -> bool {
    path.extension()
        .map(|ext| {
            let ext = ext.to_string_lossy().to_ascii_lowercase();
            MEDIA_EXTENSIONS.contains(&ext.as_str())
        })
        .unwrap_or(false)
}

fn should_skip_directory(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with('.') || SKIPPED_DIRECTORIES.contains(&name.as_str())
}

/// Recursively list media files under `root`, sorted, capped at [`MAX_FILES`].
///
/// A file path is also accepted: it is returned as-is, so the frontend can call
/// this for every dropped item without branching.
#[tauri::command]
pub fn scan_media_files(root: String) -> Result<Vec<String>, String> {
    let root_path = Path::new(&root);

    if root_path.is_file() {
        return Ok(if is_media_file(root_path) {
            vec![root.to_string()]
        } else {
            Vec::new()
        });
    }
    if !root_path.is_dir() {
        return Err(format!("not a folder: {root}"));
    }

    let mut found: Vec<String> = Vec::new();
    let mut stack: Vec<(std::path::PathBuf, usize)> = vec![(root_path.to_path_buf(), 0)];

    while let Some((dir, depth)) = stack.pop() {
        if found.len() >= MAX_FILES {
            break;
        }
        if depth > MAX_DEPTH {
            continue;
        }

        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            // Unreadable folder (permissions, deleted mid-scan): skip it and
            // keep the rest of the batch usable.
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };

            if file_type.is_dir() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if should_skip_directory(&name) {
                    continue;
                }
                stack.push((path, depth + 1));
            } else if file_type.is_file() && is_media_file(&path) {
                found.push(path.to_string_lossy().into_owned());
                if found.len() >= MAX_FILES {
                    break;
                }
            }
        }
    }

    found.sort();
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_media_by_extension() {
        assert!(is_media_file(Path::new("C:/media/Episode 01.MKV")));
        assert!(is_media_file(Path::new("/media/clip.mp4")));
        assert!(!is_media_file(Path::new("C:/media/notes.txt")));
        assert!(!is_media_file(Path::new("C:/media/no-extension")));
    }

    #[test]
    fn skips_system_folders() {
        assert!(should_skip_directory("$RECYCLE.BIN"));
        assert!(should_skip_directory(".git"));
        assert!(!should_skip_directory("Season 1"));
    }
}
