//! The P2P engine: a Syncthing daemon per app session, managed like `Rcd` manages rclone.
//! This is the third engine behind the same engine-neutral surface, for account-free, offline-
//! first device-to-device sync (no cloud in the middle).
//!
//! It is driven via `syncthing cli` rather than an HTTP client, keeping the "the engine binary
//! is its own client" pattern and adding no dependency. The GUI API key is passed by environment
//! (STGUIAPIKEY), never on a command line, so it cannot leak into a process listing.

use anyhow::{bail, Context, Result};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::jobs::config_dir;

/// Locate the syncthing binary. None if it is not installed/bundled.
pub fn syncthing_path() -> Option<String> {
    crate::which("syncthing")
}

/// Syncthing's home (config + keys + its own database): `<config dir>/syncthing`.
fn p2p_home() -> Result<PathBuf> {
    Ok(config_dir()?.join("syncthing"))
}

/// Ensure a syncthing home exists, generating the device key + config on first use and turning
/// off Syncthing's phone-home defaults before the daemon ever runs.
fn ensure_home() -> Result<PathBuf> {
    let home = p2p_home()?;
    let config = home.join("config.xml");
    if !config.exists() {
        std::fs::create_dir_all(&home)?;
        let out = Command::new("syncthing")
            .arg("generate")
            .arg("--home")
            .arg(&home)
            .output()
            .context("running `syncthing generate`")?;
        if !out.status.success() {
            bail!(
                "syncthing generate failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        harden_config(&config)?;
    }
    Ok(home)
}

/// Set the inner text of the first `<tag>...</tag>` in a freshly generated config (each option
/// appears once with a simple scalar value). Leaves the document untouched if the tag is absent.
fn set_xml_tag(xml: &str, tag: &str, value: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    match (xml.find(&open), xml.find(&close)) {
        (Some(s), Some(e)) if s + open.len() <= e => {
            let mut out = String::with_capacity(xml.len());
            out.push_str(&xml[..s + open.len()]);
            out.push_str(value);
            out.push_str(&xml[e..]);
            out
        }
        _ => xml.to_string(),
    }
}

/// Turn off Syncthing's phone-home defaults on a freshly generated config, before the daemon ever
/// runs. Oortery is offline-first and account-free, so no global discovery server, no relay
/// servers, no crash or usage reporting, and no auto-upgrade check. LOCAL (LAN) discovery stays on,
/// since that is how device-to-device pairing finds peers with no third party in the middle. Global
/// discovery and relays will become an explicit, disclosed opt-in when the P2P UI is built.
fn harden_config(config: &Path) -> Result<()> {
    let xml = std::fs::read_to_string(config).context("reading syncthing config to harden")?;
    let xml = set_xml_tag(&xml, "globalAnnounceEnabled", "false");
    let xml = set_xml_tag(&xml, "relaysEnabled", "false");
    let xml = set_xml_tag(&xml, "startBrowser", "false");
    let xml = set_xml_tag(&xml, "autoUpgradeIntervalH", "0");
    let xml = set_xml_tag(&xml, "crashReportingEnabled", "false");
    let xml = set_xml_tag(&xml, "urAccepted", "-1");
    std::fs::write(config, xml).context("writing hardened syncthing config")?;
    Ok(())
}

/// Read the GUI API key from syncthing's config. Never logged or returned to the UI.
fn read_api_key(home: &Path) -> Result<String> {
    let cfg =
        std::fs::read_to_string(home.join("config.xml")).context("reading syncthing config")?;
    cfg.split_once("<apikey>")
        .and_then(|(_, rest)| rest.split_once("</apikey>"))
        .map(|(k, _)| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .context("no API key found in syncthing config")
}

/// A running syncthing daemon the app talks to over loopback. Killed on drop, and reaped by the
/// kernel (PR_SET_PDEATHSIG) if the app dies by signal.
pub struct Syncthingd {
    port: u16,
    api_key: String,
    home: PathBuf,
    child: Child,
}

impl Syncthingd {
    /// Start the daemon on a free loopback port and wait until its API answers.
    pub fn start() -> Result<Syncthingd> {
        let home = ensure_home()?;
        let api_key = read_api_key(&home)?;
        let port = free_loopback_port().context("finding a free loopback port for syncthing")?;
        let addr = format!("127.0.0.1:{port}");

        let mut cmd = Command::new("syncthing");
        cmd.arg("serve")
            .arg("--no-browser")
            .arg("--gui-address")
            .arg(&addr)
            .arg("--home")
            .arg(&home)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
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
        let child = cmd.spawn().context("spawning `syncthing serve`")?;
        let daemon = Syncthingd {
            port,
            api_key,
            home,
            child,
        };
        daemon
            .await_ready()
            .context("syncthing did not become ready in time")?;
        Ok(daemon)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Run a `syncthing cli` subcommand against this daemon. The API key and address go via env
    /// (never argv), so they do not appear in a process listing.
    fn cli(&self, args: &[&str]) -> Result<String> {
        let out = Command::new("syncthing")
            .arg("cli")
            .args(args)
            .env("STGUIADDRESS", format!("127.0.0.1:{}", self.port))
            .env("STGUIAPIKEY", &self.api_key)
            .env("STHOMEDIR", &self.home)
            .output()
            .with_context(|| format!("running `syncthing cli {}`", args.join(" ")))?;
        if !out.status.success() {
            bail!(
                "syncthing cli {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn await_ready(&self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            if self.cli(&["show", "system"]).is_ok() {
                return Ok(());
            }
            if Instant::now() > deadline {
                bail!("timed out waiting for syncthing on port {}", self.port);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// This device's Syncthing ID: the token the user shares to pair another device.
    pub fn device_id(&self) -> Result<String> {
        let out = self.cli(&["show", "system"])?;
        let v: serde_json::Value =
            serde_json::from_str(&out).context("parsing `syncthing cli show system`")?;
        v.get("myID")
            .and_then(|x| x.as_str())
            .map(String::from)
            .context("no myID in syncthing system status")
    }
}

impl Drop for Syncthingd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_loopback_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("binding a probe socket")?;
    Ok(listener.local_addr()?.port())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_xml_tag_replaces_the_value() {
        let xml = "<opts><globalAnnounceEnabled>true</globalAnnounceEnabled></opts>";
        assert_eq!(
            set_xml_tag(xml, "globalAnnounceEnabled", "false"),
            "<opts><globalAnnounceEnabled>false</globalAnnounceEnabled></opts>"
        );
        // An absent tag leaves the document untouched.
        assert_eq!(set_xml_tag(xml, "missing", "x"), xml);
    }

    #[test]
    fn generated_config_disables_phone_home() {
        if syncthing_path().is_none() {
            eprintln!("syncthing not installed; skipping");
            return;
        }
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("oortery-p2p-harden-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_dir_all(&dir);

        let home = ensure_home().expect("ensure home");
        let cfg = std::fs::read_to_string(home.join("config.xml")).unwrap();
        // The phone-home defaults are all off before the daemon ever runs.
        assert!(cfg.contains("<globalAnnounceEnabled>false</globalAnnounceEnabled>"));
        assert!(cfg.contains("<relaysEnabled>false</relaysEnabled>"));
        assert!(cfg.contains("<crashReportingEnabled>false</crashReportingEnabled>"));
        assert!(cfg.contains("<autoUpgradeIntervalH>0</autoUpgradeIntervalH>"));
        assert!(cfg.contains("<urAccepted>-1</urAccepted>"));
        // LAN discovery stays ON: that is how pairing finds peers with no third party involved.
        assert!(cfg.contains("<localAnnounceEnabled>true</localAnnounceEnabled>"));

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn syncthingd_starts_and_reports_a_device_id() {
        if syncthing_path().is_none() {
            eprintln!("syncthing not installed; skipping");
            return;
        }
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("oortery-p2p-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_dir_all(&dir);

        let daemon = Syncthingd::start().expect("start syncthing");
        let id = daemon.device_id().expect("device id");
        // Syncthing device IDs are long, uppercase, and dash-grouped.
        assert!(
            id.len() > 40 && id.contains('-'),
            "unexpected device id: {id}"
        );
        drop(daemon);

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
