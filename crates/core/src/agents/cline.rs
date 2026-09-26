//! Cline identification (#99): a thin wrapper over
//! `crate::agents::vscode_family::identify_extension_globalstorage`,
//! applied to each host location
//! `crate::locations::cline::ClineDetector` resolves (#99's explicit
//! "model each host as a separate detector location" -- `identify` is
//! called once per host by `crate::agents::discover_and_measure`'s
//! multi-location iteration, never merged; the iteration is driven by
//! this adapter's own `decomposes_every_location` capability rather than
//! by a tool-id match in the shared layer).
//!
//! ## Linkage: the file is inside the directory this adapter already walks
//!
//! Re-verified 2026-09-22 against `cline/cline` main
//! @ `254f40c4b592d1e662b84f2ba06fe45dca77cab3` (vendored excerpts under
//! `crates/core/tests/fixtures/upstream/cline/254f40c4b5/`):
//!
//! * Task identification is on firm ground.
//!   `apps/vscode/src/sdk/legacy-state-reader.ts:53-74` builds
//!   `tasks/<id>/` with `api_conversation_history.json`,
//!   `ui_messages.json`, `context_history.json` and
//!   `task_metadata.json`.
//! * `task_metadata.json` has **no** `workspace` field
//!   (`apps/vscode/src/core/context/context-tracking/ContextTrackerTypes.ts`:
//!   `{ files_in_context, model_usage, environment_history }`), which is
//!   why the field this adapter once read resolved nothing, ever.
//! * The working directory is `HistoryItem.cwdOnTaskInitialization`
//!   (`apps/vscode/src/shared/HistoryItem.ts:16`, optional) -- **not**
//!   `cwd`, `workspace` or `workspaceFolder`, none of which exist in
//!   that type.
//! * And the store holding it is a plain JSON file *inside the
//!   globalStorage directory this adapter already walks*:
//!   `apps/vscode/src/hosts/vscode/vscode-to-file-migration.ts:25-28`
//!   says taskHistory "is NOT migrated here. It uses its own file-based
//!   storage at `{globalStorageFsPath}/state/taskHistory.json`", and
//!   `legacy-state-reader.ts:42-44` is the executable form:
//!   `path.join(resolveDataDir(dataDir), "state", "taskHistory.json")`.
//!
//! So the previous `Unresolved` reason -- "inside state.vscdb, neither
//! is read here" -- was wrong about where the answer lives, and Cline
//! linkage is achievable with the same bounded-read discipline Roo Code
//! already uses. The file is read **once per host** and indexed by task
//! id, never once per task.
//!
//! ### Upstream's own spelling is ambiguous, and this records it
//!
//! Three spellings, two of which contradict the executable code, all at
//! the same commit:
//!
//! | # | spelling | source |
//! |---|---|---|
//! | 1 | `{globalStorageFsPath}/state/taskHistory.json` | `vscode-to-file-migration.ts:26` (comment) |
//! | 2 | `tasks/taskHistory.json` | `vscode-to-file-migration.ts:67` (comment) |
//! | 3 | `~/.cline/data/tasks/taskHistory.json` | `.clinerules/storage.md` File Layout |
//! | 4 | `<dataDir>/state/taskHistory.json` | `legacy-state-reader.ts:43-44` (**code**) |
//!
//! This adapter follows the code (`state/`), reports the ambiguity in
//! the `Unresolved` reason for a task the file does not mention, and
//! does not guess at the other two. Spelling 3's *root* is separately
//! contradicted by spelling 1, which states that for the VS Code host
//! `globalStorageFsPath` is the VS-Code-managed path, **not**
//! `~/.cline/data`.
//!
//! ### The second root
//!
//! `~/.cline/data` is real and is now a detector location of its own:
//! `CLINE_DATA_DIR`, else `CLINE_DIR/data`, else `~/.cline/data`
//! (`apps/vscode/src/shared/storage/storage-context.ts:93-100` and
//! `sdk/packages/shared/src/storage/paths.ts:151-186`). It holds
//! `globalState.json`, `secrets.json` and `workspaces/<hash>/`, shared
//! by the VS Code, CLI and JetBrains clients -- but, per spelling 1
//! above, *not* the VS Code host's task history.

use super::{
    AdapterCapabilities, AgentAdapter, CandidateAgentUnit, IdentifyCtx,
    vscode_family::{EXTENSION_CAPABILITIES, TaskLinkSource},
};
use std::path::Path;

pub const CLINE_TOOL_ID: &str = "cline";

