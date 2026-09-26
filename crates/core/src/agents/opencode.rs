//! OpenCode identification (#95): version-aware session/message storage
//! (SQLite-backed in current releases, a JSON file tree in older ones),
//! git-backed checkpoint snapshots, and protected configuration under
//! the **data root**
//! `crate::locations::opencode::OpenCodeDetector` resolves first (see
//! that module's doc comment for why config/cache are separate,
//! non-decomposed locations).
//!
//! Layout researched from primary source during implementation (never
//! from a real OpenCode data directory on this machine -- PRIVACY IS A
//! HARD RULE), from <https://github.com/sst/opencode>
//! (`packages/opencode/src/storage/storage.ts`, current `dev` branch)
//! and its own DeepWiki-indexed documentation as of this chunk:
//! - Older/file-tree layout: `storage/session/<project-id>/<session-id>
//!   .json`, `storage/message/<session-id>/*.json` ("stores messages by
//!   session ID" -- same session-id-keyed exact-match discipline
//!   `crate::agents::claude_code` uses for its own companion
//!   directories), `storage/part/<message-id>/*.json` ("stores message
//!   parts by message ID" -- keyed by *message*, not session, id: this
//!   adapter does not correlate parts to sessions without reading
//!   message file content, and says so, the same honest-gap discipline
//!   `claude_code::identify` uses for its own `paste-cache`),
//!   `storage/session_diff/<session-id>/` and `storage/project/
//!   <project-id>.json` (migration code: "the git repository's initial
//!   commit ID becomes the project identifier"; the file itself carries
//!   `id`, `vcs`, `worktree`, `time` -- `worktree` is a real filesystem
//!   path, read directly rather than needing any session-body scan for
//!   project linkage).
//! - Newer/SQLite layout: `opencode.db` (+ `-wal`/`-shm`) directly under
//!   the data root, replacing per-session file-tree storage for
//!   sessions/messages/history. This adapter never opens it; it is
//!   folded into one protected, non-actionable unit, same discipline
//!   `crate::agents::codex`'s own SQLite stores use.
//! - Both layouts: `snapshot/<project-id>/<hash>` -- an internal
//!   git-backed object store, decoupled from the project's own `.git`,
//!   capturing a tree snapshot before/after every agent step so `/undo`
//!   can revert. Unique checkpoint/recovery state, never a cache.
//! - `auth.json` and `log/` live directly under the data root (not under
//!   the separate config root), per
//!   <https://opencode.ai/docs/troubleshooting/>.
//!
//! There is **no `OPENCODE_DATA_DIR` environment variable**: sst/opencode
//! `dev` @ `fe3f3a41f79ad292cc3c7c629567385a20ec5130` computes the data
//! directory in `packages/core/src/global.ts` as `$XDG_DATA_HOME/opencode`
//! through the `xdg-basedir` package, and the complete env-var registry
//! in `packages/core/src/flag/flag.ts` holds only `OPENCODE_CONFIG_DIR`,
//! `OPENCODE_CONFIG`, `OPENCODE_CONFIG_CONTENT`, `OPENCODE_DB` and
//! `OPENCODE_TEST_HOME` (<https://opencode.ai/docs/config> agrees,
//! retrieved 2026-09-21). This adapter never reads the environment
//! anyway -- the data root arrives from the detector -- but the earlier
//! "`OPENCODE_DATA_DIR`, unconfirmed, honored defensively" note that
//! `crate::locations::opencode` and `crate::agents::matrix` still carry
//! is now disproved rather than merely unconfirmed.
//!
//! Version-aware boundary (#95's explicit acceptance): `identify` checks
//! for `opencode.db`/`storage/`/`snapshot/`/`auth.json`/`log/` before
//! doing anything else. If a resolved data root exists but is non-empty
//! and matches none of them, this adapter reports one `Unclassified`,
//! non-actionable "unsupported layout version" unit rather than
//! guessing at either schema.

