//! Per-backend throttle and retry tuning.
//!
//! The research sweep (see .dev/COMPETITOR-BUGS.md) found the dominant competitor failure is
//! not a missing feature but a backend hammered past its rate limit until it hangs: Proton
//! Drive deadlocks on many small files, Google Drive 429s, pCloud stalls. rclone's own
//! defaults are tuned for throughput, not for surviving a touchy consumer backend. This table
//! dials transfers/checkers/tps down and low-level retries up, per backend, so ESP is gentle
//! by default where it matters and fast where it can be.
//!
//! The values become an rc `_config` object on sync jobs and `RCLONE_*` env on restic runs.
//! We touch only the four keys that marshal cleanly into rclone's ConfigInfo: `Transfers`,
//! `Checkers`, `TPSLimit`, `LowLevelRetries`.

use crate::engine::rc_at;
use std::collections::BTreeMap;
use std::process::Command;

/// Rate and retry settings for one backend. `None` means "leave rclone's default".
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tuning {
    /// Concurrent file transfers. Low on touchy backends to avoid the Proton semaphore-leak
    /// deadlock (a failed multi-block upload at high concurrency can wedge all uploads).
    pub transfers: Option<i64>,
    /// Concurrent directory checkers. Low on backends whose listing/metadata API rate-limits.
    pub checkers: Option<i64>,
    /// Transactions per second ceiling. The single most effective Proton/Drive throttle.
    pub tpslimit: Option<f64>,
    /// Low-level retries for transient backend errors. Research: never 1 (that turned a
    /// retriable blip into corruption); raise above the default so blips get retried.
    pub low_level_retries: Option<i64>,
}

impl Tuning {
    /// Render as an rc `_config` object. Keys match rclone's ConfigInfo field names so
    /// `rc.Reshape` applies them. Empty when every field is default.
    pub fn as_rc_config(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        if let Some(v) = self.transfers {
            m.insert("Transfers".into(), v.into());
        }
        if let Some(v) = self.checkers {
            m.insert("Checkers".into(), v.into());
        }
        if let Some(v) = self.tpslimit {
            m.insert("TPSLimit".into(), v.into());
        }
        if let Some(v) = self.low_level_retries {
            m.insert("LowLevelRetries".into(), v.into());
        }
        serde_json::Value::Object(m)
    }

    /// Render as `RCLONE_*` environment pairs, for spawning restic (which shells out to
    /// rclone and honors these). Same four knobs as the rc config.
    pub fn as_rclone_env(&self) -> Vec<(String, String)> {
        let mut env = Vec::new();
        if let Some(v) = self.transfers {
            env.push(("RCLONE_TRANSFERS".into(), v.to_string()));
        }
        if let Some(v) = self.checkers {
            env.push(("RCLONE_CHECKERS".into(), v.to_string()));
        }
        if let Some(v) = self.tpslimit {
            env.push(("RCLONE_TPSLIMIT".into(), v.to_string()));
        }
        if let Some(v) = self.low_level_retries {
            env.push(("RCLONE_LOW_LEVEL_RETRIES".into(), v.to_string()));
        }
        env
    }

    pub fn is_default(&self) -> bool {
        *self == Tuning::default()
    }
}

/// VFS + FUSE mount tuning for serving a remote as a folder.
///
/// rclone's default mount runs with `--vfs-cache-mode off`, which makes the mount behave unlike
/// a real disk: an app that opens a file read-write, seeks, or re-reads it blocks or errors, and
/// on a slow backend a storm of per-file getattr/read requests (a file manager stat-ing and
/// thumbnailing every entry of a large media folder) can saturate the daemon until the kernel
/// drops the mount and the file manager freezes. Browsing a large folder on an Internxt mount
/// (reached over WebDAV) hit exactly this and crashed the file manager. We give every cloud mount
/// a cached, disk-like profile, and a gentler one for slow network backends that cannot poll for
/// changes.
///
/// `None` fields are left at rclone's default; a backend that wants no override (local disk,
/// already a real filesystem) returns the all-default value and we send no opts, exactly as
/// before.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MountTuning {
    /// VFS cache mode: "full" serves reads and writes through a local disk cache so the mount
    /// behaves like a real disk (seeks, re-reads, open-for-write all work). The canonical rclone
    /// remedy for apps misbehaving on a mount.
    pub cache_mode: Option<&'static str>,
    /// Cap on the on-disk VFS cache so it cannot grow without bound.
    pub cache_max_size: Option<&'static str>,
    /// Evict a cached object this long after its last access.
    pub cache_max_age: Option<&'static str>,
    /// How long the kernel caches file/dir attributes. Raised on slow backends to collapse a
    /// file manager's getattr storm into far fewer daemon round-trips.
    pub attr_timeout: Option<&'static str>,
    /// How long rclone caches a directory listing. Longer on slow backends that cannot poll for
    /// changes, so re-navigating a big folder does not re-list it over the network.
    pub dir_cache_time: Option<&'static str>,
    /// Skip reading modification times, trimming a per-object op on backends where it costs one.
    pub no_mod_time: bool,
}

