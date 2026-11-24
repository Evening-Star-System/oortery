//! Internxt, connected through its own CLI's WebDAV bridge.
//!
//! Internxt moved to browser-based login, so rclone's native `internxt` backend (email + password)
//! now returns 401 for everyone. The supported path is Internxt's own CLI: it signs in through the
//! browser (doing the zero-knowledge crypto itself) and serves the drive over a local WebDAV
//! server. Oortery drives that CLI and points a WebDAV remote at the bridge, so Internxt connects
//! with nearly the one-tap feel of any other backend (the only hands-on step is the browser login,
//! exactly like the OAuth clouds).
//!
//! The CLI is a Node program. Rather than bundle a whole Node runtime into every download for one
//! backend, we detect an installed `internxt` (or install it on demand under the config dir when
//! Node is present), so only Internxt users ever pull the ~180 MB of packages.

use crate::{create_remote, verify_remote, which};
use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};

/// The loopback port the Internxt WebDAV bridge listens on. Fixed so a remote created against it
/// keeps resolving across sessions.
pub const BRIDGE_PORT: u16 = 3005;
/// The WebDAV username Oortery configures on the bridge. The password is random per setup.
pub const BRIDGE_USER: &str = "oortery";
/// The CLI version Oortery installs on demand (pinned for reproducibility).
pub const CLI_VERSION: &str = "1.6.12";

/// Where Oortery reaches the bridge once it is up.
pub fn bridge_url() -> String {
    format!("http://127.0.0.1:{BRIDGE_PORT}")
}

/// Locate the Internxt CLI: an on-demand install Oortery manages under the config dir wins over a
/// system-wide one, so our pinned copy is preferred when present.
pub fn cli_path() -> Option<String> {
    if let Ok(dir) = crate::jobs::config_dir() {
        let managed = dir.join("internxt-cli/node_modules/.bin/internxt");
        if managed.is_file() {
            return Some(managed.to_string_lossy().into_owned());
        }
    }
    which("internxt")
}

/// Whether a JavaScript runtime is present (the CLI is a Node program; needed to install or run it).
pub fn node_available() -> bool {
    which("node").is_some()
}

/// What stands between the user and a working Internxt remote, so the UI can show the next step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    /// The CLI is not installed; `node` says whether Oortery can install it on demand here.
    NeedsCli { node: bool },
    /// CLI present, but no Internxt account is signed in.
    NeedsLogin,
    /// Signed in and ready to configure + start the bridge and create the remote.
    Ready,
}

/// Inspect the current state by asking the CLI. Cheap; safe to call when the helper opens.
pub fn readiness() -> Readiness {
    if cli_path().is_none() {
        return Readiness::NeedsCli {
            node: node_available(),
        };
    }
    if is_logged_in() {
        Readiness::Ready
    } else {
        Readiness::NeedsLogin
    }
}

/// Is an Internxt account signed into the CLI? Parses `whoami --json`: the command exits 0 even
/// when logged out, so the JSON `success` flag is the real signal, not the exit code.
pub fn is_logged_in() -> bool {
    let Some(cli) = cli_path() else {
        return false;
    };
    Command::new(&cli)
        .args(["whoami", "--json", "--non-interactive"])
        .output()
        .map(|o| parse_logged_in(&o.stdout))
        .unwrap_or(false)
}

/// Is the WebDAV bridge currently serving? Parses `webdav status --json`, whose `message` carries
/// "online"/"offline" (`success` is true either way).
pub fn bridge_online() -> bool {
    let Some(cli) = cli_path() else {
        return false;
    };
    Command::new(&cli)
        .args(["webdav", "status", "--json"])
        .output()
        .map(|o| parse_bridge_online(&o.stdout))
        .unwrap_or(false)
}

