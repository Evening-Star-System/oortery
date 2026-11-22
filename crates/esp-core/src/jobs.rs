//! Sync jobs: backend-neutral job definitions and their on-disk store.
//!
//! A `Job` says what to keep in step and how. It never spells out an rclone path by hand;
//! `Endpoint::fs()` renders that so the UI stays backend-neutral. Jobs persist as a plain
//! JSON array under the user's config dir.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Set true when a corrupt store was moved aside this session, so the UI can tell the user once.
static RECOVERED_CORRUPT: AtomicBool = AtomicBool::new(false);

/// Whether a corrupt settings file was set aside since this was last checked. Reading it clears
/// the flag, so the UI toasts about the recovery exactly once.
pub fn took_corrupt_recovery() -> bool {
    RECOVERED_CORRUPT.swap(false, Ordering::Relaxed)
}

/// Read a JSON-array store. A missing file is an empty list. A CORRUPT file is never silently
/// dropped or later clobbered: it is renamed aside to `<name>.corrupt-<unixtime>` (so the user can
/// recover it) and an empty list is returned, so the app keeps working and the next save writes a
/// fresh file rather than overwriting the damaged one. Returns Err only on a real IO failure.
pub(crate) fn load_store<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    match serde_json::from_slice(&bytes) {
        Ok(v) => Ok(v),
        Err(_) => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "store".to_string());
            let aside = path.with_file_name(format!("{name}.corrupt-{}", now_unix()));
            // Best effort: if the rename fails we still degrade to empty, but we never clobber.
            if std::fs::rename(path, &aside).is_ok() {
                RECOVERED_CORRUPT.store(true, Ordering::Relaxed);
            }
            Ok(Vec::new())
        }
    }
}

/// Unix time now, in whole seconds. Used for scheduling (`Schedule::is_due`).
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// How often a job runs on its own. The interval is measured from the last run, so the app
/// only needs to be open when a job comes due (true background running, app closed, is a later
/// addition via a user service).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Schedule {
    /// Only runs when the user clicks Run. The default.
    #[default]
    Manual,
    Hourly,
    SixHourly,
    Daily,
    Weekly,
}

impl Schedule {
    /// All schedules in display order (for building a chooser).
    pub const ALL: [Schedule; 5] = [
        Schedule::Manual,
        Schedule::Hourly,
        Schedule::SixHourly,
        Schedule::Daily,
        Schedule::Weekly,
    ];

    /// Seconds between automatic runs, or `None` for a manual-only job.
    pub fn interval_secs(self) -> Option<i64> {
        match self {
            Schedule::Manual => None,
            Schedule::Hourly => Some(3_600),
            Schedule::SixHourly => Some(6 * 3_600),
            Schedule::Daily => Some(24 * 3_600),
            Schedule::Weekly => Some(7 * 24 * 3_600),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Schedule::Manual => "Manual",
            Schedule::Hourly => "Every hour",
            Schedule::SixHourly => "Every 6 hours",
            Schedule::Daily => "Every day",
            Schedule::Weekly => "Every week",
        }
    }

    /// True if a job on this schedule is due to run now, given when it last ran (`None` = never).
    /// A never-run scheduled job is due immediately; a manual job is never due.
    pub fn is_due(self, last_run: Option<i64>, now: i64) -> bool {
        match self.interval_secs() {
            None => false,
            Some(interval) => match last_run {
                None => true,
                Some(last) => now - last >= interval,
            },
        }
    }
}

/// How a sync reconciles destination against source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    /// Add and update files on the destination; never delete. The safe default.
    Copy,
    /// Make the destination identical to the source, deleting anything extra.
    Mirror,
}