impl MountTuning {
    /// Render as the `vfsOpt` and `mountOpt` objects `mount/mount` accepts. Keys match rclone's
    /// option FieldNames (verified against `rclone rc --loopback options/info`). Unset fields are
    /// omitted, so an all-default tuning yields two empty objects (same as sending no opts).
    pub fn as_rc_params(&self) -> (serde_json::Value, serde_json::Value) {
        let mut vfs = serde_json::Map::new();
        if let Some(v) = self.cache_mode {
            vfs.insert("CacheMode".into(), v.into());
        }
        if let Some(v) = self.cache_max_size {
            vfs.insert("CacheMaxSize".into(), v.into());
        }
        if let Some(v) = self.cache_max_age {
            vfs.insert("CacheMaxAge".into(), v.into());
        }
        if let Some(v) = self.dir_cache_time {
            vfs.insert("DirCacheTime".into(), v.into());
        }
        if self.no_mod_time {
            vfs.insert("NoModTime".into(), true.into());
        }
        let mut mnt = serde_json::Map::new();
        if let Some(v) = self.attr_timeout {
            mnt.insert("AttrTimeout".into(), v.into());
        }
        (
            serde_json::Value::Object(vfs),
            serde_json::Value::Object(mnt),
        )
    }

    pub fn is_default(&self) -> bool {
        *self == MountTuning::default()
    }
}

/// The mount tuning for a backend id (rclone type, e.g. "webdav", "drive", "s3").
///
/// Three tiers: a local disk needs nothing (it is already a real filesystem); slow network
/// backends that cannot poll for changes (WebDAV -- how Internxt is reached -- plus Proton) get
/// the fullest cache, the longest attribute/listing caches, and no modtime reads, so a file
/// manager browsing a big folder does not storm the daemon; every other cloud gets a cached,
/// disk-like profile with moderate caches.
pub fn mount_tuning_for(backend: &str) -> MountTuning {
    // A generous on-disk cache cap and eviction age, shared by every cloud tier.
    const CACHE_SIZE: Option<&str> = Some("10G");
    const CACHE_AGE: Option<&str> = Some("1h");
    match backend {
        // Already a real filesystem: no VFS cache, no attribute games. Send no opts.
        "local" => MountTuning::default(),
        // Slow network backends with no change-polling. WebDAV is how Internxt is mounted, and it
        // is the one that froze GNOME Files; Proton is likewise slow and touchy. Cache the most,
        // hold attributes and listings the longest, and skip modtime reads.
        "webdav" | "internxt" | "protondrive" => MountTuning {
            cache_mode: Some("full"),
            cache_max_size: CACHE_SIZE,
            cache_max_age: CACHE_AGE,
            attr_timeout: Some("10s"),
            dir_cache_time: Some("1h"),
            no_mod_time: true,
        },
        // Every other cloud (object stores, Drive-family, sftp/ftp): a cached, disk-like profile
        // with moderate attribute and listing caches.
        _ => MountTuning {
            cache_mode: Some("full"),
            cache_max_size: CACHE_SIZE,
            cache_max_age: CACHE_AGE,
            attr_timeout: Some("3s"),
            dir_cache_time: Some("5m"),
            no_mod_time: false,
        },
    }
}

