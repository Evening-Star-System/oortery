//! Desktop integration: make a mounted remote show up in the file manager's sidebar, the way a
//! plugged-in drive does. A FUSE mount is just a folder, so the desktop will not list it on its
//! own; we add a GTK "bookmark" for the mountpoint, which GNOME Files and most GTK file managers
//! read from `~/.config/gtk-3.0/bookmarks` (GTK4 apps read the same file). Added on mount, removed
//! on unmount, and pruned at startup/shutdown so a crash never leaves a dead entry pointing at an
//! empty folder.
//!
//! Linux-only. On other platforms every function is a no-op, so the cross-platform core stays
//! clean and the sidebar story gets its own native implementation later (Finder, Explorer).

/// Add a sidebar bookmark for a freshly mounted remote at `point`, labelled `label`.
pub fn add_place(point: &str, label: &str) {
    #[cfg(target_os = "linux")]
    if let Some(path) = linux::bookmarks_path() {
        linux::add_to(&path, point, label);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (point, label);
    }
}

/// Remove the sidebar bookmark for `point` (on unmount or when the remote is removed).
pub fn remove_place(point: &str) {
    #[cfg(target_os = "linux")]
    if let Some(path) = linux::bookmarks_path() {
        linux::remove_from(&path, point);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = point;
    }
}

/// Remove every bookmark we manage (any under `$HOME/Oortery/`). Run at startup and shutdown so a
/// prior crash cannot leave the sidebar pointing at mountpoints that are no longer live.
pub fn prune_places() {
    #[cfg(target_os = "linux")]
    if let (Some(path), Some(root)) = (linux::bookmarks_path(), linux::oortery_root()) {
        linux::prune_from(&path, &root);
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::fmt::Write as _;
    use std::io::Write as _;
    use std::path::{Path, PathBuf};

    /// `~/.config/gtk-3.0/bookmarks`, honouring `$XDG_CONFIG_HOME`.
    pub(super) fn bookmarks_path() -> Option<PathBuf> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(config.join("gtk-3.0").join("bookmarks"))
    }

    /// The prefix that identifies a bookmark as ours: `file://` + `$HOME/Oortery/`.
    pub(super) fn oortery_root() -> Option<String> {
        let home = std::env::var("HOME").ok()?;
        Some(to_uri(&format!("{home}/Oortery/")))
    }

    /// Percent-encode a path into a `file://` URI, leaving the unreserved set and `/` intact (so a
    /// space becomes `%20` and the file manager parses the URI as a single field).
    pub(super) fn to_uri(path: &str) -> String {
        let mut s = String::from("file://");
        for &b in path.as_bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                    s.push(b as char);
                }
                _ => {
                    let _ = write!(s, "%{b:02X}");
                }
            }
        }
        s
    }

    /// The URI is the first whitespace-delimited field of a bookmark line (the rest is the label).
    fn uri_of(line: &str) -> Option<&str> {
        line.split_whitespace().next()
    }

    fn read_lines(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .map(|c| c.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// Write the bookmark file atomically (temp file + rename) so an interrupted write can never
    /// corrupt the user's existing bookmarks.
    fn write_lines(path: &Path, lines: &[String]) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("oortery-tmp");
        {
            let mut f = std::fs::File::create(&tmp)?;
            for l in lines {
                writeln!(f, "{l}")?;
            }
        }
        std::fs::rename(&tmp, path)
    }

    pub(super) fn add_to(path: &Path, point: &str, label: &str) {
        let uri = to_uri(point);
        let mut lines = read_lines(path);
        if lines.iter().any(|l| uri_of(l) == Some(uri.as_str())) {
            return; // already present: idempotent
        }
        lines.push(format!("{uri} {label}"));
        let _ = write_lines(path, &lines);
    }

    pub(super) fn remove_from(path: &Path, point: &str) {
        let uri = to_uri(point);
        let lines = read_lines(path);
        if !lines.iter().any(|l| uri_of(l) == Some(uri.as_str())) {
            return; // nothing to do: avoid rewriting the file needlessly
        }
        let kept: Vec<String> = lines
            .into_iter()
            .filter(|l| uri_of(l) != Some(uri.as_str()))
            .collect();
        let _ = write_lines(path, &kept);
    }

    pub(super) fn prune_from(path: &Path, root_uri: &str) {
        let lines = read_lines(path);
        if !lines.iter().any(|l| matches_root(l, root_uri)) {
            return;
        }
        let kept: Vec<String> = lines
            .into_iter()
            .filter(|l| !matches_root(l, root_uri))
            .collect();
        let _ = write_lines(path, &kept);
    }

    fn matches_root(line: &str, root_uri: &str) -> bool {
        uri_of(line).is_some_and(|u| u.starts_with(root_uri))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        // A unique file per test (`name` must differ per caller) so parallel tests never share one.
        fn tmp(name: &str) -> PathBuf {
            let dir =
                std::env::temp_dir().join(format!("esp-bookmarks-{}-{}", std::process::id(), name));
            std::fs::create_dir_all(&dir).unwrap();
            dir.join("bookmarks")
        }

        #[test]
        fn uri_encodes_spaces_keeps_slashes() {
            assert_eq!(
                to_uri("/home/a/Oortery/My Drive"),
                "file:///home/a/Oortery/My%20Drive"
            );
        }

        #[test]
        fn add_is_idempotent_and_preserves_others() {
            let f = tmp("idempotent");
            std::fs::write(&f, "file:///home/a/Documents Documents\n").unwrap();
            add_to(&f, "/home/a/Oortery/Internxt", "Internxt");
            add_to(&f, "/home/a/Oortery/Internxt", "Internxt"); // twice: no duplicate
            let body = std::fs::read_to_string(&f).unwrap();
            assert_eq!(body.matches("Oortery/Internxt").count(), 1);
            assert!(body.contains("Documents Documents")); // user's bookmark untouched
            let _ = std::fs::remove_file(&f);
        }

        #[test]
        fn remove_only_our_line() {
            let f = tmp("remove");
            add_to(&f, "/home/a/Oortery/Internxt", "Internxt");
            std::fs::write(
                &f,
                format!("{}\nfile:///home/a/Pics Pics\n", {
                    let mut l = read_lines(&f);
                    l.pop().unwrap()
                }),
            )
            .unwrap();
            remove_from(&f, "/home/a/Oortery/Internxt");
            let body = std::fs::read_to_string(&f).unwrap();
            assert!(!body.contains("Oortery/Internxt"));
            assert!(body.contains("Pics Pics"));
            let _ = std::fs::remove_file(&f);
        }

        #[test]
        fn prune_removes_all_under_root_only() {
            let f = tmp("prune");
            add_to(&f, "/home/a/Oortery/Internxt", "Internxt");
            add_to(&f, "/home/a/Oortery/Backblaze", "Backblaze");
            std::fs::write(
                &f,
                format!(
                    "{}file:///home/a/Music Music\n",
                    std::fs::read_to_string(&f).unwrap()
                ),
            )
            .unwrap();
            prune_from(&f, &to_uri("/home/a/Oortery/"));
            let body = std::fs::read_to_string(&f).unwrap();
            assert!(!body.contains("Oortery/"));
            assert!(body.contains("Music Music"));
            let _ = std::fs::remove_file(&f);
        }
    }
}
