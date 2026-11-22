//! Small persisted UI settings: window geometry (so Oortery reopens at the size the user left it)
//! and simple on/off preferences. Stored next to the other config.

use crate::jobs::config_dir;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Whether to keep running (and keep mounts live) after the window is closed.
pub const KEEP_BACKGROUND: &str = "keep_background";

fn prefs_path() -> Option<PathBuf> {
    config_dir().ok().map(|d| d.join("prefs.json"))
}

/// All on/off preferences as a small map in `prefs.json`, so a new toggle does not need its own
/// file. Missing or unreadable reads as empty.
fn load_prefs() -> BTreeMap<String, bool> {
    prefs_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Read a boolean preference, defaulting to `false` (off) when unset.
pub fn pref_bool(key: &str) -> bool {
    load_prefs().get(key).copied().unwrap_or(false)
}

/// Persist a boolean preference (best-effort; a read-only config dir never breaks the toggle).
pub fn set_pref_bool(key: &str, value: bool) {
    let mut map = load_prefs();
    map.insert(key.to_string(), value);
    let Some(path) = prefs_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(&map) {
        let _ = std::fs::write(path, json);
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bool_pref_round_trips_and_defaults_false() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("oortery-prefs-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_dir_all(&dir);

        assert!(!pref_bool(KEEP_BACKGROUND)); // unset defaults to off
        set_pref_bool(KEEP_BACKGROUND, true);
        assert!(pref_bool(KEEP_BACKGROUND));
        set_pref_bool(KEEP_BACKGROUND, false);
        assert!(!pref_bool(KEEP_BACKGROUND));
        // An unrelated key stays independent and defaults off.
        assert!(!pref_bool("some_other_flag"));

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
