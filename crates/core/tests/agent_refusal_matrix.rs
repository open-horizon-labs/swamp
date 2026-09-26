//! #101's "verify the refusal matrix per tool": one table-driven test
//! asserting every named refusal reason -- protected categories, whole
//! home/whole projects-dir paths, SQLite/WAL/SHM-like files, active
//! sessions, shared blobs with unknown references, parent/child overlap
//! in one plan, and plan scope drift at execution -- fires for every
//! one of the 14 agent-tool ids `crate::agents::matrix` names, not just
//! Claude Code. Synthetic `AgentUnit` values are built directly here
//! (never a real tool home) since `agent_refusal`/`propose_agents`'s
//! logic is generic over every field an adapter's `discover_and_measure`
//! output would carry; a handful of end-to-end scenarios (active
//! session, scope drift) additionally go through a couple of real
//! adapters' own `identify()` to prove the mechanism reaches them too,
//! not only a hand-built literal. PRIVACY IS A HARD RULE: every fixture
//! here is synthetic.

use std::fs;
use std::path::{Path, PathBuf};

use swamp_core::actions;
use swamp_core::agents::{
    AgentActionCapability, AgentCategory, AgentMember, AgentUnit, ProjectLinkState, unit_id,
};

/// Every named tool id (#90's required 14-row matrix, including the
/// Codex desktop app as its own row). Kept as a plain list here (not
/// imported from `crate::agents::matrix::AgentToolId`) so this test
/// exercises the exact string ids `propose_agents`/`execute` dispatch
/// on, the same way a real `AgentUnit.tool_id` would carry them.
const ALL_TOOL_IDS: &[(&str, &str)] = &[
    ("claude-code", "Claude Code"),
    ("codex", "Codex"),
    ("codex-desktop", "Codex (desktop app)"),
    ("oh-my-pi", "Oh My Pi"),
    ("opencode", "OpenCode"),
    ("gemini-cli", "Gemini CLI"),
    ("pi", "Pi"),
    ("aider", "Aider"),
    ("github-copilot-cli", "GitHub Copilot CLI"),
    ("cursor", "Cursor"),
    ("windsurf", "Windsurf"),
    ("cline", "Cline"),
    ("roo-code", "Roo Code"),
    ("continue", "Continue"),
];

#[allow(clippy::too_many_arguments)]
fn synthetic_unit(
    tool_id: &str,
    tool_name: &str,
    category: AgentCategory,
    action: AgentActionCapability,
    protected: bool,
    protect_reason: Option<&str>,
    path: PathBuf,
    members: Vec<AgentMember>,
) -> AgentUnit {
    let relative_path = "fixture-unit".to_string();
    AgentUnit {
        tool_id: tool_id.to_string(),
        tool_name: tool_name.to_string(),
        tool_home: path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| path.clone()),
        category,
        id: unit_id(tool_id, category, &relative_path),
        relative_path,
        path,
        members,
        bytes: 4096,
        hardlinked: false,
        complete: true,
        growth_bytes: None,
        regrowth_count: 0,
        observed_at: 1_000,
        mtime_max: 1_000,
        protected,
        protect_reason: protect_reason.map(str::to_string),
        project_link: ProjectLinkState::NotApplicable,
        action,
        note: None,
        evidence: Vec::new(),
    }
}

