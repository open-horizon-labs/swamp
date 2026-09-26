//! Claude Code identification (#92): sessions, recovery/checkpoint data,
//! caches, logs, plugins, and protected configuration under the home
//! `crate::locations::claude_code::ClaudeCodeDetector` resolves.
//!
//! Layout researched from primary sources during implementation (never
//! from a real `~/.claude` on this machine -- PRIVACY IS A HARD RULE):
//! - <https://code.claude.com/docs/en/claude-directory> (directory table)
//! - <https://code.claude.com/docs/en/settings>
//! - <https://code.claude.com/docs/en/checkpointing> (`/rewind`, `file-history/`)
//! - <https://code.claude.com/docs/en/authentication> (`.credentials.json`, Keychain)
//!
//! `code.claude.com/docs/en/claude-directory` itself states the internal
//! transcript entry format is "internal to Claude Code and changes
//! between versions" -- this adapter never parses more than one JSON
//! field (`cwd`) off a session's *first line*, and treats any parse
//! failure as `ProjectLinkState::Unresolved`, never a panic or a guess.
//!
//! ## Session identity and grouping
//!
//! A session's members are collected from up to four places, three of
//! them exact (named by the session's own id), one a documented,
//! bounded heuristic:
//! - `projects/<project>/<session-id>.jsonl` -- the transcript itself.
//! - `projects/<project>/<session-id>/` -- a companion directory
//!   (`subagents/`, `tool-results/`), exact match by construction (same
//!   directory as the transcript, same basename).
//! - `file-history/<session-id>/` -- exact match (documented path).
//! - `todos/<session-id>*` -- **heuristic**: matched by filename
//!   *prefix*, because the exact todos naming convention is not
//!   documented upstream. Session ids are UUIDv4 (negligible collision
//!   risk for a prefix match); an ambiguous match (more than one
//!   session id is a prefix of the same filename, which cannot happen
//!   for UUIDs of equal length but is checked anyway) is excluded
//!   rather than guessed. See `docs/agent-storage.md`'s "Claude Code"
//!   section for this same note in prose.
//! - `image-cache/<session-id>/`, `uploads/<session-id>/` -- exact match
//!   (documented per-session subdirectories).
//!
//! Any `file-history/`, `todos/`, `image-cache/` or `uploads/` entry
//! that matches no known session id becomes its own small residual unit
//! (never silently dropped, never attached to the wrong session).
//!
//! ## Cost
//!
//! Every directory listing goes through [`IdentifyCtx::list`] (bounded,
//! single-level, symlink-refusing) and every byte total through
//! [`IdentifyCtx::folded_bytes`]. The one content read -- a session's
//! declared `cwd` -- goes through [`IdentifyCtx::derived`] under the
//! derivation kind `"cwd"`, so an unchanged session home costs *zero*
//! header bytes on a second pass rather than one capped read per
//! session. `crates/core/tests/incremental_external_and_agent_measurement.rs`
//! measures exactly that over a 5,000-session synthetic home.

use super::{
    AdapterCapabilities, AgentActionCapability, AgentAdapter, AgentCategory, AgentMember,
    AgentMemberKind, AgentUnitBuilder, CandidateAgentUnit, IdentifyCtx,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const CLAUDE_CODE_TOOL_ID: &str = "claude-code";

/// Bound on any one folded directory's entry count
/// ([`IdentifyCtx::folded_bytes`]).
const MAX_FOLD_ENTRIES: usize = 200_000;
/// Bound on how many bytes of a transcript's first line this adapter
/// will ever read looking for a `cwd` field -- "first N bytes... never
/// whole transcripts" (#91's scan-cost acceptance).
const HEADER_READ_BYTES: usize = 8192;

pub struct Adapter;

impl AgentAdapter for Adapter {
    fn id(&self) -> &'static str {
        CLAUDE_CODE_TOOL_ID
    }
    fn name(&self) -> &'static str {
        "Claude Code"
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::default()
    }
    fn identify(&self, home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
        identify(home, ctx)
    }
}

/// Identifies this tool's units inside `home`. Every `observed_at` is
/// stamped by `crate::agents::discover_and_measure`, not by the adapter
/// itself; `ctx.observed_at()` has it for an adapter that needs one.
pub fn identify(home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
    let mut units = Vec::new();
    let mut claimed_session_ids: HashSet<String> = HashSet::new();

    identify_sessions(home, ctx, &mut units, &mut claimed_session_ids);
    identify_session_keyed_top_level(
        home,
        ctx,
        "file-history",
        AgentMemberKind::FileHistory,
        &claimed_session_ids,
        &mut units,
        "unlinked-file-history",
        "checkpoint data under file-history/ with no matching current session transcript \
         (the session was already removed, or this entry predates this adapter's naming \
         assumption)",
    );
    identify_session_keyed_top_level(
        home,
        ctx,
        "image-cache",
        AgentMemberKind::Attachments,
        &claimed_session_ids,
        &mut units,
        "unlinked-image-cache",
        "attached images with no matching current session transcript",
    );
    identify_session_keyed_top_level(
        home,
        ctx,
        "uploads",
        AgentMemberKind::Attachments,
        &claimed_session_ids,
        &mut units,
        "unlinked-uploads",
        "web/mobile attachments with no matching current session transcript",
    );

    identify_unmatched_todos(home, ctx, &claimed_session_ids, &mut units);
    identify_static_categories(home, ctx, &mut units);
    units
}

