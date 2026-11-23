//! Per-remote default path ("root"), machine-local.
//!
//! Object stores address a bucket by path, and the access keys Oortery tells users to create are
//! scoped to a single bucket, so such a key cannot even list the account's other buckets. When a
//! preset connects one bucket, we remember it here and then start browsing, syncing and backing up
//! *inside* that bucket instead of at the account root (which would fail, or dump every bucket).
//!
//! Kept apart from the rclone config (which is meant to be portable across machines) because a
//! landing path is a local convenience, not part of the credential. Best-effort: a missing or
//! unreadable store just means "no root", never an error that blocks anything.

use crate::jobs::config_dir;
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn roots_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("roots.json"))
}

fn load() -> BTreeMap<String, String> {
    roots_path()
        .ok()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save(map: &BTreeMap<String, String>) -> Result<()> {
    let path = roots_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(map)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Remember `path` as the landing root for remote `name` (e.g. a bucket). An empty/blank path
/// clears any existing root rather than storing nothing useful.
pub fn set(name: &str, path: &str) -> Result<()> {
    let mut map = load();
    let path = path.trim();
    if path.is_empty() {
        map.remove(name);
    } else {
        map.insert(name.to_string(), path.to_string());
    }
    save(&map)
}

/// The landing root for `name`, if one is set (e.g. "my-bucket").
pub fn get(name: &str) -> Option<String> {
    load().get(name).cloned().filter(|s| !s.is_empty())
}

/// Forget a remote's root (call when the remote is removed).
pub fn forget(name: &str) -> Result<()> {
    let mut map = load();
    if map.remove(name).is_some() {
        save(&map)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_forget_roundtrip() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("oortery-roots-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(get("s3box"), None);
        set("s3box", "my-bucket").unwrap();
        assert_eq!(get("s3box").as_deref(), Some("my-bucket"));
        // A blank path clears it; forget removes it.
        set("s3box", "   ").unwrap();
        assert_eq!(get("s3box"), None);
        set("s3box", "b").unwrap();
        forget("s3box").unwrap();
        assert_eq!(get("s3box"), None);

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
