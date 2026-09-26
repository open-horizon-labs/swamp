//! GitHub Copilot CLI identification (#97): session/workspace artifacts,
//! command-recall history, a cross-session SQLite store, logs and
//! protected configuration under the home
//! `crate::locations::copilot_cli::CopilotCliDetector` resolves first.
//!
//! Layout sourced directly from GitHub's own reference page (fetched
//! during implementation, never from a real `~/.copilot` on this machine
//! -- PRIVACY IS A HARD RULE):
//! <https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference>,
//! current as of this chunk -- see `crate::locations::copilot_cli`'s doc
//! comment for the full item-by-item citation, including the correction
//! of #97's own issue-text guess (`history-session-state/`) to the real
//! `session-state/`/`command-history-state/` names.
//! Session linkage reads only the bounded, cached `workspace.yaml` metadata.
//! Transcript bodies and arbitrary adjacent JSON files are never searched.
//! Exact session directories can be removed independently of project linkage.

use super::{
    AdapterCapabilities, AgentActionCapability, AgentAdapter, AgentCategory, AgentMember,
    AgentMemberKind, AgentUnitBuilder, CandidateAgentUnit, IdentifyCtx, ProjectLinkState,
    mtime_secs,
};
use std::path::{Path, PathBuf};

pub const COPILOT_CLI_TOOL_ID: &str = "github-copilot-cli";

const MAX_FOLD_ENTRIES: usize = 200_000;

const PROTECTED_CONFIG_FILES: &[(&str, &str)] = &[
    (
        "config.json",
        "automatically managed application state (authentication, plugins)",
    ),
    ("settings.json", "primary user configuration"),
    ("mcp-config.json", "user-level MCP server definitions"),
    (
        "lsp-config.json",
        "user-level Language Server Protocol definitions",
    ),
    (
        "permissions-config.json",
        "saved tool and directory permissions per project",
    ),
    (
        "providers.json",
        "bring-your-own-key provider/model registry",
    ),
    (
        "copilot-instructions.md",
        "personal instructions applied across all sessions",
    ),
];
const PROTECTED_CONFIG_DIRS: &[(&str, &str)] = &[
    ("instructions", "additional cross-session instructions"),
    ("agents", "custom agent definitions"),
    ("hooks", "user-level hook scripts"),
    ("skills", "personal custom skill definitions"),
    ("extensions", "user-level extension files"),
    ("installed-plugins", "installed plugin files"),
    ("plugin-data", "persistent plugin data"),
    (
        "mcp-oauth-config",
        "MCP OAuth tokens and registration fallback storage",
    ),
    ("mcp-secrets", "MCP secret placeholders and index"),
];
const FORMAT_MARKERS: &[&str] = &[
    "settings.json",
    "config.json",
    "session-state",
    "session-store.db",
    "logs",
];

pub struct Adapter;

impl AgentAdapter for Adapter {
    fn id(&self) -> &'static str {
        COPILOT_CLI_TOOL_ID
    }
    fn name(&self) -> &'static str {
        "GitHub Copilot CLI"
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::default()
    }
    fn identify(&self, home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
        identify(home, ctx)
    }
}

pub fn identify(home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
    if !ctx.is_dir(home) {
        return Vec::new();
    }
    if !FORMAT_MARKERS.iter().any(|rel| ctx.exists(&home.join(rel))) {
        return unknown_format_residual(home, ctx);
    }
    let mut units = Vec::new();
    identify_protected(home, ctx, &mut units);
    identify_session_state(home, ctx, &mut units);
    identify_command_history(home, ctx, &mut units);
    identify_session_store(home, ctx, &mut units);
    identify_logs(home, ctx, &mut units);
    identify_ide_state(home, ctx, &mut units);
    identify_residual(home, ctx, &mut units);
    units
}

fn unknown_format_residual(home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
    if !ctx.has_entries(home) {
        return Vec::new();
    }
    let (bytes, mtime, _t) = ctx.folded_bytes(home, MAX_FOLD_ENTRIES);
    vec![
        AgentUnitBuilder::new(
            COPILOT_CLI_TOOL_ID,
            AgentCategory::Unclassified,
            home.to_path_buf(),
        )
        .relative_path("(unknown format)")
        .bytes(bytes)
        .mtime_max(mtime)
        .project_link(ProjectLinkState::NotApplicable)
        .action(AgentActionCapability::None)
        .note(
            "no GitHub Copilot CLI content markers found (settings.json/config.json/\
             session-state/session-store.db/logs) at this resolved path; this directory may \
             belong to a different tool, be empty, or use an unsupported version -- treated as \
             unknown format, not scanned further",
        )
        .build(),
    ]
}

