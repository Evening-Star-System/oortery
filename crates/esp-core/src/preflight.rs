//! Pre-flight: find names that a cloud backend will silently refuse to sync.
//!
//! OneDrive and SharePoint (and any case-insensitive or Windows-rooted target) quietly drop
//! files whose names break their rules: forbidden characters, reserved device names, trailing
//! spaces or periods, over-long paths, case-only collisions. The incumbent clients say nothing
//! and the file just never arrives. ESP's answer is to look first and tell the user plainly,
//! so a backup that reports success actually contains everything.
//!
//! The checks are pure and the walker uses only `std::fs`, so the whole module is testable
//! with a temp tree and no backend.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Characters OneDrive/SharePoint reject in a file or folder name. `/` is the path separator
/// so it never appears inside a single component; the rest can and do.
const FORBIDDEN: &[char] = &['<', '>', ':', '"', '\\', '|', '?', '*'];

/// Whole-path length ceiling. OneDrive/SharePoint reject a decoded path over 400 chars.
pub const MAX_PATH_LEN: usize = 400;

/// Single-component length ceiling (bytes). Most filesystems and backends cap a name at 255.
pub const MAX_COMPONENT_LEN: usize = 255;

/// One thing wrong with a name or path that will trip a strict backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Issue {
    ControlCharacter,
    ForbiddenCharacter(char),
    TrailingSpaceOrPeriod,
    LeadingSpace,
    ReservedDeviceName,
    ReservedSpecialName,
    ComponentTooLong(usize),
    PathTooLong(usize),
    /// Differs from a sibling only by letter case; one will overwrite the other on a
    /// case-insensitive target. Carries the sibling it collides with.
    CaseCollision(String),
}

impl Issue {
    /// A short, plain-language explanation for the UI.
    pub fn summary(&self) -> String {
        match self {
            Issue::ControlCharacter => "Contains a control character".into(),
            Issue::ForbiddenCharacter(c) => {
                format!("Contains a character some clouds reject: {c}")
            }
            Issue::TrailingSpaceOrPeriod => "Ends with a space or period".into(),
            Issue::LeadingSpace => "Starts with a space".into(),
            Issue::ReservedDeviceName => "Uses a reserved Windows name (e.g. CON, NUL)".into(),
            Issue::ReservedSpecialName => "Is a reserved system file name".into(),
            Issue::ComponentTooLong(n) => format!("Name is too long ({n} characters)"),
            Issue::PathTooLong(n) => format!("Path is too long ({n} characters)"),
            Issue::CaseCollision(other) => {
                format!("Differs only by letter case from \"{other}\"")
            }
        }
    }
}

/// A specific hazard: the offending path (relative to the scanned root) and what is wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hazard {
    pub path: String,
    pub issue: Issue,
}

/// Case-insensitive match against the Windows reserved device names, allowing an extension
/// (CON, con.txt, LPT3.log all match).
fn is_reserved_device(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (base.starts_with("COM") || base.starts_with("LPT"))
            && base.len() == 4
            && base.as_bytes()[3].is_ascii_digit()
            && base.as_bytes()[3] != b'0'
}

/// Reserved system file names that sync engines treat specially.
fn is_reserved_special(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "desktop.ini" || lower == ".lock" || lower.starts_with("_vti_")
}

/// Everything wrong with a single path component (a file or folder name), ignoring its
/// position in the tree. Does not include path-length or case-collision issues, which need
/// context.
pub fn component_issues(name: &str) -> Vec<Issue> {
    let mut out = Vec::new();
    if name.chars().any(|c| (c as u32) < 0x20) {
        out.push(Issue::ControlCharacter);
    }
    for &f in FORBIDDEN {
        if name.contains(f) {
            out.push(Issue::ForbiddenCharacter(f));
        }
    }
    if name.ends_with(' ') || name.ends_with('.') {
        out.push(Issue::TrailingSpaceOrPeriod);
    }
    if name.starts_with(' ') {
        out.push(Issue::LeadingSpace);
    }
    if is_reserved_device(name) {
        out.push(Issue::ReservedDeviceName);
    }
    if is_reserved_special(name) {
        out.push(Issue::ReservedSpecialName);
    }
    if name.len() > MAX_COMPONENT_LEN {
        out.push(Issue::ComponentTooLong(name.len()));
    }
    out
}

