//! The Overview's protection summary: a status-first "are we OK?" read computed from cheap,
//! local data (no network). The research (see .dev/DASHBOARD-RESEARCH.md) is emphatic that a
//! backup home screen should lead with one health verdict and freshness, not vanity counters.
//!
//! Richer, network-backed signals (restore-point counts, protected size, integrity) are layered
//! on separately so this read stays instant.

use crate::backups::load_backups;
use crate::jobs::{load_jobs, now_unix};
use crate::list_remotes;

/// How much grace past a scheduled interval before a missed backup counts as overdue. Two
/// intervals avoids crying wolf over a single skipped run (e.g. the machine was asleep).
const OVERDUE_GRACE: i64 = 2;

/// The single at-a-glance health state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    /// Nothing set up yet: guide the user to connect and protect.
    FirstRun,
    /// Everything that should have run, has, recently.
    Healthy,
    /// Something needs the user: an overdue or never-run backup.
    Attention,
}

/// A cheap, local snapshot of how protected the user's data is right now.
#[derive(Clone, Debug)]
pub struct Summary {
    pub remotes: usize,
    pub syncs: usize,
    pub backups: usize,
    /// Backups that have run at least once.
    pub backups_run: usize,
    /// Scheduled backups that are overdue (missed by > grace intervals) or scheduled-but-never-run.
    pub overdue: usize,
    /// Seconds since the most recent backup ran, if any has.
    pub newest_backup_age: Option<i64>,
    pub health: Health,
    /// The verdict line to show at the top, naming the specific issue when there is one.
    pub headline: String,
}

/// Compute the protection summary from local config and stored run times. No network.
pub fn summary() -> Summary {
    let now = now_unix();
    let remotes = list_remotes().map(|r| r.len()).unwrap_or(0);
    let syncs = load_jobs().map(|j| j.len()).unwrap_or(0);
    let backups = load_backups().unwrap_or_default();

    // Honest read: a backup counts as "run"/"protected" only when it has actually SUCCEEDED, not
    // when a run was merely dispatched. Success times come from the runlog, keyed by job id.
    let success = |b: &crate::backups::BackupJob| crate::runlog::last_success("backup", &b.id);
    let backups_run = backups.iter().filter(|b| success(b).is_some()).count();
    let overdue = backups
        .iter()
        .filter(|b| is_overdue(b.schedule.interval_secs(), success(b), now))
        .count();
    let newest_backup_age = backups.iter().filter_map(success).map(|lr| now - lr).min();

    let (health, headline) = verdict(remotes, syncs, backups.len(), backups_run, overdue);

    Summary {
        remotes,
        syncs,
        backups: backups.len(),
        backups_run,
        overdue,
        newest_backup_age,
        health,
        headline,
    }
}

/// A scheduled backup is overdue if it has never run, or missed its interval by the grace factor.
/// Manual backups (no interval) are never "overdue" on their own.
fn is_overdue(interval: Option<i64>, last_run: Option<i64>, now: i64) -> bool {
    match interval {
        None => false,
        Some(iv) => match last_run {
            None => true,
            Some(lr) => now - lr > iv * OVERDUE_GRACE,
        },
    }
}

fn verdict(
    remotes: usize,
    syncs: usize,
    backups: usize,
    backups_run: usize,
    overdue: usize,
) -> (Health, String) {
    if remotes == 0 && syncs == 0 && backups == 0 {
        return (
            Health::FirstRun,
            "Connect a cloud to get started".to_string(),
        );
    }
    if overdue > 0 {
        let noun = if overdue == 1 {
            "backup is"
        } else {
            "backups are"
        };
        return (Health::Attention, format!("{overdue} {noun} overdue"));
    }
    if backups == 0 {
        return (
            Health::Attention,
            "No backups yet. Protect a folder to be safe".to_string(),
        );
    }
    if backups_run == 0 {
        return (
            Health::Attention,
            "Your backups have not run yet".to_string(),
        );
    }
    (Health::Healthy, "Everything is protected".to_string())
}

/// A short human phrase for an age in seconds ("just now", "3h ago", "2d ago").
pub fn ago(secs: i64) -> String {
    let s = secs.max(0);
    if s < 90 {
        "just now".to_string()
    } else if s < 3_600 {
        format!("{}m ago", s / 60)
    } else if s < 86_400 {
        format!("{}h ago", s / 3_600)
    } else {
        format!("{}d ago", s / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overdue_logic() {
        let day = 86_400;
        // Manual (no interval) is never overdue.
        assert!(!is_overdue(None, None, 1_000_000));
        // Scheduled but never run -> overdue.
        assert!(is_overdue(Some(day), None, 1_000_000));
        // Ran within grace -> fine; past grace -> overdue.
        assert!(!is_overdue(Some(day), Some(1_000_000 - day), 1_000_000));
        assert!(is_overdue(Some(day), Some(1_000_000 - day * 3), 1_000_000));
    }

    #[test]
    fn verdicts() {
        assert_eq!(verdict(0, 0, 0, 0, 0).0, Health::FirstRun);
        assert_eq!(verdict(1, 0, 2, 2, 1).0, Health::Attention); // one overdue
        assert_eq!(verdict(1, 0, 0, 0, 0).0, Health::Attention); // remote but no backups
        assert_eq!(verdict(1, 0, 2, 0, 0).0, Health::Attention); // backups never run
        assert_eq!(verdict(1, 0, 2, 2, 0).0, Health::Healthy); // all run, none overdue
    }

    #[test]
    fn summary_counts_a_backup_only_after_real_success() {
        use crate::backups::{save_backups, BackupJob};
        use crate::jobs::{Endpoint, Schedule};
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("oortery-dash-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_dir_all(&dir);

        // A manual backup exists but has never succeeded: it is NOT counted as protected.
        save_backups(&[BackupJob {
            id: "photos".into(),
            name: "Photos".into(),
            sources: vec!["/data".into()],
            repo: Endpoint::local("/tmp/repo"),
            excludes: vec![],
            schedule: Schedule::Manual,
            last_run: None,
            at: None,
        }])
        .unwrap();
        let s = summary();
        assert_eq!(s.backups, 1);
        assert_eq!(s.backups_run, 0);
        assert_eq!(s.health, Health::Attention);

        // Only a recorded SUCCESS flips it to protected (a dispatch alone would not touch this).
        crate::runlog::record_success("backup", "photos").unwrap();
        let s = summary();
        assert_eq!(s.backups_run, 1);
        assert_eq!(s.health, Health::Healthy);
        assert!(s.newest_backup_age.is_some());

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ago_phrasing() {
        assert_eq!(ago(10), "just now");
        assert_eq!(ago(600), "10m ago");
        assert_eq!(ago(7_200), "2h ago");
        assert_eq!(ago(172_800), "2d ago");
    }
}