/// Start the browser sign-in. The CLI prints a login URL, opens the browser, and runs a local
/// callback server, blocking until the browser round-trip finishes, so run this off the UI thread.
///
/// We do NOT rely on the CLI opening the browser: its bundled opener can fail silently in a
/// sandboxed or AppImage environment (no inherited DISPLAY/xdg-open on PATH), which stranded the
/// user at a button that did nothing. Instead we run `--json`, read the `loginUrl` off stdout as
/// soon as the CLI emits it, and hand it to `on_url` so the caller opens it through the desktop's
/// own launcher. Succeeds only once an account is really signed in; on failure we surface the CLI's
/// stderr rather than a generic message.
pub fn login(mut on_url: impl FnMut(&str)) -> Result<()> {
    let cli = cli_path().context("the Internxt CLI is not installed")?;
    let mut child = Command::new(&cli)
        .args(["login", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("launching internxt login")?;

    // Pull the login URL off stdout the moment the CLI prints it, then open it ourselves. We keep
    // draining the pipe to EOF rather than stopping at the URL: closing our read end early would
    // EPIPE-kill the CLI's later stdout writes (the classic `node | head` crash), aborting the
    // sign-in. EOF arrives when the CLI closes stdout at exit, i.e. once the browser round-trip ends.
    if let Some(stdout) = child.stdout.take() {
        let mut opened = false;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if !opened {
                if let Some(url) = parse_login_url(&line) {
                    on_url(&url);
                    opened = true;
                }
            }
        }
    }

    let status = child.wait().context("waiting for internxt login")?;
    if is_logged_in() {
        return Ok(());
    }
    // Not signed in: surface whatever the CLI complained about, so a real failure is diagnosable
    // instead of a bare "did not complete".
    let mut err = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut err);
    }
    let err = err.trim();
    if err.is_empty() {
        bail!(
            "Internxt sign-in did not complete (CLI exited with {})",
            status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "no status".into())
        );
    }
    bail!("Internxt sign-in did not complete: {err}");
}

/// Pull the `loginUrl` out of a `--json` output line (`{ "loginUrl": "https://..." }`). None for any
/// other line, so we can scan stdout until the URL appears.
fn parse_login_url(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    v.get("loginUrl")?.as_str().map(str::to_string)
}

/// Configure the bridge for Oortery (plain HTTP on loopback, custom auth with `password`) and start
/// it. Requires a signed-in account.
pub fn start_bridge(password: &str) -> Result<()> {
    let cli = cli_path().context("the Internxt CLI is not installed")?;
    run_ok(
        Command::new(&cli).args([
            "webdav-config",
            "--http",
            "--port",
            &BRIDGE_PORT.to_string(),
            "--customAuth",
            "--username",
            BRIDGE_USER,
            "--password",
            password,
        ]),
        "configuring the Internxt bridge",
    )?;
    enable_bridge()?;
    Ok(())
}

/// Start the bridge with its already-stored config (host/port/custom-auth creds persist in the
/// CLI's own config across runs), without reconfiguring. Used to bring it back after a reboot.
///
/// `webdav enable` spawns the bridge under pm2 and returns before the HTTP server is actually
/// accepting connections, so we then wait for the port to come up. Without this, creating and
/// verifying the rclone remote raced the bridge and failed right at "verifying the Internxt remote".
pub fn enable_bridge() -> Result<()> {
    let cli = cli_path().context("the Internxt CLI is not installed")?;
    run_ok(
        Command::new(&cli).args(["webdav", "enable"]),
        "starting the Internxt bridge",
    )?;
    if !wait_for_bridge_port(std::time::Duration::from_secs(25)) {
        bail!("the Internxt bridge did not start listening on 127.0.0.1:{BRIDGE_PORT} in time");
    }
    Ok(())
}

/// Poll the bridge's loopback port until it accepts a TCP connection (i.e. the WebDAV server is
/// really listening), or `timeout` elapses. This is exactly the readiness rclone needs before we
/// point a remote at the bridge, and it is protocol-agnostic (no auth or HTTP round-trip required).
fn wait_for_bridge_port(timeout: std::time::Duration) -> bool {
    use std::net::{SocketAddr, TcpStream};
    let addr = SocketAddr::from(([127, 0, 0, 1], BRIDGE_PORT));
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500)).is_ok() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

/// Whether any configured remote points at the local Internxt bridge, i.e. the user actually uses
/// Internxt and the bridge is worth keeping up. Reads `rclone config dump`; false on any error.
pub fn has_bridge_remote() -> bool {
    let Ok(out) = Command::new("rclone").args(["config", "dump"]).output() else {
        return false;
    };
    config_has_bridge_remote(&out.stdout)
}

