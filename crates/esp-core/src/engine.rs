//! The operations engine: one `rclone rcd` daemon, driven by `rclone rc` subcommands.
//!
//! The foundation committed operations to rclone's remote-control API. We keep the core's
//! style (shell out to the rclone binary, parse JSON) rather than add an HTTP client: the
//! rclone binary is its own rc client. One daemon per app session will later serve mounts,
//! backups, and the Activity stream, not just syncs.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::jobs::Job;

/// A running `rclone rcd` the app talks to over loopback. Killed on drop.
pub struct Rcd {
    port: u16,
    child: Child,
}

impl Rcd {
    /// Start a daemon on a free loopback port and wait until it answers.
    pub fn start() -> Result<Rcd> {
        let port = free_loopback_port().context("finding a free loopback port for rcd")?;
        let addr = format!("127.0.0.1:{port}");
        let mut cmd = Command::new("rclone");
        cmd.args(["rcd", "--rc-addr", &addr, "--rc-no-auth"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Drop is our graceful shutdown, but it does not run if the app dies by signal or
        // crash. On Linux, ask the kernel to send the daemon SIGTERM when its parent (this
        // app) exits, so no orphaned rcd can linger.
        #[cfg(target_os = "linux")]
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn().context("failed to spawn `rclone rcd`")?;
        let rcd = Rcd { port, child };
        rcd.await_ready()
            .context("rcd did not become ready in time")?;
        Ok(rcd)
    }

    /// The loopback port the daemon listens on. A worker thread can talk to the daemon with
    /// just this (via `rc_at`), without holding the non-Send handle.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Call an rc command with a JSON object of params; return the parsed JSON reply.
    pub fn rc(&self, command: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        rc_at(self.port, command, params)
    }

    fn await_ready(&self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if self.rc("rc/noop", serde_json::json!({})).is_ok() {
                return Ok(());
            }
            if Instant::now() > deadline {
                bail!("timed out waiting for rcd on port {}", self.port);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Start `job` asynchronously, tagged with `group` so its stats can be queried apart
    /// from any other running job. Returns the rcd job id to poll.
    ///
    /// The destination backend decides throttle/retry limits: a touchy backend (Proton,
    /// Drive) is dialed down so a many-small-files run cannot hammer it into the documented
    /// hang. See `tuning`.
    pub fn start_job(&self, job: &Job, group: &str) -> Result<i64> {
        // Refuse a destructive Mirror whose local source is empty/unmounted, so rclone cannot
        // delete the whole destination to match it. Applies to every run path (manual + scheduled).
        crate::jobs::guard_local_mirror_source(job)?;
        let backend = crate::tuning::backend_of(self.port, job.dst.remote.as_deref());
        let config = crate::tuning::tuning_for(&backend).as_rc_config();
        let mut params = serde_json::json!({
            "srcFs": job.src.fs(),
            "dstFs": job.dst.fs(),
            "_async": true,
            "_group": group,
        });
        if let (Some(obj), serde_json::Value::Object(cfg)) = (params.as_object_mut(), &config) {
            if !cfg.is_empty() {
                obj.insert("_config".into(), config.clone());
            }
        }
        let reply = self.rc(job.mode.rc_command(), params)?;
        reply
            .get("jobid")
            .and_then(|v| v.as_i64())
            .context("rcd did not return a jobid")
    }

    /// Current status of a running or finished job.
    pub fn job_status(&self, jobid: i64) -> Result<JobStatus> {
        let reply = self.rc("job/status", serde_json::json!({ "jobid": jobid }))?;
        serde_json::from_value(reply).context("parsing job/status")
    }

    /// Transfer stats for a job's `group`. Missing or empty stats read as zeroed.
    pub fn job_stats(&self, group: &str) -> JobStats {
        self.rc("core/stats", serde_json::json!({ "group": group }))
            .ok()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }
}

impl Drop for Rcd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Lifecycle of a job, from `job/status`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct JobStatus {
    pub finished: bool,
    pub success: bool,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub duration: f64,
}

/// Transfer progress for a job, from `core/stats`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct JobStats {
    #[serde(default)]
    pub bytes: i64,
    #[serde(default, rename = "totalBytes")]
    pub total_bytes: i64,
    /// Bytes per second.
    #[serde(default)]
    pub speed: f64,
    /// Seconds remaining, if rclone can estimate it.
    #[serde(default)]
    pub eta: Option<f64>,
    #[serde(default)]
    pub transfers: i64,
    #[serde(default, rename = "totalTransfers")]
    pub total_transfers: i64,
}

impl JobStats {
    /// Fraction done in [0.0, 1.0], by bytes when known, else 0.0.
    pub fn fraction(&self) -> f64 {
        if self.total_bytes > 0 {
            (self.bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

fn free_loopback_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("binding a probe socket")?;
    Ok(listener.local_addr()?.port())
}

/// Make one rc call against a daemon on `port`. Free-standing so a worker thread can drive
/// the daemon (e.g. a blocking OAuth sign-in) with just the port number.
pub fn rc_at(port: u16, command: &str, params: serde_json::Value) -> Result<serde_json::Value> {
    let url = format!("http://127.0.0.1:{port}/");
    let out = Command::new("rclone")
        .args(["rc", "--url", &url, "--json", &params.to_string(), command])
        .output()
        .with_context(|| format!("failed to run `rclone rc {command}`"))?;
    if !out.status.success() {
        bail!(
            "rclone rc {command} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    if out.stdout.is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_slice(&out.stdout)
        .with_context(|| format!("parsing `rclone rc {command}` reply"))
}

/// A subdirectory, for the folder browser.
#[derive(Clone, Debug)]
pub struct DirEntry {
    /// The folder's own name (what to show).
    pub name: String,
    /// Its full path relative to the fs root (what to descend into).
    pub path: String,
}

/// List the subdirectories of `path` under `fs` (fs is a root: `"remote:"` or `"/"`),
/// via a daemon on `port`. Free-standing so a worker thread can browse without the handle.
/// Sorted by name; files are excluded.
pub fn list_dirs(port: u16, fs: &str, path: &str) -> Result<Vec<DirEntry>> {
    let reply = rc_at(
        port,
        "operations/list",
        serde_json::json!({ "fs": fs, "remote": path, "opt": { "dirsOnly": true } }),
    )?;
    let items = reply
        .get("list")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut dirs: Vec<DirEntry> = items
        .iter()
        .filter_map(|e| {
            Some(DirEntry {
                name: e.get("Name")?.as_str()?.to_string(),
                path: e.get("Path")?.as_str()?.to_string(),
            })
        })
        .collect();
    dirs.sort_by_key(|d| d.name.to_lowercase());
    Ok(dirs)
}

/// A live FUSE mount, from `mount/listmounts`.
#[derive(Clone, Debug)]
pub struct Mount {
    /// The fs that is mounted, e.g. "gdrive:".
    pub fs: String,
    /// Where it is mounted on this computer.
    pub mount_point: String,
}

/// Mount `fs` (e.g. "gdrive:") at `mount_point` through the daemon on `port`. The mount
/// lives as long as the daemon does.
pub fn mount(port: u16, fs: &str, mount_point: &str) -> Result<()> {
    rc_at(
        port,
        "mount/mount",
        serde_json::json!({ "fs": fs, "mountPoint": mount_point }),
    )?;
    Ok(())
}

/// Unmount whatever is at `mount_point`.
pub fn unmount(port: u16, mount_point: &str) -> Result<()> {
    rc_at(
        port,
        "mount/unmount",
        serde_json::json!({ "mountPoint": mount_point }),
    )?;
    Ok(())
}

/// The mounts the daemon currently serves.
pub fn list_mounts(port: u16) -> Result<Vec<Mount>> {
    let reply = rc_at(port, "mount/listmounts", serde_json::json!({}))?;
    let arr = reply
        .get("mountPoints")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(arr
        .iter()
        .filter_map(|m| {
            Some(Mount {
                fs: m.get("Fs")?.as_str()?.to_string(),
                mount_point: m.get("MountPoint")?.as_str()?.to_string(),
            })
        })
        .collect())
}

/// One file currently being transferred, from `core/stats`'s `transferring` array.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Transfer {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub bytes: i64,
    #[serde(default)]
    pub size: i64,
    /// Percent complete, 0..100.
    #[serde(default)]
    pub percentage: f64,
    /// Bytes per second.
    #[serde(default)]
    pub speed: f64,
    #[serde(default)]
    pub eta: Option<f64>,
}

/// A live snapshot of what the engine is moving right now, for the Activity view.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Activity {
    /// Total throughput, bytes per second.
    #[serde(default)]
    pub speed: f64,
    #[serde(default, rename = "transferring")]
    pub transfers: Vec<Transfer>,
}

/// The engine's current activity (what is transferring now), from the daemon on `port`.
/// Missing or empty stats read as idle.
pub fn activity(port: u16) -> Activity {
    rc_at(port, "core/stats", serde_json::json!({}))
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{Endpoint, Job, Schedule, SyncMode};

    #[test]
    fn list_dirs_browses_local_tree() {
        if crate::rclone_path().is_none() {
            eprintln!("rclone not installed; skipping");
            return;
        }
        let base = std::env::temp_dir().join(format!("esp-browse-{}", std::process::id()));
        std::fs::create_dir_all(base.join("Photos/2024")).unwrap();
        std::fs::create_dir_all(base.join("Documents")).unwrap();
        std::fs::write(base.join("loose.txt"), b"x").unwrap();

        let rcd = Rcd::start().expect("start rcd");
        let rel = base.to_string_lossy().trim_start_matches('/').to_string();

        let top = list_dirs(rcd.port(), "/", &rel).expect("list top");
        let names: Vec<_> = top.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"Photos"), "expected Photos, got {names:?}");
        assert!(names.contains(&"Documents"));
        assert!(!names.contains(&"loose.txt"), "files must be excluded");

        let photos = top.iter().find(|d| d.name == "Photos").unwrap();
        let sub = list_dirs(rcd.port(), "/", &photos.path).expect("descend");
        assert_eq!(sub.len(), 1);
        assert_eq!(sub[0].name, "2024");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn mount_lists_and_unmounts() {
        if crate::rclone_path().is_none() {
            eprintln!("rclone not installed; skipping");
            return;
        }
        if !std::path::Path::new("/dev/fuse").exists() {
            eprintln!("no /dev/fuse; skipping");
            return;
        }
        let base = std::env::temp_dir().join(format!("esp-mount-{}", std::process::id()));
        let data = base.join("data");
        let point = base.join("point");
        std::fs::create_dir_all(data.join("sub")).unwrap();
        std::fs::create_dir_all(&point).unwrap();
        std::fs::write(data.join("hello.txt"), b"hi").unwrap();

        let rcd = Rcd::start().expect("start rcd");
        // Mount a local path directly (local backend), so the test needs no remote or config.
        let fs = data.to_string_lossy().into_owned();
        let point_s = point.to_string_lossy().into_owned();
        mount(rcd.port(), &fs, &point_s).expect("mount");

        let mut listed = false;
        let mut visible = false;
        for _ in 0..40 {
            if !listed {
                listed = list_mounts(rcd.port())
                    .map(|ms| ms.iter().any(|m| m.mount_point == point_s))
                    .unwrap_or(false);
            }
            if point.join("hello.txt").exists() {
                visible = true;
            }
            if listed && visible {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(listed, "mount did not appear in listmounts");
        assert!(visible, "mounted content not visible through the mount");

        unmount(rcd.port(), &point_s).expect("unmount");
        // rcd Drop kills the daemon (and its mounts) if anything above panicked.
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn copy_roundtrip_local_to_local() {
        if crate::rclone_path().is_none() {
            eprintln!("rclone not installed; skipping");
            return;
        }
        let base = std::env::temp_dir().join(format!("esp-engine-{}", std::process::id()));
        let src = base.join("src");
        let dst = base.join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("hello.txt"), b"hello esp").unwrap();

        let rcd = Rcd::start().expect("start rcd");
        let job = Job {
            id: "t".into(),
            name: "t".into(),
            src: Endpoint::local(src.to_string_lossy().into_owned()),
            dst: Endpoint::local(dst.to_string_lossy().into_owned()),
            mode: SyncMode::Copy,
            schedule: Schedule::Manual,
            last_run: None,
        };
        let group = "esp/test-roundtrip";
        let jobid = rcd.start_job(&job, group).expect("start job");

        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let st = rcd.job_status(jobid).expect("job status");
            if st.finished {
                assert!(st.success, "job failed: {}", st.error);
                break;
            }
            assert!(Instant::now() < deadline, "job did not finish in time");
            std::thread::sleep(Duration::from_millis(100));
        }

        assert!(
            dst.join("hello.txt").exists(),
            "file did not copy to destination"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
