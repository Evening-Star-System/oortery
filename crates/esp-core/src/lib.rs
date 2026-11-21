//! esp-core: the engine-agnostic layer beneath the UI.
//!
//! Today it speaks to rclone (which reaches 70+ storage backends). The types here are
//! deliberately backend-neutral so a second engine (Syncthing for P2P, restic for
//! snapshot backups) can slot in later without the UI knowing the difference.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::process::Command;

pub mod backups;
pub mod connect;
pub mod engine;
pub mod jobs;
pub mod stall;
pub mod tuning;

pub use backups::{
    check_repo, init_repo, list_snapshots, load_backups, password_file, restic_path, restore,
    run_backup, save_backups, save_password, unlock_repo, BackupJob, BackupPhase, Snapshot,
};
pub use engine::{
    activity, list_dirs, list_mounts, mount, rc_at, unmount, Activity, DirEntry, JobStats,
    JobStatus, Mount, Rcd, Transfer,
};
pub use jobs::{load_jobs, save_jobs, slug_id, Endpoint, Job, SyncMode};
pub use stall::{StallTracker, DEFAULT_STALL_SECS};
pub use tuning::{backend_of, tuning_for, Tuning};

/// Serializes tests that mutate process-global env (XDG_CONFIG_HOME, RCLONE_CONFIG). Cargo
/// runs tests in parallel threads of one process, so without this two tests can read each
/// other's config dir. Poison-tolerant: a panicking test must not wedge the rest.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A storage backend the user has configured (one entry in rclone's config).
#[derive(Clone, Debug)]
pub struct Remote {
    pub name: String,
    /// rclone backend id, e.g. "drive", "s3", "dropbox", "sftp".
    pub backend: String,
}

/// A backend type rclone knows how to talk to, whether configured or not.
#[derive(Clone, Debug, Deserialize)]
pub struct Provider {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Description")]
    pub description: String,
    #[serde(rename = "Options", default)]
    pub options: Vec<ProviderOption>,
}

/// One configurable field of a backend, from rclone's provider schema.
#[derive(Clone, Debug, Deserialize)]
pub struct ProviderOption {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Help", default)]
    pub help: String,
    #[serde(rename = "Required", default)]
    pub required: bool,
    #[serde(rename = "IsPassword", default)]
    pub is_password: bool,
    #[serde(rename = "Advanced", default)]
    pub advanced: bool,
    #[serde(rename = "Hide", default)]
    pub hide: i64,
    #[serde(rename = "Sensitive", default)]
    pub sensitive: bool,
}

impl Provider {
    /// Options to show in the basic add form: visible and not marked advanced. The first
    /// line of Help makes a good field subtitle.
    pub fn basic_options(&self) -> impl Iterator<Item = &ProviderOption> {
        self.options.iter().filter(|o| !o.advanced && o.hide == 0)
    }

    /// True if this backend signs in via OAuth (it has a `token` option), meaning it needs
    /// an interactive browser flow rather than plain config values.
    pub fn needs_oauth(&self) -> bool {
        self.options.iter().any(|o| o.name == "token")
    }
}

/// Locate the rclone binary. Returns None if it is not installed.
pub fn rclone_path() -> Option<String> {
    which("rclone")
}

pub(crate) fn which(bin: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|p| p.join(bin))
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
}

