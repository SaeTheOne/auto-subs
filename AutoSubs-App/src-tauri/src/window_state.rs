//! Window geometry, persisted next to the rest of the app data.
//!
//! This replaces `tauri-plugin-window-state` (and its `.window-state.json` in
//! `%APPDATA%\com.autosubs`) so that nothing is left in the user profile. It
//! implements the same three behaviours the app relied on:
//!
//! * restore size/position/maximized on start,
//! * don't restore a position that no longer intersects any monitor (monitor
//!   unplugged, resolution changed),
//! * keep remembering the pre-maximized geometry so un-maximizing later returns
//!   to a sane size.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{PhysicalPosition, PhysicalSize, WebviewWindow, WindowEvent};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Geometry {
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    maximized: bool,
}

impl Default for Geometry {
    fn default() -> Self {
        // Matches the window created by `create_main_window`.
        Self {
            width: 780,
            height: 700,
            x: 0,
            y: 0,
            maximized: false,
        }
    }
}

static LAST: Mutex<Option<Geometry>> = Mutex::new(None);

fn state_path() -> PathBuf {
    crate::app_paths::file("window-state.json")
}

/// Apply the saved geometry, if any, to the freshly created window.
pub fn restore(window: &WebviewWindow) {
    let Ok(text) = std::fs::read_to_string(state_path()) else {
        return;
    };
    let Ok(saved) = serde_json::from_str::<Geometry>(&text) else {
        tracing::warn!("window-state.json is not valid JSON; ignoring it");
        return;
    };

    // Anything smaller than this is a corrupt/partial write, not a real window.
    if saved.width < 200 || saved.height < 200 {
        return;
    }

    let _ = window.set_size(PhysicalSize::new(saved.width, saved.height));
    if on_screen(window, &saved) {
        let _ = window.set_position(PhysicalPosition::new(saved.x, saved.y));
    }
    if saved.maximized {
        let _ = window.maximize();
    }

    *LAST.lock().unwrap() = Some(saved);
}

/// Keep the in-memory snapshot current as the user moves/resizes the window.
pub fn install(window: &WebviewWindow) {
    let handle = window.clone();
    window.on_window_event(move |event| {
        if matches!(
            event,
            WindowEvent::Resized(_) | WindowEvent::Moved(_) | WindowEvent::CloseRequested { .. }
        ) {
            snapshot(&handle);
        }
    });
}

/// Refresh the snapshot and write it to disk. Called on exit.
pub fn persist(window: &WebviewWindow) {
    snapshot(window);

    let Some(geometry) = *LAST.lock().unwrap() else {
        return;
    };

    let path = state_path();
    match serde_json::to_string_pretty(&geometry) {
        Ok(text) => {
            if let Err(error) = std::fs::write(&path, text) {
                tracing::warn!("could not save window geometry to {}: {error}", path.display());
            }
        }
        Err(error) => tracing::warn!("could not serialize window geometry: {error}"),
    }
}

fn snapshot(window: &WebviewWindow) {
    let maximized = window.is_maximized().unwrap_or(false);
    let minimized = window.is_minimized().unwrap_or(false);

    let mut guard = LAST.lock().unwrap();
    let geometry = guard.get_or_insert_with(Geometry::default);
    geometry.maximized = maximized;

    // While maximized or minimized the reported size/position is the
    // fullscreen/minimized box, which is not what should come back later.
    if maximized || minimized {
        return;
    }

    if let Ok(size) = window.inner_size() {
        if size.width > 0 && size.height > 0 {
            geometry.width = size.width;
            geometry.height = size.height;
        }
    }
    if let Ok(position) = window.outer_position() {
        geometry.x = position.x;
        geometry.y = position.y;
    }
}

fn on_screen(window: &WebviewWindow, saved: &Geometry) -> bool {
    let Ok(monitors) = window.available_monitors() else {
        return true;
    };
    if monitors.is_empty() {
        return true;
    }

    let (left, top) = (saved.x as i64, saved.y as i64);
    let (right, bottom) = (left + saved.width as i64, top + saved.height as i64);

    monitors.iter().any(|monitor| {
        let position = monitor.position();
        let size = monitor.size();
        let (monitor_left, monitor_top) = (position.x as i64, position.y as i64);
        let (monitor_right, monitor_bottom) = (
            monitor_left + size.width as i64,
            monitor_top + size.height as i64,
        );

        left < monitor_right && right > monitor_left && top < monitor_bottom && bottom > monitor_top
    })
}
