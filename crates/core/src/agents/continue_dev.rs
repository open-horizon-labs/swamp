//! Continue identification (#99): sessions, generated indexes/caches,
//! anonymized usage-event logs and protected configuration under the
//! home `crate::locations::continue_dev::ContinueDetector` resolves.
//! Module named `continue_dev` -- `continue` is a Rust keyword.
//!
//! `config.yaml`/`config.json` are confirmed by primary docs (see
//! `crate::locations::continue_dev`'s doc comment); `sessions/`,
//! `index/` and `dev_data/` are this catalog's own prior research, not
//! independently re-confirmed this chunk, so this adapter treats their
//! *presence* as a version marker (same discipline every other adapter
//! in this catalog uses) rather than asserting their interior schema is
//! fully known. `sessions/`'s own documented shape ("sessions/
//! <session-id> plus a session index file, separate from the session
//! bodies") is honored by excluding common index-like filenames
//! (`sessions.json`/`index.json`) from per-session identification and
//! protecting them separately instead -- deleting the index alongside a
//! kept session would otherwise corrupt it for every session that
//! remains.
//!
//! ## Per-session workspace linkage (confirmed)
//!
//! Continue records the workspace **per session**: `Session` declares a
//! required `workspaceDirectory: string` (`core/index.d.ts`), written
//! into `sessions/<id>.json` and mirrored into the
//! `sessions/sessions.json` index by `core/util/history.ts`
//! (continuedev/continue main @
//! `5522c6f44ca0ac3528b37244818fbfa39b5af470`). This adapter reads that
//! one field out of a session file's **bounded header** and resolves it
//! through `super::resolve_declared_path` -- declared metadata the tool
//! itself wrote, never a guess from the session id or filename. A
//! session file is a single JSON object rather than a line-per-record
//! transcript, so the field can in principle sit past the bounded
//! read's cap; when it is not in the header the unit stays honestly
//! `Unresolved` with a reason that says so, and nothing reads further.

use super::{
    AdapterCapabilities, AgentActionCapability, AgentAdapter, AgentCategory, AgentMember,
    AgentMemberKind, AgentUnitBuilder, CandidateAgentUnit, IdentifyCtx, ProjectLinkState,
    mtime_secs,
};
use std::path::Path;

pub const CONTINUE_TOOL_ID: &str = "continue";

const MAX_FOLD_ENTRIES: usize = 200_000;
/// Cap for one session file's header read. `Session`'s declaration order
/// in `core/index.d.ts` is `sessionId`, `title`, `workspaceDirectory`,
/// `history`, and `JSON.stringify` preserves it, so the field sits ahead
/// of the conversation body in a file Continue wrote.
const HEADER_READ_BYTES: usize = 8192;
const WORKSPACE_FIELD: &str = "workspaceDirectory";
const NO_WORKSPACE_FIELD_REASON: &str = "no workspaceDirectory field found in the session's bounded header -- a Continue session \
     file is a single JSON object rather than one record per line, so this confirmed field \
     (Session.workspaceDirectory, core/index.d.ts) can sit past the bounded read's cap; this \
     adapter stops at the cap rather than reading the conversation body";
/// Upstream's own fallback value, which is not a path and must never be
/// resolved as one. `core/util/history.ts:105` writes
/// `workspaceDirectory: ""` from the `catch` of `load(sessionId)` @
/// `5522c6f44ca0ac3528b37244818fbfa39b5af470`, so an empty string is an
/// *expected* value on disk: the session genuinely declares nothing, and
/// that is `Unresolved`, never `Missing` (which would claim a path was
/// named and has since disappeared).
const EMPTY_WORKSPACE_REASON: &str = "this session's workspaceDirectory is the empty string, which is what Continue itself \
     writes when it cannot load a session (core/util/history.ts:105) -- an expected value, not \
     a path, so the workspace is unresolved rather than reported missing";
const SESSION_DIR_REASON: &str = "this session is a directory, not the single JSON object core/util/history.ts writes, so it \
     carries no workspaceDirectory header this adapter can read; the workspace is left \
     unresolved rather than guessed from the directory name";