use super::{
    AdapterCapabilities, AgentActionCapability, AgentAdapter, AgentCategory, AgentMember,
    AgentMemberKind, AgentUnitBuilder, CandidateAgentUnit, IdentifyCtx, ProjectLinkState,
    mtime_secs, resolve_declared_path,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const OPENCODE_TOOL_ID: &str = "opencode";

const MAX_FOLD_ENTRIES: usize = 200_000;

/// The cap on the one content read this adapter performs:
/// `storage/project/<id>.json`, a four-field metadata file. Everything
/// else is identified by name and measured by `stat`.
const HEADER_READ_BYTES: usize = 8192;

pub struct Adapter;

impl AgentAdapter for Adapter {
    fn id(&self) -> &'static str {
        OPENCODE_TOOL_ID
    }
    fn name(&self) -> &'static str {
        "OpenCode"
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
    let has_db = ctx.is_file(&home.join("opencode.db"));
    let has_storage = ctx.is_dir(&home.join("storage"));
    let has_snapshot = ctx.is_dir(&home.join("snapshot"));
    let has_auth = ctx.is_file(&home.join("auth.json"));
    let has_log = ctx.is_dir(&home.join("log"));
    if !(has_db || has_storage || has_snapshot || has_auth || has_log) {
        return unknown_version_residual(home, ctx);
    }

    let mut units = Vec::new();
    if has_db {
        identify_sqlite_store(home, ctx, &mut units);
    }
    let projects = load_project_worktrees(home, ctx);
    let mut claimed_session_ids: HashSet<String> = HashSet::new();
    if has_storage {
        if !has_db {
            identify_file_tree_sessions(home, ctx, &projects, &mut claimed_session_ids, &mut units);
        }
        identify_storage_auxiliary(home, ctx, &claimed_session_ids, &mut units);
    }
    if has_snapshot {
        identify_snapshots(home, ctx, &projects, &mut units);
    }
    identify_static_categories(home, ctx, has_db, has_storage, has_snapshot, &mut units);
    units
}

fn unknown_version_residual(home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
    // See `oh_my_pi::unknown_format_residual`'s identical note: a
    // genuinely empty, existing directory is not "unsupported version",
    // and the folded mtime is non-zero for an empty directory itself, so
    // entries are checked directly instead.
    if !ctx.has_entries(home) {
        return Vec::new();
    }
    let (bytes, mtime, _truncated) = ctx.folded_bytes(home, MAX_FOLD_ENTRIES);
    vec![
        AgentUnitBuilder::new(
            OPENCODE_TOOL_ID,
            AgentCategory::Unclassified,
            home.to_path_buf(),
        )
        .relative_path("(unsupported layout version)")
        .bytes(bytes)
        .mtime_max(mtime)
        .action(AgentActionCapability::None)
        .note(
            "no recognized OpenCode data-directory markers found (opencode.db/storage/snapshot/ \
             auth.json/log) at this resolved path; unsupported or future layout version -- \
             treated as unknown, not scanned further",
        )
        .build(),
    ]
}

// ---------------------------------------------------------------------
// Project linkage: `storage/project/<project-id>.json`'s `worktree`
// field, read once per project directory and reused for every session
// under it -- declared metadata, no session-body scan needed at all.
// ---------------------------------------------------------------------

fn load_project_worktrees(home: &Path, ctx: &IdentifyCtx) -> HashMap<String, ProjectLinkState> {
    let mut map = HashMap::new();
    let dir = home.join("storage").join("project");
    for name in ctx.file_names(&dir) {
        let path = dir.join(&name);
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(project_id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // Small, bounded metadata file -- not conversation content --
        // and memoised against its own (size, mtime), so a home whose
        // projects have not changed costs zero header bytes on a second
        // pass even with thousands of sessions under them.
        let worktree = ctx.derived(
            OPENCODE_TOOL_ID,
            "project-worktree",
            &path,
            HEADER_READ_BYTES,
            &|text| {
                let value: serde_json::Value = serde_json::from_str(text).ok()?;
                value
                    .get("worktree")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            },
        );
        let link = resolve_declared_path(
            worktree,
            "no worktree field in project.json (absent, or the file did not parse as JSON within \
             the bounded read)",
        );
        map.insert(project_id.to_string(), link);
    }
    map
}

fn project_link_for(
    projects: &HashMap<String, ProjectLinkState>,
    project_id: &str,
) -> ProjectLinkState {
    projects
        .get(project_id)
        .cloned()
        .unwrap_or_else(|| ProjectLinkState::Unresolved {
            reason: format!("no storage/project/{project_id}.json found"),
        })
}

// ---------------------------------------------------------------------
// SQLite-backed layout (current releases).
// ---------------------------------------------------------------------

fn identify_sqlite_store(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    let db = home.join("opencode.db");
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
        AgentUnitBuilder::new(OPENCODE_TOOL_ID, AgentCategory::Sessions, db)
            .relative_path("opencode.db")
            .bytes(bytes)
            .members_keep_bytes(members)
            .mtime_max(mtime_max)
            .protect(
                "SQLite-backed session/message/history store (current OpenCode layout); \
                 metadata-only, never opened while writable; no per-session drill-down in this \
                 version boundary",
            )
            .action(AgentActionCapability::None)
            .build(),
    );
}

