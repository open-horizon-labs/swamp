//! Plain names for what a check is looking at. A build hash directory
//! (`target/debug/incremental/swamp_tui-114mwhjrtg1wm`) becomes
//! `swamp · swamp_tui incremental build (target/debug)`; anything the
//! rules do not recognise falls back to the project plus the path below
//! its checkout, and to the last two path parts outside any project.

use std::path::{Path, PathBuf};
use swamp_core::report::Report;

/// A crate directory name with its cargo hash suffix removed:
/// `swamp_tui-114mwhjrtg1wm` -> `swamp_tui`. A name without a trailing
/// 10-plus character alphanumeric hash is returned as is.
fn strip_hash(name: &str) -> &str {
    match name.rsplit_once('-') {
        Some((head, tail))
            if !head.is_empty()
                && tail.len() >= 10
                && tail.chars().all(|c| c.is_ascii_alphanumeric())
                && tail.chars().any(|c| c.is_ascii_digit()) =>
        {
            head
        }
        _ => name,
    }
}

/// The project and the path below its checkout that `path` lives in.
fn locate<'a>(report: &'a Report, path: &Path) -> Option<(String, PathBuf)> {
    let mut best: Option<(&'a swamp_core::report::ProjectRow, &Path)> = None;
    for p in &report.projects {
        for wt in &p.worktrees {
            if path.starts_with(&wt.path)
                && best.is_none_or(|(_, b)| wt.path.as_os_str().len() > b.as_os_str().len())
            {
                best = Some((p, wt.path.as_path()));
            }
        }
    }
    best.map(|(p, wt)| {
        (
            crate::model::project_display_name(p),
            path.strip_prefix(wt).unwrap_or(path).to_path_buf(),
        )
    })
}

/// A cargo build directory named in words, from the parts below `target`.
fn cargo_words(parts: &[String]) -> Option<String> {
    let at = parts.iter().position(|p| p == "target")?;
    let below = &parts[at + 1..];
    let profile = below.first()?;
    let place = format!("target/{profile}");
    let kind = below.get(1).map(String::as_str);
    let leaf = below.get(2).map(|s| strip_hash(s));
    Some(match (kind, leaf) {
        (None, _) => format!("{profile} build output ({place})"),
        (Some("incremental"), Some(krate)) => format!("{krate} incremental build ({place})"),
        (Some("incremental"), None) => format!("incremental builds ({place})"),
        (Some("build"), Some(krate)) => format!("{krate} build script ({place})"),
        (Some("build"), None) => format!("build scripts ({place})"),
        (Some("deps"), _) => format!("compiled dependencies ({place})"),
        (Some(".fingerprint"), _) => format!("build fingerprints ({place})"),
        (Some("examples"), _) => format!("compiled examples ({place})"),
        (Some(other), _) => format!("{other} ({place})"),
    })
}

/// The display name of the project whose checkout holds `path`.
pub fn project_of(report: &Report, path: &Path) -> Option<String> {
    locate(report, path).map(|(project, _)| project)
}

/// The name a person would give `path`, for progress lines and the
/// blocked list.
pub fn friendly_unit_name(report: &Report, path: &Path) -> String {
    let display = |p: &Path| {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    };
    match locate(report, path) {
        Some((project, rel)) => {
            let parts = display(&rel);
            if parts.is_empty() {
                return format!("{project} (whole checkout)");
            }
            match cargo_words(&parts) {
                Some(words) => format!("{project} · {words}"),
                None => format!("{project} · {}", parts.join("/")),
            }
        }
        None => {
            let parts = display(path);
            let n = parts.len();
            parts[n.saturating_sub(2)..].join("/")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_come_off_crate_directories_only() {
        assert_eq!(strip_hash("swamp_tui-114mwhjrtg1wm"), "swamp_tui");
        assert_eq!(strip_hash("libc-26ddd0231c356665"), "libc");
        assert_eq!(strip_hash("group-01"), "group-01");
        assert_eq!(strip_hash("node_modules"), "node_modules");
    }

    #[test]
    fn cargo_directories_are_named_in_words() {
        let p = |s: &str| s.split('/').map(String::from).collect::<Vec<_>>();
        assert_eq!(
            cargo_words(&p("target/debug/incremental/swamp_tui-114mwhjrtg1wm")).unwrap(),
            "swamp_tui incremental build (target/debug)"
        );
        assert_eq!(
            cargo_words(&p("target/debug/deps")).unwrap(),
            "compiled dependencies (target/debug)"
        );
        assert!(cargo_words(&p("node_modules")).is_none());
    }

    #[test]
    fn a_path_outside_any_project_keeps_its_last_two_parts() {
        let report = Report::empty(PathBuf::from("/root"));
        assert_eq!(
            friendly_unit_name(&report, Path::new("/a/b/c/.gradle")),
            "c/.gradle"
        );
    }
}
