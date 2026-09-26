//! Codex CLI identification (#93): rollout sessions (live + archived),
//! SQLite-backed state stores, and protected configuration under the
//! home `crate::locations::codex::CodexDetector` resolves.
//!
//! Layout researched from primary source during implementation, all
//! from <https://github.com/openai/codex> `codex-rs`, current `main` as
//! of this chunk. Session linkage comes from Codex's local SQLite thread
//! index (`rollout_path` + `cwd`), not transcript contents. Rollout files
//! are walked/stat'ed for storage accounting but their bytes are never
//! read:
//! - `codex-rs/utils/home-dir/src/lib.rs` (`find_codex_home`):
//!   `CODEX_HOME` env var, else `~/.codex`.
//! - `codex-rs/rollout/src/lib.rs`: `pub const SESSIONS_SUBDIR: &str =
//!   "sessions"`, `pub const ARCHIVED_SESSIONS_SUBDIR: &str =
//!   "archived_sessions"`.
//! - `codex-rs/rollout/src/list.rs`: sessions live in a `<year>/<month>/
//!   <day>/` tree under each subdir (`collect_dirs_desc`/
//!   `collect_rollout_day_files`).
//! - `codex-rs/rollout/src/rollout_file_name.rs`: filenames are
//!   `rollout-<YYYY-MM-DDTHH-MM-SS>-<thread-id>[_<rollout-id>].jsonl`
//!   (an underscore-suffixed second id only for a reverted thread).
//! - `codex-rs/app-server/src/codex_home_metrics.rs`: upstream's own
//!   background size metric walks exactly `sessions/` and
//!   `archived_sessions/` recursively, stat-only, without following
//!   symlinks and without reading file contents -- the same discipline
//!   `IdentifyCtx::folded_bytes` already gives every adapter in this
//!   module, confirming (not just assuming) it matches upstream's own
//!   practice.
//! - `codex-rs/state/src/sqlite.rs` @
//!   `ac7634b9f73ec1bf96466be7a5869f0949d20b30`:
//!   `const RUNTIME_DBS: [RuntimeDbSpec; 7]` -- `state_5.sqlite`,
//!   `logs_2.sqlite`, `goals_1.sqlite`, `memories_1.sqlite`,
//!   `memories_v2_1.sqlite`, `queue_1.sqlite`,
//!   `thread_history_1.sqlite`, each `codex_home.join(<filename>)`.
//!   Seven, not six: `memories_v2_1.sqlite` is declared as a
//!   struct-update over `MEMORIES_DB` with its filename inline, so
//!   counting the `*_DB_FILENAME` consts gives six and misses it.
//!   `CODEX_SQLITE_HOME` can relocate them outside `CODEX_HOME`; the
//!   adapter resolves `sqlite_home` from config, then that environment
//!   variable, then the Codex home. It opens only the newest strictly
//!   versioned `state_<n>.sqlite` read-only and selects exactly
//!   `threads.rollout_path` and `threads.cwd`. Missing, stale, ambiguous,
//!   or incompatible index rows stay unresolved; there is no transcript
//!   parsing fallback.
//!
//! No managed-worktree creation by the Codex CLI itself is confirmed by
//! primary source this chunk, so `AgentCategory::ManagedWorktrees` is
//! never populated by this adapter -- an honest absence, not a silent
//! gap (`docs/agent-storage.md` records the same note in prose).

use super::{
    AdapterCapabilities, AgentActionCapability, AgentAdapter, AgentCategory, AgentMember,
    AgentMemberKind, AgentUnitBuilder, CandidateAgentUnit, IdentifyCtx, ProjectLinkState,
    codex_state::{self, CwdLookup, SessionIndex},
    mtime_secs,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const CODEX_TOOL_ID: &str = "codex";

const MAX_FOLD_ENTRIES: usize = 200_000;
/// Rollout files one session-tree **container** will identify. Per
/// container, never shared: a bound a day directory shares with its
/// siblings would make its stored rows mean something different from a
/// live identification of the same directory (see [`SessionWalk::walk`]).
const MAX_CONTAINER_ENTRIES: usize = 20_000;
/// Session-tree containers one pass will identify. Caps the whole pass
/// without making any one container's contents depend on another's.
const MAX_CONTAINERS: usize = 20_000;
/// Depth below a session root at which a directory becomes a container:
/// `sessions/<yyyy>/<mm>/<dd>/`, upstream's documented layout.
const CONTAINER_DEPTH: usize = 3;
/// Bound on how many nested date directories a session-tree walk
/// descends before giving up on a subtree, so a pathologically deep or
/// cyclic (symlink) layout cannot make identification unbounded. The
/// real layout is exactly three levels (year/month/day); this leaves
/// headroom for a future layout change without becoming unbounded.
const MAX_WALK_DEPTH: usize = 8;

fn declared_project_link(
    rollout_path: &Path,
    session_index: &SessionIndex,
) -> (Option<String>, &'static str) {
    let (declared, reason) = match session_index.cwd_for(rollout_path) {
        CwdLookup::Declared(path) => (Some(path.to_string_lossy().into_owned()), ""),
        CwdLookup::NoIndex => (None, "Codex state index unavailable or unsupported"),
        CwdLookup::NoRow => (None, "no exact rollout_path row in Codex state index"),
        CwdLookup::NoUsableCwd => (None, "Codex state row has no unique absolute cwd"),
    };
    (declared, reason)
}

fn refresh_project_link(
    unit: &mut CandidateAgentUnit,
    session_index: &SessionIndex,
    ctx: &IdentifyCtx,
) {
    let (declared, reason) = declared_project_link(unit.path(), session_index);
    ctx.refresh_declared_project_link(unit, declared, reason);
}

pub struct Adapter;

impl AgentAdapter for Adapter {
    fn id(&self) -> &'static str {
        CODEX_TOOL_ID
    }
    fn name(&self) -> &'static str {
        "Codex"
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::default()
    }
    fn identify(&self, home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
        identify(home, ctx)
    }
}