/// `todos/` entries no session claimed, folded into one unit.
///
/// Previously these were in no unit at all: `todos/` is excluded from
/// the unclassified residual (its entries are normally *members* of the
/// sessions they belong to), and nothing swept the leftovers -- so an
/// orphan's bytes were silently dropped, which is the one thing
/// `docs/agent-storage.md` promises never happens. A test asserted the
/// gap rather than closing it.
///
/// The note carries what the cited page actually says about this
/// directory, which is not what this adapter used to say: `todos/` is a
/// legacy directory "from older versions", "no longer written"
/// (<https://code.claude.com/docs/en/claude-directory>, retrieved
/// 2026-09-22). So an orphan here is not a cache that will come back --
/// it is residue, and removing it costs nothing.
fn identify_unmatched_todos(
    home: &Path,
    ctx: &IdentifyCtx,
    claimed: &HashSet<String>,
    out: &mut Vec<CandidateAgentUnit>,
) {
    let todos_dir = home.join("todos");
    // `is_dir` rather than a listing, so a home without the legacy
    // directory pays nothing at all.
    if !ctx.is_dir(&todos_dir) {
        return;
    }
    let mut bytes = 0u64;
    let mut mtime_max = 0u64;
    let mut count = 0usize;
    for entry in ctx.list(&todos_dir) {
        if claimed.iter().any(|id| entry.name.starts_with(id.as_str())) {
            continue;
        }
        let path = todos_dir.join(&entry.name);
        let (b, m) = if entry.is_dir {
            let (b, m, _t) = ctx.folded_bytes(&path, MAX_FOLD_ENTRIES);
            (b, m)
        } else {
            match ctx.stat(&path) {
                Ok(meta) => (meta.len(), super::mtime_secs(&meta)),
                Err(_) => continue,
            }
        };
        bytes += b;
        mtime_max = mtime_max.max(m);
        count += 1;
    }
    if count == 0 {
        return;
    }
    out.push(
        AgentUnitBuilder::new(CLAUDE_CODE_TOOL_ID, AgentCategory::Unclassified, todos_dir)
            .relative_path("todos (unlinked)")
            .bytes(bytes)
            .mtime_max(mtime_max)
            .action(AgentActionCapability::None)
            .note(
                "todo lists with no matching current session transcript. todos/ is documented \
                 upstream as a legacy directory from older versions that is no longer written, \
                 so nothing here regenerates",
            )
            .build(),
    );
}

// ---------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------

fn identify_sessions(
    home: &Path,
    ctx: &IdentifyCtx,
    out: &mut Vec<CandidateAgentUnit>,
    claimed: &mut HashSet<String>,
) {
    let projects_dir = home.join("projects");
    let project_names = ctx.dir_names(&projects_dir);
    if project_names.is_empty() {
        return;
    }
    // `todos/` is listed once for the whole home, matched in-memory per
    // session -- never one listing per session. Lazily, because a pass
    // in which every project container is replayed from the store must
    // not pay even that one listing: the containers watch `todos/`'s own
    // stamp instead (`IdentifyCtx::watch`).
    let todos_dir = home.join("todos");
    let todos: std::cell::OnceCell<Vec<PathBuf>> = std::cell::OnceCell::new();
    let todos = || -> &Vec<PathBuf> {
        todos.get_or_init(|| {
            ctx.list(&todos_dir)
                .into_iter()
                .map(|e| todos_dir.join(e.name))
                .collect()
        })
    };

    for project_name in project_names {
        let project_path = projects_dir.join(&project_name);
        // One container per `projects/<encoded-cwd>/`. An unchanged
        // container is replayed from the folded rows the previous pass
        // persisted, paying one `stat` per watched directory and no
        // listing; a changed one is re-identified file by file, where
        // the per-file identification cache still keeps the header reads
        // of the sessions that did not move at zero.
        let units = ctx.container(CLAUDE_CODE_TOOL_ID, &project_path, &|| {
            identify_one_project(home, ctx, &project_path, &todos_dir, todos())
        });
        for unit in &units {
            // The claimed-session set is derived from the units rather
            // than accumulated as the loop runs, so a replayed container
            // claims exactly what an identified one does -- otherwise a
            // reused pass would report every `file-history/<id>` as an
            // orphan.
            if unit.category() == AgentCategory::Sessions
                && let Some(stem) = unit.path().file_stem().and_then(|s| s.to_str())
            {
                claimed.insert(stem.to_string());
            }
        }
        out.extend(units);
    }
}