/// The tuning for a backend id (rclone type, e.g. "drive", "protondrive", "pcloud").
///
/// The floor everywhere is raised low-level retries: a transient cloud error should be
/// retried, never allowed to surface as a partial/corrupt object. Touchy backends get a
/// tighter transfer/checker/tps envelope on top.
pub fn tuning_for(backend: &str) -> Tuning {
    // Raised everywhere: a transient blip gets retried instead of corrupting a transfer.
    const RETRIES: Option<i64> = Some(20);
    match backend {
        // Proton Drive: the documented many-small-files freeze. Keep concurrency and tps low
        // so a failed batch cannot leak the whole concurrency pool, and listing does not 429.
        "protondrive" => Tuning {
            transfers: Some(4),
            checkers: Some(3),
            tpslimit: Some(4.0),
            low_level_retries: RETRIES,
        },
        // Google Drive: per-user quota 429s under load; a tps cap is the known mitigation.
        "drive" => Tuning {
            transfers: Some(8),
            checkers: Some(8),
            tpslimit: Some(10.0),
            low_level_retries: RETRIES,
        },
        // Dropbox rate-limits namespace operations; stay moderate.
        "dropbox" => Tuning {
            transfers: Some(6),
            checkers: Some(6),
            tpslimit: Some(12.0),
            low_level_retries: RETRIES,
        },
        // pCloud stalls and keeps truncated revisions under heavy concurrency; be gentle.
        "pcloud" => Tuning {
            transfers: Some(4),
            checkers: Some(4),
            tpslimit: Some(10.0),
            low_level_retries: RETRIES,
        },
        // OneDrive throttles aggressively on bursts.
        "onedrive" => Tuning {
            transfers: Some(4),
            checkers: Some(4),
            tpslimit: Some(10.0),
            low_level_retries: RETRIES,
        },
        // Internxt: slow and resumes poorly; keep it to a trickle so a stall is one file, not
        // a wedged batch.
        "internxt" => Tuning {
            transfers: Some(2),
            checkers: Some(2),
            tpslimit: Some(6.0),
            low_level_retries: RETRIES,
        },
        // Local disk and well-behaved object stores (s3, b2, sftp, ...): just the retry floor.
        _ => Tuning {
            low_level_retries: RETRIES,
            ..Tuning::default()
        },
    }
}

/// Resolve the effective backend id for a configured remote, by asking the running daemon.
/// Follows one layer of `crypt` indirection (ESP's common "encrypted over Proton" pattern)
/// so the wrapping remote inherits the base backend's limits. `None` remote (local) and any
/// lookup failure resolve to "local".
pub fn backend_of(port: u16, remote: Option<&str>) -> String {
    let mut name = match remote {
        Some(n) => n.to_string(),
        None => return "local".into(),
    };
    // Bound the walk so a misconfigured crypt-of-crypt cycle cannot spin forever.
    for _ in 0..4 {
        let cfg = match rc_at(port, "config/get", serde_json::json!({ "name": name })) {
            Ok(v) => v,
            Err(_) => return "local".into(),
        };
        let backend = cfg
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("local")
            .to_string();
        if backend != "crypt" {
            return backend;
        }
        // crypt wraps another remote: its `remote` field is "<base>:path" (or a local path).
        match cfg.get("remote").and_then(|v| v.as_str()) {
            Some(inner) => match inner.split_once(':') {
                Some((base, _)) if !base.is_empty() => name = base.to_string(),
                // No remote prefix => crypt over a local path.
                _ => return "local".into(),
            },
            None => return "crypt".into(),
        }
    }
    "crypt".into()
}