const SESSION_INDEX_NAMES: &[&str] = &["sessions.json", "index.json", "sessions.json.bak"];
const FORMAT_MARKERS: &[&str] = &[
    "config.yaml",
    "config.json",
    "sessions",
    "index",
    "dev_data",
];

pub struct Adapter;

impl AgentAdapter for Adapter {
    fn id(&self) -> &'static str {
        CONTINUE_TOOL_ID
    }
    fn name(&self) -> &'static str {
        "Continue"
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
    identify_sessions(home, ctx, &mut units);
    identify_index(home, ctx, &mut units);
    identify_dev_data(home, ctx, &mut units);
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
            CONTINUE_TOOL_ID,
            AgentCategory::Unclassified,
            home.to_path_buf(),
        )
        .relative_path("(unknown format)")
        .bytes(bytes)
        .mtime_max(mtime)
        .project_link(ProjectLinkState::NotApplicable)
        .action(AgentActionCapability::None)
        .note(
            "no Continue content markers found (config.yaml/config.json/sessions/index/\
             dev_data) at this resolved path; this directory may belong to a different tool, be \
             empty, or use an unsupported version -- treated as unknown format, not scanned \
             further",
        )
        .build(),
    ]
}

fn identify_protected(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    for rel in ["config.yaml", "config.json"] {
        let path = home.join(rel);
        if let Ok(meta) = ctx.stat(&path)
            && meta.is_file()
        {
            out.push(
                AgentUnitBuilder::new(CONTINUE_TOOL_ID, AgentCategory::ProtectedConfig, path)
                    .relative_path(rel)
                    .bytes(meta.len())
                    .mtime_max(mtime_secs(&meta))
                    .protect("main configuration")
                    .project_link(ProjectLinkState::NotApplicable)
                    .action(AgentActionCapability::None)
                    .build(),
            );
        }
    }
}

/// The workspace this session declares, read once per `(size, mtime)`
/// through the memoised bounded reader -- so an unchanged 5,000-session
/// home costs zero header bytes on a second pass.
fn resolve_session_workspace(path: &Path, ctx: &IdentifyCtx) -> ProjectLinkState {
    let declared = ctx.derived(
        CONTINUE_TOOL_ID,
        "workspace-directory",
        path,
        HEADER_READ_BYTES,
        &|text| declared_workspace(text),
    );
    match declared.as_deref() {
        // Present and empty. Handing `""` to `resolve_declared_path`
        // would turn it into `Missing { path: "" }` -- a claim that
        // Continue named a directory which has since disappeared, when
        // in fact Continue named nothing.
        Some("") => ProjectLinkState::Unresolved {
            reason: EMPTY_WORKSPACE_REASON.to_string(),
        },
        _ => super::resolve_declared_path(declared, NO_WORKSPACE_FIELD_REASON),
    }
}

/// Extracts the top-level string `workspaceDirectory` from a session
/// file's bounded header, and nothing else -- never a message, never a
/// title.
fn declared_workspace(text: &str) -> Option<String> {
    // When the whole object fits inside the cap this is exact, and the
    // field is unambiguously the top-level one Continue declares.
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(text) {
        // An empty string is *kept*, not filtered away: upstream writes
        // it deliberately, and "declared empty" and "no field at all"
        // deserve different answers (see `EMPTY_WORKSPACE_REASON`).
        return map
            .get(WORKSPACE_FIELD)
            .and_then(|v| v.as_str())
            .map(str::to_string);
    }
    // A session larger than the cap arrives truncated mid-object, so it
    // cannot be parsed as JSON at all. Scan the bounded text for the
    // field's own key and decode the one JSON string literal that
    // follows it. Still bounded: the text is already capped, and a value
    // that does not close inside it is treated as absent.
    scan_json_string_field(text, WORKSPACE_FIELD)
}