/// One `projects/<encoded-cwd>/` container's units.
///
/// Everything this reads is either reached through `ctx` (and so
/// recorded as part of the container's reuse key) or declared with
/// [`IdentifyCtx::watch`]: `file-history/`, `image-cache/`, `uploads/`
/// and `todos/` all gain a `<session-id>` entry when a session acquires
/// one, and it is those parent directories' own stamps that move.
fn identify_one_project(
    home: &Path,
    ctx: &IdentifyCtx,
    project_path: &Path,
    todos_dir: &Path,
    todos: &[PathBuf],
) -> Vec<CandidateAgentUnit> {
    let mut out: Vec<CandidateAgentUnit> = Vec::new();
    let folder_slug = project_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    ctx.watch(todos_dir);
    for sibling in ["file-history", "image-cache", "uploads"] {
        ctx.watch(&home.join(sibling));
    }
    {
        let project_path = project_path.to_path_buf();
        let out = &mut out;
        let mut jsonl_files: Vec<PathBuf> = Vec::new();
        let mut companion_dirs: HashMap<String, PathBuf> = HashMap::new();
        for e in ctx.list(&project_path) {
            let p = project_path.join(&e.name);
            if !e.is_dir {
                if p.extension().and_then(|x| x.to_str()) == Some("jsonl") {
                    jsonl_files.push(p);
                }
            } else {
                companion_dirs.insert(e.name.clone(), p);
            }
        }

        for jsonl in jsonl_files {
            let Some(session_id) = jsonl.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let session_id = session_id.to_string();
            let Ok(meta) = ctx.stat(&jsonl) else {
                continue;
            };
            let file_mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let mut members = vec![AgentMember {
                path: jsonl.clone(),
                bytes: meta.len(),
                kind: AgentMemberKind::Transcript,
            }];
            let mut mtime_max = file_mtime;

            if let Some(dir) = companion_dirs.remove(&session_id) {
                let (bytes, mtime, _truncated) = ctx.folded_bytes(&dir, MAX_FOLD_ENTRIES);
                mtime_max = mtime_max.max(mtime);
                members.push(AgentMember {
                    path: dir,
                    bytes,
                    kind: AgentMemberKind::SubagentDir,
                });
            }

            let fh_dir = home.join("file-history").join(&session_id);
            if ctx.is_dir(&fh_dir) {
                let (bytes, mtime, _truncated) = ctx.folded_bytes(&fh_dir, MAX_FOLD_ENTRIES);
                mtime_max = mtime_max.max(mtime);
                members.push(AgentMember {
                    path: fh_dir,
                    bytes,
                    kind: AgentMemberKind::FileHistory,
                });
            }

            for dir_name in ["image-cache", "uploads"] {
                let d = home.join(dir_name).join(&session_id);
                if ctx.is_dir(&d) {
                    let (bytes, mtime, _truncated) = ctx.folded_bytes(&d, MAX_FOLD_ENTRIES);
                    mtime_max = mtime_max.max(mtime);
                    members.push(AgentMember {
                        path: d,
                        bytes,
                        kind: AgentMemberKind::Attachments,
                    });
                }
            }

            for todo in todos {
                let Some(name) = todo.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if name.starts_with(session_id.as_str()) {
                    let Ok(m) = ctx.stat(todo) else {
                        continue;
                    };
                    let t = m
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    mtime_max = mtime_max.max(t);
                    members.push(AgentMember {
                        path: todo.clone(),
                        bytes: m.len(),
                        kind: AgentMemberKind::Todos,
                    });
                }
            }

            let relative_path = relative_to(home, &jsonl);
            out.push(
                AgentUnitBuilder::new(CLAUDE_CODE_TOOL_ID, AgentCategory::Sessions, jsonl.clone())
                    .relative_path(relative_path)
                    .members(members)
                    .mtime_max(mtime_max)
                    // The declared path, not the resolved state: a
                    // container replayed from the store re-resolves it
                    // live, once per distinct project rather than once
                    // per session (`crate::agents::LinkBasis`).
                    // ... and the `projects/<slug>` folder name beside
                    // it, for the shared layer's bounded inference when
                    // no record carried a `cwd` (`KnownWorktrees`).
                    .project_link_declared_or_folder(
                        read_header_cwd(&jsonl, ctx),
                        "no cwd field in the session's first records",
                        folder_slug.clone(),
                    )
                    .action(AgentActionCapability::SessionRemoval)
                    .build(),
            );
        }

        // Anything left in `companion_dirs` matched no session id in
        // this project directory: e.g. `memory/` (auto memory, always
        // present and always unmatched by construction), or a companion
        // directory whose transcript was removed by hand outside this
        // adapter. Sorted, so identification output does not depend on
        // hash order.
        let mut leftovers: Vec<(String, PathBuf)> = companion_dirs.into_iter().collect();
        leftovers.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, dir) in leftovers {
            let (bytes, mtime, _truncated) = ctx.folded_bytes(&dir, MAX_FOLD_ENTRIES);
            let relative_path = relative_to(home, &dir);
            let is_memory = name == "memory";
            let note = if is_memory {
                "per-project persistent notes Claude maintains across sessions (auto memory); \
                 treated as retained work, not a cache"
            } else {
                "companion directory with no matching session transcript in this project \
                 directory"
            };
            let unit = AgentUnitBuilder::new(CLAUDE_CODE_TOOL_ID, AgentCategory::Unclassified, dir)
                .relative_path(relative_path)
                .bytes(bytes)
                .mtime_max(mtime)
                .note(note);
            // `memory/` is retained work, so it is individually
            // protected with that reason. An ordinary orphan companion
            // directory is not protected; it carries the same
            // explanation as its note (the pre-builder literal also set
            // a `protect_reason` on it, which nothing ever read -- the
            // shared layer consults `protect_reason` only for a unit
            // that is actually protected).
            let unit = if is_memory { unit.protect(note) } else { unit };
            out.push(unit.build());
        }
    }
    out
}

/// Reads only the *first line* of `jsonl` (bounded to
/// `HEADER_READ_BYTES`) and returns a top-level string `cwd` field if it
/// has one. The declared path is handed to
/// `AgentUnitBuilder::project_link_declared`, which resolves it to a
/// swamp project identity by walking upward looking for a `.git`
/// directory/file -- `crate::git`'s own identity primitives (shared
/// object store, never a filesystem path), never a basename guess and
/// never a second directory-name decoding heuristic (the
/// `projects/<encoded>` directory name is not reversible to a real path
/// in general: a literal hyphen in a real path is indistinguishable from
/// an encoded path separator).
fn read_header_cwd(jsonl: &Path, ctx: &IdentifyCtx) -> Option<String> {
    // Through the shared *memoised* capped reader, so the privacy bound
    // is one reviewed function rather than fifteen open-coded reads, the
    // cost is counted
    // (`.oh/guardrails/agent-adapters-read-bounded-headers-only.md`), and
    // an unchanged transcript is never re-read: the derived `cwd` is
    // cached against the transcript's own `(size, mtime)`, which is what
    // makes an unchanged 5,000-session home cost zero header bytes.
    ctx.derived(
        CLAUDE_CODE_TOOL_ID,
        "cwd",
        jsonl,
        HEADER_READ_BYTES,
        &|text| {
            // The first record that carries a `cwd` -- real transcripts
            // open with `queue-operation`/summary records that have none
            // (the 2026-09-23 aim review: every one of 163 sessions read
            // "unresolved (no cwd field found)"). Still within the one
            // capped header read: only the lines that fit in
            // `HEADER_READ_BYTES` are seen, and only the `cwd` field of
            // each is used.
            text.lines().find_map(|line| {
                let value: serde_json::Value = serde_json::from_str(line).ok()?;
                value
                    .get("cwd")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            })
        },
    )
}

// ---------------------------------------------------------------------
// Session-keyed top-level directories with no matching session
// (residual, folded into one unit rather than one row per orphan).
// ---------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn identify_session_keyed_top_level(
    home: &Path,
    ctx: &IdentifyCtx,
    dir_name: &str,
    kind: AgentMemberKind,
    claimed: &HashSet<String>,
    out: &mut Vec<CandidateAgentUnit>,
    relative_slug: &str,
    note: &str,
) {
    let base = home.join(dir_name);
    let mut members = Vec::new();
    let mut mtime_max = 0u64;
    for name in ctx.dir_names(&base) {
        if claimed.contains(&name) {
            continue;
        }
        let path = base.join(&name);
        let (bytes, mtime, _truncated) = ctx.folded_bytes(&path, MAX_FOLD_ENTRIES);
        mtime_max = mtime_max.max(mtime);
        members.push(AgentMember { path, bytes, kind });
    }
    if members.is_empty() {
        return;
    }
    out.push(
        AgentUnitBuilder::new(CLAUDE_CODE_TOOL_ID, AgentCategory::Attachments, base)
            .relative_path(format!("{dir_name}/({relative_slug})"))
            .members(members)
            .mtime_max(mtime_max)
            .protect(note)
            .note(note)
            .build(),
    );
}

