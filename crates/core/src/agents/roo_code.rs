//! Roo Code identification (#99): a thin wrapper over
//! `crate::agents::vscode_family::identify_extension_globalstorage`,
//! applied to each host location
//! `crate::locations::roo_code::RooCodeDetector` resolves. Same
//! per-host, never-merged discipline as `crate::agents::cline`, declared
//! through this adapter's own `decomposes_every_location` capability.
//!
//! ## Per-task project linkage, corrected and now confirmed
//!
//! Verified 2026-09-21 against `RooCodeInc/Roo-Code` main
//! @ `b867ec9145750d0ae1ff7f02d35406e9bf2a0b16`:
//!
//! * `src/shared/globalFileNames.ts` confirms the per-task files
//!   (`api_conversation_history.json`, `ui_messages.json`,
//!   `task_metadata.json`, `history_item.json`, `_index.json`) and
//!   `src/package.json` the `rooveterinaryinc.roo-cline` extension id.
//! * `src/core/context-tracking/FileContextTrackerTypes.ts` defines
//!   `TaskMetadata` as `{ files_in_context }` only -- so the
//!   `task_metadata.json` `workspace` field this adapter inherited does
//!   not exist.
//! * The workspace *is* recorded per task, in
//!   `tasks/<id>/history_item.json`:
//!   `src/core/task-persistence/taskMetadata.ts` builds a `HistoryItem`
//!   with a `workspace` field and
//!   `src/core/task-persistence/TaskHistoryStore.ts` writes it there,
//!   plus an index at `tasks/_index.json` with a `getByWorkspace`
//!   filter. That is what this adapter now reads, bounded.
//!
//! One caveat worth carrying: `src/utils/storage.ts`'s
//! `getTaskDirectoryPath()` resolves under the VS Code global storage
//! path **unless** the user sets the `roo-cline.customStoragePath`
//! setting, in which case `tasks/` lives somewhere this catalog's
//! detector does not look. A relocated store is simply not found --
//! never miscounted.
//!
//! Also restated from `crate::locations::roo_code`'s doc comment: a task
//! directory has been community-reported to embed a full Git checkpoint
//! repository, so a task's folded byte total is not necessarily small.
//! This adapter does not special-case that; `SessionRemoval`'s existing
//! loss warning already covers "removes this session's
//! resume/rewind/checkpoint history" generically
//! (`crate::actions::unit_from_agent`).

use super::{
    AdapterCapabilities, AgentAdapter, CandidateAgentUnit, IdentifyCtx,
    vscode_family::{EXTENSION_CAPABILITIES, TaskLinkSource},
};
use std::path::Path;

pub const ROO_CODE_TOOL_ID: &str = "roo-code";

/// The confirmed per-task linkage file and field (see the module docs).
const TASK_LINK: TaskLinkSource = TaskLinkSource::DeclaredField {
    file: "history_item.json",
    field: "workspace",
};

pub struct Adapter;

impl AgentAdapter for Adapter {
    fn id(&self) -> &'static str {
        ROO_CODE_TOOL_ID
    }
    fn name(&self) -> &'static str {
        "Roo Code"
    }
    fn capabilities(&self) -> AdapterCapabilities {
        EXTENSION_CAPABILITIES
    }
    fn identify(&self, home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
        identify(home, ctx)
    }
}

