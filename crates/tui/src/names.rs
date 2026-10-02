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

/// The artifact's exact suffix within its checkout. Row identity still
/// carries the absolute path; this is only the compact table label.
pub fn checkout_relative_path(checkout: &Path, path: &Path) -> String {
    path.strip_prefix(checkout)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// A short checkout discriminator for tables that aggregate several
/// worktrees of one project. Main checkout is already implied by the
/// project context; linked worktrees and clones need an extra cue.
pub fn checkout_variant(
    kind: &swamp_core::report::WorktreeKind,
    path: &Path,
    peers: &[(&swamp_core::report::WorktreeKind, &Path)],
) -> Option<String> {
    let leaf = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "checkout".to_string());
    let same_leaf: Vec<&Path> = peers
        .iter()
        .filter(|(peer_kind, peer_path)| {
            *peer_kind == kind && peer_path.file_name() == path.file_name()
        })
        .map(|(_, peer_path)| *peer_path)
        .collect();
    let name = if same_leaf.iter().any(|peer| *peer != path) {
        shortest_unique_suffix(path, &same_leaf)
    } else {
        leaf
    };
    match kind {
        swamp_core::report::WorktreeKind::Main => None,
        swamp_core::report::WorktreeKind::Linked => Some(format!("worktree {name}")),
        swamp_core::report::WorktreeKind::Clone => Some(format!("clone {name}")),
    }
}