// ---------------------------------------------------------------------
// Static top-level categories (#92's acceptance: caches/logs/checkpoints/
// plugins/protected-config, each with an explicit identification and
// retention-consequence note).
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
        rel: "settings.json",
        category: AgentCategory::ProtectedConfig,
        action: AgentActionCapability::None,
        protected: true,
        note: "global settings",
    },
    StaticEntry {
        rel: ".credentials.json",
        category: AgentCategory::ProtectedConfig,
        action: AgentActionCapability::None,
        protected: true,
        note: "OAuth fallback credential file (macOS normally migrates this into the system \
               Keychain instead); contents are never read by this adapter",
    },
    StaticEntry {
        rel: "keybindings.json",
        category: AgentCategory::ProtectedConfig,
        action: AgentActionCapability::None,
        protected: true,
        note: "custom keyboard shortcuts",
    },
    StaticEntry {
        rel: "themes",
        category: AgentCategory::ProtectedConfig,
        action: AgentActionCapability::None,
        protected: true,
        note: "custom color themes",
    },
    StaticEntry {
        rel: "rules",
        category: AgentCategory::ProtectedConfig,
        action: AgentActionCapability::None,
        protected: true,
        note: "user-level rules applied to every project",
    },
    StaticEntry {
        rel: "skills",
        category: AgentCategory::ProtectedConfig,
        action: AgentActionCapability::None,
        protected: true,
        note: "personal skill definitions",
    },
    StaticEntry {
        rel: "commands",
        category: AgentCategory::ProtectedConfig,
        action: AgentActionCapability::None,
        protected: true,
        note: "custom slash commands",
    },
    StaticEntry {
        rel: "agents",
        category: AgentCategory::ProtectedConfig,
        action: AgentActionCapability::None,
        protected: true,
        note: "personal subagent definitions",
    },
    StaticEntry {
        rel: "workflows",
        category: AgentCategory::ProtectedConfig,
        action: AgentActionCapability::None,
        protected: true,
        note: "personal dynamic workflow scripts",
    },
    StaticEntry {
        rel: "output-styles",
        category: AgentCategory::ProtectedConfig,
        action: AgentActionCapability::None,
        protected: true,
        note: "custom instruction sets for Claude's responses",
    },
    StaticEntry {
        rel: "agent-memory",
        category: AgentCategory::ProtectedConfig,
        action: AgentActionCapability::None,
        protected: true,
        note: "persistent memory for user-scoped subagents",
    },
    StaticEntry {
        rel: "plans",
        category: AgentCategory::Unclassified,
        action: AgentActionCapability::None,
        protected: true,
        note: "plan-mode documents; retained work, not a cache",
    },
    StaticEntry {
        rel: "sessions",
        category: AgentCategory::Unclassified,
        action: AgentActionCapability::None,
        protected: true,
        note: "per-running-session detection files (active-session bookkeeping); not to be \
               removed while Claude Code may be running",
    },
    StaticEntry {
        rel: "ide",
        category: AgentCategory::Unclassified,
        action: AgentActionCapability::None,
        protected: true,
        note: "named in this epic's research checklist but not confirmed by this chunk's \
               official-documentation fetch; treated as protected/unclassified pending \
               confirmation rather than assumed to be a cache",
    },
    StaticEntry {
        rel: "history.jsonl",
        category: AgentCategory::Sessions,
        action: AgentActionCapability::None,
        protected: true,
        note: "every prompt typed across all sessions, kept for recall/search (Ctrl+R); \
               contains prompt text and is never read by this adapter",
    },
    StaticEntry {
        rel: "backups",
        category: AgentCategory::Checkpoints,
        action: AgentActionCapability::None,
        protected: true,
        note: "earlier versions of the global ~/.claude.json app-state file",
    },
    StaticEntry {
        rel: "paste-cache",
        category: AgentCategory::Attachments,
        action: AgentActionCapability::None,
        protected: true,
        note: "pasted content cache, not scoped to one session; no per-session reference \
               evidence is available, so this adapter does not offer it as a supported action \
               even though Claude Code's own retention treats it as ephemeral",
    },
    StaticEntry {
        rel: "shell-snapshots",
        category: AgentCategory::Caches,
        action: AgentActionCapability::CacheOrLogTrash,
        protected: false,
        note: "shell alias/function snapshots; regenerated automatically",
    },
    // The three legacy directories, and the provenance note that was
    // wrong about all of them.
    //
    // This adapter said `statsig` was "community-documented; not found
    // in this chunk's official-documentation fetch" and modelled it as a
    // cache that "regenerated automatically". Both halves are false. The
    // cited page documents it explicitly, in one row with `todos/` and
    // `logs/`: "Legacy directories from older versions. No longer
    // written." Its own "what you lose" table answers "Nothing. Legacy
    // directories not written by current versions."
    // (https://code.claude.com/docs/en/claude-directory, retrieved
    // 2026-09-22; vendored at
    // `crates/core/tests/fixtures/upstream/claude-code/2026-09-22/claude-directory.md`.)
    //
    // So the consequence text has to change too, in the direction that
    // matters to a user: removing these does not cost a regeneration, it
    // costs nothing, because nothing writes them any more. `todos/` is
    // the exception that keeps a caveat: its per-session entries are
    // still *members* of the sessions this adapter identifies, so an
    // entry that matches a live session is reported with that session
    // and only an unmatched one reaches this rule.
    StaticEntry {
        rel: "statsig",
        category: AgentCategory::Caches,
        action: AgentActionCapability::CacheOrLogTrash,
        protected: false,
        note: "legacy feature-flag/analytics directory from older versions, documented upstream \
               as no longer written; nothing regenerates it",
    },
    StaticEntry {
        rel: "logs",
        category: AgentCategory::Logs,
        action: AgentActionCapability::CacheOrLogTrash,
        protected: false,
        note: "legacy log directory from older versions, documented upstream as no longer \
               written; nothing regenerates it",
    },
    StaticEntry {
        rel: "debug",
        category: AgentCategory::Logs,
        action: AgentActionCapability::CacheOrLogTrash,
        protected: false,
        note: "per-session debug logs; regenerated",
    },
    StaticEntry {
        rel: "plugins/.trash",
        category: AgentCategory::Caches,
        action: AgentActionCapability::CacheOrLogTrash,
        protected: false,
        note: "already marked deleted by the tool itself (deleted synced plugins staging area)",
    },
    StaticEntry {
        rel: "skills/.trash",
        category: AgentCategory::Caches,
        action: AgentActionCapability::CacheOrLogTrash,
        protected: false,
        note: "already marked deleted by the tool itself (deleted synced skills staging area)",
    },
    StaticEntry {
        rel: "plugins",
        category: AgentCategory::Plugins,
        action: AgentActionCapability::None,
        protected: false,
        note: "marketplace configuration and downloaded plugin code; not selectively actionable \
               in this chunk (native marketplace-aware removal is the preferred future \
               mechanism, not a guessed directory delete)",
    },
];