// ---------------------------------------------------------------------
// File-tree layout (older releases).
// ---------------------------------------------------------------------

fn identify_file_tree_sessions(
    home: &Path,
    ctx: &IdentifyCtx,
    projects: &HashMap<String, ProjectLinkState>,
    claimed: &mut HashSet<String>,
    out: &mut Vec<CandidateAgentUnit>,
) {
    let base = home.join("storage").join("session");
    for project_id in ctx.dir_names(&base) {
        let project_dir = base.join(&project_id);
        let project_link = project_link_for(projects, &project_id);
        // One container per `storage/session/<project-id>/`. Its units'
        // members reach outside it -- `storage/message/<session-id>/`
        // and `storage/session_diff/<session-id>.json` -- so those paths
        // are declared with `ctx.watch` inside the closure, and an event
        // at any of them re-identifies the whole project directory.
        let units = ctx.container(OPENCODE_TOOL_ID, &project_dir, &|| {
            identify_one_project_dir(home, &project_dir, &project_link, ctx)
        });
        for unit in &units {
            if let Some(stem) = unit.path().file_stem().and_then(|s| s.to_str()) {
                // Derived from the replayed units, not from a variable
                // the closure mutated: a container that is replayed must
                // claim exactly what it claimed when it was identified,
                // or `identify_storage_auxiliary` would report its
                // session diffs as orphans on every reused pass.
                claimed.insert(stem.to_string());
            }
        }
        out.extend(units);
    }
}

