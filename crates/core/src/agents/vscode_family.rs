//! Shared identification for the VS-Code-family editors/extensions in
//! this catalog: **forks** that keep their own editor profile
//! (`crate::agents::cursor`, `crate::agents::windsurf` -- a `User/`
//! directory with `globalStorage`/`workspaceStorage`/`History`), and
//! **extensions** installed into one or more of those hosts plus stock
//! VS Code (`crate::agents::cline`, `crate::agents::roo_code` -- a
//! `globalStorage/<extension-id>/` directory holding `tasks/<id>/`).
//! Both shapes share the same underlying VS Code storage conventions
//! (`state.vscdb` SQLite key-value stores, `workspace.json` folder-URI
//! linkage), so this module holds the one real implementation; each
//! tool-specific adapter is a thin, honestly-labelled wrapper -- per the
//! brief's "extend the shared model only where a tool genuinely needs a
//! new concept".
//!
//! Sourced from community reverse-engineering during implementation
//! (never a real editor profile on this machine -- PRIVACY IS A HARD
//! RULE); see `crate::locations::cursor`'s doc comment for the primary
//! citations (cursaves, cursor-chat-browser) this module's `state.vscdb`
//! shape is based on, and `crate::locations::cline`/`roo_code` for the
//! `tasks/<id>/` shape's citations.
//!
//! `state.vscdb`(+`-wal`/`-shm`) is **never** opened by this module --
//! folded as one protected `Database`-kind unit, unconditionally, same
//! discipline `crate::agents::opencode`'s `opencode.db` uses. Guardrail:
//! "WAL/SHM never removed" -- `is_sqlite_like` in `crate::actions`
//! refuses any selective action on a path with this kind regardless.
//!
//! ## Task linkage is per-tool, and one tool has none
//!
//! Where a task's project comes from is **not** shared: it is each
//! extension's own schema, so the caller passes a [`TaskLinkSource`]
//! rather than this module assuming one field name for everybody. That
//! distinction exists because the previous assumption was wrong.
//!
//! Verified 2026-09-21 against upstream source:
//!
//! * **Roo Code** does record it per task, but in
//!   `tasks/<id>/history_item.json`, not `task_metadata.json`:
//!   `src/core/task-persistence/TaskHistoryStore.ts` writes a
//!   `HistoryItem` with a `workspace` field there and indexes it in
//!   `tasks/_index.json`, while
//!   `src/core/context-tracking/FileContextTrackerTypes.ts` defines
//!   `TaskMetadata` as `{ files_in_context }` only
//!   (`RooCodeInc/Roo-Code` main @ `b867ec9145750d0ae1ff7f02d35406e9bf2a0b16`).
//! * **Cline** does not record it in any file this module reads.
//!   `apps/vscode/src/core/context/context-tracking/ContextTrackerTypes.ts`
//!   defines `TaskMetadata` as `{ files_in_context, model_usage,
//!   environment_history }` -- there is no `workspace` field. The real
//!   source is the extension's `taskHistory` global state
//!   (`apps/vscode/src/shared/HistoryItem.ts`'s *optional*
//!   `cwdOnTaskInitialization`), which lives inside `state.vscdb` --
//!   the SQLite store this module refuses to open -- or under
//!   `~/.cline/data/state/taskHistory.json`, a root this catalog does
//!   not model (`cline/cline` main @
//!   `d4d3d9f31f309f89d0327e2b48ab6f775030595e`).
//!
//! Until this chunk, both read `task_metadata.json`'s `workspace` field,
//! which exists in neither schema -- so every Cline and Roo Code task
//! was silently `Unresolved` for a reason that named the wrong file.
//! Roo Code now resolves properly; Cline says exactly why it cannot.

use super::{
    AdapterCapabilities, AgentActionCapability, AgentCategory, AgentMember, AgentMemberKind,
    AgentUnitBuilder, CandidateAgentUnit, IdentifyCtx, ProjectLinkState, mtime_secs,
    resolve_declared_path,
};
use std::path::{Path, PathBuf};

const MAX_FOLD_ENTRIES: usize = 200_000;
/// Bound on a task's own small metadata file -- never its conversation
/// content (guardrail: no prompt/transcript content in output).
const MAX_METADATA_SCAN_BYTES: usize = 65_536;
/// Bound on a *shared* history file -- one array covering every task on
/// this host, so it is read once per host rather than once per task. The
/// same hard ceiling as every other content read
/// (`bounded_io::MAX_HEADER_BYTES`); a history past it is reported
/// truncated, never silently treated as "this task declares nothing".
const MAX_HISTORY_SCAN_BYTES: usize = 65_536;

