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
    pub fn start_job(&self, job: &Job, group: &str) -> Result<i64> {
        let params = serde_json::json!({
            "srcFs": job.src.fs(),
            "dstFs": job.dst.fs(),
            "_async": true,
            "_group": group,
        });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{Endpoint, Job, SyncMode};

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