fn identify_protected(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    for (rel, note) in PROTECTED_CONFIG_FILES {
        let path = home.join(rel);
        let Ok(meta) = ctx.stat(&path) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        out.push(protected_unit(rel, path, meta.len(), mtime_secs(&meta), note).build());
    }
    for (rel, note) in PROTECTED_CONFIG_DIRS {
        let path = home.join(rel);
        if !ctx.is_dir(&path) {
            continue;
        }
        let (bytes, mtime, _t) = ctx.folded_bytes(&path, MAX_FOLD_ENTRIES);
        out.push(protected_unit(rel, path, bytes, mtime, note).build());
    }
}

/// The shared shape of every documented protected file/directory: the
/// category's own default protection plus this entry's specific stated
/// reason, which is the message the existing rows already carried.
fn protected_unit(
    rel: &str,
    path: PathBuf,
    bytes: u64,
    mtime: u64,
    note: &str,
) -> AgentUnitBuilder {
    AgentUnitBuilder::new(COPILOT_CLI_TOOL_ID, AgentCategory::ProtectedConfig, path)
        .relative_path(rel)
        .bytes(bytes)
        .mtime_max(mtime)
        .project_link(ProjectLinkState::NotApplicable)
        .action(AgentActionCapability::None)
        .protect(note)
}

/// SDK workspace-file evidence: github/copilot-sdk @
/// 4001c1da7d832c51bad1d38619c1a082af390efb,
/// nodejs/test/e2e/session_fs.e2e.test.ts. The cwd scalar layout is reproduced
/// in github/copilot-cli#2446 (1.0.14), corroborated by copilot-sdk#1735.
/// These sources do not establish a universal schema across CLI versions.
const NO_LINKAGE_SOURCE_REASON: &str =
    "workspace.yaml has no supported absolute cwd; arbitrary session files are not searched";

/// Only the observed single-line top-level scalar forms, not a general YAML
/// parser. Reject unsupported forms instead of manufacturing a project path.
fn workspace_cwd(text: &str) -> Option<String> {
    let mut matches = text.lines().filter_map(|line| line.strip_prefix("cwd:"));
    let raw = matches.next()?.trim();
    if matches.next().is_some() || !text.ends_with('\n') {
        return None;
    }
    let value = if raw.starts_with('"') {
        serde_json::from_str::<String>(raw).ok()?
    } else if raw.starts_with('\'') && raw.ends_with('\'') && raw.len() >= 2 {
        raw[1..raw.len() - 1].replace("''", "'")
    } else {
        if raw.contains(" #") || raw.contains(": ") {
            return None;
        }
        raw.to_string()
    };
    Path::new(&value).is_absolute().then_some(value)
}

fn identify_session_state(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    let base = home.join("session-state");
    for entry in ctx.list(&base) {
        let path = base.join(&entry.name);
        let (bytes, mtime, truncated) = if entry.is_dir {
            ctx.folded_bytes(&path, MAX_FOLD_ENTRIES)
        } else {
            let Ok(meta) = ctx.stat(&path) else {
                continue;
            };
            (meta.len(), mtime_secs(&meta), false)
        };
        let declared = entry
            .is_dir
            .then(|| {
                ctx.derived(
                    COPILOT_CLI_TOOL_ID,
                    "workspace-yaml-cwd-v1",
                    &path.join("workspace.yaml"),
                    8192,
                    &workspace_cwd,
                )
            })
            .flatten();
        let project_link = super::resolve_declared_path(declared, NO_LINKAGE_SOURCE_REASON);
        let member_kind = if entry.is_dir {
            AgentMemberKind::SessionData
        } else {
            AgentMemberKind::Transcript
        };
        // GitHub's directory reference (checked 2026-09-26) documents
        // session-ID directories with events.jsonl and workspace artifacts.
        // A confirmed directory is an exact local removal boundary even when
        // its project's identity is unknown. Never read transcript contents.
        let action = if entry.is_dir && ctx.is_file(&path.join("events.jsonl")) {
            AgentActionCapability::SessionRemoval
        } else {
            AgentActionCapability::None
        };
        let mut unit =
            AgentUnitBuilder::new(COPILOT_CLI_TOOL_ID, AgentCategory::Sessions, path.clone())
                .relative_to(home)
                .bytes(bytes)
                .members_keep_bytes(vec![AgentMember {
                    path,
                    bytes,
                    kind: member_kind,
                }])
                .mtime_max(mtime)
                .project_link(project_link)
                .action(action);
        if truncated {
            unit = unit.incomplete("directory entry count bound reached");
        }
        out.push(unit.build());
    }
}

