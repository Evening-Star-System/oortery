//! One-tap connect: drive rclone's interactive config state machine over rcd so signing in
//! to an OAuth cloud (Drive, Dropbox, OneDrive, ...) is a single tap.
//!
//! rclone models `config create` as a state machine: each `config/create` call returns a
//! `State` and an `Option` (a question), and you answer by calling `config/create` again
//! with `opt.{continue,state,result}`. We auto-answer every question with its default and,
//! at the "use your browser to sign in?" step, answer yes so rclone opens the browser and
//! captures the token. That OAuth step blocks until the user finishes, so the whole flow
//! runs on a worker thread, never the UI thread.

use crate::engine::rc_at;
use anyhow::{Context, Result};
use serde_json::json;

/// A question the config flow is waiting on.
pub struct Pending {
    pub state: String,
    pub option_name: String,
    pub default: String,
    /// True at the browser sign-in step (answering it opens the browser and blocks).
    pub is_oauth: bool,
}

/// Begin creating `name` as `backend`. Returns the first question, or `None` if it finished
/// with nothing to ask.
pub fn begin(port: u16, name: &str, backend: &str) -> Result<Option<Pending>> {
    begin_with(port, name, backend, json!({}))
}

/// Begin config/create with initial `parameters` already filled (e.g. email+pass for a
/// credential login), so the backend's Config can run its login without re-asking for them.
fn begin_with(
    port: u16,
    name: &str,
    backend: &str,
    parameters: serde_json::Value,
) -> Result<Option<Pending>> {
    let params = json!({
        "name": name,
        "type": backend,
        "parameters": parameters,
        "opt": { "nonInteractive": true },
    });
    let reply = rc_at(port, "config/create", params).context("starting config/create")?;
    Ok(parse(&reply))
}

/// Answer the current step with `result` and advance. Returns the next question, or `None`
/// when the remote is fully created.
pub fn advance(
    port: u16,
    name: &str,
    backend: &str,
    state: &str,
    result: &str,
) -> Result<Option<Pending>> {
    let params = json!({
        "name": name,
        "type": backend,
        "parameters": {},
        "opt": { "nonInteractive": true, "continue": true, "state": state, "result": result },
    });
    let reply = rc_at(port, "config/create", params).context("continuing config/create")?;
    Ok(parse(&reply))
}

/// Run the whole flow for an OAuth backend: auto-answer every question with its default and
/// open the browser at the sign-in step. Blocks until the remote is created (the user has
/// finished signing in) or the flow errors. Run this on a worker thread.
pub fn oauth_connect(port: u16, name: &str, backend: &str) -> Result<()> {
    let mut pending = begin(port, name, backend)?;
    let mut steps = 0;
    while let Some(p) = pending {
        steps += 1;
        if steps > 40 {
            anyhow::bail!("sign-in did not complete");
        }
        let answer = if p.is_oauth { "true" } else { &p.default };
        pending = advance(port, name, backend, &p.state, answer)?;
    }
    Ok(())
}

/// Create a credential (non-OAuth) backend by driving its config state machine to completion.
///
/// A one-shot `rclone config create --non-interactive` stops at the first continuation the
/// backend asks for. Some backends only fetch and store their encryption material in a later
/// state. Internxt's `Config` pre-logs-in at the start state, returns `ConfigGoto("login")`, then
/// does the real `DoLogin` in the "login" state and writes the mnemonic there. Stopping early
/// leaves a remote with no mnemonic that fails every operation ("mnemonic is required, please run
/// rclone config reconnect"). We instead seed the credentials and loop, answering each question
/// from `answers` (matched by option name, e.g. a 2FA code) or its default, and advancing through
/// the question-less continuation states, until the flow ends. That runs the login state, so the
/// mnemonic is saved and the remote actually works. This performs a network login, so run it on a
/// worker thread. On success the remote is created; the caller should still `verify_remote` it.
pub fn credential_connect(
    port: u16,
    name: &str,
    backend: &str,
    answers: &[(String, String)],
) -> Result<()> {
    let result = run_credential_flow(port, name, backend, answers);
    if result.is_err() {
        // rclone's state machine writes config incrementally, so a mid-flow failure (a rejected
        // login, a missing 2FA code) can leave a half-made remote with no usable credentials.
        // Never keep a remote that did not finish connecting.
        let _ = crate::delete_remote(name);
    }
    result
}

fn run_credential_flow(
    port: u16,
    name: &str,
    backend: &str,
    answers: &[(String, String)],
) -> Result<()> {
    // Seed the credentials up front so the backend's first state can log in. CAUTION: unlike the CLI
    // `rclone config create`, this rc path does NOT obscure IsPassword values before the backend's
    // Config login reads them. A backend that reveals its stored pass (e.g. Internxt) will then see
    // garbage and report "Wrong login credentials". If a backend is ever added to needs_login_flow,
    // obscure the password (rclone obscure / core obscure) before inserting it here. This is why
    // Internxt was moved off this path to create_remote. [reviewer: do not "simplify" back to plaintext]
    let mut parameters = serde_json::Map::new();
    for (k, v) in answers {
        if !v.is_empty() {
            parameters.insert(k.clone(), serde_json::Value::String(v.clone()));
        }
    }
    let mut pending = begin_with(port, name, backend, serde_json::Value::Object(parameters))?;
    let mut steps = 0;
    while let Some(p) = pending {
        steps += 1;
        if steps > 40 {
            anyhow::bail!("configuration did not complete");
        }
        if p.is_oauth {
            anyhow::bail!("this provider needs browser sign-in, not a password");
        }
        // A real question (e.g. "2fa") is answered from the form; a question-less continuation
        // (ConfigGoto, like Internxt's jump to "login") carries an empty default and just advances.
        let answer = answers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(&p.option_name))
            .map(|(_, v)| v.as_str())
            .unwrap_or(&p.default);
        pending = advance(port, name, backend, &p.state, answer)?;
    }
    Ok(())
}