impl SyncMode {
    pub fn label(self) -> &'static str {
        match self {
            SyncMode::Copy => "Copy",
            SyncMode::Mirror => "Mirror",
        }
    }

    /// True for modes that can delete files on the destination.
    pub fn is_destructive(self) -> bool {
        matches!(self, SyncMode::Mirror)
    }

    /// The rclone rc command that runs this mode.
    pub fn rc_command(self) -> &'static str {
        match self {
            SyncMode::Copy => "sync/copy",
            SyncMode::Mirror => "sync/sync",
        }
    }
}

/// One side of a job: a configured remote plus a path, or a local path (`remote = None`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Endpoint {
    #[serde(default)]
    pub remote: Option<String>,
    #[serde(default)]
    pub path: String,
}

impl Endpoint {
    /// A path on this computer.
    pub fn local(path: impl Into<String>) -> Self {
        Endpoint {
            remote: None,
            path: path.into(),
        }
    }

    /// A path on a configured remote.
    pub fn remote(remote: impl Into<String>, path: impl Into<String>) -> Self {
        Endpoint {
            remote: Some(remote.into()),
            path: path.into(),
        }
    }

    /// The rclone fs string: `remote:path`, or a bare local path.
    pub fn fs(&self) -> String {
        match &self.remote {
            Some(r) => format!("{r}:{}", self.path),
            None => self.path.clone(),
        }
    }

    /// The restic repository string for this location: a bare local path, or
    /// `rclone:remote:path` so restic backs up over our rclone rail.
    pub fn restic_repo(&self) -> String {
        match &self.remote {
            Some(r) => format!("rclone:{r}:{}", self.path),
            None => self.path.clone(),
        }
    }

    /// A short, human label for the endpoint.
    pub fn display(&self) -> String {
        match &self.remote {
            Some(r) => format!("{r}:{}", self.path),
            None if self.path.is_empty() => "this computer".to_string(),
            None => self.path.clone(),
        }
    }
}

/// A saved sync: what to keep in step, and how.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub name: String,
    pub src: Endpoint,
    pub dst: Endpoint,
    pub mode: SyncMode,
    /// How often to run on its own. `default` keeps older stored jobs loadable.
    #[serde(default)]
    pub schedule: Schedule,
    /// Unix seconds of the last run, for scheduling. `None` until it has run once.
    #[serde(default)]
    pub last_run: Option<i64>,
}

/// Oortery's config directory: `$XDG_CONFIG_HOME/oortery` (or `~/.config/oortery`).
pub fn config_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .context("no XDG_CONFIG_HOME or HOME to locate the config dir")?;
    Ok(base.join("oortery"))
}

/// Path to the jobs store: `<config dir>/jobs.json`.
pub fn jobs_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("jobs.json"))
}

/// Load saved jobs. An absent store is an empty list; a corrupt one is set aside, not lost.
pub fn load_jobs() -> Result<Vec<Job>> {
    load_store(&jobs_path()?)
}

