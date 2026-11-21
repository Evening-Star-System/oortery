//! Stuck-transfer detection.
//!
//! The most common competitor failure is not an error but a silent freeze: Proton Drive's
//! semaphore-leak deadlock parks every transfer at 0 B/s with no log, and Internxt's progress
//! bar sits at 50% for weeks. A single opaque spinner hides this; the user cannot tell "slow"
//! from "dead". This tracker watches successive `core/stats` samples and flags a transfer that
//! has made no byte progress at ~0 speed for longer than a threshold, so the UI can say
//! "stalled" and offer a restart instead of pretending all is well.
//!
//! The logic is pure: the caller supplies a monotonic time (seconds), so it is fully testable
//! without a clock or a daemon.

use crate::engine::Transfer;
use std::collections::{HashMap, HashSet};

/// How long a transfer may sit without byte progress before we call it stalled. Generous so a
/// slow-but-live backend (the research warns against calling slow "hung" prematurely) is not
/// falsely flagged.
pub const DEFAULT_STALL_SECS: f64 = 25.0;

/// A transfer is "moving" if its reported speed is at least this many bytes/sec. Below it we
/// treat the transfer as idle and lean on the no-progress timer.
const MOVING_SPEED: f64 = 1.0;

/// Tracks per-transfer progress across polls to detect stalls. One per live view.
pub struct StallTracker {
    threshold: f64,
    /// name -> (last observed bytes, time those bytes were last seen to advance)
    seen: HashMap<String, (i64, f64)>,
}

impl StallTracker {
    pub fn new(threshold_secs: f64) -> Self {
        StallTracker {
            threshold: threshold_secs,
            seen: HashMap::new(),
        }
    }

    /// Feed the current in-flight transfers and the current monotonic time (seconds). Returns
    /// the set of transfer names that are currently stalled. Transfers no longer present are
    /// forgotten, so a finished-then-restarted transfer starts fresh.
    pub fn observe(&mut self, now: f64, transfers: &[Transfer]) -> HashSet<String> {
        let mut stalled = HashSet::new();
        let mut present = HashSet::new();
        for t in transfers {
            present.insert(t.name.clone());
            let entry = self.seen.entry(t.name.clone()).or_insert((t.bytes, now));
            // Any forward byte progress resets the clock.
            if t.bytes > entry.0 {
                entry.0 = t.bytes;
                entry.1 = now;
            }
            let idle = t.speed < MOVING_SPEED;
            let no_progress_for = now - entry.1;
            if idle && no_progress_for >= self.threshold {
                stalled.insert(t.name.clone());
            }
        }
        self.seen.retain(|name, _| present.contains(name));
        stalled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx(name: &str, bytes: i64, speed: f64) -> Transfer {
        Transfer {
            name: name.to_string(),
            bytes,
            size: 1_000_000,
            percentage: 0.0,
            speed,
            eta: None,
        }
    }

    #[test]
    fn a_frozen_transfer_is_flagged_after_the_threshold() {
        let mut st = StallTracker::new(25.0);
        // Same bytes, zero speed, time advancing.
        assert!(st.observe(0.0, &[tx("big.iso", 500, 0.0)]).is_empty());
        assert!(st.observe(10.0, &[tx("big.iso", 500, 0.0)]).is_empty());
        assert!(st.observe(24.0, &[tx("big.iso", 500, 0.0)]).is_empty());
        let stalled = st.observe(26.0, &[tx("big.iso", 500, 0.0)]);
        assert!(stalled.contains("big.iso"), "should be stalled by 26s");
    }

    #[test]
    fn a_moving_transfer_never_stalls() {
        let mut st = StallTracker::new(25.0);
        for (i, bytes) in [100, 200, 300, 400, 500].into_iter().enumerate() {
            let now = i as f64 * 30.0; // long gaps, but bytes keep climbing
            assert!(
                st.observe(now, &[tx("stream", bytes, 1_000.0)]).is_empty(),
                "progressing transfer must not stall at t={now}"
            );
        }
    }

    #[test]
    fn resuming_clears_a_stall() {
        let mut st = StallTracker::new(25.0);
        st.observe(0.0, &[tx("f", 500, 0.0)]);
        assert!(st.observe(30.0, &[tx("f", 500, 0.0)]).contains("f"));
        // Bytes advance and speed returns: no longer stalled.
        let now_moving = st.observe(31.0, &[tx("f", 900, 2_000.0)]);
        assert!(!now_moving.contains("f"), "resumed transfer should clear");
    }

    #[test]
    fn idle_at_zero_speed_but_recently_advanced_is_not_stalled() {
        let mut st = StallTracker::new(25.0);
        // A brief 0-speed dip right after progress is normal, not a stall.
        st.observe(0.0, &[tx("f", 100, 5_000.0)]);
        let s = st.observe(5.0, &[tx("f", 100, 0.0)]);
        assert!(s.is_empty(), "a short lull is not a stall");
    }

    #[test]
    fn finished_transfer_is_forgotten() {
        let mut st = StallTracker::new(25.0);
        st.observe(0.0, &[tx("f", 500, 0.0)]);
        // Disappears from the sample set (completed/removed).
        st.observe(30.0, &[]);
        // Comes back as a fresh transfer at the same name: clock restarts, not instantly stalled.
        let s = st.observe(31.0, &[tx("f", 0, 0.0)]);
        assert!(s.is_empty(), "a reappearing transfer starts fresh");
    }
}
