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
        // Where folders a peer shares get auto-accepted, so an incoming share lands somewhere clear
        // (next to the mounts) instead of a surprise path. Best-effort.
        let _ = daemon.set_default_folder_path();
        Ok(daemon)
    }

    /// Point auto-accepted (incoming) shares at `$HOME/Oortery/Peers`, so a folder a peer shares has
    /// a clear, predictable home. Best-effort: a failure here never stops the daemon starting.
    fn set_default_folder_path(&self) -> Result<()> {
        if let Ok(home) = std::env::var("HOME") {
            let path = format!("{home}/Oortery/Peers");
            self.cli(&["config", "defaults", "folder", "path", "set", &path])?;
        }
        Ok(())
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
            // Wait for the device ID to be readable, not merely for the API to answer: right after
            // startup `show system` can return before `myID` is populated, and a caller that reads
            // the device ID that instant would get an empty value.
            if self.device_id().map(|id| !id.is_empty()).unwrap_or(false) {
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

    /// Pair a peer device by its Syncthing ID, under a friendly `name`. The peer is set to
    /// auto-accept folders, so after pairing a folder the peer shares simply appears here: the
    /// user never has to approve each folder by hand (the stupid-simple half of pairing).
    pub fn add_peer(&self, device_id: &str, name: &str) -> Result<()> {
        let id = device_id.trim();
        if !is_device_id(id) {
            bail!("that does not look like a device code");
        }
        self.cli(&[
            "config",
            "devices",
            "add",
            "--device-id",
            id,
            "--name",
            name,
            "--auto-accept-folders",
        ])?;
        Ok(())
    }

    /// Share a local folder with a paired peer under a stable `folder_id`, creating the folder
    /// entry on first use. Both devices sharing the same `folder_id` is what makes it sync.
    pub fn share_folder(
        &self,
        folder_id: &str,
        label: &str,
        path: &str,
        peer_id: &str,
    ) -> Result<()> {
        // Create the folder entry if it is not there yet; an already-present folder is fine.
        let _ = self.cli(&[
            "config", "folders", "add", "--id", folder_id, "--label", label, "--path", path,
        ]);
        self.cli(&[
            "config",
            "folders",
            folder_id,
            "devices",
            "add",
            "--device-id",
            peer_id,
        ])?;
        Ok(())
    }

    /// Devices that have asked to connect to us (they added our code; we have not added theirs yet).
    /// Surfacing these lets a one-way scan work: the peer scans our QR, then appears here to accept.
    pub fn pending_devices(&self) -> Result<Vec<PendingDevice>> {
        let out = self.cli(&["show", "pending", "devices"])?;
        let v: serde_json::Value = serde_json::from_str(&out).context("parsing pending devices")?;
        let mut list = Vec::new();
        if let Some(obj) = v.as_object() {
            for (id, info) in obj {
                let name = info
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                list.push(PendingDevice {
                    id: id.clone(),
                    name,
                });
            }
        }
        Ok(list)
    }

    /// Cumulative bytes moved to and from peers (from `show connections`'s totals). The Activity
    /// poll diffs this between samples to show a live P2P throughput, so device-to-device sync is
    /// visible alongside cloud transfers.
    pub fn transfer_bytes_total(&self) -> Result<i64> {
        let out = self.cli(&["show", "connections"])?;
        let v: serde_json::Value =
            serde_json::from_str(&out).context("parsing connections totals")?;
        let total = v.get("total");
        let field = |k: &str| {
            total
                .and_then(|t| t.get(k))
                .and_then(|n| n.as_i64())
                .unwrap_or(0)
        };
        Ok(field("inBytesTotal") + field("outBytesTotal"))
    }

    /// The device IDs we are currently connected to (from `show connections`), so the UI can show a
    /// peer as live rather than merely configured.
    pub fn connected_ids(&self) -> Result<Vec<String>> {
        let out = self.cli(&["show", "connections"])?;
        let v: serde_json::Value = serde_json::from_str(&out).context("parsing connections")?;
        let mut list = Vec::new();
        if let Some(conns) = v.get("connections").and_then(|c| c.as_object()) {
            for (id, info) in conns {
                if info.get("connected").and_then(|c| c.as_bool()) == Some(true) {
                    list.push(id.clone());
                }
            }
        }
        Ok(list)
    }

    /// The peer device IDs configured here (everyone except ourselves).
    pub fn peer_ids(&self) -> Result<Vec<String>> {
        let me = self.device_id().unwrap_or_default();
        let out = self.cli(&["config", "devices", "list"])?;
        Ok(out
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty() && *l != me)
            .collect())
    }
}

/// A device that has asked to connect (it added our code). Accept it by adding it as a peer.
pub struct PendingDevice {
    pub id: String,
    pub name: String,
}