fn identify_command_history(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    let path = home.join("command-history-state");
    if !ctx.is_dir(&path) {
        return;
    }
    let (bytes, mtime, truncated) = ctx.folded_bytes(&path, MAX_FOLD_ENTRIES);
    let unit = AgentUnitBuilder::new(COPILOT_CLI_TOOL_ID, AgentCategory::LocalHistory, path)
        .relative_path("command-history-state")
        .bytes(bytes)
        .mtime_max(mtime)
        .project_link(ProjectLinkState::NotApplicable)
        .action(AgentActionCapability::CacheOrLogTrash)
        .note("reverse-search command recall history, not conversation content");
    let unit = if truncated {
        unit.incomplete("directory entry count bound reached")
    } else {
        unit
    };
    out.push(unit.build());
}

fn identify_session_store(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    let db = home.join("session-store.db");
    let Ok(meta) = ctx.stat(&db) else {
        return;
    };
    if !meta.is_file() {
        return;
    }
    let mut members = vec![AgentMember {
        path: db.clone(),
        bytes: meta.len(),
        kind: AgentMemberKind::Database,
    }];
    let mut bytes = meta.len();
    let mut mtime_max = mtime_secs(&meta);
    for ext in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{ext}", db.display()));
        if let Ok(sm) = ctx.stat(&sidecar)
            && sm.is_file()
        {
            bytes += sm.len();
            mtime_max = mtime_max.max(mtime_secs(&sm));
            members.push(AgentMember {
                path: sidecar,
                bytes: sm.len(),
                kind: AgentMemberKind::Database,
            });
        }
    }
    out.push(
        AgentUnitBuilder::new(COPILOT_CLI_TOOL_ID, AgentCategory::Sessions, db)
            .relative_path("session-store.db")
            .bytes(bytes)
            .members_keep_bytes(members)
            .mtime_max(mtime_max)
            .project_link(ProjectLinkState::NotApplicable)
            .action(AgentActionCapability::None)
            .protect(
                "SQLite database for cross-session data; metadata-only, never opened while writable",
            )
            .build(),
    );
}

fn identify_logs(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    let path = home.join("logs");
    if !ctx.is_dir(&path) {
        return;
    }
    let (bytes, mtime, truncated) = ctx.folded_bytes(&path, MAX_FOLD_ENTRIES);
    let mut unit = AgentUnitBuilder::new(COPILOT_CLI_TOOL_ID, AgentCategory::Logs, path)
        .relative_path("logs")
        .bytes(bytes)
        .mtime_max(mtime)
        .project_link(ProjectLinkState::NotApplicable)
        .action(AgentActionCapability::CacheOrLogTrash);
    if truncated {
        unit = unit.incomplete("directory entry count bound reached");
    }
    out.push(unit.build());
}

/// `ide/` (IDE integration state and lock files) is deliberately not
/// actionable this chunk: a lock file backing an active IDE integration
/// is a real corruption risk if moved out from under it, and this
/// adapter has no documented way to tell which entries are safely idle.
fn identify_ide_state(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    let path = home.join("ide");
    if !ctx.is_dir(&path) {
        return;
    }
    let (bytes, mtime, truncated) = ctx.folded_bytes(&path, MAX_FOLD_ENTRIES);
    let unit = AgentUnitBuilder::new(COPILOT_CLI_TOOL_ID, AgentCategory::Unclassified, path)
        .relative_path("ide")
        .bytes(bytes)
        .mtime_max(mtime)
        .project_link(ProjectLinkState::NotApplicable)
        .action(AgentActionCapability::None)
        .note(
            "IDE integration state and lock files; not offered as a supported action this \
             chunk -- a lock file backing an active integration could be corrupted by \
             removal, and this adapter has no documented way to tell which entries are idle",
        );
    let unit = if truncated {
        unit.incomplete("directory entry count bound reached")
    } else {
        unit
    };
    out.push(unit.build());
}

