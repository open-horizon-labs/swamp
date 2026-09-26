//! Cursor identification (#98): a thin wrapper over
//! `crate::agents::vscode_family::identify_editor_profile`, applied to
//! the `~/Library/Application Support/Cursor` root
//! `crate::locations::cursor::CursorDetector` resolves first. See
//! `vscode_family`'s own doc comment for the shared `state.vscdb`/
//! `workspace.json` identification this tool needs no new concept for.
//!
//! ## Why this tool is `SupportLevel::Unverified`
//!
//! Re-checked 2026-09-21: <https://cursor.com/docs> describes local
//! caching but names no path --
//! `cursor.com/docs/troubleshooting/troubleshooting-guide` contains
//! neither `Application Support/Cursor`, nor `globalStorage`, nor
//! `workspaceStorage`, nor `state.vscdb`. The only corroboration is
//! community discussion on `forum.cursor.com` (vendor-hosted, staff
//! participate, but not documentation) and the upstream VS Code layout
//! Cursor is a fork of.
//!
//! The layout is very probably right. "Very probably right" is not the
//! bar for offering to move a developer's chat history, so the matrix
//! row is `Unverified` and `crate::agents::discover_and_measure`
//! withholds every action and leaves linkage `Unresolved` for this tool.
//! Identification still works, which is the honest half.

use super::{
    AdapterCapabilities, AgentAdapter, CandidateAgentUnit, IdentifyCtx,
    vscode_family::{FORK_CAPABILITIES, OBSERVED_PROFILE},
};
use std::path::Path;

pub const CURSOR_TOOL_ID: &str = "cursor";

pub struct Adapter;

impl AgentAdapter for Adapter {
    fn id(&self) -> &'static str {
        CURSOR_TOOL_ID
    }
    fn name(&self) -> &'static str {
        "Cursor"
    }
    fn capabilities(&self) -> AdapterCapabilities {
        FORK_CAPABILITIES
    }
    fn identify(&self, home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
        identify(home, ctx)
    }
}

