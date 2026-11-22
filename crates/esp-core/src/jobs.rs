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

    /// Interval-only due check: due if never run, or the interval has elapsed since the last run.
    /// Used for Hourly/6-hourly, and for Daily/Weekly that have no pinned time of day.
    pub fn is_due(self, last_run: Option<i64>, now: i64) -> bool {
        match self.interval_secs() {
            None => false,
            Some(interval) => match last_run {
                None => true,
                Some(last) => now - last >= interval,
            },
        }
    }

    /// Due check that honors a pinned time of day. When `at` is set and this is a Daily or Weekly
    /// schedule, the job is due once per day (or per week on the chosen weekday) after the chosen
    /// local time, and only if it has not already run for that occurrence. When `at` is `None`, or
    /// the schedule is not Daily/Weekly, this falls back to the plain interval check, so existing
    /// jobs behave exactly as before.
    pub fn is_due_at(self, at: Option<DayTime>, last_run: Option<i64>, now: i64) -> bool {
        match (self, at) {
            (Schedule::Daily, Some(dt)) | (Schedule::Weekly, Some(dt)) => {
                let occ = last_occurrence(now, dt, matches!(self, Schedule::Weekly));
                let not_yet_run = match last_run {
                    Some(lr) => lr < occ,
                    None => true,
                };
                now >= occ && not_yet_run
            }
            _ => self.is_due(last_run, now),
        }
    }
}

/// A pinned time of day for a schedule: 24-hour `hour`:`minute`, local time. For a Weekly schedule,
/// `weekday` (0 = Sunday .. 6 = Saturday) also pins the day; it is ignored for Daily.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayTime {
    pub hour: u8,
    pub minute: u8,
    #[serde(default)]
    pub weekday: u8,
}

impl DayTime {
    /// A short label like "02:00" or "Mon 02:00" (weekly).
    pub fn label(self, weekly: bool) -> String {
        let hm = format!("{:02}:{:02}", self.hour, self.minute);
        if weekly {
            const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
            format!("{} {hm}", DAYS[(self.weekday % 7) as usize])
        } else {
            hm
        }
    }
}

/// Local broken-down time of a Unix instant (system timezone), via libc. Unix-only; other targets
/// get a zeroed struct (the time-of-day feature is gated to unix in the UI).
#[cfg(unix)]
fn local_tm(unix: i64) -> libc::tm {
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let t = unix as libc::time_t;
        libc::localtime_r(&t, &mut tm);
        tm
    }
}

/// Unix seconds of `hour:minute:00` local time on the calendar day containing `day_anchor`.
/// DST-correct: `mktime` resolves the offset (`tm_isdst = -1`).
#[cfg(unix)]
fn hms_on_day(day_anchor: i64, hour: u8, minute: u8) -> i64 {
    let mut tm = local_tm(day_anchor);
    tm.tm_hour = hour as libc::c_int;
    tm.tm_min = minute as libc::c_int;
    tm.tm_sec = 0;
    tm.tm_isdst = -1;
    unsafe { libc::mktime(&mut tm) as i64 }
}

/// The most recent occurrence of the scheduled time at or before `now`. For `weekly`, steps back to
/// the chosen weekday. Each candidate day is resolved through `mktime` so DST is handled correctly.
#[cfg(unix)]
fn last_occurrence(now: i64, at: DayTime, weekly: bool) -> i64 {
    let today = hms_on_day(now, at.hour, at.minute);
    if !weekly {
        // Daily: today's time if we have reached it, else yesterday's.
        return if now >= today {
            today
        } else {
            hms_on_day(now - 86_400, at.hour, at.minute)
        };
    }
    // Weekly: find how many days back the target weekday is, then resolve that day's time. If that
    // occurrence is still ahead of now (today is the target weekday but before the time), go back a week.
    let cur_wd = local_tm(now).tm_wday as i64;
    let delta = (cur_wd - (at.weekday % 7) as i64).rem_euclid(7);
    let occ = hms_on_day(now - delta * 86_400, at.hour, at.minute);
    if now >= occ {
        occ
    } else {
        hms_on_day(now - (delta + 7) * 86_400, at.hour, at.minute)
    }
}

/// Non-unix fallback: no local-time scheduling, treat as "due now" style interval behaviour.
#[cfg(not(unix))]
fn last_occurrence(now: i64, _at: DayTime, _weekly: bool) -> i64 {
    now
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
    /// A pinned time of day for Daily/Weekly schedules. `None` keeps the plain interval behaviour.
    #[serde(default)]
    pub at: Option<DayTime>,
}

/// The user's home directory, OS-aware: `HOME` on Linux and macOS, `USERPROFILE` on Windows.
/// `None` if neither is set. Use this instead of reading `HOME` directly so path defaults (the
/// folder picker, restore target, exports, the mountpoint) work across platforms.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// Oortery's config directory: `$XDG_CONFIG_HOME/oortery` (or `~/.config/oortery`).
pub fn config_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| home_dir().map(|h| h.join(".config")))
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
            at: None,
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
            at: None,
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
    #[cfg(unix)]
    fn time_of_day_due_logic() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Pin the timezone so the assertions are deterministic (localtime_r reads TZ via tzset).
        extern "C" {
            fn tzset();
        }
        std::env::set_var("TZ", "UTC");
        unsafe { tzset() };

        let now = 1_700_000_000; // 2023-11-14 22:13:20 UTC, a Tuesday (wday=2)
        let two_am = DayTime {
            hour: 2,
            minute: 0,
            weekday: 0,
        };
        let occ = last_occurrence(now, two_am, false);
        assert_eq!(occ, 1_699_927_200, "02:00 UTC on 2023-11-14");
        assert!(occ <= now);

        // Daily: due when never run or when the last run predates this occurrence; not due once it
        // has run for this occurrence.
        assert!(Schedule::Daily.is_due_at(Some(two_am), None, now));
        assert!(!Schedule::Daily.is_due_at(Some(two_am), Some(occ), now));
        assert!(Schedule::Daily.is_due_at(Some(two_am), Some(occ - 1), now));

        // Before today's time, the most recent occurrence is yesterday's (within ~a day, DST-safe).
        let early = 1_699_921_000; // 2023-11-14 00:16:40 UTC, before 02:00
        let occ_y = last_occurrence(early, two_am, false);
        assert!(occ_y < early && early - occ_y < 25 * 3_600);

        // Weekly on today's weekday resolves to today's time; a different weekday steps back a few
        // days but stays within the past week.
        let tue = DayTime {
            hour: 2,
            minute: 0,
            weekday: 2,
        };
        assert_eq!(last_occurrence(now, tue, true), 1_699_927_200);
        let fri = DayTime {
            hour: 2,
            minute: 0,
            weekday: 5,
        };
        let occ_f = last_occurrence(now, fri, true);
        assert!(occ_f < now && now - occ_f < 8 * 86_400);

        // No pinned time is exactly the old interval behaviour; Manual is never due.
        assert_eq!(
            Schedule::Daily.is_due_at(None, Some(1_000), now),
            Schedule::Daily.is_due(Some(1_000), now)
        );
        assert!(!Schedule::Manual.is_due_at(Some(two_am), None, now));

        std::env::remove_var("TZ");
        unsafe { tzset() };
    }

    #[test]
    fn daytime_label() {
        let dt = DayTime {
            hour: 2,
            minute: 5,
            weekday: 1,
        };
        assert_eq!(dt.label(false), "02:05");
        assert_eq!(dt.label(true), "Mon 02:05");
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
            at: None,
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
            at: None,
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
