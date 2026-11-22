//! A local diagnostics log: a capped in-memory ring of timestamped events plus a per-session file,
//! so the user can SEE what happened, COPY it, and SAVE it to send for support if they choose.
//!
//! Account-free and offline by construction: nothing here is ever transmitted. Sending is a file the
//! user shares out of band, never an automatic upload, so the no-telemetry promise stays intact. No
//! secrets are recorded (the app keeps passwords and tokens out of error text); remote and file NAMES
//! can appear, which is expected and useful in a support log.

use crate::jobs::{config_dir, now_unix};
use std::collections::VecDeque;
use std::io::Write;
use std::sync::Mutex;

/// How many recent entries to keep in memory (and roughly what the dialog shows).
const CAP: usize = 500;

/// Severity of a logged event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Error,
}

impl Level {
    pub fn tag(self) -> &'static str {
        match self {
            Level::Info => "INFO",
            Level::Error => "ERROR",
        }
    }
}

/// One recorded event.
#[derive(Clone, Debug)]
pub struct LogEntry {
    /// Unix seconds.
    pub time: i64,
    pub level: Level,
    pub msg: String,
}

static LOG: Mutex<VecDeque<LogEntry>> = Mutex::new(VecDeque::new());

/// Path to the per-session diagnostics file: `<config dir>/diagnostics.log`.
fn log_path() -> Option<std::path::PathBuf> {
    config_dir().ok().map(|d| d.join("diagnostics.log"))
}

/// Begin a fresh session: clear the in-memory ring and truncate the on-disk file, so a shared log
/// is just this run and does not grow without bound. Call once at startup.
pub fn start_session() {
    LOG.lock().unwrap_or_else(|e| e.into_inner()).clear();
    if let Some(p) = log_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&p, b"");
    }
}

fn append_to_file(entry: &LogEntry) {
    if let Some(p) = log_path() {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&p)
        {
            let _ = writeln!(
                f,
                "{}  {}  {}",
                fmt_time(entry.time),
                entry.level.tag(),
                entry.msg
            );
        }
    }
}

/// Record an event. Thread-safe, so worker threads can log too.
pub fn record(level: Level, msg: impl Into<String>) {
    let entry = LogEntry {
        time: now_unix(),
        level,
        msg: msg.into(),
    };
    append_to_file(&entry);
    let mut log = LOG.lock().unwrap_or_else(|e| e.into_inner());
    if log.len() >= CAP {
        log.pop_front();
    }
    log.push_back(entry);
}

pub fn info(msg: impl Into<String>) {
    record(Level::Info, msg);
}

pub fn error(msg: impl Into<String>) {
    record(Level::Error, msg);
}

/// All current entries, oldest first.
pub fn entries() -> Vec<LogEntry> {
    LOG.lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .cloned()
        .collect()
}

/// The whole log as shareable plain text (what the dialog copies and saves).
pub fn export_text() -> String {
    entries()
        .iter()
        .map(|e| format!("{}  {}  {}", fmt_time(e.time), e.level.tag(), e.msg))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format a Unix time as `HH:MM:SS` (UTC), enough to order a session's events without a date crate.
fn fmt_time(t: i64) -> String {
    let s = t.rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", s / 3_600, (s % 3_600) / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_orders_and_exports() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("oortery-diag-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_dir_all(&dir);
        start_session();

        info("app started");
        error("could not connect: boom");
        let es = entries();
        assert_eq!(es.len(), 2);
        assert_eq!(es[0].level, Level::Info); // oldest first
        assert_eq!(es[1].level, Level::Error);
        let text = export_text();
        assert!(text.contains("INFO  app started"));
        assert!(text.contains("ERROR  could not connect: boom"));
        // The per-session file holds the same lines, for the user to find and for headless tests.
        let file = std::fs::read_to_string(dir.join("oortery/diagnostics.log")).unwrap();
        assert!(file.contains("could not connect: boom"));

        start_session(); // a new session clears it
        assert!(entries().is_empty());

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fmt_time_is_hms() {
        assert_eq!(fmt_time(3_661), "01:01:01");
        assert_eq!(fmt_time(0), "00:00:00");
    }
}