/// The bridge runs as a pm2-managed process that does NOT survive a reboot, so on launch Oortery
/// brings it back: if the user has a bridge remote and is still signed in but the bridge is down,
/// start it again (no reconfigure needed). Returns whether the bridge is up afterwards. A no-op,
/// `Ok(false)`, when the user does not use Internxt or needs to sign in again.
pub fn ensure_bridge() -> Result<bool> {
    if !has_bridge_remote() || !is_logged_in() {
        return Ok(false);
    }
    if bridge_online() {
        return Ok(true);
    }
    enable_bridge()?;
    Ok(bridge_online())
}

/// The whole Internxt connect, once signed in: pick a random bridge password, configure + start the
/// bridge, create a WebDAV remote named `name` pointed at it, and verify it lists. Off the UI thread.
pub fn connect(name: &str) -> Result<()> {
    if !is_logged_in() {
        bail!("sign in to Internxt first");
    }
    let password = random_password()?;
    start_bridge(&password)?;
    create_remote(
        name,
        "webdav",
        &[
            ("url".into(), bridge_url()),
            ("vendor".into(), "other".into()),
            ("user".into(), BRIDGE_USER.into()),
            ("pass".into(), password),
        ],
    )
    .context("creating the Internxt WebDAV remote")?;
    // The bridge accepts TCP as soon as it binds, but its WebDAV layer (database + drive metadata)
    // can take a few more seconds to actually serve a listing, so a single immediate verify raced it
    // and failed. Retry with a short backoff, surfacing the last real error if it never comes good.
    verify_remote_ready(name).context("verifying the Internxt remote")?;
    Ok(())
}

/// Verify the bridge remote, retrying for a short while because the WebDAV layer serves a beat after
/// the socket opens. Returns the last error if it never lists within the window.
fn verify_remote_ready(name: &str) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(24);
    let mut last = verify_remote(name);
    while last.is_err() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_secs(2));
        last = verify_remote(name);
    }
    last
}

/// Install the Internxt CLI on demand, into a prefix under the config dir (no root needed), so only
/// Internxt users pull the Node packages. Needs Node + npm on the machine.
pub fn install_cli() -> Result<()> {
    if which("npm").is_none() {
        bail!("Node and npm are required to install Internxt support");
    }
    let dir = crate::jobs::config_dir()?.join("internxt-cli");
    std::fs::create_dir_all(&dir)?;
    // We execute this binary (and re-execute it every launch via ensure_bridge), so lock the
    // install dir to the user (0700): another local user must not be able to drop a trojan binary
    // where we look. See the security delta review.
    restrict_dir(&dir);
    let prefix = dir.to_str().context("config path is not valid UTF-8")?;
    run_ok(
        Command::new("npm").args([
            "install",
            "--prefix",
            prefix,
            "--no-audit",
            "--no-fund",
            // Block npm lifecycle scripts: a compromised transitive dep cannot run arbitrary code
            // as the user at install time (which would then persist via the launch auto-restart).
            "--ignore-scripts",
            "--loglevel=error",
            &format!("@internxt/cli@{CLI_VERSION}"),
        ]),
        "installing Internxt support",
    )?;
    // The CLI's WebDAV bridge uses better-sqlite3, a native module whose own `install` script
    // (prebuild-install) fetches its compiled .node binary. --ignore-scripts above (which blocks every
    // dependency's lifecycle scripts, so a compromised transitive dep cannot run code at install) also
    // skips that fetch, so the bridge server crashed at boot with "Could not locate the bindings file"
    // and never listened -> connect failed at "verifying the Internxt remote". We therefore rebuild
    // JUST that one native dep (it is the only package in the tree with an install script; sharp, the
    // only other native dep, ships prebuilt platform packages and needs none). Every other package's
    // scripts stay blocked, so this is a narrow, named relaxation, not a blanket re-enable.
    run_ok(
        Command::new("npm").args([
            "rebuild",
            "--prefix",
            prefix,
            "--no-audit",
            "--no-fund",
            "--loglevel=error",
            "better-sqlite3",
        ]),
        "building Internxt's database helper",
    )?;
    if cli_path().is_none() {
        bail!("the Internxt CLI did not appear after install");
    }
    Ok(())
}

/// Lock a directory to the owner (0700) on Unix, best-effort. Used for the managed CLI install dir,
/// whose contents we execute. A no-op on platforms without Unix permissions.
#[cfg(unix)]
fn restrict_dir(dir: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}
#[cfg(not(unix))]
fn restrict_dir(_dir: &std::path::Path) {}