/// Scan a local directory tree for names a strict cloud backend would refuse. Stops once
/// `limit` hazards are found and reports whether it stopped early, so a pathological tree
/// (hundreds of thousands of files) cannot make the check run away. A `limit` of 0 means no
/// cap.
pub fn scan_dir(root: &Path, limit: usize) -> std::io::Result<(Vec<Hazard>, bool)> {
    let mut hazards = Vec::new();
    // Stack of (dir, relative prefix). The root itself is not checked, only its contents.
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        let mut names: Vec<String> = Vec::new();
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            // An unreadable subdirectory is not a name hazard; skip it quietly.
            Err(_) => continue,
        };
        let mut subdirs = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            for issue in component_issues(&name) {
                hazards.push(Hazard {
                    path: rel.clone(),
                    issue,
                });
            }
            if rel.chars().count() > MAX_PATH_LEN {
                hazards.push(Hazard {
                    path: rel.clone(),
                    issue: Issue::PathTooLong(rel.chars().count()),
                });
            }
            names.push(name.clone());
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                subdirs.push((entry.path(), rel));
            }
            if limit != 0 && hazards.len() >= limit {
                return Ok((hazards, true));
            }
        }
        // Case-only collisions among siblings in this directory.
        for hz in case_collisions(&names, &prefix) {
            hazards.push(hz);
            if limit != 0 && hazards.len() >= limit {
                return Ok((hazards, true));
            }
        }
        stack.extend(subdirs);
    }
    Ok((hazards, false))
}

/// Find sibling names that differ only by case. Reports each later name as colliding with the
/// first one seen, once per collision.
fn case_collisions(names: &[String], prefix: &str) -> Vec<Hazard> {
    let mut first: HashMap<String, String> = HashMap::new();
    let mut out = Vec::new();
    for name in names {
        let key = name.to_ascii_lowercase();
        match first.get(&key) {
            Some(original) if original != name => {
                let rel = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                };
                out.push(Hazard {
                    path: rel,
                    issue: Issue::CaseCollision(original.clone()),
                });
            }
            Some(_) => {} // exact duplicate name cannot happen in one dir
            None => {
                first.insert(key, name.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_chars_and_trailing_dot() {
        let issues = component_issues("report:final.");
        assert!(issues.contains(&Issue::ForbiddenCharacter(':')));
        assert!(issues.contains(&Issue::TrailingSpaceOrPeriod));
    }

    #[test]
    fn reserved_device_names() {
        assert!(component_issues("CON").contains(&Issue::ReservedDeviceName));
        assert!(component_issues("con.txt").contains(&Issue::ReservedDeviceName));
        assert!(component_issues("LPT3").contains(&Issue::ReservedDeviceName));
        assert!(component_issues("COM9.log").contains(&Issue::ReservedDeviceName));
        // Not reserved: COM0, a normal word, LPT with two digits.
        assert!(!component_issues("COM0").contains(&Issue::ReservedDeviceName));
        assert!(!component_issues("console.js").contains(&Issue::ReservedDeviceName));
        assert!(!component_issues("LPT10").contains(&Issue::ReservedDeviceName));
    }

    #[test]
    fn clean_name_has_no_issues() {
        assert!(component_issues("perfectly-normal_file.txt").is_empty());
    }

    #[test]
    fn scan_flags_real_tree_hazards() {
        let base = std::env::temp_dir().join(format!("esp-preflight-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let deep = base.join("sub");
        fs::create_dir_all(&deep).unwrap();
        fs::write(base.join("good.txt"), b"x").unwrap();
        fs::write(base.join("bad|name.txt"), b"x").unwrap();
        fs::write(base.join("Ideas.md"), b"x").unwrap();
        fs::write(base.join("ideas.md"), b"x").unwrap();
        fs::write(deep.join("CON.txt"), b"x").unwrap();

        let (hazards, capped) = scan_dir(&base, 0).unwrap();
        assert!(!capped);
        let has = |p: &str, pred: fn(&Issue) -> bool| {
            hazards.iter().any(|h| h.path == p && pred(&h.issue))
        };
        assert!(has("bad|name.txt", |i| matches!(
            i,
            Issue::ForbiddenCharacter('|')
        )));
        assert!(has("sub/CON.txt", |i| *i == Issue::ReservedDeviceName));
        // Exactly one of the two case-colliding names is reported, nested path preserved.
        assert_eq!(
            hazards
                .iter()
                .filter(|h| matches!(h.issue, Issue::CaseCollision(_)))
                .count(),
            1
        );
        // The clean file produced nothing.
        assert!(!hazards.iter().any(|h| h.path == "good.txt"));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn scan_respects_the_cap() {
        let base = std::env::temp_dir().join(format!("esp-preflight-cap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        for i in 0..10 {
            fs::write(base.join(format!("bad:{i}")), b"x").unwrap();
        }
        let (hazards, capped) = scan_dir(&base, 3).unwrap();
        assert!(capped, "should stop early");
        assert!(hazards.len() <= 4, "roughly bounded by the cap");
        let _ = fs::remove_dir_all(&base);
    }
}