fn scan_json_string_field(text: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\"");
    let mut from = 0usize;
    while let Some(rel) = text[from..].find(&needle) {
        let after = from + rel + needle.len();
        let rest = text[after..].trim_start();
        if let Some(rest) = rest.strip_prefix(':') {
            let rest = rest.trim_start();
            if rest.starts_with('"') {
                let start = text.len() - rest.len();
                let end = json_string_end(text, start)?;
                return serde_json::from_str::<String>(&text[start..end])
                    .ok()
                    .filter(|s| !s.is_empty());
            }
        }
        from = after;
    }
    None
}

/// One past the closing quote of the JSON string literal starting at
/// `start`, or `None` when the capped text ends before it closes.
fn json_string_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

fn identify_sessions(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    let base = home.join("sessions");
    for entry in ctx.list(&base) {
        if SESSION_INDEX_NAMES.contains(&entry.name.as_str()) {
            continue; // handled by identify_index
        }
        let path = base.join(&entry.name);
        let Ok(meta) = ctx.stat(&path) else {
            continue;
        };
        let (bytes, mtime, truncated) = if entry.is_dir {
            ctx.folded_bytes(&path, MAX_FOLD_ENTRIES)
        } else {
            (meta.len(), mtime_secs(&meta), false)
        };
        let (member_kind, link) = if entry.is_dir {
            (
                AgentMemberKind::SessionData,
                ProjectLinkState::Unresolved {
                    reason: SESSION_DIR_REASON.to_string(),
                },
            )
        } else {
            (
                AgentMemberKind::Transcript,
                resolve_session_workspace(&path, ctx),
            )
        };
        let mut builder =
            AgentUnitBuilder::new(CONTINUE_TOOL_ID, AgentCategory::Sessions, path.clone())
                .relative_path(format!("sessions/{}", entry.name))
                .bytes(bytes)
                .mtime_max(mtime)
                .members_keep_bytes(vec![AgentMember {
                    path,
                    bytes,
                    kind: member_kind,
                }])
                .project_link(link)
                .action(AgentActionCapability::SessionRemoval);
        if truncated {
            builder = builder.incomplete("directory entry count bound reached");
        }
        out.push(builder.build());
    }
}

fn identify_index(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    let sessions = home.join("sessions");
    for name in SESSION_INDEX_NAMES {
        let path = sessions.join(name);
        if let Ok(meta) = ctx.stat(&path)
            && meta.is_file()
        {
            out.push(
                AgentUnitBuilder::new(CONTINUE_TOOL_ID, AgentCategory::ProtectedConfig, path)
                    .relative_path(format!("sessions/{name}"))
                    .bytes(meta.len())
                    .mtime_max(mtime_secs(&meta))
                    .protect(
                        "session index, separate from the session bodies; removing it would \
                         corrupt lookups for every session that remains",
                    )
                    .project_link(ProjectLinkState::NotApplicable)
                    .action(AgentActionCapability::None)
                    .build(),
            );
        }
    }
    let index_dir = home.join("index");
    if ctx.is_dir(&index_dir) {
        let (bytes, mtime, truncated) = ctx.folded_bytes(&index_dir, MAX_FOLD_ENTRIES);
        let unit = AgentUnitBuilder::new(CONTINUE_TOOL_ID, AgentCategory::Caches, index_dir)
            .relative_path("index")
            .bytes(bytes)
            .mtime_max(mtime)
            .project_link(ProjectLinkState::NotApplicable)
            .action(AgentActionCapability::CacheOrLogTrash)
            .note("embeddings/tag caches, regenerated on next indexing pass");
        let unit = if truncated {
            unit.incomplete("directory entry count bound reached")
        } else {
            unit
        };
        out.push(unit.build());
    }
}

fn identify_dev_data(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    let path = home.join("dev_data");
    if !ctx.is_dir(&path) {
        return;
    }
    let (bytes, mtime, truncated) = ctx.folded_bytes(&path, MAX_FOLD_ENTRIES);
    let unit = AgentUnitBuilder::new(CONTINUE_TOOL_ID, AgentCategory::Logs, path)
        .relative_path("dev_data")
        .bytes(bytes)
        .mtime_max(mtime)
        .project_link(ProjectLinkState::NotApplicable)
        .action(AgentActionCapability::CacheOrLogTrash)
        .note("anonymized development/usage event logs");
    let unit = if truncated {
        unit.incomplete("directory entry count bound reached")
    } else {
        unit
    };
    out.push(unit.build());
}

