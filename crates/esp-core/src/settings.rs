//! Small persisted UI settings. Currently just the window geometry, so Oortery reopens at the
//! size the user left it. Stored next to the other config as `window.json`.

use crate::jobs::config_dir;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Remembered window geometry. Width/height are the unmaximized size to restore.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct WindowState {
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
}

impl Default for WindowState {
    /// A first-run size large enough to show the whole UI (all stat cards) without scrolling.
    fn default() -> Self {
        WindowState {
            width: 1360,
            height: 900,
            maximized: false,
        }
    }
}

fn state_path() -> Option<PathBuf> {
    config_dir().ok().map(|d| d.join("window.json"))
}

/// Load the saved window geometry, or the default if none/unreadable.
pub fn load_window_state() -> WindowState {
    state_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Persist the window geometry (best-effort; failures are silent so a read-only config dir
/// never breaks closing the app).
pub fn save_window_state(state: &WindowState) {
    let Some(path) = state_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(state) {
        let _ = std::fs::write(path, json);
    }
}