pub fn identify(home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
    let session_index = SessionIndex::read_from_home(home, ctx);
    let mut walk = SessionWalk {
        home,
        archived: false,
        ctx,
        session_index: &session_index,
        units: Vec::new(),
        containers_used: 0,
    };
    walk.walk(&home.join("sessions"), 0);
    walk.archived = true;
    walk.walk(&home.join("archived_sessions"), 0);
    let mut units = walk.units;
    identify_sqlite_stores(home, ctx, &mut units);
    identify_static_categories(home, ctx, &mut units);
    units
}

// ---------------------------------------------------------------------
// Sessions (live + archived): each rollout file is its own unit -- no
// companion directory is documented or found in this chunk's source
// research, so a session's `members` is always exactly the one file.
// ---------------------------------------------------------------------

/// One rollout file's unit. Factored out so the same code produces it
/// whether it was found inside a container or directly under a session
/// root.
fn session_unit(
    home: &Path,
    jsonl: PathBuf,
    archived: bool,
    ctx: &IdentifyCtx,
    session_index: &SessionIndex,
) -> Option<CandidateAgentUnit> {
    let meta = ctx.stat(&jsonl).ok()?;
    let bytes = meta.len();
    let mtime = mtime_secs(&meta);
    let (declared, missing_reason) = declared_project_link(&jsonl, session_index);
    // `project_link_declared`, not `project_link`: the declared path is
    // what a replayed container re-resolves live, so a worktree deleted
    // between two passes is never reported as still linked
    // (`crate::agents::LinkBasis`). A unit whose link is `Fixed` makes
    // its whole container unstorable.
    // `archived_sessions/` is its own category, not folded into
    // `sessions/`: stack/26's Codex reconciliation defect was exactly
    // this row reporting into `AgentCategory::Sessions` regardless of
    // `archived`, which summed live and archived bytes into one
    // "sessions" total a user could not decompose against `du`.
    let category = if archived {
        AgentCategory::ArchivedSessions
    } else {
        AgentCategory::Sessions
    };
    let mut unit = AgentUnitBuilder::new(CODEX_TOOL_ID, category, jsonl.clone())
        .relative_to(home)
        .members(vec![AgentMember {
            path: jsonl,
            bytes,
            kind: AgentMemberKind::Transcript,
        }])
        .mtime_max(mtime)
        .project_link_declared(declared, missing_reason)
        .action(AgentActionCapability::SessionRemoval);
    if archived {
        unit = unit.note(
            "archived: hidden from the default thread list, but still unique conversation \
             history -- archiving says nothing about whether this session is still needed",
        );
    }
    Some(unit.build())
}

/// Walks a session root, wrapping every **day directory**
/// (`sessions/<yyyy>/<mm>/<dd>/`) in [`IdentifyCtx::container`] so a day
/// nothing touched is replayed from the store instead of re-listed and
/// re-`stat`ed.
///
/// The bound is the reason this could not be done before. It used to be
/// one budget shared across `sessions/` and `archived_sessions/`
/// (`already_seen + out.len()`), which made a day's output depend on how
/// many files the days before it had produced -- so a day replayed from
/// the store would have meant something different from the same day
/// identified live, and the stored rows could not be trusted. The budget
/// is now **per container** ([`MAX_CONTAINER_ENTRIES`]), which each day
/// owns outright, plus a pass-level cap on how many containers are
/// identified at all ([`MAX_CONTAINERS`]). Both are deterministic from
/// the tree alone; neither depends on what a sibling produced.
struct SessionWalk<'home, 'ctx, 'index, 'data> {
    home: &'home Path,
    archived: bool,
    ctx: &'ctx IdentifyCtx<'data>,
    session_index: &'index SessionIndex,
    units: Vec<CandidateAgentUnit>,
    containers_used: usize,
}