/// Resolve a remote's backend id from `rclone config dump` (no daemon needed), following one
/// layer of `crypt`. For the restic backup path, which spawns its own rclone. `None` remote
/// and any failure resolve to "local".
pub fn backend_from_config(remote: Option<&str>) -> String {
    let mut name = match remote {
        Some(n) => n.to_string(),
        None => return "local".into(),
    };
    let out = match Command::new("rclone").args(["config", "dump"]).output() {
        Ok(o) if o.status.success() => o.stdout,
        _ => return "local".into(),
    };
    let map: BTreeMap<String, BTreeMap<String, serde_json::Value>> =
        match serde_json::from_slice(&out) {
            Ok(m) => m,
            Err(_) => return "local".into(),
        };
    for _ in 0..4 {
        let Some(cfg) = map.get(&name) else {
            return "local".into();
        };
        let backend = cfg
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("local")
            .to_string();
        if backend != "crypt" {
            return backend;
        }
        match cfg.get("remote").and_then(|v| v.as_str()) {
            Some(inner) => match inner.split_once(':') {
                Some((base, _)) if !base.is_empty() => name = base.to_string(),
                _ => return "local".into(),
            },
            None => return "crypt".into(),
        }
    }
    "crypt".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_floor_is_universal() {
        // Every backend, even unknown ones, gets retries raised off rclone's default.
        for b in ["local", "s3", "b2", "sftp", "somethingnew"] {
            assert_eq!(tuning_for(b).low_level_retries, Some(20), "{b}");
        }
    }

    #[test]
    fn proton_is_throttled() {
        let t = tuning_for("protondrive");
        assert_eq!(t.transfers, Some(4));
        assert_eq!(t.checkers, Some(3));
        assert_eq!(t.tpslimit, Some(4.0));
    }

    #[test]
    fn rc_config_uses_configinfo_field_names() {
        let cfg = tuning_for("protondrive").as_rc_config();
        assert_eq!(cfg["Transfers"], 4);
        assert_eq!(cfg["Checkers"], 3);
        assert_eq!(cfg["TPSLimit"], 4.0);
        assert_eq!(cfg["LowLevelRetries"], 20);
    }

    #[test]
    fn env_pairs_are_rclone_prefixed() {
        let env = tuning_for("drive").as_rclone_env();
        assert!(env.contains(&("RCLONE_TPSLIMIT".into(), "10".into())));
        assert!(env.contains(&("RCLONE_LOW_LEVEL_RETRIES".into(), "20".into())));
    }

    #[test]
    fn local_default_has_no_throttle_only_retries() {
        let t = tuning_for("local");
        assert!(t.transfers.is_none());
        assert!(t.tpslimit.is_none());
        assert_eq!(t.low_level_retries, Some(20));
        assert!(!t.is_default());
    }

    #[test]
    fn local_mount_sends_no_opts() {
        // A local disk is already a real filesystem: no VFS cache, no attribute games.
        let t = mount_tuning_for("local");
        assert!(t.is_default());
        let (vfs, mnt) = t.as_rc_params();
        assert_eq!(vfs, serde_json::json!({}));
        assert_eq!(mnt, serde_json::json!({}));
    }

    #[test]
    fn webdav_mount_is_fully_cached() {
        // WebDAV is how Internxt is mounted and the backend that froze the file manager: it gets
        // the fullest cache, the longest attribute/listing holds, and no modtime reads.
        let t = mount_tuning_for("webdav");
        let (vfs, mnt) = t.as_rc_params();
        assert_eq!(vfs["CacheMode"], "full");
        assert_eq!(vfs["DirCacheTime"], "1h");
        assert_eq!(vfs["NoModTime"], true);
        assert_eq!(mnt["AttrTimeout"], "10s");
        // Internxt's native type and Proton share the slow-backend profile.
        assert_eq!(mount_tuning_for("internxt"), t);
        assert_eq!(mount_tuning_for("protondrive"), t);
    }

    #[test]
    fn cloud_mount_is_cached_but_not_default() {
        // An object store / Drive-family backend still gets a cached, disk-like profile.
        let t = mount_tuning_for("s3");
        assert!(!t.is_default());
        let (vfs, mnt) = t.as_rc_params();
        assert_eq!(vfs["CacheMode"], "full");
        assert_eq!(vfs["DirCacheTime"], "5m");
        assert!(
            vfs.get("NoModTime").is_none(),
            "cloud default keeps modtime"
        );
        assert_eq!(mnt["AttrTimeout"], "3s");
    }

    #[test]
    fn mount_opts_use_verified_field_names() {
        // Keys must match rclone's option FieldNames, verified live against mount/mount.
        let (vfs, mnt) = mount_tuning_for("webdav").as_rc_params();
        for k in [
            "CacheMode",
            "CacheMaxSize",
            "CacheMaxAge",
            "DirCacheTime",
            "NoModTime",
        ] {
            assert!(vfs.get(k).is_some(), "missing vfsOpt key {k}");
        }
        assert!(mnt.get("AttrTimeout").is_some());
    }

    #[test]
    fn backend_of_local_without_daemon() {
        // A None remote never touches the daemon.
        assert_eq!(backend_of(1, None), "local");
    }
}
