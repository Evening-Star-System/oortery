//! Which remotes the user has left mounted, so they come back on the next launch (and at login,
//! via autostart). This is the persistence behind "stay mounted through reboots": a remote is
//! remembered when mounted and forgotten when the user unmounts or removes it, so only remotes
//! deliberately left mounted auto-remount.
//!
//! Machine-local, kept apart from the remote definitions on purpose: mount state is per-computer
//! and must not travel in an exported setup (which stays free of per-machine state).

use crate::jobs::config_dir;
use anyhow::Result;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Path to the keep-mounted store: `<config dir>/mounted.json`.
fn path() -> Result<PathBuf> {
    Ok(config_dir()?.join("mounted.json"))
}

/// A missing or unreadable store reads as empty: this is a convenience record, never a source of
/// truth that should fail anything.
fn load() -> BTreeSet<String> {
    path()
        .ok()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save(set: &BTreeSet<String>) -> Result<()> {
    let path = path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(set)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Remember `name` as a remote to keep mounted across sessions.
pub fn remember(name: &str) -> Result<()> {
    let mut set = load();
    if set.insert(name.to_string()) {
        save(&set)
    } else {
        Ok(())
    }
}

/// Forget `name` (the user unmounted or removed it); it will not auto-remount.
pub fn forget(name: &str) -> Result<()> {
    let mut set = load();
    if set.remove(name) {
        save(&set)
    } else {
        Ok(())
    }
}

/// The remotes to auto-remount, sorted.
pub fn remembered() -> Vec<String> {
    load().into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_and_forgets() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("oortery-mountkeep-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_dir_all(&dir);

        assert!(remembered().is_empty());
        remember("Internxt").unwrap();
        remember("Backblaze").unwrap();
        remember("Internxt").unwrap(); // idempotent
        assert_eq!(
            remembered(),
            vec!["Backblaze".to_string(), "Internxt".to_string()]
        );
        forget("Internxt").unwrap();
        assert_eq!(remembered(), vec!["Backblaze".to_string()]);
        forget("nope").unwrap(); // forgetting an absent one is fine

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