/// A random bridge password, so the local WebDAV server is never left on a guessable credential.
fn random_password() -> Result<String> {
    let mut bytes = [0u8; 18];
    getrandom::getrandom(&mut bytes).map_err(|e| anyhow::anyhow!("generating a password: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn run_ok(cmd: &mut Command, ctx: &str) -> Result<()> {
    let out = cmd.output().with_context(|| ctx.to_string())?;
    if !out.status.success() {
        bail!("{ctx}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// Does an `rclone config dump` JSON hold a WebDAV remote pointed at our local bridge URL?
fn config_has_bridge_remote(dump: &[u8]) -> bool {
    let url = bridge_url();
    serde_json::from_slice::<serde_json::Value>(dump)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .map(|remotes| {
            remotes.values().any(|cfg| {
                cfg.get("type").and_then(|t| t.as_str()) == Some("webdav")
                    && cfg.get("url").and_then(|u| u.as_str()) == Some(url.as_str())
            })
        })
        .unwrap_or(false)
}

/// `whoami --json` reports `{"success": bool, ...}`; true means a user is signed in.
fn parse_logged_in(stdout: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(stdout)
        .ok()
        .and_then(|v| v.get("success").and_then(|s| s.as_bool()))
        .unwrap_or(false)
}

/// `webdav status --json` reports the state in `message` ("...online"/"...offline").
fn parse_bridge_online(stdout: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(stdout)
        .ok()
        .and_then(|v| {
            v.get("message")
                .and_then(|m| m.as_str())
                .map(|m| m.to_lowercase())
        })
        .map(|m| m.contains("online") && !m.contains("offline"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logged_in_parses_the_json_success_flag_not_the_exit_code() {
        // Real shapes captured from the CLI (whoami exits 0 even when logged out).
        assert!(!parse_logged_in(
            br#"{"success": false, "message": "You are not logged in."}"#
        ));
        assert!(parse_logged_in(
            br#"{"success": true, "message": "me@example.com"}"#
        ));
        assert!(!parse_logged_in(b"not json"));
    }

    #[test]
    fn login_url_is_pulled_from_the_json_line() {
        // The real shape of `internxt login --json`'s first stdout line.
        assert_eq!(
            parse_login_url(r#"{ "loginUrl": "https://drive.internxt.com/login?x=1" }"#).as_deref(),
            Some("https://drive.internxt.com/login?x=1")
        );
        // Human log lines and later JSON without a loginUrl are skipped.
        assert_eq!(parse_login_url("Opening browser for login..."), None);
        assert_eq!(parse_login_url(r#"{"success": true}"#), None);
    }

    #[test]
    fn bridge_online_reads_the_status_message() {
        assert!(parse_bridge_online(
            br#"{"success": true, "message": "Internxt WebDAV server status: online", "action": "status"}"#
        ));
        // "offline" must not be read as online (it contains "line", never "online").
        assert!(!parse_bridge_online(
            br#"{"success": true, "message": "Internxt WebDAV server status: offline", "action": "status"}"#
        ));
        assert!(!parse_bridge_online(b"{}"));
    }

    #[test]
    fn bridge_remote_is_detected_by_type_and_url() {
        // A WebDAV remote at the bridge URL counts; other webdav remotes and other backends do not.
        let yes = br#"{"inxt":{"type":"webdav","url":"http://127.0.0.1:3005"},"s3":{"type":"s3"}}"#;
        assert!(config_has_bridge_remote(yes));
        let other_webdav = br#"{"nc":{"type":"webdav","url":"https://cloud.example.com/dav"}}"#;
        assert!(!config_has_bridge_remote(other_webdav));
        assert!(!config_has_bridge_remote(br#"{"gd":{"type":"drive"}}"#));
        assert!(!config_has_bridge_remote(b"not json"));
    }

    #[test]
    fn readiness_without_cli_reports_needs_cli() {
        // On a box with no system `internxt` and no managed copy under a fresh config dir, the
        // first gate is "install the CLI". (If a system internxt exists this asserts nothing, so
        // guard on cli_path to keep the test honest everywhere.)
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("oortery-ixt-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_dir_all(&dir);
        if cli_path().is_none() {
            assert!(matches!(readiness(), Readiness::NeedsCli { .. }));
        }
        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
