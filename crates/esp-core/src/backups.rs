//! Backups via restic: versioned, encrypted, deduplicated snapshots. A second engine behind
//! the same engine-neutral surface the UI already trusts. The repo can be local or, via
//! `rclone:remote:path`, any connected cloud, so backups compose with the rclone rail.
//!
//! The repo password lives in a 0600 file under the config dir (restic `--password-file`).
//! TODO: move it to the OS keyring (secret-service) before real use; a plaintext file is an
//! alpha stopgap, not the ethos-correct home for a secret.

use crate::jobs::{config_dir, Endpoint, Schedule};
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
    /// restic exclude patterns (e.g. "node_modules", "*.tmp", ".git"). Skips matching files
    /// so a huge build tree does not bloat the repo or stall a touchy backend. `default` keeps
    /// older stored jobs (without the field) loadable.
    #[serde(default)]
    pub excludes: Vec<String>,
    /// How often to back up on its own.
    #[serde(default)]
    pub schedule: Schedule,
    /// Unix seconds of the last run, for scheduling.
    #[serde(default)]
    pub last_run: Option<i64>,
    /// A pinned time of day for Daily/Weekly schedules. `None` keeps the plain interval behaviour.
    #[serde(default)]
    pub at: Option<crate::jobs::DayTime>,
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

