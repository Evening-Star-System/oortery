//! The sovereignty read: Oortery's "your data, no account" story as numbers the Overview shows.
//!
//! Remotes and devices are live, local counts; `accounts` is always zero, which is the whole
//! point: Oortery never holds an account of yours, so there is nothing to lock you in or lock you
//! out. `protected_bytes` is a cumulative, honest tally of the bytes Oortery has actually moved to
//! safety (sync transfers plus backup data added), persisted locally so it survives restarts.
//!
//! This is deliberately cheap: a config read and a small file read, no network. Richer,
//! network-backed figures (per-remote quota, live footprint) belong in a separate layer so this
//! read stays instant.

use crate::jobs::config_dir;
use crate::list_remotes;
use crate::p2p::paired_device_count;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The cumulative "moved to safety" tally, persisted at `<config dir>/protected.json`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Protected {
    /// Total bytes Oortery has synced or backed up over its lifetime on this machine.
    #[serde(default)]
    pub bytes: u64,
    /// Number of successful sync/backup runs behind that total.
    #[serde(default)]
    pub runs: u64,
}

/// The account-free standing of the user's setup, for the Overview strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sovereignty {
    /// Configured storage backends ("places").
    pub remotes: usize,
    /// Paired peer-to-peer devices.
    pub devices: usize,
    /// Always 0: Oortery holds no account of yours. The whole point.
    pub accounts: usize,
    /// Cumulative bytes moved to safety.
    pub protected_bytes: u64,
    /// Successful runs behind that total.
    pub protected_runs: u64,
}

fn protected_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("protected.json"))
}

/// Load the cumulative tally. A missing or unreadable file reads as zero: this is a best-effort
/// record, never a source of truth that should fail anything.
pub fn load_protected() -> Protected {
    protected_path()
        .ok()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Add one successful run of `bytes` to the cumulative tally. Crash-safe (atomic rename) and
/// best-effort: the byte total only ever grows (`saturating_add`), so a stat-less run that reports
/// zero still counts as a run without shrinking the total. Call after a run has genuinely
/// succeeded; never let its error fail the run.
pub fn record_protected(bytes: u64) -> Result<()> {
    let mut p = load_protected();
    p.bytes = p.bytes.saturating_add(bytes);
    p.runs = p.runs.saturating_add(1);
    let path = protected_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(&p)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// The live sovereignty read for the Overview. Cheap and local (a config read and a file read),
/// no network.
pub fn sovereignty() -> Sovereignty {
    let p = load_protected();
    Sovereignty {
        remotes: list_remotes().map(|r| r.len()).unwrap_or(0),
        devices: paired_device_count(),
        accounts: 0,
        protected_bytes: p.bytes,
        protected_runs: p.runs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_accumulates_and_never_shrinks() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("oortery-sov-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_dir_all(&dir);

        // Nothing recorded yet reads as zero.
        assert_eq!(load_protected(), Protected::default());

        record_protected(1_000).unwrap();
        record_protected(500).unwrap();
        // A zero-byte success still counts as a run and cannot shrink the total.
        record_protected(0).unwrap();
        let p = load_protected();
        assert_eq!(p.bytes, 1_500);
        assert_eq!(p.runs, 3);

        // accounts is always zero; the tally surfaces through sovereignty().
        let sv = sovereignty();
        assert_eq!(sv.accounts, 0);
        assert_eq!(sv.protected_bytes, 1_500);
        assert_eq!(sv.protected_runs, 3);

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
