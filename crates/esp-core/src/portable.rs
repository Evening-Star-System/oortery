//! Export and import the Oortery setup: sync jobs, backups, schedules, and a manifest of the
//! remotes they use. Deliberately carries NO secrets: rclone OAuth tokens, backend keys, and
//! restic repo passwords stay out of the bundle, so sharing or moving it leaks nothing. On a
//! new machine the user reconnects those remotes (reauth) and re-enters backup passwords; the
//! jobs and schedules come across intact.

use crate::backups::{load_backups, save_backups, BackupJob};
use crate::jobs::{load_jobs, save_jobs, Job};
use crate::list_remotes;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A remote referenced by the setup: its name and backend type, no secrets.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteRef {
    pub name: String,
    pub backend: String,
}

/// The portable setup bundle. `oortery_export` is a format version for forward compatibility.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bundle {
    pub oortery_export: u32,
    #[serde(default)]
    pub jobs: Vec<Job>,
    #[serde(default)]
    pub backups: Vec<BackupJob>,
    #[serde(default)]
    pub remotes: Vec<RemoteRef>,
}

/// What an import did, so the UI can tell the user what to reconnect.
#[derive(Clone, Debug, Default)]
pub struct ImportResult {
    pub jobs: usize,
    pub backups: usize,
    /// Remote names the imported setup uses that are not configured on this machine. The user
    /// must reconnect these (reauth) for the jobs to run.
    pub missing_remotes: Vec<String>,
}

/// Build the export bundle from the current setup (no secrets).
pub fn export_bundle() -> Result<String> {
    let remotes = list_remotes()
        .unwrap_or_default()
        .into_iter()
        .map(|r| RemoteRef {
            name: r.name,
            backend: r.backend,
        })
        .collect();
    let bundle = Bundle {
        oortery_export: 1,
        jobs: load_jobs().unwrap_or_default(),
        backups: load_backups().unwrap_or_default(),
        remotes,
    };
    serde_json::to_string_pretty(&bundle).context("serializing the export bundle")
}