fn identify_static_categories(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    let mut seen_top_level: HashSet<String> = HashSet::new();
    for entry in STATIC_ENTRIES {
        let path = home.join(entry.rel);
        if let Some(first) = entry.rel.split('/').next() {
            seen_top_level.insert(first.to_string());
        }
        if !ctx.exists(&path) {
            continue;
        }
        let (bytes, mtime, truncated) = ctx.folded_bytes(&path, MAX_FOLD_ENTRIES);
        let unit = AgentUnitBuilder::new(CLAUDE_CODE_TOOL_ID, entry.category, path)
            .relative_path(entry.rel)
            .bytes(bytes)
            .mtime_max(mtime)
            .action(entry.action)
            .note(entry.note);
        let unit = if truncated {
            unit.incomplete("directory entry count bound reached; total may be an undercount")
        } else {
            unit
        };
        // `protect` carries the entry's own reason, which is more
        // specific than the category default the builder already
        // applied to a `ProtectedConfig` unit.
        let unit = if entry.protected {
            unit.protect(entry.note)
        } else {
            unit
        };
        out.push(unit.build());
    }

    // Genuine unclassified residual: any other top-level entry this
    // adapter has no specific rule for (a new file/dir a future Claude
    // Code version adds), folded into one unit rather than silently
    // dropped -- #91's "retain unclassified residuals" acceptance.
    let mut residual_bytes = 0u64;
    let mut residual_mtime = 0u64;
    let mut residual_names: Vec<String> = Vec::new();
    for e in ctx.list(home) {
        let name = e.name;
        if name == "projects"
            || name == "file-history"
            || name == "image-cache"
            || name == "uploads"
            || name == "todos"
            || seen_top_level.contains(&name)
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
                CLAUDE_CODE_TOOL_ID,
                AgentCategory::Unclassified,
                home.to_path_buf(),
            )
            .relative_path("(unclassified residual)")
            .bytes(residual_bytes)
            .mtime_max(residual_mtime)
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
    use crate::agents::ProjectLinkState;
    use crate::agents::{IdentificationCache, bounded_io, contract};
    use std::fs;
    use std::time::{Duration, SystemTime};

    fn run(home: &Path) -> Vec<CandidateAgentUnit> {
        let cache = IdentificationCache::disabled();
        identify(home, &IdentifyCtx::new(1, &cache))
    }

    /// `crate::work_counters` is process-global, so this module's two
    /// large session fixtures are serialized against each other: the
    /// 500-session cost fixture's own header reads would otherwise land
    /// inside the header-cap measurement's window and make that number
    /// mean something other than what it claims.
    static MEASURED: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn measured_serial() -> std::sync::MutexGuard<'static, ()> {
        MEASURED.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn touch(path: &Path, content: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn session_line(cwd: &str, canary: &str) -> String {
        format!(
            "{{\"type\":\"user\",\"sessionId\":\"s\",\"cwd\":\"{cwd}\",\"gitBranch\":\"main\",\
             \"message\":{{\"role\":\"user\",\"content\":\"{canary}\"}}}}\n"
        )
    }

    /// The 2026-09-23 aim review on a real machine: every session's first
    /// record was a `queue-operation` with no `cwd`, so all 163 rendered
    /// "unresolved". The link comes from the first record that carries
    /// one, within the same capped header read.
    #[test]
    fn cwd_is_taken_from_the_first_record_that_carries_one() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("real-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let id = "14141414-1414-4141-8141-141414141414";
        let jsonl = home
            .join("projects")
            .join("-real-repo")
            .join(format!("{id}.jsonl"));
        let body = format!(
            "{{\"type\":\"queue-operation\",\"operation\":\"enqueue\",\"sessionId\":\"s\"}}\n\
             {{\"type\":\"summary\",\"summary\":\"x\"}}\n{}",
            session_line(&repo.display().to_string(), "canary-body")
        );
        touch(&jsonl, body.as_bytes());
        let units = run(home);
        let unit = units.iter().find(|u| u.path == jsonl).unwrap();
        match &unit.project_link() {
            ProjectLinkState::Linked { source, .. } => {
                assert_eq!(*source, crate::agents::LinkSource::Declared)
            }
            other => panic!("the third record's cwd must link the session, got {other:?}"),
        }
    }

    #[test]
    fn empty_home_yields_no_units() {
        let dir = tempfile::tempdir().unwrap();
        let units = run(dir.path());
        assert!(units.is_empty());
    }

    #[test]
    fn a_session_is_identified_with_its_companion_members() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("fixture-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let session_id = "11111111-1111-4111-8111-111111111111";
        let proj_dir = home.join("projects").join("-fixture-repo-encoded");
        let jsonl = proj_dir.join(format!("{session_id}.jsonl"));
        let canary = "CANARY-DO-NOT-LEAK-3f9a";
        touch(
            &jsonl,
            session_line(&repo.display().to_string(), canary).as_bytes(),
        );
        touch(
            &proj_dir.join(session_id).join("subagents").join("a.jsonl"),
            session_line(&repo.display().to_string(), canary).as_bytes(),
        );
        touch(
            &home.join("file-history").join(session_id).join("snap.txt"),
            b"[redacted]",
        );
        touch(
            &home
                .join("todos")
                .join(format!("{session_id}-agent-1.json")),
            b"[]",
        );

        let units = run(home);
        let session_unit = units
            .iter()
            .find(|u| u.category() == AgentCategory::Sessions && u.path == jsonl)
            .expect("session unit present");
        assert_eq!(
            session_unit.members().len(),
            4,
            "transcript + subagents + file-history + todos"
        );
        assert!(matches!(
            session_unit.project_link(),
            ProjectLinkState::Linked { .. }
        ));
        assert_eq!(session_unit.action(), AgentActionCapability::SessionRemoval);

        // Privacy: the canary never appears in any unit's identity/path
        // fields (the only place content could have leaked into).
        let serialized = format!("{units:?}");
        assert!(
            !serialized.contains(canary),
            "prompt content leaked into identification output"
        );
    }

    #[test]
    fn missing_project_path_is_reported_as_missing() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let session_id = "22222222-2222-4222-8222-222222222222";
        let jsonl = home
            .join("projects")
            .join("-nonexistent")
            .join(format!("{session_id}.jsonl"));
        touch(&jsonl, session_line("/nonexistent/gone", "x").as_bytes());
        let units = run(home);
        let unit = units.iter().find(|u| u.path == jsonl).unwrap();
        assert!(matches!(
            unit.project_link(),
            ProjectLinkState::Missing { .. }
        ));
    }

    #[test]
    fn path_exists_but_is_not_a_git_checkout() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let plain_dir = home.join("plain");
        fs::create_dir_all(&plain_dir).unwrap();
        let session_id = "33333333-3333-4333-8333-333333333333";
        let jsonl = home
            .join("projects")
            .join("-plain")
            .join(format!("{session_id}.jsonl"));
        touch(
            &jsonl,
            session_line(&plain_dir.display().to_string(), "x").as_bytes(),
        );
        let units = run(home);
        let unit = units.iter().find(|u| u.path == jsonl).unwrap();
        assert!(matches!(
            unit.project_link(),
            ProjectLinkState::NotAProject { .. }
        ));
    }

    #[test]
    fn malformed_transcript_is_unresolved_not_a_panic() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let session_id = "44444444-4444-4444-8444-444444444444";
        let jsonl = home
            .join("projects")
            .join("-x")
            .join(format!("{session_id}.jsonl"));
        touch(&jsonl, b"{not valid json at all");
        let units = run(home);
        let unit = units.iter().find(|u| u.path == jsonl).unwrap();
        assert!(matches!(
            unit.project_link(),
            ProjectLinkState::Unresolved { .. }
        ));
    }

    #[test]
    fn empty_transcript_is_unresolved_not_a_panic() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let session_id = "55555555-5555-4555-8555-555555555555";
        let jsonl = home
            .join("projects")
            .join("-x")
            .join(format!("{session_id}.jsonl"));
        touch(&jsonl, b"");
        let units = run(home);
        let unit = units.iter().find(|u| u.path == jsonl).unwrap();
        assert!(matches!(
            unit.project_link(),
            ProjectLinkState::Unresolved { .. }
        ));
    }

    #[test]
    fn in_progress_transcript_with_no_trailing_newline_still_parses() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("live-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let session_id = "66666666-6666-4666-8666-666666666666";
        let jsonl = home
            .join("projects")
            .join("-live")
            .join(format!("{session_id}.jsonl"));
        let mut line = session_line(&repo.display().to_string(), "x");
        line.pop(); // drop trailing newline: still-being-appended file
        touch(&jsonl, line.as_bytes());
        let units = run(home);
        let unit = units.iter().find(|u| u.path == jsonl).unwrap();
        assert!(matches!(
            unit.project_link(),
            ProjectLinkState::Linked { .. }
        ));
    }

    #[test]
    fn protected_config_categories_are_protected_by_default() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        touch(&home.join(".credentials.json"), b"[redacted]");
        let units = run(home);
        for rel in ["settings.json", ".credentials.json"] {
            let u = units.iter().find(|u| u.relative_path() == rel).unwrap();
            assert!(u.protected(), "{rel} must be protected");
            assert_eq!(u.action(), AgentActionCapability::None);
        }
    }

    #[test]
    fn cache_and_log_categories_are_actionable_and_unprotected() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("shell-snapshots").join("snap.sh"), b"alias x=y");
        touch(&home.join("debug").join("log.txt"), b"debug line");
        let units = run(home);
        for rel in ["shell-snapshots", "debug"] {
            let u = units.iter().find(|u| u.relative_path() == rel).unwrap();
            assert!(!u.protected(), "{rel} must not be protected");
            assert_eq!(u.action(), AgentActionCapability::CacheOrLogTrash);
        }
    }

    #[test]
    fn history_jsonl_is_individually_protected_despite_its_category() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("history.jsonl"), b"{}\n");
        let units = run(home);
        let u = units
            .iter()
            .find(|u| u.relative_path() == "history.jsonl")
            .unwrap();
        assert_eq!(u.category(), AgentCategory::Sessions);
        assert!(u.protected());
        assert_eq!(u.action(), AgentActionCapability::None);
    }

    #[test]
    fn per_project_memory_is_retained_work_not_an_orphan_cache() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(
            &home
                .join("projects")
                .join("-x")
                .join("memory")
                .join("notes.md"),
            b"[redacted]",
        );
        let units = run(home);
        let u = units
            .iter()
            .find(|u| u.relative_path() == "projects/-x/memory")
            .expect("memory directory is its own unit");
        assert_eq!(u.category(), AgentCategory::Unclassified);
        assert!(u.protected(), "auto memory is retained work");
        assert!(
            u.note
                .as_deref()
                .unwrap_or_default()
                .contains("auto memory"),
            "{:?}",
            u.note
        );
        assert_eq!(u.action(), AgentActionCapability::None);
    }

    /// THE CLOSED GAP (2026-09-22). This test used to assert the gap --
    /// "the file is not folded into the static/residual scan ...
    /// documented gap" -- which is to say it asserted that some bytes on
    /// the disk appeared in no unit at all. That is exactly what
    /// `docs/agent-storage.md` promises never happens. The assertion is
    /// now on the bytes.
    #[test]
    fn unmatched_todos_are_folded_into_one_orphan_note_not_dropped() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(
            &home.join("todos").join("no-such-session.json"),
            &vec![b'x'; 2048],
        );
        let units = run(home);
        // Still never a fabricated session.
        assert!(
            units
                .iter()
                .all(|u| u.category() != AgentCategory::Sessions)
        );
        let orphan = units
            .iter()
            .find(|u| u.relative_path() == "todos (unlinked)")
            .expect("an unmatched todos entry must be reported somewhere");
        assert_eq!(orphan.bytes(), 2048, "exactly its bytes, and only once");
        assert_eq!(orphan.action(), AgentActionCapability::None);
        let note = orphan.note.as_deref().unwrap_or_default();
        assert!(
            note.contains("no longer written"),
            "the note must carry what the cited page says about todos/: {note}"
        );
    }

    /// A todos entry that *does* belong to a live session stays with
    /// that session and must not also appear in the orphan unit --
    /// otherwise the same bytes are counted twice.
    #[test]
    fn a_matched_todos_entry_is_not_also_swept_as_an_orphan() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let id = "11111111-1111-4111-8111-111111111111";
        touch(
            &home.join("projects/-p").join(format!("{id}.jsonl")),
            b"{\"type\":\"user\"}\n",
        );
        touch(
            &home.join("todos").join(format!("{id}-agent.json")),
            &vec![b'y'; 512],
        );
        let units = run(home);
        let session = units
            .iter()
            .find(|u| u.category() == AgentCategory::Sessions)
            .expect("session identified");
        assert!(
            session
                .members()
                .iter()
                .any(|m| m.kind == AgentMemberKind::Todos),
            "the matching todos entry belongs to its session: {:?}",
            session.members()
        );
        assert!(
            !units
                .iter()
                .any(|u| u.relative_path() == "todos (unlinked)"),
            "a claimed todos entry must not also be swept as an orphan"
        );
    }

    #[test]
    fn unlinked_file_history_is_its_own_explicit_unit() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(
            &home
                .join("file-history")
                .join("99999999-9999-4999-8999-999999999999")
                .join("snap.txt"),
            b"[redacted]",
        );
        let units = run(home);
        let u = units
            .iter()
            .find(|u| u.relative_path() == "file-history/(unlinked-file-history)")
            .expect("orphan file-history is reported, never dropped");
        assert!(u.protected());
        assert_eq!(u.action(), AgentActionCapability::None);
        assert_eq!(u.members().len(), 1);
    }

    #[test]
    fn identification_cost_is_bounded_for_many_sessions() {
        // #91's scan-cost acceptance: measure identification of 500
        // synthetic sessions and assert it completes quickly (bounded
        // by directory names + first-line reads, never whole
        // transcripts). This is a cost *bound* assertion, not a formal
        // benchmark; see .oh/sessions/2026-09-21-agent-storage-claude-code.md
        // for the measured number this test's threshold is derived from.
        let _serial = measured_serial();
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("big-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let proj_dir = home.join("projects").join("-big-repo-encoded");
        for i in 0..500 {
            let session_id = format!("77777777-7777-4777-8{i:03}-777777777777");
            let jsonl = proj_dir.join(format!("{session_id}.jsonl"));
            // A larger-than-header body: only the first line may ever
            // be read by this adapter.
            let mut content = session_line(&repo.display().to_string(), "unread-canary");
            content.push_str(&"x".repeat(200_000));
            touch(&jsonl, content.as_bytes());
        }
        let start = SystemTime::now();
        let units = run(home);
        let elapsed = SystemTime::now().duration_since(start).unwrap_or_default();
        eprintln!("[measured] identify() over 500 synthetic sessions took {elapsed:?}");
        assert_eq!(
            units
                .iter()
                .filter(|u| u.category() == AgentCategory::Sessions)
                .count(),
            500
        );
        assert!(
            elapsed < Duration::from_secs(10),
            "identifying 500 sessions took {elapsed:?}, expected well under 10s from bounded reads"
        );
    }

    // --- the five contract tests ---------------------------------------

    #[test]
    fn unknown_format_is_explicit_not_empty() {
        // A home this adapter has no rule for is never an empty vec and
        // never re-interpreted as some other tool's layout: an unknown
        // top-level entry becomes the `(unclassified residual)` row
        // naming it, and a project-directory entry that is not a session
        // becomes its own unit saying exactly that.
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("some-future-file.json"), b"{}");
        touch(
            &home
                .join("projects")
                .join("-x")
                .join("some-future-companion")
                .join("data.bin"),
            b"\x00\x01",
        );
        let units = run(home);
        assert!(
            !units.is_empty(),
            "an unrecognized layout must still surface units"
        );

        let residual = units
            .iter()
            .find(|u| u.relative_path() == "(unclassified residual)")
            .expect("residual unit present");
        assert_eq!(residual.category(), AgentCategory::Unclassified);
        assert!(
            residual
                .note
                .as_deref()
                .unwrap()
                .contains("some-future-file.json"),
            "the residual must name what it could not classify: {:?}",
            residual.note
        );

        let orphan = units
            .iter()
            .find(|u| u.relative_path() == "projects/-x/some-future-companion")
            .expect("an unrecognized project-directory entry is still a unit");
        assert!(
            orphan
                .note
                .as_deref()
                .unwrap_or_default()
                .contains("no matching session transcript"),
            "{:?}",
            orphan.note
        );
        assert!(
            units
                .iter()
                .all(|u| u.category() != AgentCategory::Sessions),
            "an unrecognized entry must never be guessed into a session"
        );
    }

    #[test]
    fn canary_content_never_appears_in_output() {
        let canary = "CANARY-CLAUDE-CODE-DO-NOT-LEAK-91ab";
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("canary-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let session_id = "88888888-8888-4888-8888-888888888888";
        let proj_dir = home.join("projects").join("-canary-repo-encoded");
        // The canary is seeded on the header line this adapter *does*
        // read, and again in the body it must never reach.
        let mut content = session_line(&repo.display().to_string(), canary);
        content.push_str(&format!(
            "{{\"role\":\"assistant\",\"text\":\"{canary}\"}}\n"
        ));
        content.push_str(&format!("more body: {canary}\n"));
        touch(
            &proj_dir.join(format!("{session_id}.jsonl")),
            content.as_bytes(),
        );
        touch(
            &home.join("file-history").join(session_id).join("snap.txt"),
            format!("{canary}\n").as_bytes(),
        );
        touch(
            &home.join("history.jsonl"),
            format!("{canary}\n").as_bytes(),
        );

        let units = run(home);
        assert!(!units.is_empty());
        contract::no_content_leak(&units, canary);
    }

    #[test]
    fn identification_reads_no_more_than_header_cap() {
        // Several large transcripts: the per-session cost is one capped
        // header read, so the measured byte total is far below the
        // fixture's own size and within the shared cap.
        const SESSIONS: usize = 20;
        const BODY_BYTES: usize = 300_000;
        let _serial = measured_serial();
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("cap-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let proj_dir = home.join("projects").join("-cap-repo-encoded");
        let mut fixture_bytes = 0u64;
        for i in 0..SESSIONS {
            let session_id = format!("aaaaaaaa-aaaa-4aaa-8{i:03}-aaaaaaaaaaaa");
            let mut content = session_line(&repo.display().to_string(), "unread-canary");
            content.push_str(&"x".repeat(BODY_BYTES));
            fixture_bytes += content.len() as u64;
            touch(
                &proj_dir.join(format!("{session_id}.jsonl")),
                content.as_bytes(),
            );
        }

        let (units, counters) = contract::measured(|| run(home));
        assert_eq!(
            units
                .iter()
                .filter(|u| u.category() == AgentCategory::Sessions)
                .count(),
            SESSIONS
        );
        assert!(
            counters.header_bytes_read > 0,
            "the declared cwd has to be read from somewhere on a first pass"
        );
        assert!(
            counters.header_bytes_read < fixture_bytes,
            "identification read {} of the fixture's {fixture_bytes} bytes; transcripts are \
             never read whole",
            counters.header_bytes_read
        );
        // One capped read per session is 20 x 8 KiB = 160 KiB against a
        // ~6 MiB fixture. The factor of two is slack for the
        // process-global counter, which a concurrently running test in
        // another module can also add to; the point being proven is the
        // order of magnitude, not an exact syscall total.
        assert!(
            counters.header_bytes_read <= 2 * (SESSIONS as u64) * HEADER_READ_BYTES as u64,
            "this adapter's own per-session cap is {HEADER_READ_BYTES} bytes, and it read {}",
            counters.header_bytes_read
        );
        // And within the shared ceiling every adapter is held to.
        const { assert!(HEADER_READ_BYTES <= bounded_io::MAX_HEADER_BYTES) };
        contract::within_header_cap(counters, SESSIONS as u64);
    }

    #[test]
    fn protected_categories_default_protected() {
        // Claude Code has real default-protected units: credentials,
        // settings, keybindings, themes and rules all land in
        // `ProtectedConfig`, so the shared assertion has material to
        // work with rather than a hand-built stand-in.
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        touch(&home.join(".credentials.json"), b"[redacted]");
        touch(&home.join("keybindings.json"), b"{}");
        touch(&home.join("themes").join("dark.json"), b"{}");
        touch(&home.join("rules").join("house-style.md"), b"# rules");
        // A cache alongside them, so the helper also proves the default
        // does not spill onto categories that are meant to be actionable.
        touch(&home.join("shell-snapshots").join("snap.sh"), b"alias x=y");

        let units = run(home);
        contract::protection_defaults_hold(&units);
        assert_eq!(
            units
                .iter()
                .filter(|u| u.category() == AgentCategory::ProtectedConfig)
                .count(),
            5,
            "settings, credentials, keybindings, themes, rules"
        );
        let cache = units
            .iter()
            .find(|u| u.relative_path() == "shell-snapshots")
            .expect("cache unit present");
        assert!(!cache.protected());
        assert_eq!(cache.action(), AgentActionCapability::CacheOrLogTrash);
    }

    #[test]
    fn project_link_is_declared_or_unresolved_never_basename_guess() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();

        // (a) A session whose own metadata declares a real worktree.
        let declared_repo = home.join("declared-repo");
        fs::create_dir_all(declared_repo.join(".git")).unwrap();
        let linked_id = "12121212-1212-4121-8121-121212121212";
        let linked_jsonl = home
            .join("projects")
            .join("-declared-repo")
            .join(format!("{linked_id}.jsonl"));
        touch(
            &linked_jsonl,
            session_line(&declared_repo.display().to_string(), "x").as_bytes(),
        );

        // (b) A session sitting in a directory *named* after a real git
        // checkout, declaring nothing. The encoded directory name is not
        // reversible to a path, so the only honest answer is Unresolved.
        let tempting = home.join("basename-only-repo");
        fs::create_dir_all(tempting.join(".git")).unwrap();
        let unresolved_id = "13131313-1313-4131-8131-131313131313";
        let unresolved_jsonl = home
            .join("projects")
            .join("-basename-only-repo")
            .join(format!("{unresolved_id}.jsonl"));
        touch(&unresolved_jsonl, b"{\"type\":\"user\"}\n");

        let units = run(home);

        let linked = units.iter().find(|u| u.path == linked_jsonl).unwrap();
        match &linked.project_link() {
            ProjectLinkState::Linked { source, .. } => {
                assert_eq!(*source, crate::agents::LinkSource::Declared)
            }
            other => panic!("a declared cwd must resolve to a link, got {other:?}"),
        }

        let unresolved = units.iter().find(|u| u.path == unresolved_jsonl).unwrap();
        match &unresolved.project_link() {
            ProjectLinkState::Unresolved { reason } => {
                assert!(reason.contains("cwd"), "{reason}")
            }
            other => panic!("a session declaring nothing must be Unresolved, got {other:?}"),
        }

        contract::linkage_is_declared_or_explicit(&units, "basename-only-repo");
    }
}
