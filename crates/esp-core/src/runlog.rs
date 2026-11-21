//! A small local record of when each sync/backup last SUCCEEDED, kept apart from the job
//! definitions on purpose. The Overview's "are we protected?" verdict must come from real
//! outcomes, not from when a run was merely dispatched (a scheduled backup stamps its attempt
//! time before it finishes, so trusting that would claim protection for a backup that failed).
//!
//! Scheduling cadence still uses each job's `last_run` (last attempt); only the health read uses
//! these success times. Run history is machine-local, so it lives here rather than on the `Job`,
//! which also keeps an exported setup free of per-machine timestamps.

use crate::jobs::{config_dir, now_unix};
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Path to the run-success store: `<config dir>/runs.json`.
fn runs_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("runs.json"))
}

/// Load the whole map. A missing or unreadable store reads as empty (this is a best-effort
/// convenience record, never a source of truth that should fail a run).
fn load() -> BTreeMap<String, i64> {
    runs_path()
        .ok()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save(map: &BTreeMap<String, i64>) -> Result<()> {
    let path = runs_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(map)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// The map key for a run target. `kind` is "sync" or "backup".
fn key(kind: &str, id: &str) -> String {
    format!("{kind}:{id}")
}

/// Record that `kind`/`id` succeeded now.
pub fn record_success(kind: &str, id: &str) -> Result<()> {
    let mut map = load();
    map.insert(key(kind, id), now_unix());
    save(&map)
}

/// The Unix time `kind`/`id` last succeeded, if it ever has.
pub fn last_success(kind: &str, id: &str) -> Option<i64> {
    load().get(&key(kind, id)).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_reads_back_success() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("oortery-runlog-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_dir_all(&dir);

        // Nothing recorded yet.
        assert!(last_success("backup", "photos").is_none());
        // Record one success and read it back.
        record_success("backup", "photos").unwrap();
        let ts = last_success("backup", "photos").expect("recorded");
        assert!(ts > 0);
        // A different id, and a different kind with the same id, stay independent.
        assert!(last_success("backup", "docs").is_none());
        assert!(last_success("sync", "photos").is_none());

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