pub fn identify(profile_root: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
    super::vscode_family::identify_editor_profile(profile_root, ctx, OBSERVED_PROFILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{AgentActionCapability, AgentCategory, IdentificationCache, contract};
    use std::fs;

    fn touch(path: &Path, content: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn run(root: &Path) -> Vec<CandidateAgentUnit> {
        let cache = IdentificationCache::disabled();
        identify(root, &IdentifyCtx::new(1, &cache))
    }

    #[test]
    fn identifies_the_global_database_as_protected() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        touch(&root.join("User/globalStorage/state.vscdb"), b"sqlite");
        let units = run(root);
        let u = units
            .iter()
            .find(|u| u.category() == AgentCategory::Sessions)
            .expect("global db identified");
        assert!(u.protected());
        assert_eq!(u.action(), AgentActionCapability::None);
    }

    #[test]
    fn workspace_storage_is_marked_observed_not_confirmed() {
        // Cursor has no official layout doc, so the shared module is
        // asked for the observed shape, not the confirmed one.
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        touch(&root.join("User/globalStorage/state.vscdb"), b"sqlite");
        touch(
            &root.join("User/workspaceStorage/w1/state.vscdb"),
            b"sqlite",
        );
        let units = run(root);
        let u = units
            .iter()
            .find(|u| u.relative_path().contains("workspaceStorage"))
            .expect("workspace db identified");
        assert!(
            u.note
                .as_deref()
                .is_some_and(|n| n.contains("observed rather than confirmed")),
            "{:?}",
            u.note
        );
    }

    // --- the five contract tests ---------------------------------------

    #[test]
    fn unknown_format_is_explicit_not_empty() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        touch(&root.join("unrelated.txt"), b"hello");
        let units = run(root);
        assert_eq!(units.len(), 1, "an unrecognized profile must still surface");
        assert_eq!(units[0].relative_path(), "(unsupported layout version)");
        assert!(
            units[0]
                .note
                .as_deref()
                .is_some_and(|n| n.contains("not scanned further")),
            "{:?}",
            units[0].note
        );
    }

    #[test]
    fn canary_content_never_appears_in_output() {
        let canary = "CANARY-CURSOR-DO-NOT-LEAK-4a17";
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        touch(&root.join("User/globalStorage/state.vscdb"), b"sqlite");
        touch(
            &root.join("User/workspaceStorage/w1/workspace.json"),
            format!("{{\"folder\":\"file:///nope\",\"note\":\"{canary}\"}}").as_bytes(),
        );
        touch(
            &root.join("User/workspaceStorage/w1/state.vscdb"),
            b"sqlite",
        );
        touch(
            &root.join("User/History/e1/snapshot"),
            format!("user said {canary}").as_bytes(),
        );
        contract::no_content_leak(&run(root), canary);
    }

    #[test]
    fn identification_reads_no_more_than_header_cap() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        touch(
            &root.join("User/globalStorage/state.vscdb"),
            &b"x".repeat(500_000),
        );
        for i in 0..20 {
            touch(
                &root.join(format!("User/workspaceStorage/w{i}/state.vscdb")),
                &b"x".repeat(200_000),
            );
            touch(
                &root.join(format!("User/workspaceStorage/w{i}/workspace.json")),
                b"{\"folder\":\"file:///nope\"}",
            );
        }
        let (units, counters) = contract::measured(|| run(root));
        assert!(units.len() > 20);
        // Twenty bounded `workspace.json` reads, nothing else: the
        // `state.vscdb` stores are never opened at all.
        contract::within_header_cap(counters, 20);
        assert!(
            counters.header_bytes_read < 20 * 200_000,
            "reading {} bytes means a SQLite store was opened",
            counters.header_bytes_read
        );
    }

    #[test]
    fn protected_categories_default_protected() {
        // Cursor's own credentials/config do not live in this profile
        // root (the CLI state under ~/.cursor/ is a separate,
        // undecomposed external unit), so this adapter produces no
        // default-protected category; the shared helper asserts that
        // claim and then proves the builder's guarantee.
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        touch(&root.join("User/globalStorage/state.vscdb"), b"sqlite");
        touch(&root.join("User/History/e1/snapshot"), b"bytes");
        contract::protection_defaults_hold_with_no_protected_category(&run(root), CURSOR_TOOL_ID);
    }

    #[test]
    fn project_link_is_declared_or_unresolved_never_basename_guess() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let repo = root.join("declared-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        // (a) declared in the tool's own metadata -> Linked, Declared.
        touch(
            &root.join("User/workspaceStorage/w1/workspace.json"),
            format!("{{\"folder\":\"file://{}\"}}", repo.display()).as_bytes(),
        );
        touch(
            &root.join("User/workspaceStorage/w1/state.vscdb"),
            b"sqlite",
        );
        // (b) a workspaceStorage directory *named* after a real repo,
        // with no declared metadata -> Unresolved, never Linked.
        let named = root.join("basename-only-repo");
        fs::create_dir_all(named.join(".git")).unwrap();
        touch(
            &root.join("User/workspaceStorage/basename-only-repo/state.vscdb"),
            b"sqlite",
        );
        let units = run(root);
        let declared = units
            .iter()
            .find(|u| u.relative_path().contains("/w1/"))
            .expect("declared workspace identified");
        let crate::agents::ProjectLinkState::Linked { source, .. } = &declared.project_link()
        else {
            panic!("expected Linked, got {:?}", declared.project_link());
        };
        assert_eq!(*source, crate::agents::LinkSource::Declared);
        let guessed = units
            .iter()
            .find(|u| u.relative_path().contains("basename-only-repo"))
            .expect("named workspace identified");
        assert!(
            matches!(
                guessed.project_link(),
                crate::agents::ProjectLinkState::Unresolved { .. }
            ),
            "{:?}",
            guessed.project_link()
        );
        contract::linkage_is_declared_or_explicit(&units, "basename-only-repo");
    }
}
