//! Windsurf identification (#98): a thin wrapper over
//! `crate::agents::vscode_family::identify_editor_profile`, applied to
//! the `~/Library/Application Support/Windsurf` root
//! `crate::locations::windsurf::WindsurfDetector` resolves first.
//!
//! ## What the 2026-09-21 re-verification found
//!
//! `docs.windsurf.com` now 307-redirects to `docs.devin.ai`: the product
//! was renamed **Devin Desktop** on 2026-06-02. The official FAQ at
//! <https://docs.devin.ai/desktop/devin-desktop-faq> (retrieved
//! 2026-09-21) *does* name the per-user IDE data directory -- macOS
//! `~/Library/Application Support/Windsurf/` as the legacy, read-only
//! location and `.../Devin/` as the current read-write one, with
//! `User/settings.json`, `User/keybindings.json`, `User/snippets/`,
//! `globalStorage/`, `Workspaces/` and `argv.json` inside. So the
//! profile root and `globalStorage` are, finally, primary-source
//! confirmed.
//!
//! Two things are still not:
//!
//! * `User/workspaceStorage` is not named on that page, so this adapter
//!   asks `vscode_family` for the *observed* profile shape and labels
//!   those units accordingly; and
//! * the caches and logs beside `User/` (`Cache`, `CachedData`,
//!   `CachedExtensionVSIXs`, `logs`) are VS Code conventions this page
//!   does not confirm for this fork.
//!
//! Because the modeled shape is therefore still partly unconfirmed, the
//! matrix row is `SupportLevel::Unverified` and
//! `crate::agents::discover_and_measure` withholds every action and
//! leaves linkage `Unresolved` for this tool. An installation whose
//! actual layout differs shows up as `vscode_family`'s honest
//! "(unsupported layout version)" residual, never a silent miscount.
//!
//! The Devin-era `~/Library/Application Support/Devin` root is a
//! detector question, not an adapter one; see
//! `crate::locations::windsurf`.

use super::{
    AdapterCapabilities, AgentAdapter, CandidateAgentUnit, IdentifyCtx,
    vscode_family::{FORK_CAPABILITIES, OBSERVED_PROFILE},
};
use std::path::Path;

pub const WINDSURF_TOOL_ID: &str = "windsurf";

pub struct Adapter;

impl AgentAdapter for Adapter {
    fn id(&self) -> &'static str {
        WINDSURF_TOOL_ID
    }
    fn name(&self) -> &'static str {
        "Windsurf"
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
    fn the_confirmed_global_storage_database_is_protected() {
        // `globalStorage/` is the half the official Devin Desktop FAQ
        // does name.
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

    // --- the five contract tests ---------------------------------------

    #[test]
    fn unknown_format_is_explicit_not_empty() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        fs::create_dir_all(root).unwrap();
        fs::write(root.join("unrelated.txt"), b"hello").unwrap();
        let units = run(root);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].relative_path(), "(unsupported layout version)");
        assert!(
            units[0]
                .note
                .as_deref()
                .is_some_and(|n| n.contains("no User/ directory found")),
            "{:?}",
            units[0].note
        );
    }

    #[test]
    fn canary_content_never_appears_in_output() {
        let canary = "CANARY-WINDSURF-DO-NOT-LEAK-91bc";
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        touch(&root.join("User/globalStorage/state.vscdb"), b"sqlite");
        touch(
            &root.join("User/workspaceStorage/w1/workspace.json"),
            format!("{{\"folder\":\"file:///nope\",\"x\":\"{canary}\"}}").as_bytes(),
        );
        touch(
            &root.join("User/workspaceStorage/w1/state.vscdb"),
            b"sqlite",
        );
        contract::no_content_leak(&run(root), canary);
    }

    #[test]
    fn identification_reads_no_more_than_header_cap() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        touch(
            &root.join("User/globalStorage/state.vscdb"),
            &b"x".repeat(300_000),
        );
        touch(&root.join("Cache/big"), &b"x".repeat(400_000));
        let (units, counters) = contract::measured(|| run(root));
        assert!(!units.is_empty());
        assert_eq!(
            counters.header_bytes_read, 0,
            "with no workspace.json to read, nothing may be read at all"
        );
        contract::within_header_cap(counters, 0);
    }

    #[test]
    fn protected_categories_default_protected() {
        // Windsurf's agent/MCP configuration lives under
        // `~/.codeium/windsurf`, a separate undecomposed external unit,
        // so this profile root produces no default-protected category.
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        touch(&root.join("User/globalStorage/state.vscdb"), b"sqlite");
        touch(&root.join("logs/main.log"), b"x");
        contract::protection_defaults_hold_with_no_protected_category(&run(root), WINDSURF_TOOL_ID);
    }

    #[test]
    fn project_link_is_declared_or_unresolved_never_basename_guess() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let repo = root.join("declared-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        touch(
            &root.join("User/workspaceStorage/w1/workspace.json"),
            format!("{{\"folder\":\"file://{}\"}}", repo.display()).as_bytes(),
        );
        touch(
            &root.join("User/workspaceStorage/w1/state.vscdb"),
            b"sqlite",
        );
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
        assert!(matches!(
            declared.project_link(),
            crate::agents::ProjectLinkState::Linked {
                source: crate::agents::LinkSource::Declared,
                ..
            }
        ));
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
