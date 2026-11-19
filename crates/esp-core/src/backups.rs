//! Backups via restic: versioned, encrypted, deduplicated snapshots. A second engine behind
//! the same engine-neutral surface the UI already trusts. The repo can be local or, via
//! `rclone:remote:path`, any connected cloud, so backups compose with the rclone rail.
//!
//! The repo password lives in a 0600 file under the config dir (restic `--password-file`).
//! TODO: move it to the OS keyring (secret-service) before real use; a plaintext file is an
//! alpha stopgap, not the ethos-correct home for a secret.

use crate::jobs::{config_dir, Endpoint};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// A saved backup: what to protect, and where the (encrypted) repo lives.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupJob {
    pub id: String,
    pub name: String,
    /// Local folders to back up.
    pub sources: Vec<String>,
    /// Where the restic repository lives (local path, or a cloud via rclone).
    pub repo: Endpoint,
}

/// One restic snapshot.
#[derive(Clone, Debug, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub short_id: String,
    pub time: String,
    #[serde(default)]
    pub paths: Vec<String>,
}

/// Locate the restic binary. None if it is not installed.
pub fn restic_path() -> Option<String> {
    crate::which("restic")
}

/// Path to the backups store: `<config dir>/backups.json`.
pub fn backups_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("backups.json"))
}

/// Load saved backup jobs. An absent store is an empty list.
pub fn load_backups() -> Result<Vec<BackupJob>> {
    let path = backups_path()?;
    match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Save the whole set of backup jobs atomically.
pub fn save_backups(jobs: &[BackupJob]) -> Result<()> {
    let path = backups_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(jobs).context("serializing backups")?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// The 0600 password file for a job's repo.
pub fn password_file(id: &str) -> Result<PathBuf> {
    Ok(config_dir()?.join(format!("backup-{id}.pw")))
}

/// Write a job's repo password to its 0600 file.
pub fn save_password(id: &str, password: &str) -> Result<()> {
    let path = password_file(id)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, password)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn restic_cmd(job: &BackupJob) -> Result<Command> {
    let mut c = Command::new("restic");
    c.arg("--repo")
        .arg(job.repo.restic_repo())
        .arg("--password-file")
        .arg(password_file(&job.id)?);
    Ok(c)
}

/// Initialise the repo. Idempotent: an already-initialised repo is fine.
pub fn init_repo(job: &BackupJob) -> Result<()> {
    let out = restic_cmd(job)?
        .arg("init")
        .output()
        .context("running restic init")?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    if err.contains("already initialized") || err.contains("already exists") {
        return Ok(());
    }
    bail!("restic init failed: {}", err.trim());
}

/// Back up the job's sources into a new snapshot, streaming percent-done (0.0..1.0) to
/// `on_progress`. Returns the new snapshot id. Run this off the UI thread.
pub fn run_backup<F: FnMut(f64)>(job: &BackupJob, mut on_progress: F) -> Result<String> {
    let mut child = restic_cmd(job)?
        .arg("backup")
        .args(&job.sources)
        .arg("--json")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning restic backup")?;

    // Drain stderr on its own thread so a verbose error cannot deadlock the stdout pipe.
    let mut err_pipe = child.stderr.take().expect("piped");
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err_pipe.read_to_string(&mut s);
        s
    });

    let stdout = child.stdout.take().expect("piped");
    let mut snapshot = String::new();
    for line in BufReader::new(stdout).lines() {
        let line = line?;
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match v.get("message_type").and_then(|m| m.as_str()) {
            Some("status") => {
                if let Some(p) = v.get("percent_done").and_then(|x| x.as_f64()) {
                    on_progress(p);
                }
            }
            Some("summary") => {
                if let Some(id) = v.get("snapshot_id").and_then(|x| x.as_str()) {
                    snapshot = id.to_string();
                }
            }
            _ => {}
        }
    }

    let status = child.wait().context("waiting for restic")?;
    let err = err_thread.join().unwrap_or_default();
    if !status.success() {
        bail!("restic backup failed: {}", err.trim());
    }
    on_progress(1.0);
    Ok(snapshot)
}

/// The repo's snapshots, newest first.
pub fn list_snapshots(job: &BackupJob) -> Result<Vec<Snapshot>> {
    let out = restic_cmd(job)?
        .args(["snapshots", "--json"])
        .output()
        .context("running restic snapshots")?;
    if !out.status.success() {
        bail!(
            "restic snapshots failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let mut snaps: Vec<Snapshot> = serde_json::from_slice(&out.stdout).unwrap_or_default();
    snaps.reverse();
    Ok(snaps)
}

/// Restore `snapshot_id` (or "latest") into `target`.
pub fn restore(job: &BackupJob, snapshot_id: &str, target: &str) -> Result<()> {
    let out = restic_cmd(job)?
        .args(["restore", snapshot_id, "--target", target])
        .output()
        .context("running restic restore")?;
    if !out.status.success() {
        bail!(
            "restic restore failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_snapshot_restore_roundtrip() {
        if restic_path().is_none() {
            eprintln!("restic not installed; skipping");
            return;
        }
        let base = std::env::temp_dir().join(format!("esp-backup-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", base.join("cfg"));
        let data = base.join("data");
        let repo = base.join("repo");
        let restored = base.join("restored");
        std::fs::create_dir_all(data.join("sub")).unwrap();
        std::fs::create_dir_all(&restored).unwrap();
        std::fs::write(data.join("a.txt"), b"a precious file").unwrap();
        std::fs::write(data.join("sub/b.txt"), b"nested").unwrap();

        let job = BackupJob {
            id: "test".into(),
            name: "Test".into(),
            sources: vec![data.to_string_lossy().into_owned()],
            repo: Endpoint::local(repo.to_string_lossy().into_owned()),
        };
        save_password(&job.id, "esp-test-pw").unwrap();
        init_repo(&job).expect("init");

        let mut seen_progress = false;
        let snap = run_backup(&job, |_p| seen_progress = true).expect("backup");
        assert!(!snap.is_empty(), "no snapshot id returned");

        let snaps = list_snapshots(&job).expect("snapshots");
        assert_eq!(snaps.len(), 1);
        assert!(snaps[0].id.starts_with(&snaps[0].short_id));

        restore(&job, "latest", restored.to_str().unwrap()).expect("restore");
        let found = std::fs::read_dir(&restored)
            .unwrap()
            .flat_map(|e| walk(e.unwrap().path()))
            .any(|p| p.ends_with("a.txt"));
        assert!(found, "restored tree missing a.txt");
        let _ = seen_progress; // progress is best-effort; a tiny backup may finish in one status

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&base);
    }

    fn walk(p: PathBuf) -> Vec<PathBuf> {
        if p.is_dir() {
            std::fs::read_dir(&p)
                .map(|rd| rd.flat_map(|e| walk(e.unwrap().path())).collect())
                .unwrap_or_default()
        } else {
            vec![p]
        }
    }
}