impl SessionWalk<'_, '_, '_, '_> {
    fn walk(&mut self, dir: &Path, depth: usize) {
        if depth > MAX_WALK_DEPTH {
            return;
        }
        for entry in self.ctx.list(dir) {
            let path = dir.join(&entry.name);
            if entry.is_dir {
                if depth + 1 == CONTAINER_DEPTH {
                    if self.containers_used >= MAX_CONTAINERS {
                        return;
                    }
                    self.containers_used += 1;
                    let home = self.home;
                    let archived = self.archived;
                    let ctx = self.ctx;
                    let session_index = self.session_index;
                    let units = ctx.container(CODEX_TOOL_ID, &path, &|| {
                        let mut files = Vec::new();
                        collect_jsonl_files(&path, depth + 1, ctx, &mut files);
                        files
                            .into_iter()
                            .filter_map(|jsonl| {
                                session_unit(home, jsonl, archived, ctx, session_index)
                            })
                            .collect()
                    });
                    self.units.extend(units.into_iter().map(|mut unit| {
                        refresh_project_link(&mut unit, session_index, ctx);
                        unit
                    }));
                } else {
                    self.walk(&path, depth + 1);
                }
            } else if is_rollout(&entry.name) {
                // A rollout file sitting above the day level (an older or
                // hand-moved layout) is identified inline: it belongs to no
                // container, so it is never replayed.
                self.units.extend(session_unit(
                    self.home,
                    path,
                    self.archived,
                    self.ctx,
                    self.session_index,
                ));
            }
        }
    }
}

fn is_rollout(name: &str) -> bool {
    name.ends_with(".jsonl") && name.starts_with("rollout-")
}

/// Bounded recursive `*.jsonl` collection under one container, one
/// explicit level at a time through the shared capped listing
/// (`IdentifyCtx::list` never follows a symlink and never recurses on
/// its own). The entry budget is this container's own: see
/// [`SessionWalk::walk`].
fn collect_jsonl_files(dir: &Path, depth: usize, ctx: &IdentifyCtx, out: &mut Vec<PathBuf>) {
    if depth > MAX_WALK_DEPTH || out.len() >= MAX_CONTAINER_ENTRIES {
        return;
    }
    for entry in ctx.list(dir) {
        if out.len() >= MAX_CONTAINER_ENTRIES {
            return;
        }
        let path = dir.join(&entry.name);
        if entry.is_dir {
            collect_jsonl_files(&path, depth + 1, ctx, out);
        } else if is_rollout(&entry.name) {
            out.push(path);
        }
    }
}

// ---------------------------------------------------------------------
// SQLite-backed state stores (#93's version boundary: newer Codex keeps
// these instead of, or alongside, flat history files). Each database is
// one unit with its `-wal`/`-shm` sidecars folded in as members;
// `crate::actions::is_sqlite_like` refuses any selective action on any
// of them unconditionally regardless of this adapter's own
// protected/action fields, which are set conservatively here anyway.
// ---------------------------------------------------------------------

struct SqliteStore {
    filename: &'static str,
    purpose: &'static str,
}

const SQLITE_STORES: &[SqliteStore] = &[
    SqliteStore {
        filename: "state_5.sqlite",
        purpose: "session/thread state index",
    },
    SqliteStore {
        filename: "logs_2.sqlite",
        purpose: "structured log store",
    },
    SqliteStore {
        filename: "goals_1.sqlite",
        purpose: "goal-tracking store",
    },
    SqliteStore {
        filename: "memories_1.sqlite",
        purpose: "episodic memory store",
    },
    // The seventh. `RUNTIME_DBS: [RuntimeDbSpec; 7]` is easy to
    // under-count because this one's filename is an inline literal in a
    // struct-update expression rather than a `*_DB_FILENAME` const, and
    // there are exactly six of those. Missing it meant a state store
    // that was never folded with its `-wal`/`-shm` sidecars and never
    // put in the protected SQLite category -- the 2026-09-22 re-review
    // found it by reading the file this row already cited.
    SqliteStore {
        filename: "memories_v2_1.sqlite",
        purpose: "episodic memory store, v2 schema",
    },
    SqliteStore {
        filename: "queue_1.sqlite",
        purpose: "task queue store",
    },
    SqliteStore {
        filename: "thread_history_1.sqlite",
        purpose: "thread history index",
    },
];

fn identify_sqlite_stores(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    for store in SQLITE_STORES {
        identify_sqlite_store(
            home.join(store.filename),
            store.filename,
            store.purpose,
            ctx,
            out,
        );
    }
    // The state database's schema number is an upstream version boundary.
    // Recognize future `state_<n>.sqlite` names rather than silently
    // dropping a renamed index into the unclassified residual.
    for entry in ctx.list(home) {
        if codex_state::state_database_version(&entry.name).is_some()
            && !SQLITE_STORES
                .iter()
                .any(|store| store.filename == entry.name)
        {
            identify_sqlite_store(
                home.join(&entry.name),
                &entry.name,
                "session/thread state index",
                ctx,
                out,
            );
        }
    }
}