fn identify_one_project_dir(
    home: &Path,
    project_dir: &Path,
    project_link: &ProjectLinkState,
    ctx: &IdentifyCtx,
) -> Vec<CandidateAgentUnit> {
    let mut out: Vec<CandidateAgentUnit> = Vec::new();
    {
        for file_name in ctx.file_names(project_dir) {
            let path = project_dir.join(&file_name);
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Some(session_id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            let Ok(meta) = ctx.stat(&path) else {
                continue;
            };
            let mut members = vec![AgentMember {
                path: path.clone(),
                bytes: meta.len(),
                kind: AgentMemberKind::Transcript,
            }];
            let mut bytes = meta.len();
            let mut mtime_max = mtime_secs(&meta);
            // `storage/message/<session-id>/` is a directory of
            // per-message files. `storage/session_diff/<session-id>` is
            // **not**: the universal key->path builder appends `.json`
            // (`packages/opencode/src/storage/storage.ts:62-64` @
            // `fe3f3a41f79ad292cc3c7c629567385a20ec5130`, reached from
            // `storage.write(["session_diff", input.sessionID], diffs)`
            // in `session/revert.ts:77`), so it is a single file.
            //
            // This adapter gated both on `is_dir()`, so every byte of
            // every session diff was in no unit at all -- the one thing
            // `docs/agent-storage.md` promises never happens. Both
            // shapes are accepted here: a `.json` file is the current
            // one, a directory of the same name is folded if some
            // version writes one, and neither is assumed.
            let message_dir = home.join("storage").join("message").join(&session_id);
            // Consulted with `is_dir`/`symlink_metadata` rather than
            // through `ctx`, so the container would not otherwise record
            // it. A session acquiring its first message directory has to
            // be a change to this container.
            ctx.watch(&message_dir);
            if ctx.is_dir(&message_dir) {
                let (b, m, _t) = ctx.folded_bytes(&message_dir, MAX_FOLD_ENTRIES);
                bytes += b;
                mtime_max = mtime_max.max(m);
                members.push(AgentMember {
                    path: message_dir,
                    bytes: b,
                    kind: AgentMemberKind::SessionData,
                });
            }
            let diff_base = home.join("storage").join("session_diff");
            for diff in [
                diff_base.join(format!("{session_id}.json")),
                diff_base.join(&session_id),
            ] {
                ctx.watch(&diff);
                let Ok(meta) = ctx.stat(&diff) else {
                    continue;
                };
                let (b, m) = if meta.is_dir() {
                    let (b, m, _t) = ctx.folded_bytes(&diff, MAX_FOLD_ENTRIES);
                    (b, m)
                } else if meta.is_file() {
                    (meta.len(), mtime_secs(&meta))
                } else {
                    continue;
                };
                bytes += b;
                mtime_max = mtime_max.max(m);
                members.push(AgentMember {
                    path: diff,
                    bytes: b,
                    kind: AgentMemberKind::SessionData,
                });
            }
            let relative_path = relative_to(home, &path);
            out.push(
                AgentUnitBuilder::new(OPENCODE_TOOL_ID, AgentCategory::Sessions, path)
                    .relative_path(relative_path)
                    .bytes(bytes)
                    .members_keep_bytes(members)
                    .mtime_max(mtime_max)
                    .project_link(project_link.clone())
                    .action(AgentActionCapability::SessionRemoval)
                    .build(),
            );
        }
    }
    out
}

/// `storage/message/` and `storage/session_diff/` entries no session
/// claimed above (folded into one residual note each, never silently
/// dropped -- same discipline `claude_code::identify_session_keyed_top_level`
/// uses), plus `storage/part/`, which is *never* session-keyed (keyed by
/// message id -- see module doc comment) and is always folded whole.
fn identify_storage_auxiliary(
    home: &Path,
    ctx: &IdentifyCtx,
    claimed: &HashSet<String>,
    out: &mut Vec<CandidateAgentUnit>,
) {
    for dir_name in ["message", "session_diff"] {
        let base = home.join("storage").join(dir_name);
        let mut bytes = 0u64;
        let mut mtime_max = 0u64;
        let mut any = false;
        // Entries, not only subdirectories: `storage/session_diff/`
        // holds `<session-id>.json` *files*, and a sweep that listed
        // only directories dropped every unclaimed one on the floor.
        for entry in ctx.list(&base) {
            let key = entry
                .name
                .strip_suffix(".json")
                .unwrap_or(&entry.name)
                .to_string();
            if claimed.contains(&key) {
                continue;
            }
            let path = base.join(&entry.name);
            let (b, m) = if entry.is_dir {
                let (b, m, _t) = ctx.folded_bytes(&path, MAX_FOLD_ENTRIES);
                (b, m)
            } else {
                match ctx.stat(&path) {
                    Ok(meta) if meta.is_file() => (meta.len(), mtime_secs(&meta)),
                    _ => continue,
                }
            };
            bytes += b;
            mtime_max = mtime_max.max(m);
            any = true;
        }
        if any {
            out.push(
                AgentUnitBuilder::new(OPENCODE_TOOL_ID, AgentCategory::Unclassified, base)
                    .relative_path(format!("storage/{dir_name} (unlinked)"))
                    .bytes(bytes)
                    .mtime_max(mtime_max)
                    .action(AgentActionCapability::None)
                    .note(format!(
                        "{dir_name} entries keyed by session id with no matching current session \
                         file in storage/session/ (already removed, or the current data root uses \
                         the SQLite-backed layout with no file-tree session to correlate against)"
                    ))
                    .build(),
            );
        }
    }
    let part = home.join("storage").join("part");
    if ctx.is_dir(&part) {
        let (bytes, mtime, truncated) = ctx.folded_bytes(&part, MAX_FOLD_ENTRIES);
        let mut builder = AgentUnitBuilder::new(
            OPENCODE_TOOL_ID,
            AgentCategory::Attachments,
            part,
        )
        .relative_path("storage/part")
        .bytes(bytes)
        .mtime_max(mtime)
        .protect(
            "message parts, keyed by message id; no per-session reference evidence is available \
             without reading message file content, so this adapter does not offer it as a \
             supported action",
        )
        .action(AgentActionCapability::None);
        if truncated {
            builder = builder.incomplete("directory entry count bound reached");
        }
        out.push(builder.build());
    }
}

// ---------------------------------------------------------------------
// Git-backed checkpoint snapshots (both layout versions).
// ---------------------------------------------------------------------

fn identify_snapshots(
    home: &Path,
    ctx: &IdentifyCtx,
    projects: &HashMap<String, ProjectLinkState>,
    out: &mut Vec<CandidateAgentUnit>,
) {
    let base = home.join("snapshot");
    for project_id in ctx.dir_names(&base) {
        let path = base.join(&project_id);
        let (bytes, mtime, truncated) = ctx.folded_bytes(&path, MAX_FOLD_ENTRIES);
        let relative_path = relative_to(home, &path);
        let unit = AgentUnitBuilder::new(OPENCODE_TOOL_ID, AgentCategory::Checkpoints, path)
            .relative_path(relative_path)
            .bytes(bytes)
            .mtime_max(mtime)
            .project_link(project_link_for(projects, &project_id))
            .action(AgentActionCapability::None)
            .note(
                "git-backed checkpoint history for this project's /undo; removing it loses the \
                 ability to revert past this point -- not a supported selective action in this \
                 chunk",
            );
        let unit = if truncated {
            unit.incomplete("directory entry count bound reached")
        } else {
            unit
        };
        out.push(unit.build());
    }
}

// ---------------------------------------------------------------------
// Static top-level entries.
// ---------------------------------------------------------------------

fn identify_static_categories(
    home: &Path,
    ctx: &IdentifyCtx,
    has_db: bool,
    has_storage: bool,
    has_snapshot: bool,
    out: &mut Vec<CandidateAgentUnit>,
) {
    let auth = home.join("auth.json");
    if let Ok(meta) = ctx.stat(&auth)
        && meta.is_file()
    {
        out.push(
            AgentUnitBuilder::new(OPENCODE_TOOL_ID, AgentCategory::ProtectedConfig, auth)
                .relative_path("auth.json")
                .bytes(meta.len())
                .mtime_max(mtime_secs(&meta))
                .protect(
                    "authentication data (API keys, OAuth tokens); contents are never read by \
                     this adapter",
                )
                .action(AgentActionCapability::None)
                .build(),
        );
    }

    let log = home.join("log");
    if ctx.is_dir(&log) {
        let (bytes, mtime, truncated) = ctx.folded_bytes(&log, MAX_FOLD_ENTRIES);
        let mut builder = AgentUnitBuilder::new(OPENCODE_TOOL_ID, AgentCategory::Logs, log)
            .relative_path("log")
            .bytes(bytes)
            .mtime_max(mtime)
            .action(AgentActionCapability::CacheOrLogTrash);
        if truncated {
            builder = builder.incomplete("directory entry count bound reached");
        }
        out.push(builder.build());
    }

    let mut seen_top_level: HashSet<&str> = HashSet::from(["auth.json", "log"]);
    if has_db {
        seen_top_level.extend(["opencode.db", "opencode.db-wal", "opencode.db-shm"]);
    }
    if has_storage {
        seen_top_level.insert("storage");
    }
    if has_snapshot {
        seen_top_level.insert("snapshot");
    }
    let mut residual_bytes = 0u64;
    let mut residual_mtime = 0u64;
    let mut residual_names: Vec<String> = Vec::new();
    for entry in ctx.list(home) {
        if seen_top_level.contains(entry.name.as_str()) {
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
                OPENCODE_TOOL_ID,
                AgentCategory::Unclassified,
                home.to_path_buf(),
            )
            .relative_path("(unclassified residual)")
            .bytes(residual_bytes)
            .mtime_max(residual_mtime)
            .action(AgentActionCapability::None)
            .note(format!(
                "entries with no specific rule in this adapter: {}",
                residual_names.join(", ")
            ))
            .build(),
        );
    }
}