/// Where one extension records a task's project, if anywhere. The
/// caller's own verified schema, never this module's guess.
#[derive(Debug, Clone, Copy)]
pub enum TaskLinkSource {
    /// A JSON file inside the task directory with a top-level string
    /// field naming an absolute path.
    DeclaredField {
        file: &'static str,
        field: &'static str,
    },
    /// This extension records the project somewhere this catalog does
    /// not read. `reason` is surfaced verbatim as the `Unresolved`
    /// reason, so a user sees *which* store holds the answer.
    NotInAnyFileWeRead { reason: &'static str },
    /// One JSON array of history entries, beside the task directories
    /// rather than inside them, read **once per host** and indexed by
    /// task id -- the shape Cline uses
    /// (`{globalStorage}/state/taskHistory.json`, an array of
    /// `HistoryItem`). `file` is relative to the extension's
    /// globalStorage directory.
    ///
    /// Read through the same capped reader as everything else, so a
    /// history longer than the cap is *partly* readable and honestly
    /// reported: entries inside the window resolve, entries past it get
    /// an `Unresolved` reason that says the file was truncated at the
    /// bound rather than pretending the field is absent.
    SharedHistoryFile {
        file: &'static str,
        id_field: &'static str,
        path_field: &'static str,
        /// Named in the `Unresolved` reason for a task the file does not
        /// mention, so the user is told which upstream spelling was
        /// looked for.
        ambiguity_note: &'static str,
    },
}

/// `task id -> declared working directory`, plus whether the bounded
/// read saw the end of the file.
struct SharedHistory {
    by_id: std::collections::HashMap<String, String>,
    truncated: bool,
}

/// Parses as many complete top-level objects as the capped read window
/// contains.
///
/// Deliberately not `serde_json::from_str::<Vec<Value>>`: the read is
/// capped (privacy is the hard contract, and a task history holds the
/// first line of every prompt), so the text is usually a *prefix* of a
/// JSON array and would not parse as one. Scanning for balanced
/// `{...}` at depth 1 and parsing each on its own recovers every entry
/// the window actually contains and drops the partial one at the end.
/// String and escape state are tracked so a brace inside a task title
/// cannot end an object early.
fn parse_shared_history(text: &str, id_field: &str, path_field: &str) -> SharedHistory {
    let mut by_id = std::collections::HashMap::new();
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut closed_any = false;
    for (i, b) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if *b == b'\\' {
                escaped = true;
            } else if *b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    closed_any = true;
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text[start..=i])
                        && let Some(id) = value.get(id_field).and_then(|v| v.as_str())
                        && let Some(cwd) = value
                            .get(path_field)
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                    {
                        by_id.insert(id.to_string(), cwd.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    // The window ended mid-object, or ended without ever closing one:
    // either way there may be entries beyond it.
    SharedHistory {
        by_id,
        truncated: depth > 0 || !closed_any,
    }
}

/// Which host directories a VS-Code-family profile keeps beside `User/`.
/// Passed in rather than assumed, because the set is per-editor.
#[derive(Debug, Clone, Copy)]
pub struct ProfileShape {
    /// Whether `User/workspaceStorage/<id>/` is a confirmed part of this
    /// editor's layout. When false the directory is still measured if
    /// present, but as an explicitly unconfirmed residual.
    pub workspace_storage_confirmed: bool,
}

/// A `state.vscdb` (+ `-wal`/`-shm`) file at `path`, folded into one
/// protected, non-actionable `Database`-kind unit. `category` lets a
/// caller distinguish the global database from a per-workspace one in
/// its own labelling even though both are handled identically here.
fn vscdb_unit(
    path: PathBuf,
    category: AgentCategory,
    relative_path: String,
    project_link: ProjectLinkState,
    note: &str,
    ctx: &IdentifyCtx,
) -> Option<CandidateAgentUnit> {
    let meta = ctx.stat(&path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mut members = vec![AgentMember {
        path: path.clone(),
        bytes: meta.len(),
        kind: AgentMemberKind::Database,
    }];
    let mut bytes = meta.len();
    let mut mtime_max = mtime_secs(&meta);
    for ext in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{ext}", path.display()));
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
    Some(
        AgentUnitBuilder::new("vscode-family", category, path)
            .relative_path(relative_path)
            .members_keep_bytes(members)
            .bytes(bytes)
            .mtime_max(mtime_max)
            .protect(
                "SQLite chat/composer state (WAL mode); metadata-only, never opened while \
                 writable, WAL/SHM sidecars never removed individually",
            )
            .project_link(project_link)
            .action(AgentActionCapability::None)
            .note(note)
            .build(),
    )
}

// ---------------------------------------------------------------------
// Editor-profile shape (Cursor, Windsurf): a `User/` directory with
// globalStorage/workspaceStorage/History, plus Electron-conventional
// cache/log siblings.
// ---------------------------------------------------------------------

const PROFILE_MARKERS: &[&str] = &["User"];

pub fn identify_editor_profile(
    profile_root: &Path,
    ctx: &IdentifyCtx,
    shape: ProfileShape,
) -> Vec<CandidateAgentUnit> {
    if !ctx.is_dir(profile_root) {
        return Vec::new();
    }
    let user = profile_root.join("User");
    if !PROFILE_MARKERS
        .iter()
        .any(|rel| ctx.exists(&profile_root.join(rel)))
        || !ctx.is_dir(&user)
    {
        return unknown_layout_residual(profile_root, ctx, "no User/ directory found");
    }
    let mut units = Vec::new();

    if let Some(u) = vscdb_unit(
        user.join("globalStorage").join("state.vscdb"),
        AgentCategory::Sessions,
        "User/globalStorage/state.vscdb".to_string(),
        ProjectLinkState::NotApplicable,
        "shared chat/composer content for every project this editor has opened",
        ctx,
    ) {
        units.push(u);
    }

    identify_workspace_storage(&user, ctx, shape, &mut units);

    let history = user.join("History");
    if ctx.is_dir(&history) {
        let (bytes, mtime, truncated) = ctx.folded_bytes(&history, MAX_FOLD_ENTRIES);
        let unit = AgentUnitBuilder::new("vscode-family", AgentCategory::LocalHistory, history)
            .relative_path("User/History")
            .bytes(bytes)
            .mtime_max(mtime)
            .action(AgentActionCapability::CacheOrLogTrash)
            .note("local file-history/undo snapshots, unrelated to AI chat content");
        let unit = if truncated {
            unit.incomplete("directory entry count bound reached")
        } else {
            unit
        };
        units.push(unit.build());
    }

    for (rel, category) in [
        ("Cache", AgentCategory::Caches),
        ("CachedData", AgentCategory::Caches),
        ("CachedExtensionVSIXs", AgentCategory::Caches),
        ("logs", AgentCategory::Logs),
    ] {
        let path = profile_root.join(rel);
        if !ctx.is_dir(&path) {
            continue;
        }
        let (bytes, mtime, truncated) = ctx.folded_bytes(&path, MAX_FOLD_ENTRIES);
        let mut b = AgentUnitBuilder::new("vscode-family", category, path)
            .relative_path(rel)
            .bytes(bytes)
            .mtime_max(mtime)
            .action(AgentActionCapability::CacheOrLogTrash);
        if truncated {
            b = b.incomplete("directory entry count bound reached");
        }
        units.push(b.build());
    }
    units
}

fn identify_workspace_storage(
    user: &Path,
    ctx: &IdentifyCtx,
    shape: ProfileShape,
    out: &mut Vec<CandidateAgentUnit>,
) {
    let base = user.join("workspaceStorage");
    let note = if shape.workspace_storage_confirmed {
        "per-workspace chat/composer index; the conversation content itself lives in the \
         shared global database"
    } else {
        "per-workspace chat/composer index; this editor's own documentation does not name a \
         workspaceStorage directory, so its presence here is observed rather than confirmed"
    };
    for name in ctx.dir_names(&base) {
        let ws_dir = base.join(&name);
        let project_link = workspace_json_link(&ws_dir, ctx);
        let relative_path = format!("User/workspaceStorage/{name}");
        if let Some(u) = vscdb_unit(
            ws_dir.join("state.vscdb"),
            AgentCategory::Sessions,
            format!("{relative_path}/state.vscdb"),
            project_link,
            note,
            ctx,
        ) {
            out.push(u);
        }
    }
}

fn workspace_json_link(ws_dir: &Path, ctx: &IdentifyCtx) -> ProjectLinkState {
    let path = ws_dir.join("workspace.json");
    let Ok(meta) = ctx.stat(&path) else {
        return ProjectLinkState::Unresolved {
            reason: "no workspace.json found in this workspaceStorage directory".to_string(),
        };
    };
    if !meta.is_file() {
        return ProjectLinkState::Unresolved {
            reason: "workspace.json missing or exceeded the bounded read size".to_string(),
        };
    }
    // Read at most the bounded metadata window, and only once per
    // `(size, mtime)`: a workspace that has not changed costs nothing on
    // the next pass.
    let declared = ctx.derived(
        "vscode-family",
        "workspace-json-folder",
        &path,
        MAX_METADATA_SCAN_BYTES,
        &|text| {
            let value = serde_json::from_str::<serde_json::Value>(text).ok()?;
            let uri = value.get("folder")?.as_str()?;
            uri.strip_prefix("file://").map(str::to_string)
        },
    );
    match declared {
        Some(p) => resolve_declared_path(Some(p), ""),
        None => ProjectLinkState::Unresolved {
            reason: "workspace.json held no folder field as a file:// URI within the bounded \
                     metadata read"
                .to_string(),
        },
    }
}

// ---------------------------------------------------------------------
// VS Code extension shape (Cline, Roo Code): globalStorage/<ext-id>/
// with tasks/<task-id>/.
// ---------------------------------------------------------------------

/// Directory-name fragments this catalog recognizes as a VS-Code-family
/// host, checked against `ext_home`'s ancestors so every unit this
/// adapter identifies can be labelled with which host it came from --
/// #99's explicit "model each host as a separate detector location"
/// acceptance extends naturally to display, not just discovery.
const KNOWN_HOSTS: &[&str] = &[
    "Code - Insiders",
    "Code",
    "Cursor",
    "Windsurf",
    ".vscode-server",
];

pub fn host_label(ext_home: &Path) -> &'static str {
    for ancestor in ext_home.ancestors() {
        let Some(name) = ancestor.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        for host in KNOWN_HOSTS {
            if name == *host {
                return match *host {
                    ".vscode-server" => "VS Code Server (remote)",
                    "Code - Insiders" => "VS Code Insiders",
                    "Code" => "VS Code",
                    other => other,
                };
            }
        }
    }
    "unknown host"
}

pub fn identify_extension_globalstorage(
    ext_home: &Path,
    ctx: &IdentifyCtx,
    link_source: TaskLinkSource,
) -> Vec<CandidateAgentUnit> {
    if !ctx.is_dir(ext_home) {
        return Vec::new();
    }
    let host = host_label(ext_home);
    let tasks = ext_home.join("tasks");
    if !ctx.is_dir(&tasks) {
        return unknown_layout_residual(ext_home, ctx, "no tasks/ directory found")
            .into_iter()
            .map(|mut u| {
                u.prefix_relative_path(host);
                u
            })
            .collect();
    }
    // One shared history file per host, read once for every task in it
    // rather than once per task. Lazily, so an extension that does not
    // use this shape never opens it.
    let shared: std::cell::OnceCell<SharedHistory> = std::cell::OnceCell::new();
    let mut units = Vec::new();
    for name in ctx.dir_names(&tasks) {
        let task_dir = tasks.join(&name);
        let (bytes, mtime, truncated) = ctx.folded_bytes(&task_dir, MAX_FOLD_ENTRIES);
        let project_link = match link_source {
            TaskLinkSource::SharedHistoryFile {
                file,
                id_field,
                path_field,
                ambiguity_note,
            } => {
                let history = shared.get_or_init(|| {
                    let path = ext_home.join(file);
                    match ctx.read_header(&path, MAX_HISTORY_SCAN_BYTES) {
                        Some(text) => parse_shared_history(&text, id_field, path_field),
                        // No file at all: nothing is resolvable, and
                        // saying so once is the same answer as saying it
                        // per task.
                        None => SharedHistory {
                            by_id: std::collections::HashMap::new(),
                            truncated: false,
                        },
                    }
                });
                match history.by_id.get(&name) {
                    Some(declared) => resolve_declared_path(Some(declared.clone()), ""),
                    None if history.truncated => ProjectLinkState::Unresolved {
                        reason: format!(
                            "{file} is longer than the {MAX_HISTORY_SCAN_BYTES}-byte bounded \
                             read and this task's entry is past the bound; {ambiguity_note}"
                        ),
                    },
                    None => ProjectLinkState::Unresolved {
                        reason: format!(
                            "this task has no {path_field} entry in {file}; {ambiguity_note}"
                        ),
                    },
                }
            }
            other => task_link(&task_dir, ctx, other),
        };
        let unit =
            AgentUnitBuilder::new("vscode-family", AgentCategory::Sessions, task_dir.clone())
                .relative_path(format!("{host}/tasks/{name}"))
                .members_keep_bytes(vec![AgentMember {
                    path: task_dir,
                    bytes,
                    kind: AgentMemberKind::SessionData,
                }])
                .bytes(bytes)
                .mtime_max(mtime)
                .project_link(project_link)
                .action(AgentActionCapability::SessionRemoval)
                .note(
                    "conversation history, UI messages and any checkpoint snapshots for this task",
                );
        let unit = if truncated {
            unit.incomplete("directory entry count bound reached")
        } else {
            unit
        };
        units.push(unit.build());
    }
    units
}

/// One task's project, from whichever file the *caller's* verified
/// schema names -- or an explicit statement that this extension keeps it
/// somewhere this catalog does not read. See the module docs for the
/// upstream citations behind each.
fn task_link(task_dir: &Path, ctx: &IdentifyCtx, source: TaskLinkSource) -> ProjectLinkState {
    let (file, field) = match source {
        TaskLinkSource::NotInAnyFileWeRead { reason } => {
            return ProjectLinkState::Unresolved {
                reason: reason.to_string(),
            };
        }
        TaskLinkSource::DeclaredField { file, field } => (file, field),
        // Handled by the caller, which reads the one shared file per
        // host rather than once per task.
        TaskLinkSource::SharedHistoryFile { .. } => {
            return ProjectLinkState::Unresolved {
                reason: "shared history file not read for this task".to_string(),
            };
        }
    };
    let path = task_dir.join(file);
    let Ok(meta) = ctx.stat(&path) else {
        return ProjectLinkState::Unresolved {
            reason: format!("no {file} found in this task directory"),
        };
    };
    if !meta.is_file() {
        return ProjectLinkState::Unresolved {
            reason: format!("{file} is not a regular file"),
        };
    }
    let declared = ctx.derived(
        "vscode-family",
        field,
        &path,
        MAX_METADATA_SCAN_BYTES,
        &|text| {
            let value = serde_json::from_str::<serde_json::Value>(text).ok()?;
            value
                .get(field)?
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        },
    );
    resolve_declared_path(
        declared,
        &format!(
            "no {field} field found in {file} within the bounded metadata read; project linkage \
             is left unresolved rather than guessed from the task id"
        ),
    )
}

fn unknown_layout_residual(root: &Path, ctx: &IdentifyCtx, why: &str) -> Vec<CandidateAgentUnit> {
    if !ctx.has_entries(root) {
        return Vec::new();
    }
    let (bytes, mtime, _t) = ctx.folded_bytes(root, MAX_FOLD_ENTRIES);
    vec![
        AgentUnitBuilder::new(
            "vscode-family",
            AgentCategory::Unclassified,
            root.to_path_buf(),
        )
        .relative_path("(unsupported layout version)")
        .bytes(bytes)
        .mtime_max(mtime)
        .action(AgentActionCapability::None)
        .note(format!(
            "{why}; unsupported or future layout version, treated as unknown, not scanned \
             further"
        ))
        .build(),
    ]
}

/// The shape a fork whose profile layout this catalog has confirmed
/// against that editor's own documentation uses.
pub const CONFIRMED_PROFILE: ProfileShape = ProfileShape {
    workspace_storage_confirmed: true,
};

/// The shape for a fork whose `workspaceStorage` directory is observed
/// but not named by its own documentation.
pub const OBSERVED_PROFILE: ProfileShape = ProfileShape {
    workspace_storage_confirmed: false,
};

/// Every VS-Code-family adapter is a thin wrapper, so the wrapper's own
/// `capabilities()` is the only thing that differs; this is the default
/// for a fork's editor profile.
pub const FORK_CAPABILITIES: AdapterCapabilities = AdapterCapabilities {
    decomposes_every_location: false,
    project_local_units: false,
};

/// An extension installed into several editor hosts at once: every
/// authorized location is decomposed, never merged (#99).
pub const EXTENSION_CAPABILITIES: AdapterCapabilities = AdapterCapabilities {
    decomposes_every_location: true,
    project_local_units: false,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::IdentificationCache;
    use std::fs;
    use std::time::{Duration, SystemTime};

    /// Roo Code's confirmed per-task linkage file.
    const ROO: TaskLinkSource = TaskLinkSource::DeclaredField {
        file: "history_item.json",
        field: "workspace",
    };
    /// The honest "not in any file we read" shape. No shipped adapter
    /// uses it today -- Cline moved to `SharedHistoryFile` once the
    /// 2026-09-22 re-review showed the file is inside the directory the
    /// adapter already walks -- but the variant is the one place an
    /// extension whose store really is unreachable can say so, and an
    /// untested variant is a variant nobody has shown works.
    const ELSEWHERE: TaskLinkSource = TaskLinkSource::NotInAnyFileWeRead {
        reason: "this extension records a task's working directory in a store this catalog \
                 does not read (state.vscdb)",
    };

    fn touch(path: &Path, content: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn profile(root: &Path) -> Vec<CandidateAgentUnit> {
        let cache = IdentificationCache::disabled();
        identify_editor_profile(root, &IdentifyCtx::new(1, &cache), CONFIRMED_PROFILE)
    }

    fn ext(home: &Path, link: TaskLinkSource) -> Vec<CandidateAgentUnit> {
        let cache = IdentificationCache::disabled();
        identify_extension_globalstorage(home, &IdentifyCtx::new(1, &cache), link)
    }

    #[test]
    fn empty_profile_root_yields_no_units() {
        let dir = tempfile::tempdir().unwrap();
        assert!(profile(dir.path()).is_empty());
    }

    #[test]
    fn no_user_dir_yields_unsupported_layout_not_a_guess() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("unrelated.txt"), b"hello");
        let units = profile(dir.path());
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].relative_path(), "(unsupported layout version)");
    }

    #[test]
    fn global_vscdb_is_protected_and_folds_sidecars() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        touch(&root.join("User/globalStorage/state.vscdb"), b"sqlite");
        touch(&root.join("User/globalStorage/state.vscdb-wal"), b"wal");
        let units = profile(root);
        let u = units
            .iter()
            .find(|u| u.relative_path() == "User/globalStorage/state.vscdb")
            .expect("global db identified");
        assert!(u.protected());
        assert_eq!(u.action(), AgentActionCapability::None);
        assert_eq!(u.members().len(), 2);
    }

    #[test]
    fn workspace_storage_links_via_workspace_json() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let repo = root.join("fixture-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        touch(
            &root.join("User/workspaceStorage/abc123/workspace.json"),
            format!("{{\"folder\":\"file://{}\"}}", repo.display()).as_bytes(),
        );
        touch(
            &root.join("User/workspaceStorage/abc123/state.vscdb"),
            b"sqlite",
        );
        let units = profile(root);
        let u = units
            .iter()
            .find(|u| u.relative_path().contains("workspaceStorage"))
            .expect("workspace db identified");
        assert!(u.protected());
        assert!(matches!(u.project_link(), ProjectLinkState::Linked { .. }));
    }

    #[test]
    fn an_unconfirmed_workspace_storage_says_so() {
        // Windsurf's own documentation names `globalStorage` but not
        // `workspaceStorage`; the directory is still measured, and the
        // note is what stops that being read as a confirmed layout.
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        touch(&root.join("User/globalStorage/state.vscdb"), b"sqlite");
        touch(
            &root.join("User/workspaceStorage/abc123/state.vscdb"),
            b"sqlite",
        );
        let cache = IdentificationCache::disabled();
        let units = identify_editor_profile(root, &IdentifyCtx::new(1, &cache), OBSERVED_PROFILE);
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

    #[test]
    fn history_and_caches_are_actionable() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        touch(&root.join("User/globalStorage/state.vscdb"), b"sqlite");
        touch(&root.join("User/History/entry1/snapshot"), b"bytes");
        touch(&root.join("CachedExtensionVSIXs/ext.vsix"), b"bytes");
        let units = profile(root);
        let history = units
            .iter()
            .find(|u| u.relative_path() == "User/History")
            .unwrap();
        assert_eq!(history.action(), AgentActionCapability::CacheOrLogTrash);
        assert_eq!(history.category(), AgentCategory::LocalHistory);
        let cache = units
            .iter()
            .find(|u| u.relative_path() == "CachedExtensionVSIXs")
            .unwrap();
        assert_eq!(cache.action(), AgentActionCapability::CacheOrLogTrash);
    }

    #[test]
    fn host_label_recognizes_every_known_host() {
        assert_eq!(
            host_label(Path::new(
                "/tmp/fixture/Library/Application Support/Code/User/globalStorage/ext"
            )),
            "VS Code"
        );
        assert_eq!(
            host_label(Path::new(
                "/tmp/fixture/Library/Application Support/Cursor/User/globalStorage/ext"
            )),
            "Cursor"
        );
        assert_eq!(
            host_label(Path::new(
                "/tmp/fixture/.vscode-server/data/User/globalStorage/ext"
            )),
            "VS Code Server (remote)"
        );
    }

    #[test]
    fn extension_no_tasks_dir_yields_unsupported_layout() {
        let dir = tempfile::tempdir().unwrap();
        let ext_home = dir
            .path()
            .join("Library/Application Support/Code/User/globalStorage/some.ext");
        touch(&ext_home.join("unrelated.json"), b"{}");
        let units = ext(&ext_home, ROO);
        assert_eq!(units.len(), 1);
        assert!(
            units[0]
                .relative_path()
                .ends_with("(unsupported layout version)")
        );
    }

    #[test]
    fn a_roo_task_links_via_history_item_not_task_metadata() {
        // The whole point of the 2026-09-21 upstream re-verification:
        // `task_metadata.json` carries no workspace field in either
        // extension's schema, so reading it linked nothing, ever.
        let dir = tempfile::tempdir().unwrap();
        let ext_home = dir
            .path()
            .join("Library/Application Support/Cursor/User/globalStorage/some.ext");
        let repo = dir.path().join("fixture-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let canary = "CANARY-VSCODE-DO-NOT-LEAK-88ee";
        touch(
            &ext_home.join("tasks/t1/history_item.json"),
            format!("{{\"workspace\":\"{}\"}}", repo.display()).as_bytes(),
        );
        // The file the old code read, with the field it assumed: it must
        // not be what resolves this, and it must not resolve anything on
        // its own.
        touch(
            &ext_home.join("tasks/t1/task_metadata.json"),
            b"{\"files_in_context\":[]}",
        );
        touch(
            &ext_home.join("tasks/t1/api_conversation_history.json"),
            format!("[{{\"content\":\"{canary}\"}}]").as_bytes(),
        );
        let units = ext(&ext_home, ROO);
        let u = units
            .iter()
            .find(|u| u.relative_path().contains("t1"))
            .unwrap();
        assert!(u.relative_path().starts_with("Cursor/tasks/"));
        assert!(matches!(u.project_link(), ProjectLinkState::Linked { .. }));
        assert_eq!(u.action(), AgentActionCapability::SessionRemoval);
        let serialized = format!("{units:?}");
        assert!(!serialized.contains(canary), "content leaked");
    }

    #[test]
    fn a_task_metadata_workspace_field_is_never_believed() {
        // If a future schema *did* grow one, reading it would still be a
        // guess until it is verified; this pins that the caller's
        // declared source is the only thing consulted.
        let dir = tempfile::tempdir().unwrap();
        let ext_home = dir
            .path()
            .join("Library/Application Support/Code/User/globalStorage/some.ext");
        let repo = dir.path().join("fixture-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        touch(
            &ext_home.join("tasks/t9/task_metadata.json"),
            format!("{{\"workspace\":\"{}\"}}", repo.display()).as_bytes(),
        );
        let units = ext(&ext_home, ROO);
        assert!(matches!(
            units[0].project_link(),
            ProjectLinkState::Unresolved { .. }
        ));
    }

    #[test]
    fn an_extension_that_keeps_linkage_elsewhere_says_which_store() {
        let dir = tempfile::tempdir().unwrap();
        let ext_home = dir
            .path()
            .join("Library/Application Support/Code/User/globalStorage/some.ext");
        touch(&ext_home.join("tasks/t1/ui_messages.json"), b"[]");
        let units = ext(&ext_home, ELSEWHERE);
        let ProjectLinkState::Unresolved { reason } = &units[0].project_link() else {
            panic!("expected Unresolved, got {:?}", units[0].project_link());
        };
        assert!(reason.contains("state.vscdb"), "{reason}");
    }

    /// A shared history file longer than the bounded read is reported
    /// *truncated*, never as "this task declares nothing": the entries
    /// inside the window still resolve, and the ones past it say why.
    #[test]
    fn a_history_file_past_the_read_bound_says_so_rather_than_claiming_absence() {
        let dir = tempfile::tempdir().unwrap();
        let ext_home = dir
            .path()
            .join("Library/Application Support/Code/User/globalStorage/some.ext");
        let repo = dir.path().join("in-window-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        touch(&ext_home.join("tasks/first/ui_messages.json"), b"[]");
        touch(&ext_home.join("tasks/last/ui_messages.json"), b"[]");
        let mut history = format!("[{{\"id\":\"first\",\"cwd0\":\"{}\"}}", repo.display());
        // Pad past the cap so `last`'s entry can never be in the window.
        while history.len() < MAX_HISTORY_SCAN_BYTES + 4096 {
            history.push_str(",{\"id\":\"pad\",\"note\":\"");
            history.push_str(&"p".repeat(512));
            history.push_str("\"}");
        }
        history.push_str(",{\"id\":\"last\",\"cwd0\":\"/nowhere\"}]");
        touch(&ext_home.join("state/taskHistory.json"), history.as_bytes());

        let source = TaskLinkSource::SharedHistoryFile {
            file: "state/taskHistory.json",
            id_field: "id",
            path_field: "cwd0",
            ambiguity_note: "note",
        };
        let units = ext(&ext_home, source);
        let first = units
            .iter()
            .find(|u| u.relative_path().ends_with("tasks/first"))
            .unwrap();
        assert!(
            matches!(&first.project_link(), ProjectLinkState::Linked { .. }),
            "an entry inside the window must still resolve: {:?}",
            first.project_link()
        );
        let last = units
            .iter()
            .find(|u| u.relative_path().ends_with("tasks/last"))
            .unwrap();
        let ProjectLinkState::Unresolved { reason } = &last.project_link() else {
            panic!("expected Unresolved, got {:?}", last.project_link());
        };
        assert!(
            reason.contains("bounded read"),
            "a truncated history must say so rather than claim the field is absent: {reason}"
        );
    }

    #[test]
    fn task_without_metadata_is_unresolved_not_guessed() {
        let dir = tempfile::tempdir().unwrap();
        let ext_home = dir
            .path()
            .join("Library/Application Support/Code/User/globalStorage/some.ext");
        touch(&ext_home.join("tasks/t2/ui_messages.json"), b"[]");
        let units = ext(&ext_home, ROO);
        let u = units
            .iter()
            .find(|u| u.relative_path().contains("t2"))
            .unwrap();
        assert!(matches!(
            u.project_link(),
            ProjectLinkState::Unresolved { .. }
        ));
    }

    #[test]
    fn identification_cost_is_bounded_for_many_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let ext_home = dir
            .path()
            .join("Library/Application Support/Code/User/globalStorage/some.ext");
        for i in 0..500 {
            touch(
                &ext_home.join(format!("tasks/t{i}/api_conversation_history.json")),
                &b"x".repeat(50_000),
            );
        }
        let start = SystemTime::now();
        let (units, counted) = crate::work_counters::measured(|| ext(&ext_home, ROO));
        let elapsed = SystemTime::now().duration_since(start).unwrap_or_default();
        eprintln!(
            "[measured] vscode_family identify_extension_globalstorage() over 500 synthetic \
             tasks took {elapsed:?} and read {} header bytes",
            counted.header_bytes_read
        );
        assert_eq!(units.len(), 500);
        assert_eq!(
            counted.header_bytes_read, 0,
            "no task declares a linkage file, so nothing may be read"
        );
        assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
    }

    #[test]
    fn an_unchanged_second_pass_reads_no_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let ext_home = dir
            .path()
            .join("Library/Application Support/Code/User/globalStorage/some.ext");
        let repo = dir.path().join("fixture-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        for i in 0..20 {
            touch(
                &ext_home.join(format!("tasks/t{i}/history_item.json")),
                format!("{{\"workspace\":\"{}\"}}", repo.display()).as_bytes(),
            );
        }
        let cache = IdentificationCache::load(store.path());
        let ctx = IdentifyCtx::new(1, &cache);
        let first = identify_extension_globalstorage(&ext_home, &ctx, ROO);
        assert_eq!(first.len(), 20);
        let (second, counted) = crate::work_counters::measured(|| {
            identify_extension_globalstorage(&ext_home, &ctx, ROO)
        });
        assert_eq!(
            counted.header_bytes_read, 0,
            "an unchanged task must not be re-read"
        );
        assert!(
            second
                .iter()
                .all(|u| matches!(u.project_link(), ProjectLinkState::Linked { .. })),
            "the cached answer must be the same answer"
        );
    }
}