/// Load saved backup jobs. An absent store is an empty list; a corrupt one is set aside, not lost.
pub fn load_backups() -> Result<Vec<BackupJob>> {
    crate::jobs::load_store(&backups_path()?)
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

/// Store a job's repo password. Prefers the OS keyring; falls back to a 0600 file only where no
/// keyring is available (headless/minimal systems). On a successful keyring store, any stale
/// fallback file is removed so there is one source of truth, not a lingering plaintext copy.
pub fn save_password(id: &str, password: &str) -> Result<()> {
    if crate::secret::keyring_set(id, password) {
        if let Ok(p) = password_file(id) {
            let _ = std::fs::remove_file(p);
        }
        return Ok(());
    }
    write_password_file(id, password)
}

fn write_password_file(id: &str, password: &str) -> Result<()> {
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

/// The repo password for `id`: from the OS keyring, else the 0600 fallback file. If it is read from
/// the file and a keyring is now available, it is migrated into the keyring and the plaintext file
/// removed, so an existing on-disk password is upgraded automatically on the next backup.
pub fn repo_password(id: &str) -> Result<String> {
    if let Some(pw) = crate::secret::keyring_get(id) {
        return Ok(pw);
    }
    let path = password_file(id)?;
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading the backup password for {id}"))?;
    let pw = raw.trim_end_matches(['\n', '\r']).to_string();
    if crate::secret::keyring_set(id, &pw) {
        let _ = std::fs::remove_file(&path);
    }
    Ok(pw)
}

/// Forget a job's repo password from everywhere (keyring and any fallback file). On backup delete.
pub fn delete_password(id: &str) {
    crate::secret::keyring_delete(id);
    if let Ok(p) = password_file(id) {
        let _ = std::fs::remove_file(p);
    }
}

fn restic_cmd(job: &BackupJob) -> Result<Command> {
    let mut c = Command::new("restic");
    c.arg("--repo").arg(job.repo.restic_repo());
    // The password goes to restic via env, not a file or argv: with the keyring there is no
    // plaintext file at all, and env is only readable by the same user's processes.
    c.env("RESTIC_PASSWORD", repo_password(&job.id)?);
    // When the repo is a cloud remote, restic shells out to rclone, which honors RCLONE_*
    // env. Apply the destination backend's throttle/retry envelope so a cloud backup is as
    // gentle as a sync to the same backend (the Proton many-small-files hang bites restic too;
    // wrapping in restic reduces file count but the pack uploads still face the rate limit).
    for (k, v) in crate::tuning::tuning_for(&crate::tuning::backend_from_config(
        job.repo.remote.as_deref(),
    ))
    .as_rclone_env()
    {
        c.env(k, v);
    }
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

/// Remove stale locks from the repo (restic only removes locks whose owning process is
/// gone or long expired, so this is safe to call even if another backup is running). This
/// is the recovery for the documented "repository is already locked" wedge: a backend 429
/// or an interrupted run can leave a lock that blocks every future backup.
pub fn unlock_repo(job: &BackupJob) -> Result<()> {
    let out = restic_cmd(job)?
        .arg("unlock")
        .output()
        .context("running restic unlock")?;
    if !out.status.success() {
        bail!(
            "restic unlock failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Structural integrity check: verifies the repo's index, trees, and that every referenced
/// pack blob exists. Catches the restic-over-rclone corruption class (a snapshot pointing at
/// a missing or partially-uploaded pack) that would otherwise surface only at restore time.
/// Does not download file data, so it is bounded by metadata size, not repo size.
pub fn check_repo(job: &BackupJob) -> Result<()> {
    let out = restic_cmd(job)?
        .arg("check")
        .output()
        .context("running restic check")?;
    if !out.status.success() {
        // restic writes its findings to both streams; surface whatever is there.
        let err = String::from_utf8_lossy(&out.stderr);
        let msg = if err.trim().is_empty() {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        } else {
            err.trim().to_string()
        };
        bail!("repository integrity check failed: {msg}");
    }
    Ok(())
}

/// True if a restic error reads as a lock contention we can try to clear.
fn is_lock_error(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("already locked") || m.contains("unable to create lock")
}

/// Where a running backup is in its lifecycle. The UI shows this verbatim because the point
/// of ESP is honest state: "verifying" is a real phase, not padding at 100%.
#[derive(Clone, Copy, Debug)]
pub enum BackupPhase {
    /// Uploading data; fraction done in 0.0..1.0.
    Backing(f64),
    /// Data written; verifying repository integrity before the backup counts as done.
    Verifying,
}

/// Back up the job's sources into a new snapshot, reporting lifecycle phases to `on`.
/// Returns the new snapshot id. Run this off the UI thread.
///
/// Reliability, per the research checklist: if the repo is wedged by a stale lock we clear
/// it once and retry (P0 #5), and we never return success until a structural integrity check
/// passes (P0 #3) so a partial/corrupt upload can never masquerade as a good backup.
pub fn run_backup<F: FnMut(BackupPhase)>(job: &BackupJob, mut on: F) -> Result<String> {
    let (snapshot, data_added) = match backup_once(job, &mut on) {
        Ok(out) => out,
        Err(e) if is_lock_error(&e.to_string()) => {
            // Stale lock: clear it and try exactly once more.
            unlock_repo(job).context("clearing a stale repository lock")?;
            backup_once(job, &mut on)?
        }
        Err(e) => return Err(e),
    };
    // Integrity gate: a backup is not "done" until the repository verifies.
    on(BackupPhase::Verifying);
    check_repo(job).context("verifying the backup after it was written")?;
    // Tally what this snapshot added to the "protected by Oortery" total. Best-effort: a failure
    // to record a vanity counter must never fail a verified backup.
    let _ = crate::sovereignty::record_protected(data_added);
    Ok(snapshot)
}

/// One restic backup attempt. Reports `Backing` progress; does not verify or recover (see
/// `run_backup`).
/// Returns the new snapshot id and the bytes restic actually added to the repo (`data_added`
/// from its summary, 0 if absent), so the caller can tally what was moved to safety.
fn backup_once<F: FnMut(BackupPhase)>(job: &BackupJob, on: &mut F) -> Result<(String, u64)> {
    let mut cmd = restic_cmd(job)?;
    cmd.arg("backup").arg("--json");
    // Skip anything the user excluded (build trees, caches). Blank patterns are ignored.
    for pat in &job.excludes {
        let pat = pat.trim();
        if !pat.is_empty() {
            cmd.arg("--exclude").arg(pat);
        }
    }
    // End of options: everything after `--` is a path, so a source beginning with `-` can never
    // be read as a restic flag (e.g. `-o rclone.program=...`, which would run an arbitrary binary).
    cmd.arg("--");
    for src in &job.sources {
        cmd.arg(src);
    }
    let mut child = cmd
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
    let mut data_added = 0u64;
    for line in BufReader::new(stdout).lines() {
        let line = line?;
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match v.get("message_type").and_then(|m| m.as_str()) {
            Some("status") => {
                if let Some(p) = v.get("percent_done").and_then(|x| x.as_f64()) {
                    on(BackupPhase::Backing(p));
                }
            }
            Some("summary") => {
                if let Some(id) = v.get("snapshot_id").and_then(|x| x.as_str()) {
                    snapshot = id.to_string();
                }
                // What this snapshot genuinely wrote to the repo (deduped/compressed), for the
                // "protected by Oortery" tally. Absent on very old restic: treat as zero.
                if let Some(n) = v.get("data_added").and_then(|x| x.as_u64()) {
                    data_added = n;
                }
            }
            _ => {}
        }
    }

    let status = child.wait().context("waiting for restic")?;
    let err = err_thread.join().unwrap_or_default();
    if !status.success() {
        // Keep restic's own wording in the message so run_backup can spot a lock error.
        bail!("restic backup failed: {}", err.trim());
    }
    Ok((snapshot, data_added))
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
    // Backstop even if the GUI is bypassed: never restore onto the filesystem root, which would
    // overwrite live files at their original paths. The caller picks a specific folder.
    let t = target.trim();
    if t.is_empty() || t == "/" {
        bail!("refusing to restore into \"{target}\": choose a specific folder, not the filesystem root");
    }
    // `--` so the snapshot id (a positional) cannot be read as a flag.
    let out = restic_cmd(job)?
        .arg("restore")
        .arg("--target")
        .arg(target)
        .arg("--")
        .arg(snapshot_id)
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
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
            excludes: Vec::new(),
            schedule: Schedule::Manual,
            last_run: None,
            at: None,
        };
        save_password(&job.id, "esp-test-pw").unwrap();
        init_repo(&job).expect("init");

        let mut seen_progress = false;
        let snap = run_backup(&job, |_phase| seen_progress = true).expect("backup");
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

    #[test]
    fn restore_refuses_filesystem_root() {
        // The guard runs before restic is ever invoked, so this needs no repo/restic.
        let job = BackupJob {
            id: "r".into(),
            name: "R".into(),
            sources: vec![],
            repo: Endpoint::local("/tmp/repo"),
            excludes: Vec::new(),
            schedule: Schedule::Manual,
            last_run: None,
            at: None,
        };
        assert!(restore(&job, "latest", "/").is_err());
        assert!(restore(&job, "latest", "  ").is_err());
        assert!(restore(&job, "latest", "").is_err());
    }

    #[test]
    fn lock_error_classification() {
        assert!(is_lock_error(
            "Fatal: repository is already locked by PID 1"
        ));
        assert!(is_lock_error("unable to create lock in backend"));
        assert!(!is_lock_error("restic backup failed: connection refused"));
    }

    #[test]
    fn unlock_is_safe_on_clean_repo() {
        if restic_path().is_none() {
            eprintln!("restic not installed; skipping");
            return;
        }
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("esp-unlock-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", base.join("cfg"));
        let repo = base.join("repo");
        let job = BackupJob {
            id: "unlock".into(),
            name: "Unlock".into(),
            sources: vec![],
            repo: Endpoint::local(repo.to_string_lossy().into_owned()),
            excludes: Vec::new(),
            schedule: Schedule::Manual,
            last_run: None,
            at: None,
        };
        save_password(&job.id, "pw").unwrap();
        init_repo(&job).expect("init");
        // No locks held: unlock is a safe no-op, and check passes on a fresh repo.
        unlock_repo(&job).expect("unlock clean repo");
        check_repo(&job).expect("check clean repo");
        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn check_detects_a_missing_pack() {
        if restic_path().is_none() {
            eprintln!("restic not installed; skipping");
            return;
        }
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("esp-check-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", base.join("cfg"));
        let data = base.join("data");
        let repo = base.join("repo");
        std::fs::create_dir_all(&data).unwrap();
        // Enough unique bytes to make at least one data pack.
        for i in 0..64 {
            std::fs::write(
                data.join(format!("f{i}.bin")),
                format!("payload-{i}").repeat(512),
            )
            .unwrap();
        }
        let job = BackupJob {
            id: "check".into(),
            name: "Check".into(),
            sources: vec![data.to_string_lossy().into_owned()],
            repo: Endpoint::local(repo.to_string_lossy().into_owned()),
            excludes: Vec::new(),
            schedule: Schedule::Manual,
            last_run: None,
            at: None,
        };
        save_password(&job.id, "pw").unwrap();
        init_repo(&job).expect("init");
        // A good backup must pass its own integrity gate.
        run_backup(&job, |_phase| {}).expect("backup + verify");

        // Now simulate the restic-over-rclone corruption class: a referenced pack never
        // finished uploading. Delete one data pack and prove check_repo catches it.
        let packs: Vec<PathBuf> = walk(repo.join("data"));
        let victim = packs.first().expect("repo should have data packs").clone();
        std::fs::remove_file(&victim).expect("remove a pack");
        let err = check_repo(&job).expect_err("check must fail on a missing pack");
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("integrity check failed"),
            "unexpected error: {msg}"
        );

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn backup_excludes_matching_paths() {
        if restic_path().is_none() {
            eprintln!("restic not installed; skipping");
            return;
        }
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("esp-excl-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", base.join("cfg"));
        let data = base.join("data");
        let repo = base.join("repo");
        let restored = base.join("restored");
        std::fs::create_dir_all(data.join("node_modules/dep")).unwrap();
        std::fs::create_dir_all(&restored).unwrap();
        std::fs::write(data.join("keep.txt"), b"keep me").unwrap();
        std::fs::write(data.join("node_modules/dep/junk.js"), b"skip me").unwrap();

        let job = BackupJob {
            id: "excl".into(),
            name: "Excl".into(),
            sources: vec![data.to_string_lossy().into_owned()],
            repo: Endpoint::local(repo.to_string_lossy().into_owned()),
            excludes: vec!["node_modules".into()],
            schedule: Schedule::Manual,
            last_run: None,
            at: None,
        };
        save_password(&job.id, "pw").unwrap();
        init_repo(&job).expect("init");
        run_backup(&job, |_phase| {}).expect("backup + verify");

        restore(&job, "latest", restored.to_str().unwrap()).expect("restore");
        let files: Vec<PathBuf> = std::fs::read_dir(&restored)
            .unwrap()
            .flat_map(|e| walk(e.unwrap().path()))
            .collect();
        assert!(
            files.iter().any(|p| p.ends_with("keep.txt")),
            "kept file should be present"
        );
        assert!(
            !files
                .iter()
                .any(|p| p.to_string_lossy().contains("node_modules")),
            "excluded node_modules must not be in the snapshot"
        );

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