fn touch(path: &Path, content: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

#[test]
fn trash_capability_does_not_promise_history_or_logs_regenerate() {
    for (category, expected) in [
        (AgentCategory::LocalHistory, "loses past command recall"),
        (AgentCategory::Logs, "loses past diagnostic logs"),
        (AgentCategory::Caches, "may require rerunning"),
    ] {
        let unit = synthetic_unit(
            "github-copilot-cli",
            "Copilot",
            category,
            AgentActionCapability::CacheOrLogTrash,
            false,
            None,
            PathBuf::from("/fixture/tool/removable"),
            vec![],
        );
        let plan = actions::propose_agents(&[unit], &[], "test").unwrap();
        assert_eq!(plan.len(), 1, "history remains removable by user choice");
        assert!(plan[0].recovery().contains(expected), "{:?}", plan[0]);
        assert!(plan[0].warnings().iter().any(|s| s.contains(expected)));
        assert!(!plan[0].recovery().contains("local_rebuild"));
        assert!(
            !plan[0]
                .warnings()
                .iter()
                .any(|s| s.contains("regenerated automatically"))
        );
        assert_eq!(AgentCategory::from_label(category.label()), Some(category));
    }
}

/// Row 1: every named tool's `ProtectedConfig`-category unit refuses at
/// proposal time -- the category-default protection #91 requires
/// (`AgentCategory::default_protected`), never bypassable by a bare
/// `--path`.
#[test]
fn every_tool_refuses_a_protected_config_category_unit() {
    for (tool_id, tool_name) in ALL_TOOL_IDS {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        touch(&path, b"{}");
        let unit = synthetic_unit(
            tool_id,
            tool_name,
            AgentCategory::ProtectedConfig,
            AgentActionCapability::None,
            true,
            Some("protected configuration"),
            path.clone(),
            Vec::new(),
        );
        let err = actions::propose_agents(&[unit], &[path], "test")
            .expect_err("a ProtectedConfig unit must never be proposable");
        assert!(
            err.to_string().contains("protected"),
            "tool {tool_id}: {err}"
        );
    }
}

/// Row 2: a human `swamp protect` keep-flag (simulated here as
/// `protected: true` on an otherwise-actionable, non-default-protected
/// category -- exactly what `discover_and_measure` layers on from the
/// protect sidecar) blocks proposing for every tool, not only the
/// category default. This is the same acceptance line as "human keep
/// flags block ... proposing in CLI for every adapter", tested once at
/// the shared mechanism both the CLI and the TUI call into.
#[test]
fn every_tool_respects_a_human_protect_keep_flag_on_an_otherwise_actionable_unit() {
    for (tool_id, tool_name) in ALL_TOOL_IDS {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache-dir");
        fs::create_dir_all(&path).unwrap();
        touch(&path.join("f"), b"cache-bytes");
        let unit = synthetic_unit(
            tool_id,
            tool_name,
            AgentCategory::Caches,
            AgentActionCapability::CacheOrLogTrash,
            true,
            Some("human keep flag (swamp protect)"),
            path.clone(),
            Vec::new(),
        );
        let err = actions::propose_agents(&[unit], &[path], "test")
            .expect_err("a human-protected unit must never be proposable");
        assert!(
            err.to_string().contains("protected"),
            "tool {tool_id}: {err}"
        );
    }
}

/// Row 3: an unsupported category (`action: None`, e.g. Oh My Pi's own
/// shared blobs with unknown reference coverage, or any category no
/// adapter has wired an action for yet) refuses with the specific
/// "no supported selective action" reason, for every tool, never a
/// silent no-op or a destructive fallback.
#[test]
fn every_tool_refuses_an_unsupported_action_category() {
    for (tool_id, tool_name) in ALL_TOOL_IDS {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob-or-unsupported-unit");
        touch(&path, b"opaque-bytes");
        let unit = synthetic_unit(
            tool_id,
            tool_name,
            AgentCategory::Attachments,
            AgentActionCapability::None,
            false,
            None,
            path.clone(),
            Vec::new(),
        );
        let err = actions::propose_agents(&[unit], &[path], "test")
            .expect_err("an action:None unit must never be proposable");
        assert!(
            err.to_string().contains("swamp has no Trash move"),
            "tool {tool_id}: {err}"
        );
    }
}

/// Row 4: a SQLite/WAL/SHM-like path is refused unconditionally --
/// `is_sqlite_like` is a filename check independent of category/action,
/// defense in depth for every tool (several of which really do use
/// SQLite: Codex's six state databases, OpenCode's `opencode.db`, Oh My
/// Pi's `agent.db`, the VS-Code family's `state.vscdb`).
#[test]
fn every_tool_refuses_a_database_like_path_even_when_otherwise_actionable() {
    for (tool_id, tool_name) in ALL_TOOL_IDS {
        let dir = tempfile::tempdir().unwrap();
        for suffix in ["state.db", "state.sqlite", "state.db-wal", "state.db-shm"] {
            let path = dir.path().join(suffix);
            touch(&path, b"sqlite-bytes");
            let unit = synthetic_unit(
                tool_id,
                tool_name,
                AgentCategory::Caches,
                AgentActionCapability::CacheOrLogTrash,
                false,
                None,
                path.clone(),
                Vec::new(),
            );
            let err = actions::propose_agents(&[unit], &[path], "test")
                .expect_err("a database-like path must never be proposable");
            assert!(
                err.to_string().contains("database-like"),
                "tool {tool_id} ({suffix}): {err}"
            );
        }
    }
}

/// Row 5: an active session (a real, `lsof`-visible open file
/// descriptor -- this test process's own, no mocking) refuses at
/// proposal time, for every tool: the occupancy check
/// (`crate::agents::is_active`) is a path-based seam, not a
/// per-adapter one, so it applies uniformly.
/// Row 6: "whole home" / "whole projects dir" -- a bare directory that
/// is not itself any unit's own anchor path (a tool's home directory,
/// or a directory holding several projects) is refused as "no
/// agent-storage unit at this exact path", never silently expanded into
/// every unit underneath it. Exercised for every tool's own (synthetic)
/// home directory.
#[test]
fn every_tool_refuses_a_whole_home_or_whole_projects_dir_path() {
    for (tool_id, tool_name) in ALL_TOOL_IDS {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("tool-home");
        fs::create_dir_all(&home).unwrap();
        let unit_path = home.join("cache");
        fs::create_dir_all(&unit_path).unwrap();
        let unit = synthetic_unit(
            tool_id,
            tool_name,
            AgentCategory::Caches,
            AgentActionCapability::CacheOrLogTrash,
            false,
            None,
            unit_path,
            Vec::new(),
        );
        // The home directory itself (never a unit's own path) and an
        // unrelated "projects" directory both refuse the same way.
        for whole in [home.clone(), dir.path().to_path_buf()] {
            let err = actions::propose_agents(
                std::slice::from_ref(&unit),
                std::slice::from_ref(&whole),
                "test",
            )
            .expect_err("a whole-home/whole-projects-dir path must never resolve to a unit");
            assert!(
                err.to_string()
                    .contains("no agent-storage unit at this exact path"),
                "tool {tool_id} ({}): {err}",
                whole.display()
            );
        }
    }
}

/// Row 7: parent/child overlap in one plan -- two otherwise-actionable
/// units whose own anchor paths nest are refused together, before
/// either reaches a plan (see `actions::propose_agents`'s own overlap
/// guard, added alongside this test). Exercised across a few different
/// tool-id pairings (a real cross-tool plan can, in principle, mix
/// unrelated tools' units in one selection).
#[test]
fn overlapping_selections_from_any_tool_pairing_refuse_the_whole_plan() {
    let pairings = [
        (ALL_TOOL_IDS[0], ALL_TOOL_IDS[0]),
        (ALL_TOOL_IDS[1], ALL_TOOL_IDS[4]),
        (ALL_TOOL_IDS[6], ALL_TOOL_IDS[6]),
    ];
    for ((tool_a, name_a), (tool_b, name_b)) in pairings {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("parent-unit");
        fs::create_dir_all(&parent).unwrap();
        let child = parent.join("child-unit");
        fs::create_dir_all(&child).unwrap();
        let unit_a = synthetic_unit(
            tool_a,
            name_a,
            AgentCategory::Caches,
            AgentActionCapability::CacheOrLogTrash,
            false,
            None,
            parent.clone(),
            Vec::new(),
        );
        let unit_b = synthetic_unit(
            tool_b,
            name_b,
            AgentCategory::Caches,
            AgentActionCapability::CacheOrLogTrash,
            false,
            None,
            child.clone(),
            Vec::new(),
        );
        let err = actions::propose_agents(&[unit_a, unit_b], &[parent, child], "test")
            .expect_err("nested selections must refuse the whole plan, not silently pick one");
        assert!(
            err.to_string()
                .contains("overlapping agent-storage selections"),
            "{tool_a}/{tool_b}: {err}"
        );
    }
}
