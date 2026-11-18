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
    let params = json!({
        "name": name,
        "type": backend,
        "parameters": {},
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
}