fn shortest_unique_suffix(path: &Path, peers: &[&Path]) -> String {
    let parts = |path: &Path| {
        path.components()
            .filter(|component| {
                !matches!(
                    component,
                    std::path::Component::RootDir | std::path::Component::Prefix(_)
                )
            })
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    };
    let own = parts(path);
    for count in 1..=own.len() {
        let suffix = &own[own.len() - count..];
        let unique = peers.iter().filter(|peer| **peer != path).all(|peer| {
            let peer_parts = parts(peer);
            peer_parts.len() < count || &peer_parts[peer_parts.len() - count..] != suffix
        });
        if unique {
            return suffix.join("/");
        }
    }
    path.display().to_string()
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

/// The report's own name (not the display name) of the project whose
/// checkout holds `path`.
pub fn project_key_of(report: &Report, path: &Path) -> Option<String> {
    let mut best: Option<(&str, usize)> = None;
    for p in &report.projects {
        for wt in &p.worktrees {
            let len = wt.path.as_os_str().len();
            if path.starts_with(&wt.path) && best.is_none_or(|(_, l)| len > l) {
                best = Some((p.name.as_str(), len));
            }
        }
    }
    best.map(|(n, _)| n.to_string())
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

/// Compact sourced warning labels for the human action review.
pub fn compact_warning(warning: &str) -> String {
    let text = swamp_core::reclaim_trash::plain(warning);
    let sourced = |prefix: &str, heading: &str| {
        text.strip_prefix(prefix).map(|rest| {
            let rest = rest.trim().trim_start_matches(':').trim();
            if let Some((facts, source)) = rest
                .strip_suffix(')')
                .and_then(|s| s.rsplit_once(" (from "))
            {
                format!("{heading} · {} · source {}", facts.trim(), source.trim())
            } else {
                format!("{heading} · {}", rest.trim())
            }
        })
    };
    sourced("regeneration cost not established", "Cost unknown")
        .or_else(|| sourced("getting it back", "Cost to restore"))
        .or_else(|| sourced("cannot be regenerated", "Cannot be regenerated"))
        .or_else(|| {
            text.strip_prefix("last used:")
                .map(|s| format!("Last used · {}", s.trim()))
        })
        .or_else(|| {
            text.strip_prefix("who needs it:")
                .map(|s| format!("Consumers · {}", s.trim()))
        })
        .or_else(|| {
            text.strip_prefix("in use right now:")
                .map(|s| format!("Open files · {}", s.trim()))
        })
        .or_else(|| {
            text.strip_prefix("whether a process has it open")
                .map(|s| format!("Open files · whether a process has it open{}", s))
        })
        .unwrap_or(text)
}

/// Decision facts for the primary review. Only known bookkeeping is
/// disclosed in details instead; unfamiliar warnings always remain visible.
pub fn decision_warning(warning: &str) -> Option<String> {
    let text = swamp_core::reclaim_trash::plain(warning);
    if is_member_warning(&text)
        || [
            "internal file history and subgroup hardlink attribution",
            "swamp identifies this",
            "swamp's selection rules for this build folder",
            "size is selected allocation, not promised free space",
            "declared consumers: none found",
            "who needs it: not established for this path",
            "this is the whole folder, including the ",
        ]
        .iter()
        .any(|prefix| text.starts_with(prefix))
    {
        return None;
    }
    if text.starts_with("moves only this selected path to Trash;")
        || text.starts_with("exact selected build, NOT proven obsolete;")
    {
        return Some("Stop builds using this folder.".into());
    }
    if text.starts_with("moving this manifest frees none of its layers:")
        && let Some(bytes) = text
            .split("the layers (")
            .nth(1)
            .and_then(|s| s.split(')').next())
    {
        return Some(format!("Model layers ({bytes}) stay in blobs/."));
    }
    if let Some(rest) = text.strip_prefix("last used:") {
        return Some(format!(
            "Last used · {}",
            rest.trim().replace(" of its model layer", "")
        ));
    }
    if text.starts_with("regeneration cost not established") {
        return Some("Cost to restore · unknown".into());
    }
    if text.starts_with("getting it back:") {
        if let Some(command) = text.split('`').nth(1) {
            return Some(format!("Restore · {command} (if available)"));
        }
        let words = text.strip_prefix("getting it back:").unwrap().trim();
        let words = words.split(" (from ").next().unwrap_or(words);
        return Some(format!("Restore · {words}"));
    }
    if text.starts_with("cannot be regenerated") {
        return Some("Cannot be downloaded or rebuilt.".into());
    }
    Some(compact_warning(&text))
}

pub fn decision_fact_is_note(fact: &str) -> bool {
    ["Last used", "Restore", "Cost to restore"]
        .iter()
        .any(|prefix| fact.starts_with(prefix))
}

/// Whether a warning is the per-member path echo also represented in the
/// full action inventory. Keep prefix parsing in this presentation module.
pub fn is_member_warning(warning: &str) -> bool {
    swamp_core::reclaim_trash::plain(warning).starts_with("member: ")
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

    #[test]
    fn compact_checkout_context_keeps_relative_path_and_checkout_kind() {
        assert_eq!(
            checkout_relative_path(Path::new("/src/project"), Path::new("/src/project/target")),
            "target"
        );
        assert_eq!(
            checkout_variant(
                &swamp_core::report::WorktreeKind::Linked,
                Path::new("/src/project/.worktrees/fix"),
                &[]
            )
            .as_deref(),
            Some("worktree fix")
        );
        assert_eq!(
            checkout_variant(
                &swamp_core::report::WorktreeKind::Clone,
                Path::new("/src/project-copy"),
                &[]
            )
            .as_deref(),
            Some("clone project-copy")
        );
        assert_eq!(
            checkout_variant(
                &swamp_core::report::WorktreeKind::Main,
                Path::new("/src/project"),
                &[]
            ),
            None
        );
    }

    #[test]
    fn worktree_labels_disambiguate_equal_leaf_names_without_losing_identity() {
        use swamp_core::report::WorktreeKind as Kind;
        let linked = Kind::Linked;
        let main = Kind::Main;
        let a = Path::new("/src/project-a/.worktrees/fix");
        let b = Path::new("/src/project-b/.worktrees/fix");
        let peers = [
            (&linked, a),
            (&linked, b),
            (&main, Path::new("/src/project")),
        ];
        assert_eq!(
            checkout_variant(&linked, a, &peers).as_deref(),
            Some("worktree project-a/.worktrees/fix")
        );
        assert_eq!(
            checkout_variant(&linked, b, &peers).as_deref(),
            Some("worktree project-b/.worktrees/fix")
        );
        assert_ne!(
            a, b,
            "these display labels still name separate checkout paths"
        );
    }
}
