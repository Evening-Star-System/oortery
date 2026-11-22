//! esp-core: the engine-agnostic layer beneath the UI.
//!
//! Today it speaks to rclone (which reaches 70+ storage backends). The types here are
//! deliberately backend-neutral so a second engine (Syncthing for P2P, restic for
//! snapshot backups) can slot in later without the UI knowing the difference.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::process::Command;

pub mod autostart;
pub mod backups;
pub mod connect;
pub mod dashboard;
pub mod diag;
pub mod engine;
pub mod jobs;
pub mod lock;
pub mod p2p;
pub mod portable;
pub mod preflight;
pub mod presets;
pub mod runlog;
pub mod settings;
pub mod stall;
pub mod tuning;

pub use autostart::{autostart_enabled, set_autostart};
pub use backups::{
    check_repo, init_repo, list_snapshots, load_backups, password_file, restic_path, restore,
    run_backup, save_backups, save_password, unlock_repo, BackupJob, BackupPhase, Snapshot,
};
pub use dashboard::{ago, summary, Health, Summary};
pub use engine::{
    activity, list_dirs, list_mounts, mount, rc_at, unmount, Activity, DirEntry, JobStats,
    JobStatus, Mount, Rcd, Transfer,
};
pub use jobs::{
    home_dir, load_jobs, now_unix, save_jobs, slug_id, took_corrupt_recovery, Endpoint, Job,
    Schedule, SyncMode,
};
pub use lock::{clear_lock, lock_enabled, set_lock, verify as verify_lock};
pub use p2p::{syncthing_path, Syncthingd};
pub use portable::{export_bundle, import_bundle, ImportResult};
pub use preflight::{scan_dir, Hazard, Issue};
pub use settings::{load_window_state, save_window_state, WindowState};
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

    /// True if this backend signs in via a browser OAuth flow, rather than by collecting config
    /// values in a form. Checked against an allowlist of backends verified to actually reach
    /// rclone's interactive browser step, NOT just "has a `token` option": several token-bearing
    /// backends (linkbox, mailru, shade, filefabric, jottacloud, zoho) are NOT clean browser-OAuth,
    /// and routing them to the browser flow created an empty remote while claiming success. They go
    /// to the field form instead, where their real credentials are collected.
    pub fn needs_oauth(&self) -> bool {
        OAUTH_BACKENDS.contains(&self.name.as_str())
    }
}

/// Backends whose sign-in is a real browser OAuth flow (verified to reach `config_is_local`).
const OAUTH_BACKENDS: &[&str] = &[
    "box",
    "drive",
    "dropbox",
    "google cloud storage",
    "google photos",
    "hidrive",
    "huaweidrive",
    "onedrive",
    "pcloud",
    "premiumizeme",
    "putio",
    "sharefile",
    "yandex",
];

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
    if name.starts_with('-') {
        anyhow::bail!("a remote name cannot begin with '-'");
    }
    // Flags first, then `--`, then the positionals (name, type, key/value pairs). After `--` a
    // value beginning with '-' is stored literally instead of being read as an rclone flag.
    let mut args: Vec<String> = vec![
        "config".into(),
        "create".into(),
        "--non-interactive".into(),
        "--".into(),
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

/// Read a configured remote's stored settings: its backend `type` plus each key/value. Password
/// values come back obscured and are not meant to be displayed, so the editor leaves password fields
/// blank (filling one replaces it; leaving it blank keeps the current one).
pub fn remote_values(name: &str) -> Result<std::collections::BTreeMap<String, String>> {
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
    let all: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("parsing rclone config dump")?;
    let obj = all
        .get(name)
        .and_then(|v| v.as_object())
        .with_context(|| format!("remote \"{name}\" not found"))?;
    Ok(obj
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect())
}

/// Update an existing remote's settings. Only the keys given are changed; blank values are skipped,
/// so a password field left blank keeps its current value. rclone obscures password fields itself.
pub fn update_remote(name: &str, params: &[(String, String)]) -> Result<()> {
    let filled: Vec<&(String, String)> = params.iter().filter(|(_, v)| !v.is_empty()).collect();
    if filled.is_empty() {
        return Ok(()); // nothing to change
    }
    let mut args: Vec<String> = vec![
        "config".into(),
        "update".into(),
        "--non-interactive".into(),
        "--".into(),
        name.into(),
    ];
    for (k, v) in filled {
        args.push(k.clone());
        args.push(v.clone());
    }
    let out = Command::new("rclone")
        .args(&args)
        .output()
        .context("failed to run `rclone config update`")?;
    if !out.status.success() {
        anyhow::bail!(
            "rclone config update failed: {}",
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