/// True if `s` looks like a Syncthing device ID: uppercase alphanumeric in dash-grouped runs,
/// long enough to be real. A light guard before handing a value to the engine, not full validation.
fn is_device_id(s: &str) -> bool {
    let stripped: String = s.chars().filter(|c| *c != '-').collect();
    stripped.len() >= 50
        && stripped
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// Count the peer devices paired in the local Syncthing config, without starting the daemon.
/// Reads `config.xml` and counts distinct valid device IDs, minus this device itself. Returns 0
/// when P2P has never been set up (no config yet).
///
/// Robust to the file's formatting: each peer appears as a top-level `<device>` and again as a
/// per-folder device reference with the SAME id, so counting distinct ids collapses the
/// duplicates; the template device in `<defaults>` carries an empty id that `is_device_id`
/// rejects. A config always holds this machine's own device, so paired peers = distinct ids - 1.
pub fn paired_device_count() -> usize {
    let Ok(home) = p2p_home() else {
        return 0;
    };
    let Ok(xml) = std::fs::read_to_string(home.join("config.xml")) else {
        return 0;
    };
    let needle = "device id=\"";
    let mut ids = std::collections::BTreeSet::new();
    let mut rest = xml.as_str();
    while let Some(i) = rest.find(needle) {
        rest = &rest[i + needle.len()..];
        let Some(end) = rest.find('"') else { break };
        let id = &rest[..end];
        if is_device_id(id) {
            ids.insert(id.to_string());
        }
        rest = &rest[end + 1..];
    }
    // The set always includes this machine's own device; the rest are paired peers.
    ids.len().saturating_sub(1)
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
    fn is_device_id_guards_shape() {
        assert!(is_device_id(
            "EF7XNME-RXQ45CH-HYRKLCP-QWLG4AZ-NFBZG2U-2CWDMTU-6CHU2XC-26HNIAF"
        ));
        assert!(!is_device_id("nope"));
        assert!(!is_device_id("-rm -rf /")); // a flag-shaped value is not a device id
    }

    #[test]
    fn paired_device_count_excludes_self_and_counts_distinct_peers() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("oortery-p2p-count-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_dir_all(&dir);

        // No config yet: zero peers, not a panic.
        assert_eq!(paired_device_count(), 0);

        const SELF: &str = "ZO764NX-62BHV25-7ILX72U-3YLGNTX-ZM26HQI-5LJKX52-7XRMDAI-NRVDNQ3";
        const PEER: &str = "EF7XNME-RXQ45CH-HYRKLCP-QWLG4AZ-NFBZG2U-2CWDMTU-6CHU2XC-26HNIAF";
        // A config shaped like syncthing's: self + peer as top-level devices, the peer repeated as
        // a per-folder device reference, and the empty-id template from <defaults>.
        let xml = format!(
            "<configuration version=\"52\">\n\
             <device id=\"{SELF}\" name=\"me\"></device>\n\
             <device id=\"{PEER}\" name=\"friend\"></device>\n\
             <folder id=\"x\">\n\
               <device id=\"{SELF}\"></device>\n\
               <device id=\"{PEER}\"></device>\n\
             </folder>\n\
             <defaults><device id=\"\"></device></defaults>\n\
             </configuration>"
        );
        let home = p2p_home().unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("config.xml"), xml).unwrap();

        // Distinct valid ids = {self, peer}; minus self = one paired peer. The folder refs
        // (duplicate ids) and the empty template id do not inflate the count.
        assert_eq!(paired_device_count(), 1);

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pairs_a_peer_and_shares_a_folder() {
        if syncthing_path().is_none() {
            eprintln!("syncthing not installed; skipping");
            return;
        }
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("oortery-p2p-pair-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_dir_all(&dir);

        let daemon = Syncthingd::start().expect("start syncthing");
        // A real, checksum-valid device id (device ids are public, not secrets).
        const PEER: &str = "EF7XNME-RXQ45CH-HYRKLCP-QWLG4AZ-NFBZG2U-2CWDMTU-6CHU2XC-26HNIAF";
        daemon.add_peer(PEER, "A Friend").expect("add peer");

        let share = dir.join("shared");
        std::fs::create_dir_all(&share).unwrap();
        daemon
            .share_folder("oort-test", "Test Share", share.to_str().unwrap(), PEER)
            .expect("share folder");

        // The peer is configured here now.
        let peers = daemon.peer_ids().expect("peers");
        assert!(peers.iter().any(|p| p == PEER), "peer missing: {peers:?}");
        // A malformed id is refused before it ever reaches the engine.
        assert!(daemon.add_peer("not-a-real-id", "x").is_err());
        drop(daemon);

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