fn identify_sqlite_store(
    path: PathBuf,
    filename: &str,
    purpose: &str,
    ctx: &IdentifyCtx,
    out: &mut Vec<CandidateAgentUnit>,
) {
    let Ok(meta) = ctx.stat(&path) else {
        return;
    };
    if !meta.is_file() {
        return;
    }
    let mut members = vec![AgentMember {
        path: path.clone(),
        bytes: meta.len(),
        kind: AgentMemberKind::Database,
    }];
    let mut bytes = meta.len();
    let mut mtime_max = mtime_secs(&meta);
    for sidecar_ext in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{sidecar_ext}", path.display()));
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
        // `ProtectedDatabases`, not `Sessions`: these are state
        // stores, not conversation history, and folding them into
        // `Sessions` was stack/26's Codex reconciliation defect --
        // it inflated the reported "sessions" total by every
        // SQLite store's bytes (plus `-wal`/`-shm`) against what
        // `du` shows per top-level entry.
        AgentUnitBuilder::new(CODEX_TOOL_ID, AgentCategory::ProtectedDatabases, path)
            .relative_path(filename)
            .bytes(bytes)
            .members_keep_bytes(members)
            .mtime_max(mtime_max)
            .project_link(ProjectLinkState::NotApplicable)
            .action(AgentActionCapability::None)
            .protect(format!(
                "SQLite {purpose}; protected and never actionable. The state index is \
                     queried read-only for rollout_path/cwd linkage only; no conversation \
                     content or other row fields are read."
            ))
            .build(),
    );
}

// ---------------------------------------------------------------------
// Static top-level categories.
// ---------------------------------------------------------------------

struct StaticEntry {
    rel: &'static str,
    category: AgentCategory,
    action: AgentActionCapability,
    protected: bool,
    note: &'static str,
}

const STATIC_ENTRIES: &[StaticEntry] = &[
    StaticEntry {
        rel: "config.toml",
        category: AgentCategory::ProtectedConfig,
        action: AgentActionCapability::None,
        protected: true,
        note: "main CLI configuration",
    },
    StaticEntry {
        rel: "auth.json",
        category: AgentCategory::ProtectedConfig,
        action: AgentActionCapability::None,
        protected: true,
        note: "OAuth/API credentials; contents are never read by this adapter",
    },
    StaticEntry {
        rel: "history.jsonl",
        category: AgentCategory::Sessions,
        action: AgentActionCapability::None,
        protected: true,
        note: "cross-session prompt history; contains prompt text and is never read by this \
               adapter",
    },
    StaticEntry {
        rel: "skills",
        category: AgentCategory::ProtectedConfig,
        action: AgentActionCapability::None,
        protected: true,
        note: "personal skill definitions; exact directory name found in source search but not \
               independently confirmed by a primary docs page this chunk -- treated as \
               protected pending confirmation, same discipline claude_code.rs uses for its own \
               unconfirmed entries",
    },
    StaticEntry {
        rel: "log",
        category: AgentCategory::Logs,
        action: AgentActionCapability::CacheOrLogTrash,
        protected: false,
        note: "CLI debug logs; regenerated automatically. Directory name is this epic's prior \
               research, not independently re-confirmed by source in this chunk",
    },
    StaticEntry {
        rel: "plugins",
        category: AgentCategory::Plugins,
        action: AgentActionCapability::None,
        protected: false,
        note: "installed plugin cache (`~/.codex/plugins/cache/<marketplace>/<plugin>/<version>/`, \
               per developers.openai.com/codex/plugins/build: \"ChatGPT installs plugins into \
               ~/.codex/plugins/cache/$MARKETPLACE_NAME/$PLUGIN_NAME/$VERSION/\"); previously fell \
               into the unclassified residual, which is stack/26's Codex reconciliation defect",
    },
];