fn identify_residual(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (rel, _) in PROTECTED_CONFIG_FILES {
        seen.insert(rel);
    }
    for (rel, _) in PROTECTED_CONFIG_DIRS {
        seen.insert(rel);
    }
    for rel in [
        "session-state",
        "command-history-state",
        "session-store.db",
        "session-store.db-wal",
        "session-store.db-shm",
        "logs",
        "ide",
    ] {
        seen.insert(rel);
    }
    let mut residual_bytes = 0u64;
    let mut residual_mtime = 0u64;
    let mut residual_names: Vec<String> = Vec::new();
    for e in ctx.list(home) {
        let name = e.name;
        if seen.contains(name.as_str()) {
            continue;
        }
        let (bytes, mtime, _t) = ctx.folded_bytes(&home.join(&name), MAX_FOLD_ENTRIES);
        residual_bytes += bytes;
        residual_mtime = residual_mtime.max(mtime);
        residual_names.push(name);
    }
    if !residual_names.is_empty() {
        residual_names.sort();
        out.push(
            AgentUnitBuilder::new(
                COPILOT_CLI_TOOL_ID,
                AgentCategory::Unclassified,
                home.to_path_buf(),
            )
            .relative_path("(unclassified residual)")
            .bytes(residual_bytes)
            .mtime_max(residual_mtime)
            .project_link(ProjectLinkState::NotApplicable)
            .action(AgentActionCapability::None)
            .note(format!(
                "entries with no specific rule in this adapter: {}",
                residual_names.join(", ")
            ))
            .build(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{IdentificationCache, bounded_io, contract};
    use std::fs;
    use std::time::{Duration, SystemTime};

    fn run(home: &Path) -> Vec<CandidateAgentUnit> {
        let cache = IdentificationCache::disabled();
        identify(home, &IdentifyCtx::new(1, &cache))
    }

    fn touch(path: &Path, content: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn empty_home_yields_no_units() {
        let dir = tempfile::tempdir().unwrap();
        assert!(run(dir.path()).is_empty());
    }

    #[test]
    fn no_markers_yields_unknown_format_not_a_guess() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("unrelated.txt"), b"hello");
        let units = run(dir.path());
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].relative_path(), "(unknown format)");
    }

    #[test]
    fn config_json_is_protected_by_default() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("config.json"), b"{}");
        let units = run(home);
        let u = units
            .iter()
            .find(|u| u.relative_path() == "config.json")
            .unwrap();
        assert!(u.protected());
        assert_eq!(u.action(), AgentActionCapability::None);
    }

    #[test]
    fn mcp_secrets_dir_is_protected() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        touch(&home.join("mcp-secrets").join("index.json"), b"{}");
        let units = run(home);
        let u = units
            .iter()
            .find(|u| u.relative_path() == "mcp-secrets")
            .unwrap();
        assert!(u.protected());
    }

    #[test]
    fn workspace_yaml_reads_only_evidenced_top_level_path() {
        assert_eq!(
            workspace_cwd("id: session\ncwd: /tmp/project\nsummary: private\n"),
            Some("/tmp/project".into())
        );
        assert_eq!(
            workspace_cwd("cwd: \"/tmp/a b\"\n"),
            Some("/tmp/a b".into())
        );
        assert_eq!(
            workspace_cwd("cwd: '/tmp/it''s a project'\n"),
            Some("/tmp/it's a project".into())
        );
        for text in [
            "cwd: relative\n",
            "summary:\n  cwd: /tmp/fake\n",
            "cwd: |\n  /tmp/fake\n",
            "cwd: /tmp/a\ncwd: /tmp/b\n",
            "cwd: /tmp/truncated",
            "cwd: /tmp/a # comment\n",
        ] {
            assert!(workspace_cwd(text).is_none(), "{text}");
        }
    }

    #[test]
    fn unchanged_workspace_metadata_is_not_reread() {
        let home = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        touch(
            &home.path().join("session-state/s/workspace.yaml"),
            b"id: s\ncwd: /nonexistent/copilot-project\n",
        );
        let cache = IdentificationCache::load(store.path());
        let (_, cold) =
            crate::work_counters::measured(|| identify(home.path(), &IdentifyCtx::new(1, &cache)));
        assert!(cold.header_bytes_read > 0);
        cache.save(store.path(), 1).unwrap();
        let warm_cache = IdentificationCache::load(store.path());
        let (units, warm) = crate::work_counters::measured(|| {
            identify(home.path(), &IdentifyCtx::new(2, &warm_cache))
        });
        assert_eq!(warm.header_bytes_read, 0);
        assert!(matches!(
            units
                .iter()
                .find(|u| u.category() == AgentCategory::Sessions)
                .unwrap()
                .project_link(),
            ProjectLinkState::Missing { .. }
        ));
    }

    #[test]
    fn workspace_yaml_links_without_reading_events() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        crate::work_counters::record_spawn();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(&repo)
                .status()
                .unwrap()
                .success()
        );
        let home = tmp.path().join("copilot");
        touch(
            &home.join("session-state/s/workspace.yaml"),
            format!("id: s\ncwd: {}\nsummary: PRIVATE-SUMMARY\n", repo.display()).as_bytes(),
        );
        touch(
            &home.join("session-state/s/events.jsonl"),
            b"PRIVATE-TRANSCRIPT",
        );
        let units = run(&home);
        let session = units
            .iter()
            .find(|u| u.category() == AgentCategory::Sessions)
            .unwrap();
        assert!(
            matches!(session.project_link(), ProjectLinkState::Linked { .. }),
            "{:?}",
            session.project_link()
        );
        assert_eq!(session.action(), AgentActionCapability::SessionRemoval);
        assert!(!format!("{units:?}").contains("PRIVATE-"));
    }

    #[test]
    fn every_documented_protected_directory_is_still_protected() {
        // The exact set the reference page lists: a conversion must not
        // quietly drop one of them.
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        for (rel, _) in PROTECTED_CONFIG_DIRS {
            touch(&home.join(rel).join("entry.txt"), b"x");
        }
        for (rel, _) in PROTECTED_CONFIG_FILES {
            touch(&home.join(rel), b"{}");
        }
        let units = run(home);
        for (rel, note) in PROTECTED_CONFIG_DIRS.iter().chain(PROTECTED_CONFIG_FILES) {
            let u = units
                .iter()
                .find(|u| u.relative_path() == *rel)
                .unwrap_or_else(|| panic!("{rel} must still be identified"));
            assert_eq!(u.category(), AgentCategory::ProtectedConfig, "{rel}");
            assert!(u.protected(), "{rel} must be protected");
            assert_eq!(
                u.protect_reason(),
                Some(*note),
                "{rel} must keep its own stated reason"
            );
        }
        assert!(
            !units
                .iter()
                .any(|u| u.relative_path() == "(unclassified residual)"),
            "every documented entry has a rule, so nothing falls through"
        );
    }

    /// A `cwd` that *looks* like the answer is still not a citation.
    ///
    /// This adapter used to parse `cwd`/`workspace`/`workspaceFolder`
    /// out of the small JSON files beside a session, and this test used
    /// to assert it linked. The 2026-09-22 re-review checked the only
    /// cited source -- GitHub's `cli-config-dir-reference` -- and found
    /// zero occurrences of `cwd` or `workspaceFolder`; a re-check across
    /// four pinned `github/docs` pages @
    /// `72e940d15a9aff06b6e84216f3c97dac25c47d9b` found the same, and
    /// the CLI is closed source. So the field names were the guess the
    /// doc promised never to make, and a fixture that contains one
    /// proves only that the fixture was written by the same guess.
    ///
    /// Arbitrary adjacent JSON still must not link. This fixture also lacks
    /// the events.jsonl marker that establishes an exact session boundary.
    #[test]
    fn a_plausible_cwd_field_is_not_a_citation_and_must_not_link() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        let repo = home.join("fixture-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let canary = "CANARY-COPILOT-DO-NOT-LEAK-99aa";
        touch(
            &home.join("session-state/s1/meta.json"),
            format!("{{\"cwd\":\"{}\",\"note\":\"{canary}\"}}", repo.display()).as_bytes(),
        );
        let units = run(home);
        let s = units
            .iter()
            .find(|u| u.category() == AgentCategory::Sessions && u.relative_path().contains("s1"))
            .expect("session unit present");
        let ProjectLinkState::Unresolved { reason } = &s.project_link() else {
            panic!(
                "an undocumented field must not produce a link: {:?}",
                s.project_link()
            );
        };
        assert!(
            reason.contains("workspace.yaml"),
            "the reason must name the supported metadata source: {reason}"
        );
        assert_eq!(
            s.action(),
            AgentActionCapability::None,
            "no selective action on an unrecognized session directory"
        );
        // The bytes are still identified and measured -- the directory
        // layout is confirmed; only the claim about what is inside is
        // withdrawn.
        assert!(s.bytes() > 0, "the session's bytes must still be reported");
        let serialized = format!("{units:?}");
        assert!(!serialized.contains(canary), "content leaked");
    }

    #[test]
    fn session_state_without_metadata_is_unresolved_not_guessed() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        touch(&home.join("session-state/s2/blob.bin"), b"opaque");
        let units = run(home);
        let s = units
            .iter()
            .find(|u| u.relative_path().contains("s2"))
            .unwrap();
        assert!(matches!(
            s.project_link(),
            ProjectLinkState::Unresolved { .. }
        ));
    }

    #[test]
    fn command_history_state_is_actionable() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        touch(
            &home.join("command-history-state/recall.log"),
            b"cmd1\ncmd2",
        );
        let units = run(home);
        let u = units
            .iter()
            .find(|u| u.relative_path() == "command-history-state")
            .unwrap();
        assert!(!u.protected());
        assert_eq!(u.action(), AgentActionCapability::CacheOrLogTrash);
    }

    #[test]
    fn session_store_db_is_protected_and_folds_sidecars() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("session-store.db"), b"sqlite");
        touch(&home.join("session-store.db-wal"), b"wal");
        let units = run(home);
        let db = units
            .iter()
            .find(|u| u.relative_path() == "session-store.db")
            .unwrap();
        assert!(db.protected());
        assert_eq!(db.action(), AgentActionCapability::None);
        assert_eq!(db.members().len(), 2);
    }

    #[test]
    fn ide_state_is_unclassified_and_never_actionable() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        touch(&home.join("ide/lock.json"), b"{}");
        let units = run(home);
        let u = units.iter().find(|u| u.relative_path() == "ide").unwrap();
        assert_eq!(u.action(), AgentActionCapability::None);
        assert!(
            u.note
                .as_deref()
                .is_some_and(|n| n.contains("lock file backing an active integration")),
            "the reason it is not actionable must survive"
        );
    }

    #[test]
    fn logs_are_actionable() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("logs/session.log"), b"debug");
        let units = run(home);
        let u = units.iter().find(|u| u.relative_path() == "logs").unwrap();
        assert_eq!(u.action(), AgentActionCapability::CacheOrLogTrash);
    }

    #[test]
    fn identification_cost_is_bounded_for_many_sessions() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        for i in 0..500 {
            touch(
                &home.join(format!("session-state/s{i}/blob.bin")),
                &b"x".repeat(200_000),
            );
        }
        let start = SystemTime::now();
        let units = run(home);
        let elapsed = SystemTime::now().duration_since(start).unwrap_or_default();
        eprintln!("[measured] copilot_cli identify() over 500 synthetic sessions took {elapsed:?}");
        assert_eq!(
            units
                .iter()
                .filter(|u| u.category() == AgentCategory::Sessions)
                .count(),
            500
        );
        assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
    }

    // --- the five contract tests ---------------------------------------

    #[test]
    fn unknown_format_is_explicit_not_empty() {
        // A non-empty home with none of this tool's markers is reported
        // as an explicit `(unknown format)` row saying which markers
        // were looked for -- never an empty vec, and never scanned as if
        // it were some other tool's layout.
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("unrelated.txt"), b"hello");
        fs::create_dir_all(dir.path().join("some-other-tool")).unwrap();
        let units = run(dir.path());
        assert_eq!(units.len(), 1, "an unrecognized home must still speak");
        assert_eq!(units[0].relative_path(), "(unknown format)");
        assert_eq!(units[0].category(), AgentCategory::Unclassified);
        assert_eq!(units[0].action(), AgentActionCapability::None);
        let note = units[0].note.as_deref().unwrap_or_default();
        assert!(
            note.contains("no GitHub Copilot CLI content markers found")
                && note.contains("not scanned further"),
            "the row must say what was looked for and what was not done: {note}"
        );
        assert!(
            matches!(units[0].project_link(), ProjectLinkState::NotApplicable),
            "an unknown-format home is not a project linkage question"
        );
    }

    #[test]
    fn canary_content_never_appears_in_output() {
        let canary = "CANARY-COPILOT-DO-NOT-LEAK-4d71";
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        // On the first line, which the metadata scan does read...
        touch(
            &home.join("session-state/s1/meta.json"),
            format!("{{\"cwd\":\"/nonexistent\",\"prompt\":\"{canary}\"}}\n").as_bytes(),
        );
        // ...and in a body this adapter must never open at all.
        touch(
            &home.join("session-state/s1/transcript.txt"),
            format!("line one\nuser said: {canary}\n").as_bytes(),
        );
        touch(
            &home.join("logs/session.log"),
            format!("debug\n{canary}\n").as_bytes(),
        );
        let units = run(home);
        assert!(
            units.iter().any(
                |u| u.category() == AgentCategory::Sessions && u.relative_path().contains("s1")
            ),
            "session identified"
        );
        contract::no_content_leak(&units, canary);
    }

    #[test]
    fn identification_reads_no_more_than_header_cap() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        let sessions = 40usize;
        let per_body = 100_000usize;
        for i in 0..sessions {
            // One small declaration file (read, capped) plus a large
            // body (never read).
            touch(
                &home.join(format!("session-state/s{i}/meta.json")),
                b"{\"cwd\":\"/nonexistent\"}",
            );
            touch(
                &home.join(format!("session-state/s{i}/blob.bin")),
                &b"x".repeat(per_body),
            );
        }
        let (units, counters) = contract::measured(|| run(home));
        assert_eq!(
            units
                .iter()
                .filter(|u| u.category() == AgentCategory::Sessions)
                .count(),
            sessions
        );
        // This fixture contains no workspace.yaml. Do not compensate by
        // searching transcript bodies or arbitrary adjacent files.
        assert_eq!(
            counters.header_bytes_read, 0,
            "without workspace.yaml this adapter must read no contents; it read {} bytes",
            counters.header_bytes_read
        );
        assert!(
            counters.header_bytes_read < (sessions * per_body) as u64,
            "identification read a session body's worth of bytes"
        );
        const {
            assert!(
                bounded_io::MAX_HEADER_BYTES <= 64 * 1024,
                "the shared ceiling any future read here must sit under"
            )
        };
        contract::within_header_cap(counters, sessions as u64);
    }

    #[test]
    fn protected_categories_default_protected() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        touch(&home.join("config.json"), b"{}");
        touch(&home.join("mcp-secrets/index.json"), b"{}");
        touch(&home.join("skills/mine/SKILL.md"), b"# redacted");
        touch(&home.join("session-store.db"), b"sqlite");
        let units = run(home);
        contract::protection_defaults_hold(&units);
        let db = units
            .iter()
            .find(|u| u.relative_path() == "session-store.db")
            .expect("the SQLite store is identified");
        assert!(db.protected());
        assert!(
            db.protect_reason()
                .is_some_and(|r| r.contains("never opened while writable")),
            "the adapter's own protection reason must survive the builder"
        );
    }

    #[test]
    fn project_link_is_declared_or_unresolved_never_basename_guess() {
        // (a) a session whose own metadata carries a plausible `cwd`
        // pointing at a real worktree. Not a link: no upstream source
        // documents that field (see
        // `a_plausible_cwd_field_is_not_a_citation_and_must_not_link`).
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = repo_dir.path().join("declared-worktree");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        touch(
            &home.join("session-state/declared/meta.json"),
            format!("{{\"cwd\":\"{}\"}}", repo.display()).as_bytes(),
        );
        // (b) a session directory *named* like a project, declaring
        // nothing: a basename is not evidence.
        touch(
            &home.join("session-state/looks-like-a-project/blob.bin"),
            b"opaque",
        );
        let units = run(home);
        let a = units
            .iter()
            .find(|u| u.relative_path().contains("declared"))
            .unwrap();
        assert!(
            matches!(a.project_link(), ProjectLinkState::Unresolved { .. }),
            "an undocumented field must never become a link: {:?}",
            a.project_link()
        );
        let b = units
            .iter()
            .find(|u| u.relative_path().contains("looks-like-a-project"))
            .unwrap();
        assert!(
            matches!(b.project_link(), ProjectLinkState::Unresolved { .. }),
            "a directory name must never become a link: {:?}",
            b.project_link()
        );
        contract::linkage_is_declared_or_explicit(&units, "looks-like-a-project");
    }
}