/// Cline's task history: one JSON array beside the task directories,
/// keyed by task id, with the working directory in
/// `cwdOnTaskInitialization`.
const TASK_LINK: TaskLinkSource = TaskLinkSource::SharedHistoryFile {
    file: "state/taskHistory.json",
    id_field: "id",
    path_field: "cwdOnTaskInitialization",
    ambiguity_note: "upstream spells this store three ways at the same commit \
                     ({globalStorage}/state/taskHistory.json in \
                     vscode-to-file-migration.ts's comment, tasks/taskHistory.json in the same \
                     file's skip list, ~/.cline/data/tasks/taskHistory.json in \
                     .clinerules/storage.md); this reads the one the executable code builds \
                     (legacy-state-reader.ts: <dataDir>/state/taskHistory.json), and \
                     cwdOnTaskInitialization is itself optional upstream",
};

pub struct Adapter;

impl AgentAdapter for Adapter {
    fn id(&self) -> &'static str {
        CLINE_TOOL_ID
    }
    fn name(&self) -> &'static str {
        "Cline"
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
    use crate::agents::{AgentActionCapability, IdentificationCache, ProjectLinkState, contract};
    use std::fs;

    fn touch(path: &Path, content: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn host(dir: &Path) -> std::path::PathBuf {
        dir.join("Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev")
    }

    fn run(ext_home: &Path) -> Vec<CandidateAgentUnit> {
        let cache = IdentificationCache::disabled();
        identify(ext_home, &IdentifyCtx::new(1, &cache))
    }

    #[test]
    fn a_task_is_identified_with_its_host_label() {
        let dir = tempfile::tempdir().unwrap();
        let ext_home = host(dir.path());
        touch(
            &ext_home.join("tasks/t1/api_conversation_history.json"),
            b"[]",
        );
        let units = run(&ext_home);
        assert_eq!(units.len(), 1);
        assert!(units[0].relative_path().starts_with("VS Code/tasks/"));
        assert_eq!(units[0].action(), AgentActionCapability::SessionRemoval);
    }

    #[test]
    fn every_host_is_decomposed_not_deduplicated() {
        assert!(
            Adapter.capabilities().decomposes_every_location,
            "Cline storage can exist in several editor hosts at once; merging them would \
             under-report"
        );
    }

    // --- the five contract tests ---------------------------------------

    #[test]
    fn unknown_format_is_explicit_not_empty() {
        let dir = tempfile::tempdir().unwrap();
        let ext_home = host(dir.path());
        touch(&ext_home.join("unrelated.json"), b"{}");
        let units = run(&ext_home);
        assert_eq!(units.len(), 1, "an unrecognized layout must still surface");
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
        let canary = "CANARY-CLINE-DO-NOT-LEAK-2c58";
        let dir = tempfile::tempdir().unwrap();
        let ext_home = host(dir.path());
        touch(
            &ext_home.join("tasks/t1/api_conversation_history.json"),
            format!("[{{\"role\":\"user\",\"content\":\"{canary}\"}}]").as_bytes(),
        );
        touch(
            &ext_home.join("tasks/t1/ui_messages.json"),
            format!("[{{\"text\":\"{canary}\"}}]").as_bytes(),
        );
        touch(
            &ext_home.join("tasks/t1/task_metadata.json"),
            format!("{{\"files_in_context\":[\"{canary}\"]}}").as_bytes(),
        );
        contract::no_content_leak(&run(&ext_home), canary);
    }

    #[test]
    fn identification_reads_no_more_than_header_cap() {
        let dir = tempfile::tempdir().unwrap();
        let ext_home = host(dir.path());
        for i in 0..100 {
            touch(
                &ext_home.join(format!("tasks/t{i}/api_conversation_history.json")),
                &b"x".repeat(100_000),
            );
            touch(
                &ext_home.join(format!("tasks/t{i}/task_metadata.json")),
                b"{\"files_in_context\":[]}",
            );
        }
        let (units, counters) = contract::measured(|| run(&ext_home));
        assert_eq!(units.len(), 100);
        // Cline's linkage is not in any file this adapter reads, so a
        // task costs no content read at all -- which is also why this
        // number cannot creep back up unnoticed.
        assert_eq!(
            counters.header_bytes_read, 0,
            "no Cline task file is read for identification"
        );
        contract::within_header_cap(counters, 0);
    }

    #[test]
    fn protected_categories_default_protected() {
        // An extension's globalStorage directory holds tasks, not the
        // extension's credentials or settings (those are VS Code global
        // state inside state.vscdb), so there is no default-protected
        // category here.
        let dir = tempfile::tempdir().unwrap();
        let ext_home = host(dir.path());
        touch(&ext_home.join("tasks/t1/ui_messages.json"), b"[]");
        contract::protection_defaults_hold_with_no_protected_category(
            &run(&ext_home),
            CLINE_TOOL_ID,
        );
    }

    #[test]
    fn project_link_is_declared_or_unresolved_never_basename_guess() {
        let dir = tempfile::tempdir().unwrap();
        let ext_home = host(dir.path());
        let repo = dir.path().join("basename-only-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        // A task directory named after a real checkout, and a
        // `task_metadata.json` carrying a `workspace` field pointing at
        // it: neither may produce a link. The directory name is a
        // basename guess, and `workspace` is in no Cline schema.
        touch(
            &ext_home.join("tasks/basename-only-repo/task_metadata.json"),
            format!("{{\"workspace\":\"{}\"}}", repo.display()).as_bytes(),
        );
        let units = run(&ext_home);
        let ProjectLinkState::Unresolved { reason } = &units[0].project_link() else {
            panic!("expected Unresolved, got {:?}", units[0].project_link());
        };
        assert!(
            reason.contains("cwdOnTaskInitialization") && reason.contains("state/taskHistory.json"),
            "the reason must name the field and the store this adapter looked in: {reason}"
        );
        assert!(
            reason.contains(".clinerules/storage.md"),
            "upstream's disagreement about the path is part of the answer: {reason}"
        );
        contract::linkage_is_declared_or_explicit(&units, "basename-only-repo");
    }

    /// The linkage the 2026-09-22 re-review showed was available all
    /// along: `{globalStorage}/state/taskHistory.json` is a plain JSON
    /// file *inside the directory this adapter already walks*
    /// (`apps/vscode/src/sdk/legacy-state-reader.ts:42-44` @
    /// `254f40c4b592d1e662b84f2ba06fe45dca77cab3`, vendored at
    /// `crates/core/tests/fixtures/upstream/cline/254f40c4b5/legacy-state-reader.ts`).
    #[test]
    fn a_task_in_the_history_file_resolves_to_its_declared_project() {
        let dir = tempfile::tempdir().unwrap();
        let ext_home = host(dir.path());
        let repo = dir.path().join("a-real-checkout");
        fs::create_dir_all(repo.join(".git")).unwrap();
        touch(&ext_home.join("tasks/t-linked/ui_messages.json"), b"[]");
        touch(&ext_home.join("tasks/t-absent/ui_messages.json"), b"[]");
        touch(
            &ext_home.join("state/taskHistory.json"),
            format!(
                "[{{\"id\":\"t-linked\",\"ts\":1,\"task\":\"CANARY-PROMPT-TEXT\",\
                 \"cwdOnTaskInitialization\":\"{}\"}},\
                 {{\"id\":\"t-absent\",\"ts\":2,\"task\":\"CANARY-PROMPT-TEXT\"}}]",
                repo.display()
            )
            .as_bytes(),
        );
        let units = run(&ext_home);
        let linked = units
            .iter()
            .find(|u| u.relative_path().ends_with("tasks/t-linked"))
            .unwrap();
        assert!(
            matches!(&linked.project_link(), ProjectLinkState::Linked { .. }),
            "a task whose history entry declares a cwd must link: {:?}",
            linked.project_link()
        );
        // The optional field really is optional upstream; absent is
        // `Unresolved`, never a guess from the task id.
        let absent = units
            .iter()
            .find(|u| u.relative_path().ends_with("tasks/t-absent"))
            .unwrap();
        assert!(matches!(
            &absent.project_link(),
            ProjectLinkState::Unresolved { .. }
        ));
        // And the prompt text that sits in the same file next to the
        // path must never reach a unit.
        let serialized = format!("{units:?}");
        assert!(
            !serialized.contains("CANARY-PROMPT-TEXT"),
            "task titles live in taskHistory.json beside the cwd; only the cwd may be read"
        );
    }

    /// The history file is read **once per host**, not once per task:
    /// otherwise linkage would cost one capped read per task and the
    /// 5,000-task case would be exactly the cost this catalog exists to
    /// avoid.
    #[test]
    fn the_history_file_is_read_once_per_host_not_once_per_task() {
        let dir = tempfile::tempdir().unwrap();
        let ext_home = host(dir.path());
        for i in 0..25 {
            touch(
                &ext_home.join(format!("tasks/t{i}/ui_messages.json")),
                b"[]",
            );
        }
        let history = "[{\"id\":\"t0\",\"ts\":1}]";
        touch(&ext_home.join("state/taskHistory.json"), history.as_bytes());
        let (_, counted) = contract::measured(|| run(&ext_home));
        assert_eq!(
            counted.header_bytes_read,
            history.len() as u64,
            "exactly one read of the shared history file, whatever the task count"
        );
    }
}