/// Save the whole set of jobs atomically (temp file + rename).
pub fn save_jobs(jobs: &[Job]) -> Result<()> {
    let path = jobs_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let json = serde_json::to_vec_pretty(jobs).context("serializing jobs")?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

/// Normalize an arbitrary string to a safe id slug: lowercased, only `[a-z0-9-]`, runs of any
/// other characters collapsed to a single dash and trimmed, with `fallback` when nothing is left.
/// This is the ONLY shape an id may take, so `/`, `..`, and control characters cannot survive into
/// a filesystem path (e.g. the per-job `.pw` file). Shared by `slug_id` and `sanitize_id`.
pub fn normalize_slug(name: &str, fallback: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let base = slug.trim_matches('-').to_string();
    if base.is_empty() {
        fallback.to_string()
    } else {
        base
    }
}

/// Disambiguate `base` against ids already `taken`, appending `-2`, `-3`, ... until unique.
pub fn unique_id(base: &str, taken: &[String]) -> String {
    if !taken.iter().any(|id| id == base) {
        return base.to_string();
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !taken.iter().any(|id| id == &candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// A stable, id-friendly slug from a name, disambiguated against existing jobs.
pub fn slug_id(name: &str, existing: &[Job]) -> String {
    let taken: Vec<String> = existing.iter().map(|j| j.id.clone()).collect();
    unique_id(&normalize_slug(name, "sync"), &taken)
}

/// Re-slug an id arriving from an untrusted source (an imported bundle) and disambiguate it
/// against ids already `taken`, so it can never carry `/`, `..`, or control characters.
pub fn sanitize_id(raw: &str, taken: &[String]) -> String {
    unique_id(&normalize_slug(raw, "imported"), taken)
}

/// True if a value would be read as a command-line flag (begins with `-`). Bundle-supplied paths,
/// remotes, and sources must not be flag-shaped, or they could reach restic/rclone's option parser
/// (e.g. restic's `-o rclone.program=...`).
pub fn is_flaglike(s: &str) -> bool {
    s.starts_with('-')
}

/// For a destructive Mirror, refuse when a LOCAL source is missing, unmounted, or empty: rclone
/// would otherwise delete the whole destination to match an empty source (the external-drive
/// footgun). `Ok(())` means safe to run. Copy is always safe; remote sources are left to the engine.
pub fn guard_local_mirror_source(job: &Job) -> Result<()> {
    if job.mode != SyncMode::Mirror || job.src.remote.is_some() {
        return Ok(());
    }
    let empty = match std::fs::read_dir(&job.src.path) {
        Ok(mut entries) => entries.next().is_none(),
        Err(_) => true, // missing / unmounted / unreadable
    };
    if empty {
        bail!(
            "the source \"{}\" looks empty or unmounted; refusing to Mirror so the destination is not wiped",
            job.src.path
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fs_renders_remote_and_local() {
        assert_eq!(Endpoint::remote("gdrive", "backup").fs(), "gdrive:backup");
        assert_eq!(Endpoint::local("/home/me/docs").fs(), "/home/me/docs");
    }

    #[test]
    fn slug_is_stable_and_disambiguated() {
        let a = Job {
            id: "home-photos".into(),
            name: "Home Photos".into(),
            src: Endpoint::local("/a"),
            dst: Endpoint::remote("gdrive", "photos"),
            mode: SyncMode::Copy,
            schedule: Schedule::Manual,
            last_run: None,
        };
        assert_eq!(slug_id("Home Photos!!", &[]), "home-photos");
        assert_eq!(
            slug_id("Home Photos", std::slice::from_ref(&a)),
            "home-photos-2"
        );
        assert_eq!(slug_id("   ", &[]), "sync");
    }

    #[test]
    fn sanitize_id_strips_path_traversal() {
        // A hostile imported id cannot keep slashes or dots: no escaping the config dir.
        assert_eq!(sanitize_id("../../../../tmp/evil", &[]), "tmp-evil");
        assert_eq!(sanitize_id("a/b/c", &[]), "a-b-c");
        assert_eq!(sanitize_id("", &[]), "imported");
        assert_eq!(sanitize_id("...", &[]), "imported");
        // Disambiguates against ids already taken.
        assert_eq!(sanitize_id("docs", &["docs".to_string()]), "docs-2");
    }

    #[test]
    fn flaglike_detects_option_shaped_values() {
        assert!(is_flaglike("-o"));
        assert!(is_flaglike("--target"));
        assert!(!is_flaglike("/home/me/docs"));
        assert!(!is_flaglike("gdrive:backup"));
    }

    #[test]
    fn mirror_guard_refuses_empty_or_missing_local_source() {
        let mk = |path: &str, mode| Job {
            id: "j".into(),
            name: "J".into(),
            src: Endpoint::local(path),
            dst: Endpoint::remote("gdrive", "x"),
            mode,
            schedule: Schedule::Manual,
            last_run: None,
        };
        let base = std::env::temp_dir().join(format!("esp-mirror-guard-{}", std::process::id()));
        let empty = base.join("empty");
        let full = base.join("full");
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(&full).unwrap();
        std::fs::write(full.join("f.txt"), b"x").unwrap();
        let missing = base.join("does-not-exist");

        // Mirror from an empty or missing local source is refused.
        assert!(guard_local_mirror_source(&mk(empty.to_str().unwrap(), SyncMode::Mirror)).is_err());
        assert!(
            guard_local_mirror_source(&mk(missing.to_str().unwrap(), SyncMode::Mirror)).is_err()
        );
        // A non-empty source is fine, and Copy is never guarded.
        assert!(guard_local_mirror_source(&mk(full.to_str().unwrap(), SyncMode::Mirror)).is_ok());
        assert!(guard_local_mirror_source(&mk(empty.to_str().unwrap(), SyncMode::Copy)).is_ok());
        // A remote source is left to the engine, never refused here.
        let mut remote = mk("x", SyncMode::Mirror);
        remote.src = Endpoint::remote("gdrive", "data");
        assert!(guard_local_mirror_source(&remote).is_ok());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn schedule_due_logic() {
        // Manual never runs on its own.
        assert!(!Schedule::Manual.is_due(None, 10_000));
        assert!(!Schedule::Manual.is_due(Some(0), 10_000_000));
        // A never-run scheduled job is due immediately.
        assert!(Schedule::Daily.is_due(None, 10_000));
        // Not due until the interval elapses, then due.
        let day = 24 * 3_600;
        assert!(!Schedule::Daily.is_due(Some(1_000), 1_000 + day - 1));
        assert!(Schedule::Daily.is_due(Some(1_000), 1_000 + day));
        assert!(Schedule::Hourly.is_due(Some(1_000), 1_000 + 3_600));
        assert_eq!(Schedule::Weekly.interval_secs(), Some(7 * 24 * 3_600));
    }

    #[test]
    fn corrupt_store_is_set_aside_not_lost_or_clobbered() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("esp-corrupt-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = took_corrupt_recovery(); // clear any prior flag

        // Write a deliberately corrupt jobs.json.
        let path = jobs_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ this is not valid json").unwrap();

        // Loading degrades to empty (does not error/crash) and flags the recovery.
        let loaded = load_jobs().unwrap();
        assert!(loaded.is_empty());
        assert!(took_corrupt_recovery(), "recovery flag should be set");

        // The damaged file is preserved aside, and the original path no longer holds the bad bytes.
        assert!(!path.exists(), "corrupt file should have been moved aside");
        let aside: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".corrupt-"))
            .collect();
        assert_eq!(aside.len(), 1, "exactly one .corrupt- backup expected");
        let preserved = std::fs::read(aside[0].path()).unwrap();
        assert_eq!(preserved, b"{ this is not valid json");

        // A following save writes a valid file and leaves the backup intact (no clobber).
        save_jobs(&[Job {
            id: "new".into(),
            name: "New".into(),
            src: Endpoint::local("/a"),
            dst: Endpoint::remote("gdrive", "x"),
            mode: SyncMode::Copy,
            schedule: Schedule::Manual,
            last_run: None,
        }])
        .unwrap();
        assert_eq!(load_jobs().unwrap().len(), 1);
        assert!(aside[0].path().exists(), "backup must survive the save");

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn jobs_roundtrip_through_disk() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("esp-jobs-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        assert!(load_jobs().unwrap().is_empty());
        let jobs = vec![Job {
            id: "docs".into(),
            name: "Docs".into(),
            src: Endpoint::local("/home/me/docs"),
            dst: Endpoint::remote("gdrive", "docs"),
            mode: SyncMode::Mirror,
            schedule: Schedule::Manual,
            last_run: None,
        }];
        save_jobs(&jobs).unwrap();
        let back = load_jobs().unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].id, "docs");
        assert_eq!(back[0].mode, SyncMode::Mirror);
        assert_eq!(back[0].src.fs(), "/home/me/docs");
        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