/// The user's configured remotes, read from `rclone config dump`.
pub fn list_remotes() -> Result<Vec<Remote>> {
    let out = Command::new("rclone")
        .args(["config", "dump"])
        .output()
        .context("failed to run `rclone config dump`")?;
    if !out.status.success() {
        anyhow::bail!(
            "rclone config dump failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    // Shape: { "<remote name>": { "type": "<backend>", ...fields } }
    let map: BTreeMap<String, BTreeMap<String, serde_json::Value>> =
        serde_json::from_slice(&out.stdout).context("parsing rclone config dump")?;
    let mut remotes: Vec<Remote> = map
        .into_iter()
        .map(|(name, cfg)| {
            let backend = cfg
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            Remote { name, backend }
        })
        .collect();
    remotes.sort_by_key(|r| r.name.to_lowercase());
    Ok(remotes)
}

/// Every backend rclone can reach, from `rclone config providers`. The count is the
/// "connect to anything" breadth figure the dashboard shows.
pub fn list_providers() -> Result<Vec<Provider>> {
    let out = Command::new("rclone")
        .args(["config", "providers"])
        .output()
        .context("failed to run `rclone config providers`")?;
    if !out.status.success() {
        anyhow::bail!(
            "rclone config providers failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let mut providers: Vec<Provider> =
        serde_json::from_slice(&out.stdout).context("parsing rclone config providers")?;
    providers.sort_by_key(|p| p.name.to_lowercase());
    Ok(providers)
}

/// Create a new remote non-interactively: `rclone config create <name> <backend> k v ...`.
/// Empty values are skipped. Suitable for config-value backends; OAuth backends need an
/// interactive browser flow handled separately.
pub fn create_remote(name: &str, backend: &str, params: &[(String, String)]) -> Result<()> {
    let mut args: Vec<String> = vec![
        "config".into(),
        "create".into(),
        name.into(),
        backend.into(),
    ];
    for (k, v) in params {
        if v.is_empty() {
            continue;
        }
        args.push(k.clone());
        args.push(v.clone());
    }
    args.push("--non-interactive".into());
    let out = Command::new("rclone")
        .args(&args)
        .output()
        .context("failed to run `rclone config create`")?;
    if !out.status.success() {
        anyhow::bail!(
            "rclone config create failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// Delete a configured remote.
pub fn delete_remote(name: &str) -> Result<()> {
    let out = Command::new("rclone")
        .args(["config", "delete", name])
        .output()
        .context("failed to run `rclone config delete`")?;
    if !out.status.success() {
        anyhow::bail!(
            "rclone config delete failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// A friendly, human label for a backend id (for icons/subtitles). Falls back to the id.
pub fn backend_label(backend: &str) -> &str {
    match backend {
        "drive" => "Google Drive",
        "dropbox" => "Dropbox",
        "onedrive" => "OneDrive",
        "s3" => "Amazon S3 / compatible",
        "b2" => "Backblaze B2",
        "sftp" => "SFTP",
        "ftp" => "FTP",
        "webdav" => "WebDAV",
        "nextcloud" | "owncloud" => "Nextcloud / ownCloud",
        "pcloud" => "pCloud",
        "mega" => "MEGA",
        "box" => "Box",
        "crypt" => "Encrypted",
        "local" => "This computer",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_enumerate_when_rclone_present() {
        if rclone_path().is_none() {
            eprintln!("rclone not installed; skipping");
            return;
        }
        let providers = list_providers().expect("list providers");
        // rclone ships dozens of backends; sanity-check the breadth claim.
        assert!(
            providers.len() > 30,
            "expected many providers, got {}",
            providers.len()
        );
        // Option schemas must parse: sftp should expose a required `host`.
        let sftp = providers.iter().find(|p| p.name == "sftp").expect("sftp");
        assert!(
            sftp.basic_options().any(|o| o.name == "host"),
            "sftp should have a visible host option"
        );
        // OAuth detection: drive signs in via token.
        let drive = providers.iter().find(|p| p.name == "drive").expect("drive");
        assert!(drive.needs_oauth(), "drive should be detected as OAuth");
    }

    #[test]
    fn create_list_delete_roundtrip() {
        if rclone_path().is_none() {
            return;
        }
        let _env = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("esp-core-rt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("RCLONE_CONFIG", dir.join("rclone.conf"));

        create_remote(
            "esptestsftp",
            "sftp",
            &[
                ("host".into(), "example.com".into()),
                ("user".into(), "bob".into()),
            ],
        )
        .expect("create");
        assert!(list_remotes()
            .unwrap()
            .iter()
            .any(|r| r.name == "esptestsftp" && r.backend == "sftp"));

        delete_remote("esptestsftp").expect("delete");
        assert!(!list_remotes()
            .unwrap()
            .iter()
            .any(|r| r.name == "esptestsftp"));

        std::env::remove_var("RCLONE_CONFIG");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remotes_list_is_readable() {
        if rclone_path().is_none() {
            return;
        }
        // Should not error even with zero configured remotes.
        let _ = list_remotes().expect("list remotes");
    }
}