/// Import a bundle, merging its jobs and backups into the current setup (an existing job or
/// backup with the same id is replaced; others are kept). Returns a summary including which
/// referenced remotes are not yet configured here.
pub fn import_bundle(json: &str) -> Result<ImportResult> {
    let bundle: Bundle =
        serde_json::from_str(json).context("this file is not a valid Oortery export")?;
    if bundle.oortery_export == 0 {
        anyhow::bail!("unrecognized export format");
    }

    // An imported bundle is UNTRUSTED input (the product encourages sharing it). Reject any
    // flag-shaped value: a path/remote/source beginning with '-' could be read as a restic or
    // rclone option (e.g. restic's `-o rclone.program=...`, arbitrary command execution).
    use crate::jobs::is_flaglike;
    for j in &bundle.jobs {
        if is_flaglike(&j.src.path)
            || is_flaglike(&j.dst.path)
            || j.src.remote.as_deref().is_some_and(is_flaglike)
            || j.dst.remote.as_deref().is_some_and(is_flaglike)
        {
            anyhow::bail!("this export contains an unsafe value and was not imported");
        }
    }
    for b in &bundle.backups {
        if b.sources.iter().any(|s| is_flaglike(s))
            || is_flaglike(&b.repo.path)
            || b.repo.remote.as_deref().is_some_and(is_flaglike)
        {
            anyhow::bail!("this export contains an unsafe value and was not imported");
        }
    }

    // Remotes the imported setup actually uses (from the jobs/backups themselves).
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for j in &bundle.jobs {
        referenced.extend(j.src.remote.clone());
        referenced.extend(j.dst.remote.clone());
    }
    for b in &bundle.backups {
        referenced.extend(b.repo.remote.clone());
    }

    // Imported jobs are made SAFE before they land: schedule forced to Manual and last_run to now,
    // so nothing the scheduler sees can auto-run (a Mirror can't silently wipe a destination), and
    // every id is re-slugged so it cannot carry '/' or '..' into the config path for its .pw file.
    let now = crate::jobs::now_unix();
    let mut jobs = load_jobs().unwrap_or_default();
    let n_jobs = bundle.jobs.len();
    for mut j in bundle.jobs {
        j.id = crate::jobs::normalize_slug(&j.id, "imported");
        j.schedule = crate::jobs::Schedule::Manual;
        j.last_run = Some(now);
        match jobs.iter().position(|x| x.id == j.id) {
            Some(pos) => jobs[pos] = j,
            None => jobs.push(j),
        }
    }
    save_jobs(&jobs)?;

    let mut backups = load_backups().unwrap_or_default();
    let n_backups = bundle.backups.len();
    for mut b in bundle.backups {
        b.id = crate::jobs::normalize_slug(&b.id, "imported");
        b.schedule = crate::jobs::Schedule::Manual;
        b.last_run = Some(now);
        match backups.iter().position(|x| x.id == b.id) {
            Some(pos) => backups[pos] = b,
            None => backups.push(b),
        }
    }
    save_backups(&backups)?;

    let configured: BTreeSet<String> = list_remotes()
        .unwrap_or_default()
        .into_iter()
        .map(|r| r.name)
        .collect();
    let missing_remotes = referenced.difference(&configured).cloned().collect();

    Ok(ImportResult {
        jobs: n_jobs,
        backups: n_backups,
        missing_remotes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{Endpoint, Schedule, SyncMode};

    #[test]
    fn export_import_roundtrip_merges_and_flags_missing_remotes() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("oortery-portable-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        // Start from a clean config.
        let _ = std::fs::remove_dir_all(&dir);

        // Seed one job that uses a remote "gdrive" which will NOT be configured here.
        save_jobs(&[Job {
            id: "a".into(),
            name: "A".into(),
            src: Endpoint::local("/data"),
            dst: Endpoint::remote("gdrive", "backup"),
            mode: SyncMode::Copy,
            schedule: Schedule::Daily,
            last_run: None,
            at: None,
        }])
        .unwrap();
        let exported = export_bundle().unwrap();
        assert!(exported.contains("gdrive"));
        assert!(!exported.contains("token")); // no secrets

        // Wipe local jobs, then import the bundle back.
        save_jobs(&[]).unwrap();
        let res = import_bundle(&exported).unwrap();
        assert_eq!(res.jobs, 1);
        let back = load_jobs().unwrap();
        assert_eq!(back.len(), 1);
        // Imported jobs land as Manual so a shared bundle can never auto-run (was Daily on export).
        assert_eq!(back[0].schedule, Schedule::Manual);
        assert!(back[0].last_run.is_some());
        // gdrive is referenced but (almost certainly) not configured on this box.
        if !list_remotes()
            .unwrap_or_default()
            .iter()
            .any(|r| r.name == "gdrive")
        {
            assert!(res.missing_remotes.contains(&"gdrive".to_string()));
        }

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_non_oortery_json() {
        assert!(import_bundle("{\"something\": 1}").is_err());
    }

    #[test]
    fn import_neutralizes_hostile_id_and_schedule() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir =
            std::env::temp_dir().join(format!("oortery-portable-hostile-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_dir_all(&dir);
        save_jobs(&[]).unwrap();

        // A crafted bundle: a Daily Mirror whose id tries to escape the config dir.
        let bundle = Bundle {
            oortery_export: 1,
            jobs: vec![Job {
                id: "../../../../tmp/evil".into(),
                name: "x".into(),
                src: Endpoint::local("/tmp"),
                dst: Endpoint::local("/home/victim/Documents"),
                mode: SyncMode::Mirror,
                schedule: Schedule::Daily,
                last_run: None,
                at: None,
            }],
            backups: vec![],
            remotes: vec![],
        };
        let json = serde_json::to_string(&bundle).unwrap();
        import_bundle(&json).unwrap();
        let back = load_jobs().unwrap();
        assert_eq!(back.len(), 1);
        // Id can no longer carry '/' or '..'; schedule is neutered to Manual (cannot auto-run).
        assert!(!back[0].id.contains('/') && !back[0].id.contains(".."));
        assert_eq!(back[0].schedule, Schedule::Manual);
        assert!(back[0].last_run.is_some());

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_rejects_flaglike_source() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir =
            std::env::temp_dir().join(format!("oortery-portable-flag-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_dir_all(&dir);

        // A backup source beginning with '-' would reach restic's option parser; reject the import.
        let bundle = Bundle {
            oortery_export: 1,
            jobs: vec![],
            backups: vec![BackupJob {
                id: "b".into(),
                name: "B".into(),
                sources: vec![
                    "-o".into(),
                    "rclone.program=/tmp/evil".into(),
                    "/data".into(),
                ],
                repo: Endpoint::local("/tmp/repo"),
                excludes: vec![],
                schedule: Schedule::Daily,
                last_run: None,
                at: None,
            }],
            remotes: vec![],
        };
        let json = serde_json::to_string(&bundle).unwrap();
        assert!(import_bundle(&json).is_err());

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
