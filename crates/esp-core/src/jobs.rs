//! Sync jobs: backend-neutral job definitions and their on-disk store.
//!
//! A `Job` says what to keep in step and how. It never spells out an rclone path by hand;
//! `Endpoint::fs()` renders that so the UI stays backend-neutral. Jobs persist as a plain
//! JSON array under the user's config dir.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// How a sync reconciles destination against source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    /// Add and update files on the destination; never delete. The safe default.
    Copy,
    /// Make the destination identical to the source, deleting anything extra.
    Mirror,
}

impl SyncMode {
    pub fn label(self) -> &'static str {
        match self {
            SyncMode::Copy => "Copy",
            SyncMode::Mirror => "Mirror",
        }
    }

    /// True for modes that can delete files on the destination.
    pub fn is_destructive(self) -> bool {
        matches!(self, SyncMode::Mirror)
    }

    /// The rclone rc command that runs this mode.
    pub fn rc_command(self) -> &'static str {
        match self {
            SyncMode::Copy => "sync/copy",
            SyncMode::Mirror => "sync/sync",
        }
    }
}

/// One side of a job: a configured remote plus a path, or a local path (`remote = None`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Endpoint {
    #[serde(default)]
    pub remote: Option<String>,
    #[serde(default)]
    pub path: String,
}

impl Endpoint {
    /// A path on this computer.
    pub fn local(path: impl Into<String>) -> Self {
        Endpoint {
            remote: None,
            path: path.into(),
        }
    }

    /// A path on a configured remote.
    pub fn remote(remote: impl Into<String>, path: impl Into<String>) -> Self {
        Endpoint {
            remote: Some(remote.into()),
            path: path.into(),
        }
    }

    /// The rclone fs string: `remote:path`, or a bare local path.
    pub fn fs(&self) -> String {
        match &self.remote {
            Some(r) => format!("{r}:{}", self.path),
            None => self.path.clone(),
        }
    }

    /// The restic repository string for this location: a bare local path, or
    /// `rclone:remote:path` so restic backs up over our rclone rail.
    pub fn restic_repo(&self) -> String {
        match &self.remote {
            Some(r) => format!("rclone:{r}:{}", self.path),
            None => self.path.clone(),
        }
    }

    /// A short, human label for the endpoint.
    pub fn display(&self) -> String {
        match &self.remote {
            Some(r) => format!("{r}:{}", self.path),
            None if self.path.is_empty() => "this computer".to_string(),
            None => self.path.clone(),
        }
    }
}

/// A saved sync: what to keep in step, and how.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub name: String,
    pub src: Endpoint,
    pub dst: Endpoint,
    pub mode: SyncMode,
}

/// ESP's config directory: `$XDG_CONFIG_HOME/esp` (or `~/.config/esp`).
pub fn config_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .context("no XDG_CONFIG_HOME or HOME to locate the config dir")?;
    Ok(base.join("esp"))
}

/// Path to the jobs store: `<config dir>/jobs.json`.
pub fn jobs_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("jobs.json"))
}

/// Load saved jobs. An absent store is an empty list, not an error.
pub fn load_jobs() -> Result<Vec<Job>> {
    let path = jobs_path()?;
    match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Save the whole set of jobs atomically (temp file + rename).
pub fn save_jobs(jobs: &[Job]) -> Result<()> {
    let path = jobs_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let json = serde_json::to_vec_pretty(jobs).context("serializing jobs")?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

/// A stable, id-friendly slug from a name, disambiguated against existing ids.
pub fn slug_id(name: &str, existing: &[Job]) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let base = slug.trim_matches('-').to_string();
    let base = if base.is_empty() {
        "sync".to_string()
    } else {
        base
    };
    if !existing.iter().any(|j| j.id == base) {
        return base;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !existing.iter().any(|j| j.id == candidate) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fs_renders_remote_and_local() {
        assert_eq!(Endpoint::remote("gdrive", "backup").fs(), "gdrive:backup");
        assert_eq!(Endpoint::local("/home/me/docs").fs(), "/home/me/docs");
    }

    #[test]
    fn slug_is_stable_and_disambiguated() {
        let a = Job {
            id: "home-photos".into(),
            name: "Home Photos".into(),
            src: Endpoint::local("/a"),
            dst: Endpoint::remote("gdrive", "photos"),
            mode: SyncMode::Copy,
        };
        assert_eq!(slug_id("Home Photos!!", &[]), "home-photos");
        assert_eq!(
            slug_id("Home Photos", std::slice::from_ref(&a)),
            "home-photos-2"
        );
        assert_eq!(slug_id("   ", &[]), "sync");
    }

    #[test]
    fn jobs_roundtrip_through_disk() {
        let dir = std::env::temp_dir().join(format!("esp-jobs-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        assert!(load_jobs().unwrap().is_empty());
        let jobs = vec![Job {
            id: "docs".into(),
            name: "Docs".into(),
            src: Endpoint::local("/home/me/docs"),
            dst: Endpoint::remote("gdrive", "docs"),
            mode: SyncMode::Mirror,
        }];
        save_jobs(&jobs).unwrap();
        let back = load_jobs().unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].id, "docs");
        assert_eq!(back[0].mode, SyncMode::Mirror);
        assert_eq!(back[0].src.fs(), "/home/me/docs");
        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