fn identify_static_categories(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    let mut seen_top_level: HashSet<String> = HashSet::new();
    for entry in STATIC_ENTRIES {
        seen_top_level.insert(entry.rel.to_string());
        let path = home.join(entry.rel);
        if !ctx.exists(&path) {
            continue;
        }
        let (bytes, mtime, truncated) = ctx.folded_bytes(&path, MAX_FOLD_ENTRIES);
        let mut unit = AgentUnitBuilder::new(CODEX_TOOL_ID, entry.category, path)
            .relative_path(entry.rel)
            .bytes(bytes)
            .mtime_max(mtime)
            .project_link(ProjectLinkState::NotApplicable)
            .action(entry.action)
            .note(entry.note);
        if truncated {
            unit =
                unit.incomplete("directory entry count bound reached; total may be an undercount");
        }
        if entry.protected {
            unit = unit.protect(entry.note);
        }
        out.push(unit.build());
    }
    for store in SQLITE_STORES {
        seen_top_level.insert(store.filename.to_string());
        // The sidecars too. They are already members of the store's own
        // unit; leaving them out of this set counted their bytes a
        // second time in the unclassified residual, and reported a
        // `-wal` file as an unrecognized top-level entry of a layout
        // this adapter does in fact recognize.
        for sidecar_ext in ["-wal", "-shm"] {
            seen_top_level.insert(format!("{}{sidecar_ext}", store.filename));
        }
    }

    let mut residual_bytes = 0u64;
    let mut residual_mtime = 0u64;
    let mut residual_names: Vec<String> = Vec::new();
    for e in ctx.list(home) {
        let name = e.name;
        if name == "sessions"
            || name == "archived_sessions"
            || seen_top_level.contains(&name)
            || codex_state::state_database_version(&name).is_some()
            || ["-wal", "-shm"].iter().any(|suffix| {
                name.strip_suffix(suffix)
                    .is_some_and(|base| codex_state::state_database_version(base).is_some())
            })
        {
            continue;
        }
        let (bytes, mtime, _truncated) = ctx.folded_bytes(&home.join(&name), MAX_FOLD_ENTRIES);
        residual_bytes += bytes;
        residual_mtime = residual_mtime.max(mtime);
        residual_names.push(name);
    }
    if !residual_names.is_empty() {
        residual_names.sort();
        out.push(
            AgentUnitBuilder::new(
                CODEX_TOOL_ID,
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

    fn write_index(home: &Path, rows: &[(&Path, &Path)]) {
        let db = rusqlite::Connection::open(home.join("state_5.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE threads (rollout_path TEXT, cwd TEXT, title TEXT, preview TEXT, first_user_message TEXT);").unwrap();
        for (rollout, cwd) in rows {
            db.execute(
                "INSERT INTO threads (rollout_path, cwd, title, preview, first_user_message) VALUES (?1, ?2, 'private title', 'private preview', 'private message')",
                rusqlite::params![rollout.to_string_lossy(), cwd.to_string_lossy()],
            ).unwrap();
        }
    }

    #[test]
    fn empty_home_yields_no_units() {
        let dir = tempfile::tempdir().unwrap();
        assert!(run(dir.path()).is_empty());
    }

    #[test]
    fn a_session_in_the_date_tree_is_identified_and_linked() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("fixture-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let canary = "CANARY-CODEX-DO-NOT-LEAK-91a3";
        let jsonl = home
            .join("sessions/2026/09/21/rollout-2026-09-21T10-00-00-11111111-1111-4111-8111-111111111111.jsonl");
        touch(&jsonl, format!("transcript canary: {canary}\n").as_bytes());
        write_index(home, &[(&jsonl, &repo)]);
        let units = run(home);
        let session = units
            .iter()
            .find(|u| u.category() == AgentCategory::Sessions && u.path == jsonl)
            .expect("session identified");
        assert!(matches!(
            session.project_link(),
            ProjectLinkState::Linked { .. }
        ));
        assert_eq!(session.action(), AgentActionCapability::SessionRemoval);
        assert_eq!(session.members().len(), 1);
        let serialized = format!("{units:?}");
        assert!(!serialized.contains(canary), "prompt content leaked");
    }

    #[test]
    fn only_an_exact_indexed_rollout_path_links() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = repo_dir.path().join("declared-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let home = tempfile::tempdir().unwrap();
        let indexed = home
            .path()
            .join("sessions/2026/09/21/rollout-indexed.jsonl");
        let unindexed = home
            .path()
            .join("sessions/2026/09/21/rollout-unindexed.jsonl");
        touch(&indexed, format!("{}\n", repo.display()).as_bytes());
        touch(
            &unindexed,
            format!("{{\"cwd\":\"{}\"}}\n", repo.display()).as_bytes(),
        );
        write_index(home.path(), &[(&indexed, &repo)]);
        let units = run(home.path());
        assert!(matches!(
            units
                .iter()
                .find(|u| u.path == indexed)
                .unwrap_or_else(|| panic!("indexed unit missing: {units:#?}"))
                .project_link(),
            ProjectLinkState::Linked { .. }
        ));
        assert!(matches!(
            units
                .iter()
                .find(|u| u.path == unindexed)
                .unwrap()
                .project_link(),
            ProjectLinkState::Unresolved { .. }
        ));
    }

    #[test]
    fn archived_sessions_are_identified_with_an_explicit_note() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let jsonl = home.join(
            "archived_sessions/2026/01/02/rollout-2026-01-02T00-00-00-22222222-2222-4222-8222-222222222222.jsonl",
        );
        touch(&jsonl, b"opaque transcript\n");
        let units = run(home);
        let session = units.iter().find(|u| u.path == jsonl).unwrap();
        assert!(session.note.as_deref().unwrap().contains("archived"));
    }

    #[test]
    fn missing_state_index_row_is_unresolved_not_a_panic() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let jsonl = home.join("sessions/2026/01/01/rollout-2026-01-01T00-00-00-x.jsonl");
        touch(&jsonl, b"not valid json");
        let units = run(home);
        let session = units.iter().find(|u| u.path == jsonl).unwrap();
        assert!(matches!(
            session.project_link(),
            ProjectLinkState::Unresolved { .. }
        ));
    }

    #[test]
    fn sqlite_stores_are_protected_and_fold_wal_shm_sidecars() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("state_5.sqlite"), b"sqlite-bytes");
        touch(&home.join("state_5.sqlite-wal"), b"wal-bytes");
        touch(&home.join("state_5.sqlite-shm"), b"shm-bytes");
        let units = run(home);
        let db = units
            .iter()
            .find(|u| u.relative_path() == "state_5.sqlite")
            .expect("db unit present");
        assert!(db.protected());
        assert_eq!(db.action(), AgentActionCapability::None);
        assert_eq!(db.members().len(), 3, "db + wal + shm folded together");
        assert_eq!(
            db.bytes(),
            db.members().iter().map(|member| member.bytes).sum::<u64>(),
            "folded size must reflect actual post-query SQLite sidecars"
        );
        assert!(
            db.members()
                .iter()
                .all(|m| m.kind == AgentMemberKind::Database),
            "every member of a SQLite store stays a Database member"
        );
    }

    /// All **seven** runtime databases, each folded with its sidecars and
    /// each protected. The count is the assertion: `RUNTIME_DBS` is
    /// declared `[RuntimeDbSpec; 7]` upstream
    /// (`codex-rs/state/src/sqlite.rs:105-113` @
    /// `ac7634b9f73ec1bf96466be7a5869f0949d20b30`, vendored at
    /// `crates/core/tests/fixtures/upstream/codex/ac7634b9f7/sqlite.rs`),
    /// and this adapter modelled six -- `memories_v2_1.sqlite` was
    /// unprotected, unfolded and uncounted.
    #[test]
    fn every_one_of_the_seven_runtime_databases_is_folded_and_protected() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let expected = [
            "state_5.sqlite",
            "logs_2.sqlite",
            "goals_1.sqlite",
            "memories_1.sqlite",
            "memories_v2_1.sqlite",
            "queue_1.sqlite",
            "thread_history_1.sqlite",
        ];
        for name in expected {
            touch(&home.join(name), b"sqlite-bytes");
            touch(&home.join(format!("{name}-wal")), b"wal");
            touch(&home.join(format!("{name}-shm")), b"shm");
        }
        let units = run(home);
        for name in expected {
            let db = units
                .iter()
                .find(|u| u.relative_path() == name)
                .unwrap_or_else(|| panic!("{name} must be identified as its own store"));
            assert!(db.protected(), "{name} must be protected");
            assert_eq!(db.action(), AgentActionCapability::None, "{name}");
            assert_eq!(
                db.members().len(),
                3,
                "{name}: db + wal + shm folded together"
            );
            assert!(
                db.members()
                    .iter()
                    .all(|m| m.kind == AgentMemberKind::Database),
                "{name}"
            );
        }
        // And nothing leaks into the unclassified residual, which is
        // where an unmodelled store would otherwise have shown up.
        let residual = units
            .iter()
            .find(|u| u.category() == AgentCategory::Unclassified);
        assert!(
            residual.is_none(),
            "every runtime database must have its own unit: {:?}",
            residual.map(|u| u.relative_path().to_string())
        );
    }

    #[test]
    fn future_state_database_versions_remain_protected_and_folded() {
        let home = tempfile::tempdir().unwrap();
        touch(&home.path().join("state_12.sqlite"), b"future-state");
        touch(&home.path().join("state_12.sqlite-wal"), b"wal");
        touch(&home.path().join("state_12.sqlite-shm"), b"shm");
        let units = run(home.path());
        let db = units
            .iter()
            .find(|unit| unit.relative_path() == "state_12.sqlite")
            .expect("future state DB is still modeled");
        assert!(db.protected());
        assert_eq!(db.action(), AgentActionCapability::None);
        assert_eq!(db.members().len(), 3);
    }

    #[test]
    fn protected_config_is_protected_by_default() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("auth.json"), b"[redacted]");
        touch(&home.join("config.toml"), b"[redacted]");
        let units = run(home);
        for rel in ["auth.json", "config.toml"] {
            let u = units.iter().find(|u| u.relative_path() == rel).unwrap();
            assert!(u.protected(), "{rel} must be protected");
            assert_eq!(u.action(), AgentActionCapability::None);
        }
    }

    #[test]
    fn log_directory_is_actionable_cache_or_log_trash() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("log").join("codex.log"), b"debug line");
        let units = run(home);
        let u = units.iter().find(|u| u.relative_path() == "log").unwrap();
        assert!(!u.protected());
        assert_eq!(u.action(), AgentActionCapability::CacheOrLogTrash);
    }

    #[test]
    fn unclassified_residual_captures_unknown_top_level_entries() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("some-future-file.json"), b"{}");
        let units = run(home);
        let residual = units
            .iter()
            .find(|u| u.relative_path() == "(unclassified residual)")
            .expect("residual present");
        assert!(
            residual
                .note
                .as_deref()
                .unwrap()
                .contains("some-future-file.json")
        );
    }

    #[test]
    fn identification_cost_is_bounded_for_many_sessions() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("big-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        for i in 0..500 {
            let jsonl = home.join(format!(
                "sessions/2026/09/21/rollout-2026-09-21T10-00-{i:02}-77777777-7777-4777-8{i:03}-777777777777.jsonl"
            ));
            touch(&jsonl, &vec![b'x'; 200_000]);
        }
        let start = SystemTime::now();
        let units = run(home);
        let elapsed = SystemTime::now().duration_since(start).unwrap_or_default();
        eprintln!("[measured] codex identify() over 500 synthetic sessions took {elapsed:?}");
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
        // A home holding nothing this adapter has a rule for is reported
        // as an explicit `(unclassified residual)` row naming what was
        // found -- never an empty vec, and never re-read as some other
        // Codex-family layout.
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("unrecognised-v9-store.bin"), b"\x00\x01\x02");
        fs::create_dir_all(home.join("future-layout")).unwrap();
        touch(&home.join("future-layout/thing.dat"), b"opaque");
        let units = run(home);
        assert!(!units.is_empty(), "an unrecognized layout must still speak");
        let residual = units
            .iter()
            .find(|u| u.relative_path() == "(unclassified residual)")
            .expect("an explicit residual row, not silence");
        assert_eq!(residual.category(), AgentCategory::Unclassified);
        assert_eq!(residual.action(), AgentActionCapability::None);
        let note = residual.note.as_deref().unwrap_or_default();
        assert!(
            note.contains("no specific rule")
                && note.contains("unrecognised-v9-store.bin")
                && note.contains("future-layout"),
            "the residual must name what it could not classify: {note}"
        );
        assert!(
            matches!(residual.project_link(), ProjectLinkState::NotApplicable),
            "an unclassified residual is tool-wide, never linked to a project"
        );
    }

    #[test]
    fn canary_content_never_appears_in_output() {
        let canary = "CANARY-CODEX-DO-NOT-LEAK-7c02";
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let jsonl = home.join("sessions/2026/09/21/rollout-2026-09-21T10-00-00-canary.jsonl");
        let content = format!(
            "{canary}\n{{\"role\":\"user\",\"text\":\"{canary}\"}}\nplain body line: {canary}\n"
        );
        touch(&jsonl, content.as_bytes());
        touch(&home.join("history.jsonl"), canary.as_bytes());
        let units = run(home);
        assert!(units.iter().any(|u| u.path == jsonl), "session identified");
        contract::no_content_leak(&units, canary);
    }

    #[test]
    fn rollout_content_is_never_read_and_stale_index_rows_stay_unresolved() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let canary = "CANARY-instructions-3c9e";
        let indexed = home.join("sessions/2026/09/25/rollout-indexed.jsonl");
        let stale = home.join("sessions/2026/09/25/rollout-stale.jsonl");
        touch(
            &indexed,
            format!("{canary} {}\n", "private transcript ".repeat(2048)).as_bytes(),
        );
        touch(
            &stale,
            format!("{{\"cwd\":\"{}\"}}\n", repo.display()).as_bytes(),
        );
        write_index(home, &[(&indexed, &repo)]);

        let (units, counters) = contract::measured(|| run(home));
        assert!(matches!(
            units
                .iter()
                .find(|u| u.path() == indexed)
                .unwrap()
                .project_link(),
            ProjectLinkState::Linked { .. }
        ));
        assert!(matches!(
            units
                .iter()
                .find(|u| u.path() == stale)
                .unwrap()
                .project_link(),
            ProjectLinkState::Unresolved { .. }
        ));
        assert_eq!(
            counters.header_bytes_read, 16,
            "only the SQLite file signature is read; no rollout byte is read"
        );
        contract::no_content_leak(&units, canary);
    }

    #[test]
    fn identification_reads_no_rollout_header_bytes() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let sessions = 40usize;
        let per_file = 100_000usize;
        for i in 0..sessions {
            let jsonl = home.join(format!("sessions/2026/09/21/rollout-cap-{i}.jsonl"));
            touch(&jsonl, &vec![b'x'; per_file]);
        }
        let (units, counters) = contract::measured(|| run(home));
        assert_eq!(
            units
                .iter()
                .filter(|u| u.category() == AgentCategory::Sessions)
                .count(),
            sessions
        );
        assert_eq!(
            counters.header_bytes_read, 0,
            "rollout contents are never read"
        );
    }

    #[test]
    fn protected_categories_default_protected() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("auth.json"), b"[redacted]");
        touch(&home.join("config.toml"), b"model = \"redacted\"\n");
        touch(&home.join("skills/mine/SKILL.md"), b"# redacted");
        touch(&home.join("history.jsonl"), b"{}\n");
        let units = run(home);
        contract::protection_defaults_hold(&units);
        // The two non-default-protected-category units this adapter
        // still protects on its own judgment keep saying why.
        let history = units
            .iter()
            .find(|u| u.relative_path() == "history.jsonl")
            .expect("history.jsonl identified");
        assert!(history.protected());
        assert!(
            history
                .protect_reason()
                .is_some_and(|r| r.contains("prompt history")),
            "the adapter's own protection reason must survive the builder"
        );
    }

    #[test]
    fn project_link_is_declared_or_unresolved_never_basename_guess() {
        // (a) declared metadata naming a real worktree resolves.
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = repo_dir.path().join("declared-worktree");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let declared = home.join("sessions/2026/09/21/rollout-declared.jsonl");
        touch(&declared, b"transcript content cannot declare its project");
        write_index(home, &[(&declared, &repo)]);
        // (b) a session sitting in a directory *named* like a project,
        // declaring nothing, must never become a link.
        let guessed = home.join("sessions/guessable-project-name/rollout-guessed.jsonl");
        touch(&guessed, b"not a json header at all\n");
        let units = run(home);
        let a = units.iter().find(|u| u.path == declared).unwrap();
        match &a.project_link() {
            ProjectLinkState::Linked { source, .. } => {
                assert_eq!(*source, crate::agents::LinkSource::Declared)
            }
            other => panic!("declared cwd must link: {other:?}"),
        }
        let b = units.iter().find(|u| u.path == guessed).unwrap();
        assert!(
            matches!(b.project_link(), ProjectLinkState::Unresolved { .. }),
            "a basename is not evidence: {:?}",
            b.project_link()
        );
        contract::linkage_is_declared_or_explicit(&units, "guessable-project-name");
    }

    /// stack/26's Codex reconciliation defect, falsified directly: build
    /// a home with one entry in every category this adapter knows about
    /// (live session, archived session, every SQLite store with its
    /// `-wal`/`-shm` sidecars, `config.toml`, `auth.json`, `skills/`,
    /// `log/`, `plugins/`) plus one genuinely unrecognized entry, and
    /// assert the identified units' bytes sum to *exactly* the home's
    /// own folded byte total -- not "close", not "within a category or
    /// two of each other". Before this chunk, `state_5.sqlite`/
    /// `logs_2.sqlite`/`thread_history_1.sqlite` (etc.) reported into
    /// `AgentCategory::Sessions`, `plugins/` fell into the unclassified
    /// residual, and `archived_sessions/` was indistinguishable from
    /// `sessions/` -- category *totals* were wrong even when this
    /// grand total happened to still add up, which is exactly the
    /// silent failure mode a per-category breakdown (`--view agents`)
    /// exists to prevent.
    #[test]
    fn category_totals_reconcile_against_the_homes_folded_bytes() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();

        touch(
            &home.join("sessions/2026/09/21/rollout-live.jsonl"),
            b"live transcript",
        );
        touch(
            &home.join(
                "archived_sessions/2026/01/02/rollout-2026-01-02T00-00-00-\
                 22222222-2222-4222-8222-222222222222.jsonl",
            ),
            b"archived transcript",
        );
        for store in SQLITE_STORES {
            touch(&home.join(store.filename), b"sqlite-bytes-payload");
            touch(&home.join(format!("{}-wal", store.filename)), b"wal-bytes");
            touch(&home.join(format!("{}-shm", store.filename)), b"shm-bytes");
        }
        touch(&home.join("config.toml"), b"model = \"gpt\"\n");
        touch(&home.join("auth.json"), b"{\"token\":\"x\"}");
        touch(&home.join("skills/foo/SKILL.md"), b"# a skill\n");
        touch(&home.join("log/codex.log"), b"debug line\n");
        touch(
            &home.join("plugins/marketplace/example-plugin/1.0.0/plugin.json"),
            b"{\"name\":\"example-plugin\"}",
        );
        // The one entry nothing here has a rule for.
        touch(&home.join("some-future-store.bin"), b"opaque-future-bytes");

        let units = run(home);

        // Every category this fixture touches is represented, and
        // distinctly: a defect that folds two of them together would
        // still pass a bytes-only reconciliation (the totals can agree
        // by coincidence), so this asserts the *set* of categories
        // present first.
        let categories: std::collections::HashSet<AgentCategory> =
            units.iter().map(|u| u.category()).collect();
        for expected in [
            AgentCategory::Sessions,
            AgentCategory::ArchivedSessions,
            AgentCategory::ProtectedDatabases,
            AgentCategory::ProtectedConfig,
            AgentCategory::Logs,
            AgentCategory::Plugins,
            AgentCategory::Unclassified,
        ] {
            assert!(
                categories.contains(&expected),
                "{expected:?} missing from {categories:?}"
            );
        }

        let identified_total: u64 = units.iter().map(|u| u.bytes()).sum();
        let (home_total, _mtime, truncated) = crate::agents::folded_bytes(home, MAX_FOLD_ENTRIES);
        assert!(!truncated, "fixture is far under the fold bound");
        assert_eq!(
            identified_total,
            home_total,
            "identified units: {:?}",
            units
                .iter()
                .map(|u| (u.category(), u.relative_path().to_string(), u.bytes()))
                .collect::<Vec<_>>()
        );
    }
}