fn identify_residual(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    let seen: std::collections::HashSet<&str> = [
        "config.yaml",
        "config.json",
        "config.ts",
        "sessions",
        "index",
        "dev_data",
    ]
    .into_iter()
    .collect();
    let mut residual_bytes = 0u64;
    let mut residual_mtime = 0u64;
    let mut residual_names: Vec<String> = Vec::new();
    for entry in ctx.list(home) {
        if seen.contains(entry.name.as_str()) {
            continue;
        }
        let (bytes, mtime, _t) = ctx.folded_bytes(&home.join(&entry.name), MAX_FOLD_ENTRIES);
        residual_bytes += bytes;
        residual_mtime = residual_mtime.max(mtime);
        residual_names.push(entry.name);
    }
    if !residual_names.is_empty() {
        residual_names.sort();
        out.push(
            AgentUnitBuilder::new(
                CONTINUE_TOOL_ID,
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
    use crate::agents::{IdentificationCache, contract};
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

    /// A session file shaped the way `core/util/history.ts` writes one:
    /// a single JSON object whose `workspaceDirectory` precedes the
    /// conversation body.
    fn session_json(workspace: &str, canary: &str) -> String {
        format!(
            "{{\"sessionId\":\"s1\",\"title\":\"{canary}\",\"workspaceDirectory\":\"{workspace}\",\
             \"history\":[{{\"message\":{{\"role\":\"user\",\"content\":\"{canary}\"}}}}]}}"
        )
    }

    /// Upstream's own empty-string fallback is `unresolved`, never
    /// `missing`. `core/util/history.ts:105` writes
    /// `workspaceDirectory: ""` from the `catch` of `load(sessionId)`
    /// (@ `5522c6f44ca0ac3528b37244818fbfa39b5af470`, vendored at
    /// `crates/core/tests/fixtures/upstream/continue/5522c6f44c/history.ts`),
    /// so it is an expected value on disk. `Missing` would assert that a
    /// path was named and has since gone.
    #[test]
    fn an_empty_workspace_directory_is_unresolved_not_missing() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(
            &home.join("sessions/s-empty.json"),
            b"{\"sessionId\":\"s-empty\",\"title\":\"t\",\"workspaceDirectory\":\"\",\"history\":[]}",
        );
        let units = run(home);
        let session = units
            .iter()
            .find(|u| u.relative_path().ends_with("s-empty.json"))
            .expect("session identified");
        let ProjectLinkState::Unresolved { reason } = &session.project_link() else {
            panic!(
                "an empty workspaceDirectory must be Unresolved, got {:?}",
                session.project_link()
            );
        };
        assert!(
            reason.contains("empty string") && reason.contains("history.ts"),
            "the reason must say it is upstream's own fallback: {reason}"
        );
    }

    #[test]
    fn empty_home_yields_no_units() {
        let dir = tempfile::tempdir().unwrap();
        assert!(run(dir.path()).is_empty());
    }

    #[test]
    fn config_yaml_is_protected() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("config.yaml"), b"models: []");
        let units = run(home);
        let u = units
            .iter()
            .find(|u| u.relative_path() == "config.yaml")
            .unwrap();
        assert!(u.protected());
    }

    #[test]
    fn a_session_is_actionable_and_the_index_is_protected_separately() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let canary = "CANARY-CONTINUE-DO-NOT-LEAK-66gg";
        touch(
            &home.join("sessions/s1.json"),
            format!("{{\"content\":\"{canary}\"}}").as_bytes(),
        );
        touch(&home.join("sessions/sessions.json"), b"[{\"id\":\"s1\"}]");
        let units = run(home);
        let session = units
            .iter()
            .find(|u| u.relative_path() == "sessions/s1.json")
            .expect("session identified");
        assert_eq!(session.action(), AgentActionCapability::SessionRemoval);
        assert!(!session.protected());
        let index = units
            .iter()
            .find(|u| u.relative_path() == "sessions/sessions.json")
            .expect("index identified");
        assert!(index.protected());
        assert_eq!(index.action(), AgentActionCapability::None);
        let serialized = format!("{units:?}");
        assert!(!serialized.contains(canary), "content leaked");
    }

    #[test]
    fn index_dir_and_dev_data_are_actionable_caches_and_logs() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("index/embeddings.bin"), b"vector-bytes");
        touch(&home.join("dev_data/events.jsonl"), b"{}");
        let units = run(home);
        assert_eq!(
            units
                .iter()
                .find(|u| u.relative_path() == "index")
                .unwrap()
                .action(),
            AgentActionCapability::CacheOrLogTrash
        );
        assert_eq!(
            units
                .iter()
                .find(|u| u.relative_path() == "dev_data")
                .unwrap()
                .action(),
            AgentActionCapability::CacheOrLogTrash
        );
    }

    #[test]
    fn a_truncated_session_header_still_yields_the_declared_workspace() {
        // A real session is far larger than the cap, so its bounded
        // header is a *truncated* JSON object that cannot be parsed as
        // JSON at all. The declared field is still read out of it, and
        // the conversation body past the cap is never touched.
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("truncated-header-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let canary = "CANARY-CONTINUE-DO-NOT-LEAK-88ii";
        let body = format!("{}{canary}", "z".repeat(HEADER_READ_BYTES * 4));
        touch(
            &home.join("sessions/s1.json"),
            format!(
                "{{\"workspaceDirectory\":\"{}\",\"history\":[\"{body}\"]}}",
                repo.display()
            )
            .as_bytes(),
        );
        let units = run(home);
        let session = units
            .iter()
            .find(|u| u.relative_path() == "sessions/s1.json")
            .expect("session identified");
        assert!(
            matches!(session.project_link(), ProjectLinkState::Linked { .. }),
            "{:?}",
            session.project_link()
        );
        contract::no_content_leak(&units, canary);
    }

    #[test]
    fn a_workspace_directory_beyond_the_cap_stays_unresolved() {
        // The honest half of the same fact: the field can sit past the
        // cap in a session Continue serialized differently, and then this
        // adapter says so rather than reading the body to find it.
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("never-reached-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        touch(
            &home.join("sessions/s1.json"),
            format!(
                "{{\"history\":[\"{}\"],\"workspaceDirectory\":\"{}\"}}",
                "z".repeat(HEADER_READ_BYTES * 2),
                repo.display()
            )
            .as_bytes(),
        );
        let units = run(home);
        let session = units
            .iter()
            .find(|u| u.relative_path() == "sessions/s1.json")
            .expect("session identified");
        match &session.project_link() {
            ProjectLinkState::Unresolved { reason } => assert!(
                reason.contains("past the bounded read's cap"),
                "the reason must say the field may sit past the cap: {reason}"
            ),
            other => panic!("a field past the cap must not be resolved: {other:?}"),
        }
    }

    #[test]
    fn identification_cost_is_bounded_for_many_sessions() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        for i in 0..500 {
            touch(
                &home.join(format!("sessions/s{i}.json")),
                &b"x".repeat(50_000),
            );
        }
        let start = SystemTime::now();
        let units = run(home);
        let elapsed = SystemTime::now().duration_since(start).unwrap_or_default();
        eprintln!(
            "[measured] continue_dev identify() over 500 synthetic sessions took {elapsed:?}"
        );
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
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("unrelated.txt"), b"hello");
        let units = run(dir.path());
        assert_eq!(units.len(), 1, "an unrecognized home still surfaces a row");
        assert_eq!(units[0].relative_path(), "(unknown format)");
        assert_eq!(units[0].category(), AgentCategory::Unclassified);
        assert_eq!(units[0].action(), AgentActionCapability::None);
        assert!(
            units[0]
                .note
                .as_deref()
                .unwrap_or_default()
                .contains("treated as unknown format"),
            "the row must say why, not guess another tool's shape"
        );
    }

    #[test]
    fn canary_content_never_appears_in_output() {
        let canary = "CANARY-CONTINUE-DO-NOT-LEAK-77hh";
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("workspace-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        // The canary sits on the first line (which this adapter reads for
        // the workspace field) and in the body.
        touch(
            &home.join("sessions/s1.json"),
            format!(
                "{}\nsecond line: {canary}\n",
                session_json(&repo.display().to_string(), canary)
            )
            .as_bytes(),
        );
        touch(&home.join("config.yaml"), format!("# {canary}").as_bytes());
        let units = run(home);
        assert!(
            units
                .iter()
                .any(|u| matches!(u.project_link(), ProjectLinkState::Linked { .. })),
            "the fixture must actually exercise the header read"
        );
        contract::no_content_leak(&units, canary);
    }

    #[test]
    fn identification_reads_no_more_than_header_cap() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let sessions = 50u64;
        let mut total = 0u64;
        for i in 0..sessions {
            let body = format!(
                "{{\"workspaceDirectory\":\"/nowhere/{i}\",\"history\":\"{}\"}}",
                "x".repeat(60_000)
            );
            total += body.len() as u64;
            touch(&home.join(format!("sessions/s{i}.json")), body.as_bytes());
        }
        let (units, counters) = contract::measured(|| run(home));
        assert_eq!(
            units
                .iter()
                .filter(|u| u.category() == AgentCategory::Sessions)
                .count(),
            sessions as usize
        );
        contract::within_header_cap(counters, sessions);
        assert!(
            counters.header_bytes_read <= sessions * HEADER_READ_BYTES as u64,
            "identification read {} bytes, above {sessions} x this adapter's own \
             {HEADER_READ_BYTES} byte cap",
            counters.header_bytes_read
        );
        assert!(
            counters.header_bytes_read < total,
            "identification read {} of {total} fixture bytes -- a header read must be a small \
             fraction of the sessions it looks at",
            counters.header_bytes_read
        );
    }

    #[test]
    fn protected_categories_default_protected() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("config.yaml"), b"models: []");
        touch(&home.join("sessions/sessions.json"), b"[]");
        touch(&home.join("sessions/s1.json"), b"{}");
        let units = run(home);
        contract::protection_defaults_hold(&units);
        assert!(
            units
                .iter()
                .any(|u| u.relative_path() == "config.yaml" && u.protected()),
            "the main configuration is protected"
        );
        assert!(
            units
                .iter()
                .any(|u| u.relative_path() == "sessions/sessions.json" && u.protected()),
            "the session index is protected separately from the bodies"
        );
    }

    #[test]
    fn project_link_is_declared_or_unresolved_never_basename_guess() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        // (a) a session whose declared `workspaceDirectory` names a real
        // worktree resolves `Linked` from declared metadata.
        let repo = home.join("declared-workspace");
        fs::create_dir_all(repo.join(".git")).unwrap();
        touch(
            &home.join("sessions/linked.json"),
            session_json(&repo.display().to_string(), "no-canary").as_bytes(),
        );
        // (b) a session *named* like a project, with no declared field,
        // stays unresolved -- the name is never a link.
        touch(
            &home.join("sessions/my-repo-name.json"),
            b"{\"sessionId\":\"my-repo-name\",\"title\":\"t\"}",
        );
        let units = run(home);
        let linked = units
            .iter()
            .find(|u| u.relative_path() == "sessions/linked.json")
            .expect("declared session identified");
        match &linked.project_link() {
            ProjectLinkState::Linked { source, .. } => {
                assert_eq!(*source, crate::agents::LinkSource::Declared);
            }
            other => panic!("a declared workspaceDirectory must resolve Linked: {other:?}"),
        }
        let guessed = units
            .iter()
            .find(|u| u.relative_path() == "sessions/my-repo-name.json")
            .expect("undeclared session identified");
        match &guessed.project_link() {
            ProjectLinkState::Unresolved { reason } => {
                assert!(
                    reason.contains(WORKSPACE_FIELD),
                    "the reason must name the field that was missing: {reason}"
                );
            }
            other => panic!("a session with no declared workspace must be Unresolved: {other:?}"),
        }
        contract::linkage_is_declared_or_explicit(&units, "my-repo-name");
    }
}