fn relative_to(home: &Path, path: &Path) -> String {
    path.strip_prefix(home)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{IdentificationCache, LinkSource, bounded_io, contract};
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

    fn project_json(worktree: &str) -> String {
        format!("{{\"id\":\"p1\",\"vcs\":\"git\",\"worktree\":\"{worktree}\"}}")
    }

    #[test]
    fn empty_home_yields_no_units() {
        let dir = tempfile::tempdir().unwrap();
        assert!(run(dir.path()).is_empty());
    }

    #[test]
    fn file_tree_session_is_identified_and_linked_via_project_json() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("fixture-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        touch(
            &home.join("storage/project/p1.json"),
            project_json(&repo.display().to_string()).as_bytes(),
        );
        let canary = "CANARY-OC-DO-NOT-LEAK-44cc";
        touch(
            &home.join("storage/session/p1/s1.json"),
            format!("{{\"id\":\"s1\",\"title\":\"{canary}\"}}").as_bytes(),
        );
        touch(
            &home.join("storage/message/s1/m1.json"),
            format!("{{\"content\":\"{canary}\"}}").as_bytes(),
        );
        let units = run(home);
        let session = units
            .iter()
            .find(|u| u.category() == AgentCategory::Sessions)
            .expect("session identified");
        assert!(matches!(
            session.project_link(),
            ProjectLinkState::Linked { .. }
        ));
        assert_eq!(session.action(), AgentActionCapability::SessionRemoval);
        assert_eq!(session.members().len(), 2, "transcript + message companion");
        let serialized = format!("{units:?}");
        assert!(!serialized.contains(canary), "content leaked");
    }

    /// `storage/session_diff/<id>.json` is a **file**, and its bytes have
    /// to be in some unit.
    ///
    /// Upstream builds every storage path as
    /// `path.join(dir, ...key) + ".json"`
    /// (`packages/opencode/src/storage/storage.ts:62-64` @
    /// `fe3f3a41f79ad292cc3c7c629567385a20ec5130`, vendored at
    /// `crates/core/tests/fixtures/upstream/opencode/fe3f3a41f7/storage.ts`),
    /// and `session/revert.ts:77` writes
    /// `storage.write(["session_diff", input.sessionID], diffs)`. This
    /// adapter gated on `is_dir()` and swept with `dir_names`, so every
    /// diff -- claimed or not -- appeared in no unit at all. The
    /// assertion is on the *byte total*, because a member list that
    /// merely names the file would satisfy a weaker one.
    #[test]
    fn session_diff_files_are_counted_claimed_or_not() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("fixture-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        touch(
            &home.join("storage/project/p1.json"),
            project_json(&repo.display().to_string()).as_bytes(),
        );
        touch(&home.join("storage/session/p1/s1.json"), b"{\"id\":\"s1\"}");
        let diff = vec![b'd'; 4096];
        touch(&home.join("storage/session_diff/s1.json"), &diff);
        // A diff whose session is gone: still bytes on the disk, still
        // never silently dropped.
        let orphan = vec![b'o'; 2048];
        touch(&home.join("storage/session_diff/gone.json"), &orphan);

        let units = run(home);
        let session = units
            .iter()
            .find(|u| u.category() == AgentCategory::Sessions)
            .expect("session identified");
        assert!(
            session.members().iter().any(|m| m
                .path
                .to_string_lossy()
                .ends_with("storage/session_diff/s1.json")),
            "the diff file must be a member of its session: {:?}",
            session.members()
        );
        assert!(
            session.bytes() >= 4096,
            "the diff's bytes must be in the session's total, not merely named: {}",
            session.bytes()
        );

        let residual = units
            .iter()
            .find(|u| u.relative_path() == "storage/session_diff (unlinked)")
            .expect("an unclaimed diff file must still be reported");
        assert_eq!(
            residual.bytes(),
            2048,
            "exactly the orphan's bytes, and only once"
        );
    }

    #[test]
    fn sqlite_layout_is_protected_and_skips_file_tree_sessions() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("opencode.db"), b"sqlite-bytes");
        touch(&home.join("opencode.db-wal"), b"wal-bytes");
        // Legacy file-tree debris that must NOT be double-counted as a
        // live session once the DB-backed layout is in play.
        touch(&home.join("storage/session/p1/s1.json"), b"{}");
        let units = run(home);
        let db = units
            .iter()
            .find(|u| u.relative_path() == "opencode.db")
            .expect("db unit present");
        assert!(db.protected());
        assert_eq!(db.action(), AgentActionCapability::None);
        assert_eq!(db.members().len(), 2);
        assert!(
            units
                .iter()
                .all(|u| u.category() != AgentCategory::Sessions
                    || u.relative_path() == "opencode.db"),
            "no separate file-tree session unit once DB-backed"
        );
    }

    #[test]
    fn part_directory_is_protected_pending_session_correlation() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("storage/part/msg1/p1.json"), b"{}");
        let units = run(home);
        let part = units
            .iter()
            .find(|u| u.relative_path() == "storage/part")
            .unwrap();
        assert!(part.protected());
        assert_eq!(part.action(), AgentActionCapability::None);
    }

    #[test]
    fn snapshot_is_checkpoint_category_and_not_actionable() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("fixture-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        touch(
            &home.join("storage/project/p1.json"),
            project_json(&repo.display().to_string()).as_bytes(),
        );
        touch(&home.join("snapshot/p1/abcd1234"), b"git-object-bytes");
        let units = run(home);
        let snap = units
            .iter()
            .find(|u| u.category() == AgentCategory::Checkpoints)
            .expect("snapshot identified");
        assert_eq!(snap.action(), AgentActionCapability::None);
        assert!(matches!(
            snap.project_link(),
            ProjectLinkState::Linked { .. }
        ));
    }

    #[test]
    fn auth_json_is_protected_by_default() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("auth.json"), b"[redacted]");
        let units = run(home);
        let u = units
            .iter()
            .find(|u| u.relative_path() == "auth.json")
            .unwrap();
        assert!(u.protected());
        assert_eq!(u.action(), AgentActionCapability::None);
    }

    #[test]
    fn log_directory_is_actionable() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("log").join("app.log"), b"debug line");
        let units = run(home);
        let u = units.iter().find(|u| u.relative_path() == "log").unwrap();
        assert!(!u.protected());
        assert_eq!(u.action(), AgentActionCapability::CacheOrLogTrash);
    }

    #[test]
    fn identification_cost_is_bounded_for_many_sessions() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("big-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        touch(
            &home.join("storage/project/p1.json"),
            project_json(&repo.display().to_string()).as_bytes(),
        );
        for i in 0..500 {
            let mut content =
                format!("{{\"id\":\"s{i}\",\"body\":\"unread-canary\"}}").into_bytes();
            content.extend_from_slice(&b"x".repeat(200_000));
            touch(
                &home.join(format!("storage/session/p1/s{i}.json")),
                &content,
            );
        }
        let start = SystemTime::now();
        let units = run(home);
        let elapsed = SystemTime::now().duration_since(start).unwrap_or_default();
        eprintln!("[measured] opencode identify() over 500 synthetic sessions took {elapsed:?}");
        assert_eq!(
            units
                .iter()
                .filter(|u| u.category() == AgentCategory::Sessions)
                .count(),
            500
        );
        assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
    }

    #[test]
    fn an_unchanged_project_file_costs_no_header_bytes_on_a_second_pass() {
        // What `ctx.derived` buys: the project.json read is memoised
        // against its own (size, mtime), so re-identifying an unchanged
        // home reads nothing at all.
        let store = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("declared-checkout");
        fs::create_dir_all(repo.join(".git")).unwrap();
        touch(
            &home.join("storage/project/p1.json"),
            project_json(&repo.display().to_string()).as_bytes(),
        );
        touch(&home.join("storage/session/p1/s1.json"), b"{\"id\":\"s1\"}");

        let cache = IdentificationCache::load(store.path());
        let (first, first_counters) =
            contract::measured(|| identify(home, &IdentifyCtx::new(1, &cache)));
        assert!(
            first_counters.header_bytes_read > 0,
            "the first pass must actually read the project file"
        );
        let (second, second_counters) =
            contract::measured(|| identify(home, &IdentifyCtx::new(2, &cache)));
        assert_eq!(
            second_counters.header_bytes_read, 0,
            "an unchanged project file must be a cache hit, not a re-read"
        );
        assert_eq!(format!("{first:?}"), format!("{second:?}"));
    }

    // --- the five contract tests ---------------------------------------

    #[test]
    fn unknown_format_is_explicit_not_empty() {
        // A data root that exists, holds something, and matches none of
        // the documented markers is reported as exactly one explicit
        // "unsupported layout version" row -- never an empty vec, and
        // never re-read as the other layout's shape.
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("unrelated.txt"), b"hello");
        touch(&dir.path().join("future-layout/db.sqlite3"), b"nope");
        let units = run(dir.path());
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].relative_path(), "(unsupported layout version)");
        assert_eq!(units[0].category(), AgentCategory::Unclassified);
        assert_eq!(units[0].action(), AgentActionCapability::None);
        let note = units[0].note.as_deref().unwrap_or_default();
        assert!(
            note.contains("unsupported or future layout version"),
            "the unit must say why it is unclassified: {note}"
        );
        assert!(units[0].bytes() > 0, "an unknown layout is still measured");
    }

    #[test]
    fn canary_content_never_appears_in_output() {
        let canary = "CANARY-OC-CONTRACT-DO-NOT-LEAK-6b12";
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("declared-checkout");
        fs::create_dir_all(repo.join(".git")).unwrap();
        // The one file this adapter *does* read a header from: the
        // canary sits beside the field it parses, on the first line.
        touch(
            &home.join("storage/project/p1.json"),
            format!(
                "{{\"id\":\"p1\",\"worktree\":\"{}\",\"note\":\"{canary}\"}}",
                repo.display()
            )
            .as_bytes(),
        );
        // And in a session body, first line and onwards.
        touch(
            &home.join("storage/session/p1/s1.json"),
            format!("{{\"id\":\"s1\",\"title\":\"{canary}\"}}\n{canary}\n").as_bytes(),
        );
        touch(
            &home.join("storage/message/s1/m1.json"),
            format!("{{\"content\":\"{canary}\"}}").as_bytes(),
        );
        touch(
            &home.join("log/app.log"),
            format!("prompt: {canary}\n").as_bytes(),
        );
        let units = run(home);
        assert!(!units.is_empty());
        contract::no_content_leak(&units, canary);
    }

    #[test]
    fn identification_reads_no_more_than_header_cap() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("declared-checkout");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let mut fixture_bytes = 0u64;
        let project = project_json(&repo.display().to_string());
        touch(&home.join("storage/project/p1.json"), project.as_bytes());
        fixture_bytes += project.len() as u64;
        // Sessions are identified by name and measured by `stat`; none
        // of these bytes may be read.
        for i in 0..25 {
            let mut content = format!("{{\"id\":\"s{i}\"}}").into_bytes();
            content.extend_from_slice(&b"x".repeat(100_000));
            touch(
                &home.join(format!("storage/session/p1/s{i}.json")),
                &content,
            );
            fixture_bytes += content.len() as u64;
        }
        let (units, counters) = contract::measured(|| run(home));
        assert_eq!(
            units
                .iter()
                .filter(|u| u.category() == AgentCategory::Sessions)
                .count(),
            25
        );
        // One project.json is the *only* read: 25 sessions cost zero
        // content bytes between them.
        contract::within_header_cap(counters, 1);
        assert!(
            counters.header_bytes_read <= HEADER_READ_BYTES as u64,
            "identification read {} bytes, above this adapter's own {HEADER_READ_BYTES} byte cap",
            counters.header_bytes_read
        );
        assert!(
            counters.header_bytes_read < fixture_bytes,
            "{} vs {fixture_bytes} fixture bytes",
            counters.header_bytes_read
        );
        const { assert!(bounded_io::MAX_HEADER_BYTES >= HEADER_READ_BYTES) };
    }

    #[test]
    fn protected_categories_default_protected() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("auth.json"), b"[redacted]");
        touch(&home.join("log/app.log"), b"debug line");
        touch(&home.join("storage/part/msg1/p1.json"), b"{}");
        let units = run(home);
        let auth = units
            .iter()
            .find(|u| u.relative_path() == "auth.json")
            .expect("auth.json identified");
        assert_eq!(auth.category(), AgentCategory::ProtectedConfig);
        assert!(auth.protected());
        assert!(
            auth.protect_reason()
                .unwrap_or_default()
                .contains("authentication data"),
            "the protection reason must name what it protects: {:?}",
            auth.protect_reason()
        );
        contract::protection_defaults_hold(&units);
    }

    #[test]
    fn project_link_is_declared_or_unresolved_never_basename_guess() {
        // (a) A project.json that declares a real checkout resolves
        // `Linked` from `LinkSource::Declared`.
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("declared-checkout");
        fs::create_dir_all(repo.join(".git")).unwrap();
        touch(
            &home.join("storage/project/p1.json"),
            project_json(&repo.display().to_string()).as_bytes(),
        );
        touch(&home.join("storage/session/p1/s1.json"), b"{\"id\":\"s1\"}");
        let units = run(home);
        let session = units
            .iter()
            .find(|u| u.category() == AgentCategory::Sessions)
            .expect("session identified");
        match &session.project_link() {
            ProjectLinkState::Linked { source, .. } => assert_eq!(*source, LinkSource::Declared),
            other => panic!("a declared worktree must link: {other:?}"),
        }

        // (b) A session file *named* like a real checkout that sits right
        // next to it, with no project.json declaring anything, must come
        // back `Unresolved` with a reason -- never linked to the
        // same-named repo.
        let bare = tempfile::tempdir().unwrap();
        let bare = bare.path();
        fs::create_dir_all(bare.join("my-repo-name/.git")).unwrap();
        let oc = bare.join("oc-data");
        touch(&oc.join("storage/session/p1/my-repo-name.json"), b"{}");
        let bare_units = run(&oc);
        let session = bare_units
            .iter()
            .find(|u| u.category() == AgentCategory::Sessions)
            .expect("session identified");
        match &session.project_link() {
            ProjectLinkState::Unresolved { reason } => assert!(
                reason.contains("no storage/project/p1.json found"),
                "{reason}"
            ),
            other => panic!("an undeclared session must not link: {other:?}"),
        }
        contract::linkage_is_declared_or_explicit(&bare_units, "my-repo-name");
        contract::linkage_is_declared_or_explicit(&units, "my-repo-name");
    }
}