pub fn identify(ext_home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
    super::vscode_family::identify_extension_globalstorage(ext_home, ctx, TASK_LINK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{
        AgentActionCapability, IdentificationCache, LinkSource, ProjectLinkState, contract,
    };
    use std::fs;

    fn touch(path: &Path, content: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn remote_host(dir: &Path) -> std::path::PathBuf {
        dir.join(".vscode-server/data/User/globalStorage/rooveterinaryinc.roo-cline")
    }

    fn run(ext_home: &Path) -> Vec<CandidateAgentUnit> {
        let cache = IdentificationCache::disabled();
        identify(ext_home, &IdentifyCtx::new(1, &cache))
    }

    #[test]
    fn a_task_under_the_remote_host_is_labelled() {
        let dir = tempfile::tempdir().unwrap();
        let ext_home = remote_host(dir.path());
        touch(&ext_home.join("tasks/t1/ui_messages.json"), b"[]");
        let units = run(&ext_home);
        assert_eq!(units.len(), 1);
        assert!(
            units[0]
                .relative_path()
                .starts_with("VS Code Server (remote)/tasks/")
        );
        assert_eq!(units[0].action(), AgentActionCapability::SessionRemoval);
    }

    #[test]
    fn every_host_is_decomposed_not_deduplicated() {
        assert!(Adapter.capabilities().decomposes_every_location);
    }

    // --- the five contract tests ---------------------------------------

    #[test]
    fn unknown_format_is_explicit_not_empty() {
        let dir = tempfile::tempdir().unwrap();
        let ext_home = remote_host(dir.path());
        touch(&ext_home.join("unrelated.json"), b"{}");
        let units = run(&ext_home);
        assert_eq!(units.len(), 1);
        assert!(
            units[0]
                .relative_path()
                .ends_with("(unsupported layout version)"),
            "{}",
            units[0].relative_path()
        );
        assert!(
            units[0]
                .note
                .as_deref()
                .is_some_and(|n| n.contains("no tasks/ directory found")),
            "{:?}",
            units[0].note
        );
    }

    #[test]
    fn canary_content_never_appears_in_output() {
        let canary = "CANARY-ROO-DO-NOT-LEAK-6d02";
        let dir = tempfile::tempdir().unwrap();
        let ext_home = remote_host(dir.path());
        let repo = dir.path().join("fixture-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        // The canary sits in the *same file* the adapter reads for
        // linkage, beside the field it wants: a bounded read that
        // carried the object out would be caught here.
        touch(
            &ext_home.join("tasks/t1/history_item.json"),
            format!(
                "{{\"workspace\":\"{}\",\"task\":\"{canary}\"}}",
                repo.display()
            )
            .as_bytes(),
        );
        touch(
            &ext_home.join("tasks/t1/api_conversation_history.json"),
            format!("[{{\"content\":\"{canary}\"}}]").as_bytes(),
        );
        let units = run(&ext_home);
        assert!(matches!(
            units[0].project_link(),
            ProjectLinkState::Linked { .. }
        ));
        contract::no_content_leak(&units, canary);
    }

    #[test]
    fn identification_reads_no_more_than_header_cap() {
        let dir = tempfile::tempdir().unwrap();
        let ext_home = remote_host(dir.path());
        for i in 0..100 {
            touch(
                &ext_home.join(format!("tasks/t{i}/api_conversation_history.json")),
                &b"x".repeat(100_000),
            );
            touch(
                &ext_home.join(format!("tasks/t{i}/history_item.json")),
                b"{\"workspace\":\"/nope\"}",
            );
        }
        let (units, counters) = contract::measured(|| run(&ext_home));
        assert_eq!(units.len(), 100);
        contract::within_header_cap(counters, 100);
        assert!(
            counters.header_bytes_read < 100 * 100_000,
            "{} bytes means a conversation file was read",
            counters.header_bytes_read
        );
    }

    #[test]
    fn protected_categories_default_protected() {
        let dir = tempfile::tempdir().unwrap();
        let ext_home = remote_host(dir.path());
        touch(&ext_home.join("tasks/t1/ui_messages.json"), b"[]");
        contract::protection_defaults_hold_with_no_protected_category(
            &run(&ext_home),
            ROO_CODE_TOOL_ID,
        );
    }

    #[test]
    fn project_link_is_declared_or_unresolved_never_basename_guess() {
        let dir = tempfile::tempdir().unwrap();
        let ext_home = remote_host(dir.path());
        let declared_repo = dir.path().join("declared-repo");
        fs::create_dir_all(declared_repo.join(".git")).unwrap();
        // (a) declared in Roo Code's own history_item.json.
        touch(
            &ext_home.join("tasks/t1/history_item.json"),
            format!("{{\"workspace\":\"{}\"}}", declared_repo.display()).as_bytes(),
        );
        // (b) a task directory *named* after a real checkout, with the
        // field only in the file upstream does not put it in.
        let named = dir.path().join("basename-only-repo");
        fs::create_dir_all(named.join(".git")).unwrap();
        touch(
            &ext_home.join("tasks/basename-only-repo/task_metadata.json"),
            format!("{{\"workspace\":\"{}\"}}", named.display()).as_bytes(),
        );
        let units = run(&ext_home);
        let linked = units
            .iter()
            .find(|u| u.relative_path().ends_with("/t1"))
            .expect("declared task identified");
        let ProjectLinkState::Linked { source, .. } = &linked.project_link() else {
            panic!("expected Linked, got {:?}", linked.project_link());
        };
        assert_eq!(*source, LinkSource::Declared);
        let guessed = units
            .iter()
            .find(|u| u.relative_path().ends_with("/basename-only-repo"))
            .expect("named task identified");
        let ProjectLinkState::Unresolved { reason } = &guessed.project_link() else {
            panic!("expected Unresolved, got {:?}", guessed.project_link());
        };
        assert!(
            reason.contains("history_item.json"),
            "the reason must name the file actually consulted: {reason}"
        );
        contract::linkage_is_declared_or_explicit(&units, "basename-only-repo");
    }
}