/// Backends that MUST be created through the multi-state rc `config/create` flow
/// (`credential_connect`) rather than a one-shot `rclone config create`. Currently NONE.
///
/// Internxt was listed here on two beliefs that both proved false (verified 2026-11-24 against a real
/// account). First, that a one-shot create saved a remote with no mnemonic; in fact `rclone config
/// create <n> internxt email <e> pass <p> --non-interactive` logs in, writes the mnemonic, and lists
/// files. Second, and the real bug: the rc flow seeded the password as PLAINTEXT (see
/// `run_credential_flow`), but Internxt's login reveals/de-obscures the stored pass, so plaintext
/// became garbage and the backend returned "Wrong login credentials (status 401)" on CORRECT
/// credentials. That bogus 401 is what sent us chasing a non-existent Internxt outage and building a
/// whole CLI/WebDAV bridge. Internxt now goes through `create_remote` (the CLI path, which obscures
/// the password the way the backend expects) like any other form backend. Keep this returning false
/// unless a backend genuinely needs the staged flow AND its password is obscured before seeding.
pub fn needs_login_flow(_backend: &str) -> bool {
    false
}

/// Cancel an in-progress browser sign-in (closes rclone's local auth server).
pub fn oauth_stop(port: u16) {
    let _ = rc_at(port, "config/oauthstop", json!({}));
}

fn parse(reply: &serde_json::Value) -> Option<Pending> {
    let state = reply.get("State").and_then(|v| v.as_str()).unwrap_or("");
    if state.is_empty() {
        return None;
    }
    let opt = reply.get("Option");
    let option_name = opt
        .and_then(|o| o.get("Name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let default = opt
        .and_then(|o| o.get("ValueStr"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let is_oauth = option_name == "config_is_local" || state.starts_with("*oauth");
    Some(Pending {
        state: state.to_string(),
        option_name,
        default,
        is_oauth,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Rcd;

    /// Drive a real OAuth backend (drive) through its deterministic questions and confirm we
    /// reach the browser sign-in step. We stop there: answering it would open a browser and
    /// block, which only completes on a real machine with the user's account. This proves the
    /// protocol (body shape, continuation, default-answering); the browser leg is verified by
    /// running the app on a real desktop.
    #[test]
    fn oauth_flow_reaches_browser_step() {
        if crate::rclone_path().is_none() {
            eprintln!("rclone not installed; skipping");
            return;
        }
        let dir = std::env::temp_dir().join(format!("esp-connect-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("RCLONE_CONFIG", dir.join("rclone.conf"));

        let rcd = Rcd::start().expect("start rcd");
        let port = rcd.port();

        let mut pending = begin(port, "esptestdrive", "drive").expect("begin");
        let mut reached = false;
        for _ in 0..20 {
            let Some(p) = pending else { break };
            if p.is_oauth {
                reached = true;
                break;
            }
            pending =
                advance(port, "esptestdrive", "drive", &p.state, &p.default).expect("advance");
        }
        assert!(reached, "never reached the browser sign-in step");

        oauth_stop(port);
        let _ = crate::delete_remote("esptestdrive");
        std::env::remove_var("RCLONE_CONFIG");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Drive the credential flow for Internxt with throwaway creds: it must run the login (not a
    /// dry one-shot) and fail cleanly with a credential error, leaving no half-made remote behind.
    /// The flow reaches Internxt's start state, which validates the login and rejects bogus creds;
    /// with real creds that same loop continues into the "login" state that writes the mnemonic
    /// (verified by source and on a real desktop, not reproducible here without an account).
    #[test]
    fn credential_flow_attempts_login_and_leaves_no_remote() {
        if crate::rclone_path().is_none() {
            eprintln!("rclone not installed; skipping");
            return;
        }
        let dir = std::env::temp_dir().join(format!("esp-cred-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("RCLONE_CONFIG", dir.join("rclone.conf"));

        let rcd = Rcd::start().expect("start rcd");
        let answers = vec![
            ("email".to_string(), "nobody@example.invalid".to_string()),
            (
                "pass".to_string(),
                "definitely-not-a-real-password".to_string(),
            ),
        ];
        let res = credential_connect(rcd.port(), "esptestinternxt", "internxt", &answers);
        assert!(res.is_err(), "bogus creds must not report success");
        // The flow must have tried to log in (credential rejection) or at least reached out
        // (network error in an offline CI). Either way it is a real attempt, not a plumbing bug.
        // Format with the alternate `{:#}` so the rclone cause (not just the top context) is seen.
        let msg = format!("{:#}", res.unwrap_err()).to_lowercase();
        assert!(
            [
                "login",
                "credential",
                "access",
                "connect",
                "network",
                "dial",
                "lookup",
                "timeout"
            ]
            .iter()
            .any(|needle| msg.contains(needle)),
            "error should reflect a login attempt, got: {msg}"
        );
        // No usable remote should linger from a failed login.
        let remotes = crate::list_remotes().unwrap_or_default();
        assert!(
            !remotes.iter().any(|r| r.name == "esptestinternxt"),
            "a failed login must not leave a remote behind"
        );

        let _ = crate::delete_remote("esptestinternxt");
        std::env::remove_var("RCLONE_CONFIG");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
