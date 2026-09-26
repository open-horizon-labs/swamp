//! Shared agent-tool storage discovery, model, and privacy-preserving
//! history (#91), built on chunk B2's external-unit model.
//!
//! An agent tool's home directory (e.g. Claude Code's `~/.claude`) is an
//! ordinary [`crate::external::ExternalUnit`]: detector-resolved
//! (`crate::locations`), identity independent of any project, measured
//! and history-tracked exactly like a Cargo registry or a Homebrew
//! prefix -- no new mechanism for the home level. This module identifies
//! the *interior* of that home into finer-grained [`AgentUnit`]s
//! (sessions, caches, logs, checkpoints, protected config, ...), the way
//! `crate::cargo_artifacts` identifies the interior of a `target/`
//! artifact row into [`crate::artifact::NestedArtifact`]s -- same shape,
//! different domain.
//!
//! History reuse is literal, not just architectural: an [`AgentUnit`]'s
//! growth/regrowth comes from the *same* current+reverse-delta Parquet
//! key family `crate::external`'s home-level units already use
//! (`crate::growth::observe_and_annotate_external` and friends). The key
//! is `(detector_id, category, device, path)`; nothing about that key
//! scheme assumes its `path` is a whole detector-resolved location
//! rather than a path *within* one, so an agent unit's key is simply
//! this tool's `detector_id`, an `"agent:"`-prefixed category string
//! (never collides with `crate::locations::StorageCategory`'s own kebab
//! strings), the home's device, and the unit's own canonical path. One
//! store, one key family, two granularities -- never a third model.
//!
//! Identification cost is bounded by construction: category directories
//! are folded (byte totals, not per-file retention), and session project
//! linkage uses declared metadata. Codex reads only its local SQLite
//! index's `rollout_path` and `cwd` columns; it never reads rollout
//! transcript contents. See
//! `identification_cost_is_bounded` in `claude_code.rs`'s tests for the
//! measured shape, and `.oh/sessions/2026-09-21-agent-storage-claude-code.md`
//! for the recorded number.
//!
//! Privacy is a hard contract, not a convention: session content is not
//! read for linkage, and nothing here puts transcript contents into any
//! `AgentUnit` field. Tests across
//! this module and `claude_code.rs` seed a canary string into fixture
//! session bodies and assert it never appears in any `AgentUnit`,
//! `Debug`, or JSON-serialized output.

pub mod aider;
pub mod bounded_io;
pub mod claude_code;
pub mod cline;
pub mod codex;
pub mod codex_desktop;
mod codex_state;
pub mod continue_dev;
pub mod copilot_cli;
pub mod cursor;
pub mod gemini_cli;
pub mod matrix;
pub mod oh_my_pi;
pub mod opencode;
pub mod pi;
pub mod pi_family;
pub mod registry;
pub mod roo_code;
pub mod vscode_family;
pub mod windsurf;

use crate::fs_gate as fs;
use crate::growth::{ObservedExternal, annotate_readonly_external, observe_and_annotate_external};
use crate::scope::EffectiveScope;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------
// Categories (#91 acceptance: distinct categories only where evidence
// supports them, plus an unclassified residual bucket).
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentCategory {
    Sessions,
    /// A session hidden from a tool's default/live listing but not
    /// deleted -- distinct from [`Self::Sessions`] so the two are never
    /// silently summed into one number a user cannot decompose (the
    /// stack/26 Codex reconciliation defect: archived sessions counted
    /// as ordinary sessions inflated the "sessions" total against `du`).
    ArchivedSessions,
    Attachments,
    Checkpoints,
    Caches,
    /// Command recall and editor undo snapshots: removable, but not rebuildable.
    LocalHistory,
    Logs,
    ManagedWorktrees,
    Plugins,
    ProtectedConfig,
    /// A protected database (SQLite and its `-wal`/`-shm` sidecars, or
    /// equivalent): never opened for a row-level drill-down, never a
    /// target for guessed cleanup (see the store-side guardrail against
    /// guessed SQLite/WAL/SHM cleanup), but not configuration/credential
    /// material either -- its own category, distinct from both
    /// [`Self::Sessions`] (which it used to be folded into) and
    /// [`Self::ProtectedConfig`].
    ProtectedDatabases,
    Unclassified,
}

impl AgentCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sessions => "sessions",
            Self::ArchivedSessions => "archived-sessions",
            Self::Attachments => "attachments",
            Self::Checkpoints => "checkpoints",
            Self::Caches => "caches",
            Self::LocalHistory => "local-history",
            Self::Logs => "logs",
            Self::ManagedWorktrees => "managed-worktrees",
            Self::Plugins => "plugins",
            Self::ProtectedConfig => "protected-config",
            Self::ProtectedDatabases => "protected-databases",
            Self::Unclassified => "unclassified",
        }
    }

    /// The inverse of [`AgentCategory::label`], for the container table.
    /// Written as an exhaustive `match` rather than a serde round-trip
    /// so a new variant fails to compile here instead of silently
    /// becoming `Unclassified` on the way back out of the store.
    pub fn from_label(label: &str) -> Option<Self> {
        let all = [
            Self::Sessions,
            Self::ArchivedSessions,
            Self::Attachments,
            Self::Checkpoints,
            Self::Caches,
            Self::LocalHistory,
            Self::Logs,
            Self::ManagedWorktrees,
            Self::Plugins,
            Self::ProtectedConfig,
            Self::ProtectedDatabases,
            Self::Unclassified,
        ];
        all.into_iter().find(|c| c.label() == label)
    }

    /// The `"agent:"`-prefixed string used in the growth-store key, kept
    /// distinct from `StorageCategory`'s own kebab strings (see module
    /// docs) even though nothing currently collides in practice.
    fn key_str(self) -> String {
        format!("agent:{}", self.label())
    }

    /// Categories protected from any selective action by *default*
    /// (guardrail: "protect credentials/config/skills/automation
    /// definitions by default"). A unit outside these categories can
    /// still be individually protected (`AgentUnit.protected`) by the
    /// adapter (e.g. Claude Code's `history.jsonl`) or by a human
    /// `swamp protect` entry.
    pub fn default_protected(self) -> bool {
        matches!(self, Self::ProtectedConfig | Self::ProtectedDatabases)
    }
}

// ---------------------------------------------------------------------
// Project linkage (#91's required project-linkage acceptance).
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinkSource {
    /// Read directly from metadata the tool itself wrote (a session
    /// index's `cwd` field, a per-project directory's declared path).
    /// Never a basename guess.
    Declared,
    /// Derived by a bounded inference step, never claimed as declared.
    /// Produced today for Claude Code when its `projects/<slug>` folder
    /// name re-encodes exactly one of this pass's known worktree paths
    /// ([`KnownWorktrees`]) and the declared cwd is absent or cannot be
    /// resolved. A successful declared cwd always wins. The slug encoding
    /// is lossy, so a name that re-encodes two known paths is not used;
    /// an inferred link retains the failed cwd reason. It is re-derived
    /// against the current known set on every pass -- never replayed.
    Inferred,
}

/// A tool session/workspace's relationship to a swamp project/worktree.
/// Every variant is a first-class, explicitly reported outcome -- never
/// silently collapsed into "no link" or a fabricated match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum ProjectLinkState {
    /// Resolved to a known Git checkout/worktree identity
    /// (`crate::git`'s own object-store-based project id -- never a
    /// filesystem path or a basename match).
    Linked {
        project_id: String,
        project_name: String,
        project_path: PathBuf,
        source: LinkSource,
        /// Why the session fell back to a tool-specific folder name
        /// instead of resolving its declared cwd. Present only for an
        /// inferred link; preserves missing/non-project cwd evidence.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fallback_reason: Option<String>,
        /// `"main"` or `"linked"` (`crate::report::WorktreeKind`,
        /// stringified for display; never re-typed here).
        worktree_kind: String,
    },
    /// No project metadata could be extracted at all (malformed/empty
    /// metadata, no `cwd` field found in the supported source).
    Unresolved { reason: String },
    /// Metadata names a path that no longer exists on disk.
    Missing { path: PathBuf },
    /// Metadata names a path that exists but is not inside any Git
    /// checkout/worktree this adapter can identify.
    NotAProject { path: PathBuf },
    /// The declared path used to resolve to one project identity and
    /// now resolves to a different one (or none). Not populated by the
    /// Claude Code adapter in this chunk -- it has no record of a
    /// session's *previous* linkage to compare against; recorded here,
    /// honestly unused, rather than guessed.
    Moved { from: PathBuf, to: PathBuf },
    /// The declared path is on a different host than this observation
    /// is running on. Not populated by the Claude Code adapter in this
    /// chunk (no reliable remote-host signal in a session header).
    Remote { host: String, path: PathBuf },
    /// This unit's members collectively named more than one distinct
    /// project identity. Not produced by the Claude Code adapter this
    /// chunk (one session unit always has exactly one declared cwd);
    /// kept for adapters/aggregates where it can genuinely happen.
    Shared { project_ids: Vec<String> },
    /// This unit is inherently tool-wide (a cache, a log directory,
    /// protected config): project linkage does not apply, which is a
    /// different, more honest fact than "we tried and could not tell".
    NotApplicable,
}

// ---------------------------------------------------------------------
// Units
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentMemberKind {
    Transcript,
    SubagentDir,
    Todos,
    FileHistory,
    Attachments,
    CategoryDir,
    ConfigFile,
    /// A SQLite database file (or one of its `-wal`/`-shm` sidecars)
    /// backing session/message/state storage for a tool whose newer
    /// layout moved off flat JSON/JSONL files (#93 Codex, #95 OpenCode's
    /// `opencode.db`, #94 Oh My Pi's `agent.db`). Always folded into one
    /// unit with its sidecars as members, never split -- `is_sqlite_like`
    /// in `crate::actions` refuses any selective action on a path with
    /// this kind unconditionally, independent of category/protection.
    Database,
    /// A session-keyed companion directory/file that is neither a raw
    /// transcript, a subagent dir, todos, file-history, nor an
    /// attachment -- e.g. OpenCode's `storage/message/<session-id>/` and
    /// `storage/session_diff/<session-id>/`, matched to a session by the
    /// same exact-id-match discipline `crate::agents::claude_code` uses
    /// for `file-history/`/`image-cache/`/`uploads/`, never guessed.
    SessionData,
}

impl AgentMemberKind {
    /// The stored spelling used by the container table. Explicit, so a
    /// new variant is a compile error here rather than a value that
    /// round-trips into the wrong kind.
    pub fn label(self) -> &'static str {
        match self {
            Self::Transcript => "transcript",
            Self::SubagentDir => "subagent-dir",
            Self::Todos => "todos",
            Self::FileHistory => "file-history",
            Self::Attachments => "attachments",
            Self::CategoryDir => "category-dir",
            Self::ConfigFile => "config-file",
            Self::Database => "database",
            Self::SessionData => "session-data",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        let all = [
            Self::Transcript,
            Self::SubagentDir,
            Self::Todos,
            Self::FileHistory,
            Self::Attachments,
            Self::CategoryDir,
            Self::ConfigFile,
            Self::Database,
            Self::SessionData,
        ];
        all.into_iter().find(|k| k.label() == label)
    }
}

/// One physical path this unit's byte total is made of. A session unit
/// typically has several (transcript file, companion subagent dir,
/// matching todos entries, a `file-history/<session>/` directory); a
/// category unit typically has exactly one (its own folded directory or
/// file). #101's session removal moves exactly these members together,
/// after re-verifying each still belongs only to this unit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMember {
    pub path: PathBuf,
    pub bytes: u64,
    pub kind: AgentMemberKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentActionCapability {
    /// No supported selective action for this unit yet. The default;
    /// identification always works with cleanup disabled.
    None,
    /// A single recoverable Trash move of this unit's one category
    /// directory (cache/log categories only -- see `agents_actions`).
    CacheOrLogTrash,
    /// Removing this unit means moving its whole member set together,
    /// with reference/occupancy re-verification and explicit loss
    /// warnings (#101's "explicit individual session removal").
    SessionRemoval,
}

impl AgentActionCapability {
    /// The stored spelling used by the container table.
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::CacheOrLogTrash => "cache-or-log-trash",
            Self::SessionRemoval => "session-removal",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        let all = [Self::None, Self::CacheOrLogTrash, Self::SessionRemoval];
        all.into_iter().find(|a| a.label() == label)
    }
}

/// One identified unit of agent-tool storage: a session, or a folded
/// category directory/file. Identity is `(tool_id, category,
/// relative_path)` -- independent of any project attribution, per
/// `crate::artifact`'s own stated discipline for nested units.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentUnit {
    pub tool_id: String,
    pub tool_name: String,
    /// This tool's home directory, as resolved this pass (detector
    /// override or convention). Carried per-unit, not assumed shared
    /// across every unit in a list, so a caller acting on a mixed
    /// selection across multiple tools always knows which home to
    /// re-derive facts against (e.g. `execute_agent_session_removal`'s
    /// fresh re-identification).
    pub tool_home: PathBuf,
    pub category: AgentCategory,
    /// Stable content-addressed id: `blake3(tool_id, category, relative_path)`.
    pub id: String,
    /// Relative to the tool home (`crate::locations`-resolved), forward
    /// slashes, never absolute.
    pub relative_path: String,
    /// This unit's own canonical anchor path (a session's transcript
    /// file, or a category's own directory/file).
    pub path: PathBuf,
    pub members: Vec<AgentMember>,
    pub bytes: u64,
    pub hardlinked: bool,
    /// `false` when `bytes` is a lower bound rather than this unit's
    /// confirmed size this pass (the bounded fold hit its entry cap or
    /// an unreadable subdirectory a few levels in --
    /// [`crate::agents::unit::CandidateAgentUnit::complete`]). `note`
    /// carries the human-readable reason; this field is the structured
    /// signal a caller can act on without parsing text. An incomplete
    /// unit never carries a fresh `growth_bytes`/`regrowth_count`
    /// (`.oh/guardrails/coverage-changes-are-not-storage-changes.md`).
    #[serde(default = "default_true")]
    pub complete: bool,
    pub growth_bytes: Option<i64>,
    pub regrowth_count: u32,
    pub observed_at: u64,
    pub mtime_max: u64,
    /// True when this unit may never be selectively acted on, either by
    /// category default (`AgentCategory::default_protected`), by the
    /// adapter's own judgment (e.g. `history.jsonl`), or by a human
    /// `swamp protect` entry. `protect_reason` says which.
    pub protected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protect_reason: Option<String>,
    pub project_link: ProjectLinkState,
    pub action: AgentActionCapability,
    /// Coverage/unknown notes (e.g. "coverage incomplete this pass:
    /// could not be read"), never content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Decision evidence (#53): activity/current-use facts for this
    /// unit -- see `crate::evidence`. Populated in
    /// [`discover_and_measure`] from `mtime_max` (already recorded by
    /// the adapter); never a new per-unit scan.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<crate::evidence::Evidence>,
}

/// Where a unit's [`ProjectLinkState`] came from.
///
/// Container-level reuse ([`IdentifyCtx::container`]) replays a
/// container's units from the store when none of the directories that
/// identification listed have moved. That is sound for bytes and
/// members, which live inside those directories -- and *not* sound for
/// project linkage, which is resolved against a declared path somewhere
/// else on the disk entirely: a worktree deleted between two passes
/// would keep reporting `Linked` forever.
///
/// So linkage is not replayed. A unit whose link came from
/// [`resolve_declared_path`] records the declared path instead, and a
/// reused container re-resolves it live, memoised per distinct declared
/// path -- which is one resolution per project, not one per session, and
/// so still scales with containers rather than files.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LinkBasis {
    /// Computed by [`resolve_declared_path`] from `declared`; recomputed
    /// live whenever the container is reused.
    Declared {
        declared: Option<String>,
        /// Further workspace roots the same unit declared, when the tool
        /// records more than one (Oh My Pi's `additionalDirectories`).
        /// Resolved alongside `declared`, and widening the answer to
        /// [`ProjectLinkState::Shared`] when they name a different
        /// project -- never fabricating single ownership for a session
        /// whose own metadata names two.
        additional: Vec<String>,
        missing_reason: String,
        /// The tool's own per-project folder name, when the tool keys
        /// its storage by an encoding of the workspace path (Claude
        /// Code's `projects/<slug>`). Used only when `declared` is
        /// absent, to look for exactly one known worktree whose path
        /// re-encodes to it ([`KnownWorktrees::infer`]); the result is
        /// [`LinkSource::Inferred`] and recomputed every pass.
        folder_slug: Option<String>,
    },
    /// Nothing to recompute. Only a state that cannot go stale --
    /// [`ProjectLinkState::NotApplicable`] and
    /// [`ProjectLinkState::Unresolved`] -- may carry this basis and
    /// still be persisted; any other state makes its whole container
    /// unpersistable, so a link this layer cannot refresh is never
    /// replayed from the store.
    #[default]
    Fixed,
}

// ---------------------------------------------------------------------
// The adapter interface (guardrail spec sections 13/14).
//
// Before this, `identify_for_tool` was a fourteen-arm `match tool_id`
// here and a second, independently maintained fourteen-arm match in
// `crate::actions`; `multi_location_tool` hardcoded two tool ids; Aider
// had a bespoke call path; and `pi.rs` fell back to Oh My Pi's header
// shape. Adding a tool meant editing four places and hoping. The
// location detectors already had the right shape (`locations::Detector`
// + a static registry), so this mirrors it exactly.
// ---------------------------------------------------------------------

/// One entry of a bounded, single-level directory listing, as an adapter
/// sees it. Deliberately a type of this module rather than
/// `locations::ShallowEntry`: an adapter knows the home it is handed and
/// nothing about detectors
/// (`.oh/guardrails/agent-adapters-do-not-reach-detectors.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
}

/// Bumped whenever an adapter's metadata derivation changes what a
/// derived value *means*, so a value cached by an older binary is never trusted.
/// Part of every identification-cache fingerprint.
pub const ADAPTER_VERSION: &str = "2026-09-25.3";

/// The memo that makes an unchanged observation cost zero content reads.
///
/// An adapter derives a small fact from session metadata (the declared
/// `cwd`, a workspace path, a format marker). Re-deriving it means
/// re-reading the source, which on a 5,000-session home is 5,000 capped
/// reads on *every* pass -- the gap the previous repair measured and
/// left open. The value is cached against the source file's own
/// `(size, mtime)` plus [`ADAPTER_VERSION`], so an unchanged file is a
/// lookup, a changed one is read exactly once, and a rewritten file can
/// never serve a stale answer.
///
/// [`IdentificationCache::disabled`] is the execution-time form: it
/// never reads or writes the table, so a recheck always re-derives from
/// the live filesystem. A plan is never spent against a cached
/// derivation.
pub struct IdentificationCache {
    entries: std::cell::RefCell<HashMap<String, crate::assoc_store::CachedRows>>,
    enabled: bool,
}

impl IdentificationCache {
    /// No cache at all: every derivation reads live. Used by
    /// `reidentify` at every execution sink, and by callers with no
    /// store.
    pub fn disabled() -> Self {
        Self {
            entries: std::cell::RefCell::new(HashMap::new()),
            enabled: false,
        }
    }

    pub fn load(swamp_dir: &Path) -> Self {
        Self {
            entries: std::cell::RefCell::new(
                crate::assoc_store::IdentificationTable::open(swamp_dir).load(),
            ),
            enabled: true,
        }
    }

    pub fn save(&self, swamp_dir: &Path, observed_at: u64) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        crate::assoc_store::IdentificationTable::open(swamp_dir)
            .save(&self.entries.borrow(), observed_at)
    }
}

/// The identity a cached derivation is valid for.
///
/// **Not just `(size, mtime_secs)`.** Two independent adversarial passes
/// over this cache found the same hole with the same shape: rewriting a
/// session's declared `cwd` to a path of the *same length* within the
/// same wall-clock second left size and whole-second mtime unchanged, so
/// the stale answer was served and a re-linked session never moved. A
/// cache that cannot see a same-second, same-size rewrite is a cache that
/// silently lies, and the only reason the unit tests missed it is that
/// they slept a second first.
///
/// So the fingerprint carries, where the platform has them:
///
/// * `len` -- the obvious one;
/// * the modification time in **nanoseconds**, not seconds;
/// * `ctime` in nanoseconds -- the inode change time, which moves on a
///   rename-over even when the content's mtime is preserved; and
/// * the inode number -- a replaced file is a different file, whatever
///   its timestamps say.
///
/// Plus [`ADAPTER_VERSION`], so a derivation whose *meaning* changed is
/// never answered from a value computed by an older binary.
///
/// None of this costs a read: it is all in the `stat` the caller already
/// needed.
fn file_fingerprint(meta: &fs::Metadata) -> Vec<(String, u64)> {
    let mut parts = vec![
        (format!("{ADAPTER_VERSION}\u{2}len"), meta.len()),
        (
            "mtime_ns".to_string(),
            meta.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0),
        ),
    ];
    #[cfg(unix)]
    {
        use crate::fs_gate::MetadataExt;
        parts.push((
            "ctime_ns".to_string(),
            (meta.ctime() as u64)
                .saturating_mul(1_000_000_000)
                .saturating_add(meta.ctime_nsec() as u64),
        ));
        parts.push(("ino".to_string(), meta.ino()));
    }
    parts
}

/// Bumped whenever the *encoding* of a container's persisted units
/// changes, so rows written by an older binary are a miss rather than a
/// misread. Part of every container's stored shape key.
const CONTAINER_VERSION: &str = "agent-container/2026-09-25.3";

/// The memo that makes an unchanged container cost `stat`s instead of a
/// listing and a `stat` per file.
///
/// [`IdentificationCache`] removed the header *reads* from an unchanged
/// pass. It could not remove the `stat`s: its validity key is each
/// session file's own `(len, mtime_ns, ctime_ns, inode)`, so knowing a
/// session is unchanged costs one `stat` per session by construction.
/// The 2026-09-22 cost measurement recorded what that means on a
/// 5,000-session home: 10,580 `stat`s and 46 listings on a pass where
/// nothing had changed -- work that scales with files, which the handoff
/// forbids.
///
/// This is the container-level half, and it is deliberately the same
/// bargain `external/folded.parquet` strikes for external units:
///
/// * an adapter wraps the identification of one **container directory**
///   (`projects/<encoded-cwd>/`, `sessions/<yyyy>/<mm>/<dd>/`, a tool's
///   session parent) in [`IdentifyCtx::container`];
/// * every directory that identification lists or folds is recorded,
///   and stored alongside the units it produced;
/// * a container is replayed only when this pass's
///   [`crate::fs_events::EventCoverage`] can show that **no event
///   touched any of those directories** since the rows were written --
///   costing no listing, no `stat` and no header read at all;
/// * a container with no such evidence is re-identified file by file,
///   where the per-file [`IdentificationCache`] still keeps its header
///   reads at zero for the sessions that did not move.
///
/// **Why the gate is events and not directory stamps.** The first
/// version of this cache keyed reuse on the recorded directories' own
/// `mtime`/`ctime`, mirroring
/// [`crate::folded_measurement::reuse_folded_measurement`]. A directory
/// stamp moves when an entry is created, deleted, renamed or replaced
/// -- and *not* when a file inside it is rewritten or appended to in
/// place. For agent storage that is not a corner case: appending to an
/// open session transcript is the normal way these files grow, so a
/// stamp-keyed cache reported a growing session at its old size until
/// something else happened in its container. "What grew" is the whole
/// question this tool answers, so the 2026-09-22 integration decision
/// removed stamp-only reuse as a sufficient condition and replaced it
/// with trusted event coverage, which sees the append because FSEvents
/// reports writes.
///
/// **What this still cannot see.** Nothing, within a trusted window --
/// but a window is available only for paths under a root this pass
/// replayed successfully. With no window (a full walk, any
/// [`crate::fs_events::RefreshRefusal`], a first observation, a
/// store-less caller) there is no reuse and every container is
/// re-identified, which is slower and always correct. The safety
/// boundary for *acting* on any of this remains elsewhere: the reviewed
/// snapshot re-derived from the live filesystem at execution
/// (`reidentify_for_tool`, which runs with both caches disabled).
pub struct ContainerCache {
    entries: std::cell::RefCell<HashMap<String, crate::assoc_store::CachedRows>>,
    /// This pass's known worktrees, for folder-name inference
    /// ([`LinkBasis::Declared::folder_slug`]). Empty for a caller with
    /// no project discovery (a bare adapter test): inference then
    /// never fires and a slug-only session stays unresolved.
    known: KnownWorktrees,
    /// Live re-resolution of declared paths, memoised per distinct
    /// declared path for this pass: see [`LinkBasis`].
    links: std::cell::RefCell<HashMap<(String, String), ProjectLinkState>>,
    /// The only thing that authorizes a replay. Empty means "no
    /// evidence", which means "no reuse".
    coverage: crate::fs_events::EventCoverage,
    enabled: bool,
}

/// One container's partial contribution to a home-level aggregate, as
/// [`IdentifyCtx::container_with_facts`] hands it back.
///
/// Two states and not one `Vec`, because "this container contributed
/// nothing" and "this container's contribution is not in the stored rows
/// at all" are different answers with different consequences. A caller
/// summing partials may add the first and must refuse to print a total
/// on the second: a shared-blob reference count that is quietly short is
/// worse than an absent one, because it is the number a future GC would
/// act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerFacts {
    /// The adapter's fact strings for this container, fresh or replayed.
    Recorded(Vec<String>),
    /// Replayed rows that carry no fact section at all (rows an earlier
    /// stored shape wrote). Unknown, never empty.
    Unrecorded,
}

/// The directories one container's identification depended on, recorded
/// while it ran. Paths only: the stamps are read when the fingerprint is
/// built, so a recording costs nothing on the hot path.
#[derive(Default)]
struct ContainerRecorder {
    dirs: Vec<PathBuf>,
    seen: HashSet<PathBuf>,
    /// Set when this container measured something directory stamps
    /// cannot describe (a truncated fold), so it must be re-identified
    /// every pass rather than replayed from a key that cannot see the
    /// change.
    unstorable: bool,
}

impl ContainerRecorder {
    fn record(&mut self, dir: &Path) {
        if self.seen.insert(dir.to_path_buf()) {
            self.dirs.push(dir.to_path_buf());
        }
    }
}

impl ContainerCache {
    /// No container reuse at all: every container is identified live.
    /// Used by `reidentify` at every execution sink, and by callers with
    /// no store.
    pub fn disabled() -> Self {
        Self {
            entries: std::cell::RefCell::new(HashMap::new()),
            known: KnownWorktrees::default(),
            links: std::cell::RefCell::new(HashMap::new()),
            coverage: crate::fs_events::EventCoverage::untrusted(),
            enabled: false,
        }
    }

    /// Loads the stored containers and the evidence this pass is allowed
    /// to replay them on. A caller with no event coverage still gets a
    /// usable cache: it stores what this pass identifies, so the *next*
    /// pass -- which may have a window -- has something to replay.
    pub fn load(swamp_dir: &Path, coverage: crate::fs_events::EventCoverage) -> Self {
        Self {
            entries: std::cell::RefCell::new(
                crate::assoc_store::ContainerTable::open(swamp_dir).load(),
            ),
            known: KnownWorktrees::default(),
            links: std::cell::RefCell::new(HashMap::new()),
            coverage,
            enabled: true,
        }
    }

    /// The worktrees this pass's project discovery found -- the bounded
    /// candidate set folder-name inference matches against. Nothing
    /// else is ever consulted for an inferred link.
    pub fn with_known_worktrees(mut self, worktrees: &[PathBuf]) -> Self {
        self.known = KnownWorktrees::from_paths(worktrees);
        self
    }

    /// Resolves a unit's link exactly as a replayed one is resolved:
    /// the declared path live, then folder-name inference against this
    /// pass's known worktrees when the declared path cannot be linked.
    /// Called for
    /// every freshly identified unit too, so an identified and a
    /// replayed session cannot disagree about the same evidence.
    pub fn finish_link(&self, unit: &mut CandidateAgentUnit) {
        if let LinkBasis::Declared {
            declared,
            additional,
            missing_reason,
            folder_slug,
        } = unit.link_basis().clone()
        {
            unit.set_resolved_link(self.resolve_memoised(
                &declared,
                &additional,
                &missing_reason,
                folder_slug.as_deref(),
            ));
        }
    }

    pub fn save(&self, swamp_dir: &Path, observed_at: u64) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        crate::assoc_store::ContainerTable::open(swamp_dir)
            .save(&self.entries.borrow(), observed_at)
    }

    /// Resolves `declared` once per pass, however many sessions in a
    /// container declared it. This is why replaying a container does not
    /// have to replay its linkage: the honest answer costs one
    /// resolution per *project*, not one per session.
    fn resolve_memoised(
        &self,
        declared: &Option<String>,
        additional: &[String],
        reason: &str,
        folder_slug: Option<&str>,
    ) -> ProjectLinkState {
        let key = (
            format!(
                "{}{DECLARED_SEP}{}{FOLDER_SEP}{}",
                declared.clone().unwrap_or_default(),
                additional.join(&DECLARED_SEP.to_string()),
                folder_slug.unwrap_or_default()
            ),
            reason.to_string(),
        );
        if let Some(hit) = self.links.borrow().get(&key) {
            return hit.clone();
        }
        let resolved = resolve_declared_workspace(declared, additional, reason);
        // A successfully resolved cwd is authoritative. If it is absent,
        // missing, or not a checkout, Claude's full encoded path slug is
        // a useful fallback only when it uniquely re-encodes one current
        // known worktree. Keep the failed cwd in fallback_reason rather
        // than silently replacing that evidence.
        let resolved = match (&resolved, folder_slug) {
            (ProjectLinkState::Unresolved { reason }, Some(slug)) => self.known.infer(slug, reason),
            (ProjectLinkState::Missing { path }, Some(slug)) => {
                let reason = format!("declared cwd is missing: {}", path.display());
                match self.known.infer(slug, &reason) {
                    linked @ ProjectLinkState::Linked { .. } => linked,
                    _ => resolved,
                }
            }
            (ProjectLinkState::NotAProject { path }, Some(slug)) => {
                let reason = format!("declared cwd is not a known checkout: {}", path.display());
                match self.known.infer(slug, &reason) {
                    linked @ ProjectLinkState::Linked { .. } => linked,
                    _ => resolved,
                }
            }
            _ => resolved,
        };
        self.links.borrow_mut().insert(key, resolved.clone());
        resolved
    }
}

/// The stored key for one container's recorded directory list: the
/// encoding version plus the directories themselves, and **no `stat` at
/// all**.
///
/// Before 2026-09-22 this was a stamp fingerprint -- one `stat` per
/// recorded directory, every pass, whose only job was to notice a change
/// it could not actually see (an in-place append). Trusted event
/// coverage answers that question properly, so what remains here is the
/// part a stored row still needs: proof that these rows describe *this*
/// directory list under *this* encoding, so a container whose watched
/// set changed shape, or whose rows were written by an older binary, is
/// a miss rather than a misread.
fn container_shape_key(dirs: &[PathBuf]) -> String {
    let mut parts: Vec<(String, u64)> = vec![(CONTAINER_VERSION.to_string(), 1)];
    for (i, d) in dirs.iter().enumerate() {
        parts.push((format!("{}\u{2}dir", d.display()), i as u64));
    }
    crate::assoc_store::fingerprint_string(&parts)
}

/// Everything an adapter is allowed to see and do during identification.
///
/// It carries the folded/listing primitives (so no adapter calls
/// `read_dir`), the one capped content reader (so no adapter calls
/// `read_to_string`), the identification cache (so an unchanged session
/// is never re-read) and the container cache (so an unchanged container
/// is never re-listed). An adapter that wants a fact it cannot get from
/// here is asking for a capability the guardrails deny.
pub struct IdentifyCtx<'a> {
    observed_at: u64,
    cache: &'a IdentificationCache,
    containers: Option<&'a ContainerCache>,
    codex_sqlite_home_override: Option<PathBuf>,
    /// The container currently being recorded, if any. One level deep by
    /// design: see [`IdentifyCtx::container`].
    recording: std::cell::RefCell<Option<ContainerRecorder>>,
}

impl<'a> IdentifyCtx<'a> {
    pub fn new(observed_at: u64, cache: &'a IdentificationCache) -> Self {
        Self {
            observed_at,
            cache,
            containers: None,
            codex_sqlite_home_override: std::env::var_os("CODEX_SQLITE_HOME").map(PathBuf::from),
            recording: std::cell::RefCell::new(None),
        }
    }

    /// The identification context the ordinary report path uses: with
    /// container-level reuse as well as the per-file derivation cache.
    pub fn with_containers(
        observed_at: u64,
        cache: &'a IdentificationCache,
        containers: &'a ContainerCache,
    ) -> Self {
        Self {
            observed_at,
            cache,
            containers: Some(containers),
            codex_sqlite_home_override: std::env::var_os("CODEX_SQLITE_HOME").map(PathBuf::from),
            recording: std::cell::RefCell::new(None),
        }
    }

    pub(super) fn codex_sqlite_home_override(&self) -> Option<&Path> {
        self.codex_sqlite_home_override.as_deref()
    }

    /// Refreshes one cached declared link from a tool's current metadata,
    /// then resolves it through the shared project/worktree machinery.
    /// This keeps externally indexed metadata fresh without making a
    /// session-size container depend on the external index's write events.
    pub fn refresh_declared_project_link(
        &self,
        unit: &mut CandidateAgentUnit,
        declared: Option<String>,
        missing_reason: &str,
    ) {
        unit.set_declared_project_link(declared, missing_reason);
        if let Some(containers) = self.containers {
            containers.finish_link(unit);
        }
    }

    /// Records `dir` as one of the directories the container currently
    /// being identified depends on, without listing it.
    ///
    /// Needed where a container's units are affected by a directory it
    /// does not itself list -- Claude Code's `file-history/`,
    /// `image-cache/`, `uploads/` and `todos/` all gain a
    /// `<session-id>` entry when a session acquires one, and the parent
    /// directory's own stamp is what moves. Outside a container this is
    /// a no-op.
    pub fn watch(&self, dir: &Path) {
        if let Some(rec) = self.recording.borrow_mut().as_mut() {
            rec.record(dir);
        }
    }

    /// Identifies one container directory, replaying the stored result
    /// when every directory that identification listed still carries the
    /// stamp it did last pass.
    ///
    /// `identify` must be self-contained: everything its units depend on
    /// has to be reached through this same `ctx` (so it is recorded) or
    /// declared with [`IdentifyCtx::watch`]. Nesting is deliberately not
    /// supported -- a `container` call inside another container's
    /// `identify` runs inline and folds its directories into the outer
    /// container's fingerprint, because a nested container that missed
    /// while its parent hit could not be re-identified without re-running
    /// the parent, which is the work the reuse exists to avoid.
    ///
    /// A container whose units cannot be re-derived honestly is run and
    /// **not stored**: see [`LinkBasis`].
    pub fn container(
        &self,
        adapter_id: &str,
        container: &Path,
        identify: &dyn Fn() -> Vec<CandidateAgentUnit>,
    ) -> Vec<CandidateAgentUnit> {
        self.container_with_facts(adapter_id, container, &|| (identify(), Vec::new()))
            .0
    }

    /// [`Self::container`] for an adapter whose home-level output is an
    /// **aggregate over its containers** rather than just their units.
    ///
    /// `identify` returns its units and a list of adapter-private fact
    /// strings -- the container's partial contribution to that aggregate
    /// -- which are stored with the rows and handed back verbatim when
    /// the container is replayed. The caller sums stored partials and
    /// fresh ones and prints one total, so a pass that replayed some
    /// containers and re-identified others still prints a *right* total
    /// rather than the total of the containers it happened to look at.
    ///
    /// The returned facts are [`ContainerFacts::Unrecorded`] when the
    /// replayed rows carry no fact section at all (rows an earlier shape
    /// wrote). That is not the same as an empty list, and the caller
    /// must treat it as unknown: a missing partial makes the whole sum
    /// unknown, never a smaller number presented as complete. Oh My Pi's
    /// shared-blob reference count is the case this exists for -- a
    /// count that is quietly short is worse than no count, because it is
    /// the number a future GC would act on.
    pub fn container_with_facts(
        &self,
        adapter_id: &str,
        container: &Path,
        identify: &dyn Fn() -> (Vec<CandidateAgentUnit>, Vec<String>),
    ) -> (Vec<CandidateAgentUnit>, ContainerFacts) {
        let Some(store) = self.containers.filter(|c| c.enabled) else {
            let (units, facts) = identify();
            return (units, ContainerFacts::Recorded(facts));
        };
        if self.recording.borrow().is_some() {
            let (units, facts) = identify();
            return (units, ContainerFacts::Recorded(facts));
        }
        let key = format!("{adapter_id}\u{1}{}", container.display());
        if let Some((units, facts)) = self.replay(store, &key) {
            crate::work_counters::record_container_reused();
            // Replayed *and re-verified*: the window vouched for it just
            // now, so the rows are as fresh as an identification would
            // have made them. Without this the next pass's window --
            // which starts where this one ended -- could never vouch for
            // them again, and reuse would work only on alternate passes.
            if let Some(entry) = store.entries.borrow_mut().get_mut(&key) {
                entry.observed_at = self.observed_at;
            }
            return (units, facts);
        }
        crate::work_counters::record_container_identified();
        *self.recording.borrow_mut() = Some(ContainerRecorder::default());
        let (units, facts) = identify();
        let recorder = self.recording.borrow_mut().take().unwrap_or_default();
        let facts = ContainerFacts::Recorded(facts);
        if recorder.unstorable {
            return (units, facts);
        }
        if let Some(rows) = encode_container(&recorder.dirs, &facts, &units) {
            let fingerprint = container_shape_key(&recorder.dirs);
            store.entries.borrow_mut().insert(
                key,
                crate::assoc_store::CachedRows {
                    fingerprint,
                    // Identified live, right now: this is the one place
                    // a container may claim to have been verified as of
                    // this observation.
                    observed_at: self.observed_at,
                    rows,
                },
            );
        }
        (units, facts)
    }

    /// The stored units for `key`, when -- and only when -- this pass's
    /// event coverage can vouch that every directory the container
    /// depends on is untouched since the rows were written.
    ///
    /// Costs no syscall on either branch: the decision is made from the
    /// decoded rows and the replay window alone.
    fn replay(
        &self,
        store: &ContainerCache,
        key: &str,
    ) -> Option<(Vec<CandidateAgentUnit>, ContainerFacts)> {
        // The borrow of `entries` is scoped: `resolve_memoised` below
        // takes `links` mutably, and a future edit that reaches
        // `entries` again would otherwise panic at runtime rather than
        // fail to compile.
        let (units, facts) = {
            let entries = store.entries.borrow();
            let cached = entries.get(key)?;
            let (dirs, facts, units) = decode_container(&cached.rows)?;
            // The shape key is checked first and costs nothing, so a
            // corrupt or older-format row set is a free miss.
            if container_shape_key(&dirs) != cached.fingerprint {
                return None;
            }
            // Every directory, not merely the container root: an
            // adapter's `watch` declares siblings whose contents decide
            // this container's units (Claude Code's `todos/`,
            // `file-history/`), and a window that does not cover one of
            // them cannot vouch for the whole.
            if !dirs
                .iter()
                .all(|d| store.coverage.unchanged_since(d, cached.observed_at))
            {
                return None;
            }
            (units, facts)
        };
        Some((
            units
                .into_iter()
                .map(|mut u| {
                    store.finish_link(&mut u);
                    u
                })
                .collect(),
            facts,
        ))
    }

    pub fn observed_at(&self) -> u64 {
        self.observed_at
    }

    /// One `lstat(2)` of `path` (never follows a symlink). Adapters stat
    /// through their context, never through `fs_gate` directly
    /// (`adapters_do_not_reach_gates`).
    pub fn stat(&self, path: &Path) -> std::io::Result<crate::fs_gate::Metadata> {
        crate::fs_gate::symlink_metadata(path)
    }

    /// Whether `path` is a directory (following a symlink, as
    /// `Path::is_dir` does).
    pub fn is_dir(&self, path: &Path) -> bool {
        crate::fs_gate::is_dir(path)
    }

    /// Whether `path` is a regular file (following a symlink, as
    /// `Path::is_file` does).
    pub fn is_file(&self, path: &Path) -> bool {
        crate::fs_gate::is_file(path)
    }

    /// Whether anything exists at `path` (following a symlink).
    pub fn exists(&self, path: &Path) -> bool {
        crate::fs_gate::exists(path)
    }

    /// One bounded, single-level, symlink-refusing listing. Sorted, so
    /// identification output does not depend on directory order.
    pub fn list(&self, dir: &Path) -> Vec<Entry> {
        self.list_checked(dir).0
    }

    /// [`IdentifyCtx::list`], with whether the listing stopped at its cap.
    /// A caller whose output is an aggregate over the entries -- a count
    /// someone could act on -- must read this, not assume completeness.
    pub fn list_checked(&self, dir: &Path) -> (Vec<Entry>, crate::locations::Truncation) {
        self.watch(dir);
        let listing = crate::locations::shallow_list(dir);
        let truncation = listing.truncation;
        (
            listing
                .into_iter()
                .map(|e| Entry {
                    name: e.name,
                    is_dir: e.is_dir,
                })
                .collect(),
            truncation,
        )
    }

    pub fn dir_names(&self, dir: &Path) -> Vec<String> {
        self.list(dir)
            .into_iter()
            .filter(|e| e.is_dir)
            .map(|e| e.name)
            .collect()
    }

    pub fn file_names(&self, dir: &Path) -> Vec<String> {
        self.list(dir)
            .into_iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.name)
            .collect()
    }

    /// Whether `dir` holds anything at all -- the "is this home empty or
    /// merely unrecognized" question several adapters ask before
    /// reporting an unknown-format residual.
    pub fn has_entries(&self, dir: &Path) -> bool {
        !self.list(dir).is_empty()
    }

    pub fn folded_bytes(&self, path: &Path, max_entries: usize) -> (u64, u64, bool) {
        let (bytes, mtime_max, truncated, stamps) =
            crate::folded_measurement::folded_bytes_bounded_stamped(path, max_entries);
        if self.recording.borrow().is_some() {
            if truncated {
                // A fold that stopped at its bound does not describe the
                // whole subtree, so nothing about it may be replayed:
                // watching the anchor path (whose stamp will not move
                // when a deep file changes) would be a reuse key that
                // cannot see the thing it is caching.
                self.abandon_container_recording();
            } else {
                for stamp in &stamps {
                    self.watch(&stamp.path);
                }
                // A path that folded to nothing because it does not
                // exist (or is a plain file) produces no stamps, and its
                // *appearance* still has to be a change.
                if stamps.is_empty() {
                    self.watch(path);
                }
            }
        }
        (bytes, mtime_max, truncated)
    }

    /// Marks the container being recorded as one that must not be
    /// stored: whatever was just measured cannot be described by
    /// directory stamps alone.
    fn abandon_container_recording(&self) {
        if let Some(rec) = self.recording.borrow_mut().as_mut() {
            rec.unstorable = true;
        }
    }

    /// An uncached capped header read. Prefer [`IdentifyCtx::derived`]:
    /// this one costs its bytes on every pass.
    pub fn read_header(&self, path: &Path, max_bytes: usize) -> Option<String> {
        bounded_io::read_header(path, max_bytes)
    }

    /// A value derived from `path`'s capped header, read at most once
    /// per `(size, mtime, adapter version)`.
    ///
    /// `kind` names the derivation, so two facts taken from the same file
    /// do not collide. `derive` receives the header text and returns the
    /// value, or `None` for "this file genuinely declares nothing" --
    /// which is itself cached, or an absent field would be re-read
    /// forever.
    pub fn derived(
        &self,
        adapter_id: &str,
        kind: &str,
        path: &Path,
        max_bytes: usize,
        derive: &dyn Fn(&str) -> Option<String>,
    ) -> Option<String> {
        self.derived_by(adapter_id, kind, path, &|| {
            bounded_io::read_header(path, max_bytes).and_then(|text| derive(&text))
        })
    }

    fn derived_by(
        &self,
        adapter_id: &str,
        kind: &str,
        path: &Path,
        read: &dyn Fn() -> Option<String>,
    ) -> Option<String> {
        let Ok(meta) = crate::fs_gate::symlink_metadata(path) else {
            return None;
        };
        let fingerprint = crate::assoc_store::fingerprint_string(&file_fingerprint(&meta));
        let key = format!("{adapter_id}\u{1}{kind}\u{1}{}", path.display());
        if self.cache.enabled {
            let entries = self.cache.entries.borrow();
            if let Some(hit) = entries.get(&key)
                && hit.fingerprint == fingerprint
            {
                crate::work_counters::record_cache_hit();
                return hit.rows.first().and_then(|r| r.first()).cloned();
            }
        }
        crate::work_counters::record_cache_miss();
        let value = read();
        if self.cache.enabled {
            self.cache.entries.borrow_mut().insert(
                key,
                crate::assoc_store::CachedRows {
                    fingerprint,
                    observed_at: 0,
                    // An absent value is stored as a fingerprint-only
                    // row, which `assoc_store` already round-trips as
                    // "this key genuinely has nothing".
                    rows: value.iter().map(|v| vec![v.clone()]).collect(),
                },
            );
        }
        value
    }
}

// ---------------------------------------------------------------------
// The container table's row encoding.
//
// Columns, not a JSON blob in a Parquet cell: the guardrail is about the
// shape of stored data, not the file extension
// (`.oh/guardrails/store-data-is-parquet-not-json-sidecars.md`). Every
// enum is written through its own explicit `label`/`from_label`, so a
// new variant is a compile error rather than a value that silently
// round-trips into the wrong one.
// ---------------------------------------------------------------------

/// `None` -> `""`, `Some(s)` -> `"=s"`. A one-character tag, so
/// `Some("")` and `None` are different stored values rather than the
/// same one.
fn encode_opt(value: &Option<String>) -> String {
    match value {
        None => String::new(),
        Some(s) => format!("={s}"),
    }
}

fn decode_opt(value: &str) -> Option<String> {
    value.strip_prefix('=').map(str::to_string)
}

/// Separates a unit's primary declared path from the further workspace
/// roots it declared, inside the one stored column. A C0 control, which
/// no path a tool writes into its own metadata contains; rows written
/// before this existed carry no separator and decode to an empty
/// `additional`.
const DECLARED_SEP: char = '\u{1}';
/// Separates the declared-path list from the folder slug in the memo
/// key and the stored `link_declared` cell.
const FOLDER_SEP: char = '\u{2}';

/// This pass's known worktree paths, indexed by their Claude Code
/// folder slug, for [`LinkBasis::Declared::folder_slug`] inference.
///
/// The slug encoding ([`claude_folder_slug`]) maps every character that
/// is not ASCII alphanumeric to `-`, so it is not injective: `/a/b-c`
/// and `/a-b/c` both become `-a-b-c`. A slug is therefore only ever
/// *matched* against known paths, never decoded, and a slug two known
/// paths share resolves nothing. What "known" means is exactly the
/// worktree list project discovery handed this pass -- no filesystem
/// listing, no basename, no memory of paths that were once known.
#[derive(Debug, Default, Clone)]
pub struct KnownWorktrees {
    by_slug: HashMap<String, Vec<PathBuf>>,
}

/// Claude Code's `projects/<slug>` encoding of a workspace path: every
/// character that is not ASCII alphanumeric becomes `-`. Lossy by
/// construction (`/`, `-`, `.`, `_` and every non-ASCII character all
/// map to the same byte), which is why it is only ever compared, never
/// inverted.
pub fn claude_folder_slug(path: &Path) -> String {
    path.display()
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

impl KnownWorktrees {
    pub fn from_paths(paths: &[PathBuf]) -> Self {
        let mut by_slug: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for p in paths {
            let entry = by_slug.entry(claude_folder_slug(p)).or_default();
            // The same worktree listed twice (two projects sharing a
            // path row, a re-fold) is one candidate, not an ambiguity.
            if !entry.contains(p) {
                entry.push(p.clone());
            }
        }
        Self { by_slug }
    }

    /// The fallback link a session whose declared path cannot be linked
    /// gets from its folder name: [`LinkSource::Inferred`] when exactly one known worktree
    /// re-encodes to `slug` *and* that path still resolves to a
    /// checkout; otherwise [`ProjectLinkState::Unresolved`] with the
    /// original reason extended by what the folder name did say.
    pub fn infer(&self, slug: &str, unresolved_reason: &str) -> ProjectLinkState {
        let Some(candidates) = self.by_slug.get(slug) else {
            return ProjectLinkState::Unresolved {
                reason: unresolved_reason.to_string(),
            };
        };
        if candidates.len() != 1 {
            return ProjectLinkState::Unresolved {
                reason: format!(
                    "{unresolved_reason}; the projects folder name re-encodes {} known \
                     worktree paths, which the lossy encoding cannot tell apart",
                    candidates.len()
                ),
            };
        }
        let candidate = &candidates[0];
        match resolve_declared_path(Some(candidate.display().to_string()), unresolved_reason) {
            ProjectLinkState::Linked {
                project_id,
                project_name,
                project_path,
                worktree_kind,
                ..
            } => ProjectLinkState::Linked {
                project_id,
                project_name,
                project_path,
                source: LinkSource::Inferred,
                fallback_reason: Some(unresolved_reason.to_string()),
                worktree_kind,
            },
            _ => ProjectLinkState::Unresolved {
                reason: format!(
                    "{unresolved_reason}; the projects folder name re-encodes the known \
                     worktree {} but that path is no longer a git checkout",
                    candidate.display()
                ),
            },
        }
    }
}

fn encode_declared(
    declared: &Option<String>,
    additional: &[String],
    folder_slug: &Option<String>,
) -> String {
    let mut out = encode_opt(declared);
    for extra in additional {
        out.push(DECLARED_SEP);
        out.push_str(extra);
    }
    if let Some(slug) = folder_slug {
        out.push(FOLDER_SEP);
        out.push_str(slug);
    }
    out
}

fn decode_declared(value: &str) -> (Option<String>, Vec<String>, Option<String>) {
    let (paths, slug) = match value.split_once(FOLDER_SEP) {
        Some((paths, slug)) => (paths, Some(slug.to_string())),
        None => (value, None),
    };
    let mut parts = paths.split(DECLARED_SEP);
    let declared = decode_opt(parts.next().unwrap_or_default());
    (declared, parts.map(str::to_string).collect(), slug)
}

/// Resolves a unit's declared workspace: its primary path, widened to
/// [`ProjectLinkState::Shared`] when any further declared root resolves
/// to a *different* project identity.
///
/// Lives here rather than in an adapter because the widening is a
/// property of linkage, not of any one tool's file format, and because
/// a replayed container must be able to redo it without re-running the
/// adapter that produced the unit.
pub fn resolve_declared_workspace(
    declared: &Option<String>,
    additional: &[String],
    missing_reason: &str,
) -> ProjectLinkState {
    let primary = resolve_declared_path(declared.clone(), missing_reason);
    if additional.is_empty() {
        return primary;
    }
    let mut project_ids: Vec<String> = Vec::new();
    if let ProjectLinkState::Linked { project_id, .. } = &primary {
        project_ids.push(project_id.clone());
    }
    for extra in additional {
        if let ProjectLinkState::Linked { project_id, .. } =
            resolve_declared_path(Some(extra.clone()), "")
            && !project_ids.contains(&project_id)
        {
            project_ids.push(project_id);
        }
    }
    if project_ids.len() > 1 {
        ProjectLinkState::Shared { project_ids }
    } else {
        primary
    }
}

fn col(row: &[String], n: usize) -> &str {
    row.get(n).map(String::as_str).unwrap_or_default()
}

/// The rows for one container, or `None` when this container must not be
/// stored at all.
///
/// The one thing that makes a container unstorable is a unit whose
/// project link cannot be recomputed on replay and cannot go stale
/// either: see [`LinkBasis`]. Refusing to store is the conservative
/// outcome -- the container is simply re-identified next pass.
fn encode_container(
    dirs: &[PathBuf],
    facts: &ContainerFacts,
    units: &[CandidateAgentUnit],
) -> Option<Vec<Vec<String>>> {
    let n = crate::assoc_store::ContainerTable::COLUMNS.len();
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(dirs.len() + units.len());
    let blank = || vec![String::new(); n];
    for dir in dirs {
        let mut row = blank();
        row[0] = "dir".to_string();
        row[1] = dir.display().to_string();
        rows.push(row);
    }
    // The marker row is written even for an empty fact list, so a
    // replayed container can tell "this adapter contributed no facts"
    // from "these rows predate facts entirely", which is the difference
    // between a partial aggregate of zero and an unknown one.
    if let ContainerFacts::Recorded(facts) = facts {
        let mut marker = blank();
        marker[0] = "facts".to_string();
        rows.push(marker);
        for fact in facts {
            let mut row = blank();
            row[0] = "fact".to_string();
            row[1] = fact.clone();
            rows.push(row);
        }
    }
    for unit in units {
        let (link_kind, link_declared, link_reason) =
            match (&unit.link_basis(), &unit.project_link()) {
                (
                    LinkBasis::Declared {
                        declared,
                        additional,
                        missing_reason,
                        folder_slug,
                    },
                    _,
                ) => (
                    "declared",
                    encode_declared(declared, additional, folder_slug),
                    missing_reason.clone(),
                ),
                (LinkBasis::Fixed, ProjectLinkState::NotApplicable) => {
                    ("not-applicable", String::new(), String::new())
                }
                (LinkBasis::Fixed, ProjectLinkState::Unresolved { reason }) => {
                    ("unresolved", String::new(), reason.clone())
                }
                // Linked/Missing/NotAProject/Moved/Remote/Shared with no
                // declared path to re-resolve: replaying it could report a
                // project that has since been deleted or moved. Do not store
                // the container.
                (LinkBasis::Fixed, _) => return None,
            };
        let mut row = blank();
        row[0] = "unit".to_string();
        row[1] = unit.path().display().to_string();
        row[2] = unit.category().label().to_string();
        row[3] = unit.relative_path().to_string();
        row[4] = unit.bytes().to_string();
        row[5] = unit.mtime_max().to_string();
        row[6] = unit.action().label().to_string();
        row[7] = encode_opt(&unit.note().map(str::to_string));
        row[8] = if unit.protected() { "1" } else { "0" }.to_string();
        row[9] = encode_opt(&unit.protect_reason().map(str::to_string));
        row[10] = link_kind.to_string();
        row[11] = link_declared;
        row[12] = link_reason;
        rows.push(row);
        for member in unit.members() {
            let mut row = blank();
            row[0] = "member".to_string();
            row[1] = member.path.display().to_string();
            row[2] = member.kind.label().to_string();
            row[4] = member.bytes.to_string();
            rows.push(row);
        }
    }
    Some(rows)
}

/// The inverse. `None` for any row set this binary cannot read back
/// exactly -- an unknown category, kind or action, a `member` row with
/// no unit above it, a malformed number. A cache that cannot be decoded
/// is a cache miss, never a wrong answer and never an error.
fn decode_container(
    rows: &[Vec<String>],
) -> Option<(Vec<PathBuf>, ContainerFacts, Vec<CandidateAgentUnit>)> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut facts: Option<Vec<String>> = None;
    let mut units: Vec<CandidateAgentUnit> = Vec::new();
    for row in rows {
        match col(row, 0) {
            "dir" => dirs.push(PathBuf::from(col(row, 1))),
            "facts" => facts = Some(Vec::new()),
            "fact" => facts.as_mut()?.push(col(row, 1).to_string()),
            "unit" => {
                let category = AgentCategory::from_label(col(row, 2))?;
                let action = AgentActionCapability::from_label(col(row, 6))?;
                let (link_declared, link_additional, link_slug) = decode_declared(col(row, 11));
                let link_reason = col(row, 12).to_string();
                let (project_link, link_basis) = match col(row, 10) {
                    "declared" => (
                        // Replaced live by `ContainerCache::replay`; this
                        // placeholder is never the value a caller sees.
                        ProjectLinkState::Unresolved {
                            reason: link_reason.clone(),
                        },
                        LinkBasis::Declared {
                            declared: link_declared,
                            additional: link_additional,
                            missing_reason: link_reason,
                            folder_slug: link_slug,
                        },
                    ),
                    "not-applicable" => (ProjectLinkState::NotApplicable, LinkBasis::Fixed),
                    "unresolved" => (
                        ProjectLinkState::Unresolved {
                            reason: link_reason,
                        },
                        LinkBasis::Fixed,
                    ),
                    _ => return None,
                };
                units.push(CandidateAgentUnit::replayed(
                    category,
                    col(row, 3).to_string(),
                    PathBuf::from(col(row, 1)),
                    col(row, 4).parse().ok()?,
                    col(row, 5).parse().ok()?,
                    col(row, 8) == "1",
                    decode_opt(col(row, 9)),
                    project_link,
                    action,
                    decode_opt(col(row, 7)),
                    link_basis,
                ));
            }
            "member" => {
                let kind = AgentMemberKind::from_label(col(row, 2))?;
                units.last_mut()?.push_replayed_member(AgentMember {
                    path: PathBuf::from(col(row, 1)),
                    bytes: col(row, 4).parse().ok()?,
                    kind,
                });
            }
            _ => return None,
        }
    }
    Some((
        dirs,
        match facts {
            Some(f) => ContainerFacts::Recorded(f),
            None => ContainerFacts::Unrecorded,
        },
        units,
    ))
}

/// What an adapter's storage *shape* requires of the shared layer.
/// Declared by the adapter rather than matched on its id, so the shared
/// layer never grows a second table of tool ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AdapterCapabilities {
    /// This tool's storage can be installed into more than one editor
    /// host at once (#99's "model each host as a separate detector
    /// location, dedupe nothing that is genuinely separate storage"), so
    /// *every* authorized location is decomposed rather than only the
    /// first. Was `multi_location_tool`'s hardcoded Cline/Roo match.
    pub decomposes_every_location: bool,
    /// This tool keeps units inside each project checkout rather than
    /// under a tool home (Aider's `.aider.chat.history.md` and friends),
    /// so the shared layer also calls
    /// [`AgentAdapter::project_local_units`] once per known worktree.
    pub project_local_units: bool,
}

/// One tool's identification code. Copy the smallest adapter
/// (`cursor.rs`) for the pattern; `docs/architecture.md` has the worked
/// description.
///
/// An adapter names no other adapter, reads no file outside
/// [`IdentifyCtx`], never acts, and never emits -- each of those is a
/// separate audit, not a convention.
pub trait AgentAdapter: Send + Sync {
    /// Stable id, equal to this tool's detector id and to its
    /// `matrix::AgentToolId::slug()`.
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::default()
    }
    /// Identify this tool's units inside `home`.
    fn identify(&self, home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit>;
    /// Re-identify for an execution-time recheck. The default is
    /// [`AgentAdapter::identify`] with whatever context it is given --
    /// and the sink always hands it an
    /// [`IdentificationCache::disabled`] one, so a recheck reads live
    /// state and an approval is never spent against a cached
    /// derivation.
    fn reidentify(&self, home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
        self.identify(home, ctx)
    }
    /// Units that live inside one project checkout rather than under a
    /// tool home. Called only when
    /// [`AdapterCapabilities::project_local_units`] is set.
    fn project_local_units(
        &self,
        _worktree_root: &Path,
        _ctx: &IdentifyCtx,
    ) -> Vec<CandidateAgentUnit> {
        Vec::new()
    }
}

/// `path` relative to `home`, forward-slashed; the whole path if it is
/// not beneath `home`.
pub fn relative_to(home: &Path, path: &Path) -> String {
    path.strip_prefix(home)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

/// `serde(default = ...)` for [`AgentUnit::complete`]: a unit
/// deserialized from a shape written before this field existed was, by
/// construction, whatever that shape always reported -- a complete
/// total.
fn default_true() -> bool {
    true
}

pub fn unit_id(tool_id: &str, category: AgentCategory, relative_path: &str) -> String {
    crate::entities::id_for(&format!(
        "agent-unit:v1:{tool_id}:{}:{relative_path}",
        category.label()
    ))
}

fn unit_key(tool_id: &str, category: AgentCategory, device: u64, path: &Path) -> String {
    crate::growth::external_row_key(
        tool_id,
        &category.key_str(),
        device,
        &path.display().to_string(),
    )
}

pub(crate) fn device_of(path: &Path) -> u64 {
    #[cfg(unix)]
    {
        use crate::fs_gate::MetadataExt;
        crate::fs_gate::metadata_following(path)
            .map(|m| m.dev())
            .unwrap_or(0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        0
    }
}

/// Bounded, stat-only folded byte total for `path`, delegating to
/// `crate::folded_measurement` -- the one module on the ordinary report
/// path allowed to traverse. Adapters reach it through
/// [`IdentifyCtx::folded_bytes`]; this alias exists for the shared layer
/// and for `crate::actions`' own member sizing.
pub fn folded_bytes(path: &Path, max_entries: usize) -> (u64, u64, bool) {
    crate::folded_measurement::folded_bytes_bounded(path, max_entries)
}

pub(crate) fn mtime_secs(meta: &fs::Metadata) -> u64 {
    crate::folded_measurement::mtime_secs(meta)
}

// ---------------------------------------------------------------------
// Shared declared-path -> project-identity resolution (#91's original
// contract, factored out during #93/#94/#95 so every adapter that reads
// a declared absolute path out of its own tool's metadata --
// `claude_code`'s transcript `cwd`, `codex`'s SQLite index `cwd`,
// `oh_my_pi`'s session-header `cwd`, `opencode`'s `project.json`
// `worktree` -- resolves it against swamp's project/worktree identity
// the same way, once. Never a basename guess: this walks upward from
// `path` looking for a `.git` directory/file and resolves through
// `crate::git`'s own object-store-based project identity primitives.
// ---------------------------------------------------------------------

/// Resolves an absolute path declared by some tool's own metadata (never
/// a filename/directory-name guess) to this swamp instance's project
/// identity. `field_missing_reason` is the adapter-specific explanation
/// for why no path could be extracted at all (e.g. "no cwd field found
/// in the session metadata source"), used only for the `Unresolved` case.
pub(crate) fn resolve_declared_path(
    declared: Option<String>,
    field_missing_reason: &str,
) -> ProjectLinkState {
    let Some(declared) = declared else {
        return ProjectLinkState::Unresolved {
            reason: field_missing_reason.to_string(),
        };
    };
    let path = PathBuf::from(&declared);
    if !crate::fs_gate::exists(&path) {
        return ProjectLinkState::Missing { path };
    }
    for ancestor in path.ancestors() {
        let git_path = ancestor.join(".git");
        let Ok(git_meta) = crate::fs_gate::symlink_metadata(&git_path) else {
            continue;
        };
        if git_meta.is_dir() {
            if let Some(dw) = crate::git::classify_main_checkout(ancestor, &git_path) {
                return ProjectLinkState::Linked {
                    project_id: dw.project_id,
                    project_name: dw.project_name,
                    project_path: dw.path,
                    source: LinkSource::Declared,
                    fallback_reason: None,
                    worktree_kind: "main".to_string(),
                };
            }
        } else if git_meta.is_file()
            && let Some(dw) = crate::git::classify_git_file(ancestor, &git_path)
        {
            let kind = if dw.kind == crate::report::WorktreeKind::Linked {
                "linked"
            } else {
                "main"
            };
            return ProjectLinkState::Linked {
                project_id: dw.project_id,
                project_name: dw.project_name,
                project_path: dw.path,
                source: LinkSource::Declared,
                fallback_reason: None,
                worktree_kind: kind.to_string(),
            };
        }
    }
    ProjectLinkState::NotAProject { path }
}

mod unit;
pub use unit::{AgentUnitBuilder, CandidateAgentUnit};

// Human keep/protect intent lives in `crate::protection`; re-exported
// here, where every caller has always found it.
#[cfg(feature = "testing")]
pub use crate::protection::protect_list;
pub use crate::protection::{
    ProtectList, ProtectListing, load_protect, protect_add, protect_listing, protect_path,
    protect_remove,
};

// ---------------------------------------------------------------------
// Active-session check (#92's acceptance): occupancy is checked only at
// the point of proposing/executing an action on a *specific* unit, not
// during ordinary identification -- an `lsof`-style check per unit would
// mean hundreds of process spawns on an ordinary `report`, which is
// exactly the "unacceptable scanning cost" #91 guards against, and would
// violate the no-blocking-scan discipline the TUI's render/event path
// depends on. `crate::occupancy::probe_path` is the existing seam.
// ---------------------------------------------------------------------

/// The boolean occupancy view, for test fixtures only (`testing`
/// feature): it collapses `Unknown` into `Occupied` and cannot say why,
/// so no production code may have it -- sinks take occupancy only
/// through `recheck::run_all`.
#[cfg(any(test, feature = "testing"))]
pub use crate::occupancy::is_active;

// ---------------------------------------------------------------------
// Discovery orchestration
// ---------------------------------------------------------------------

/// Every tool home the *authorized* scope lets this pass identify, as
/// `(tool_id, home)`.
///
/// This replaces the old loop over `scope.detectors`' raw `Resolved`
/// candidates, which never saw exclusions, disabled detectors or
/// explicit-root replacement -- the review's
/// `excluded_agent_home_must_not_be_scanned` counterexample. Only
/// `crate::scope` interprets detector output now
/// (`.oh/guardrails/discovery-consumes-effective-scope.md`).
///
/// A tool whose adapter declares
/// [`AdapterCapabilities::decomposes_every_location`] contributes every
/// authorized location; every other tool contributes its first, matching
/// the pre-existing contract. That used to be a hardcoded id match here.
fn authorized_tool_homes(
    scope: &EffectiveScope,
    registry: &registry::Registry,
) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    let mut seen_single: HashSet<String> = HashSet::new();
    let mut push = |tool_id: String, path: PathBuf, out: &mut Vec<(String, PathBuf)>| {
        let every = registry
            .get(&tool_id)
            .is_some_and(|a| a.capabilities().decomposes_every_location);
        if every {
            if !out.iter().any(|(t, p)| t == &tool_id && p == &path) {
                out.push((tool_id, path));
            }
        } else if seen_single.insert(tool_id.clone()) {
            out.push((tool_id, path));
        }
    };
    let roots = if scope.explicit {
        scope.authorized_detector_paths_in_explicit_roots()
    } else {
        scope.authorized_roots().0
    };
    for root in roots {
        if let Some(tool_id) = root.detector_id {
            push(tool_id, root.path, &mut out);
        }
    }
    out
}

/// The display name the authorized scope carries for this tool. Falls
/// back to the id: a name is presentation, and an unnamed tool is
/// better than a discovery pass reaching back into detector output for
/// one (`.oh/guardrails/discovery-consumes-effective-scope.md`).
fn tool_name_for(tool_id: &str, scope: &EffectiveScope) -> String {
    let roots = if scope.explicit {
        scope.authorized_detector_paths_in_explicit_roots()
    } else {
        scope.authorized_roots().0
    };
    roots
        .into_iter()
        .find(|r| r.detector_id.as_deref() == Some(tool_id))
        .and_then(|r| r.detector_name)
        .unwrap_or_else(|| tool_id.to_string())
}

/// Discovers and measures every agent unit for every implemented adapter
/// whose tool home resolved in `scope`. Mirrors
/// `external::discover_and_measure`'s `swamp_dir`/`observe` contract
/// exactly (`None` => no history; `observe: false` => read-only
/// annotation of existing history; `observe: true` => persist this
/// pass). Never walks a tool home not resolved by the detector registry,
/// and never reads past what each adapter's own bounded contract allows.
/// The test-fixture spelling of [`discover_and_measure_in`] (`testing`
/// feature only; the reviewers' counterexample files call it by this
/// name). Production discovery runs only inside the report pipeline,
/// which holds the [`crate::report::DiscoveryPass`].
#[cfg(feature = "testing")]
#[allow(clippy::too_many_arguments)]
pub fn discover_and_measure(
    scope: &EffectiveScope,
    project_worktrees: &[PathBuf],
    swamp_dir: Option<&Path>,
    observe: bool,
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
    coverage: &crate::fs_events::EventCoverage,
) -> Result<Vec<AgentUnit>> {
    discover_and_measure_in(
        &crate::report::DiscoveryPass::for_tests(),
        scope,
        project_worktrees,
        swamp_dir,
        observe,
        observed_at,
        retention_days,
        since_secs,
        coverage,
    )
}

/// Agent-storage discovery for one observation. Takes the
/// [`crate::report::DiscoveryPass`] only `report::observe_scope` mints,
/// so a second pass over the shared history table cannot be written
/// anywhere else (`.oh/guardrails/discovery-owned-by-report-pipeline.md`).
#[allow(clippy::too_many_arguments)]
pub fn discover_and_measure_in(
    _pass: &crate::report::DiscoveryPass,
    scope: &EffectiveScope,
    project_worktrees: &[PathBuf],
    swamp_dir: Option<&Path>,
    observe: bool,
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
    coverage: &crate::fs_events::EventCoverage,
) -> Result<Vec<AgentUnit>> {
    // Protection state is consulted here and again, freshly, at every
    // sink. Corrupt/unreadable state is *unknown*, not empty: rather
    // than fail the whole report, every unit is marked protected with
    // that reason, so identification still works and nothing is
    // proposable (`.oh/guardrails/protection-fails-closed.md`).
    let (protected_paths, protection_unknown): (ProtectList, Option<String>) = match swamp_dir {
        Some(dir) => match load_protect(dir) {
            Ok(p) => (p, None),
            Err(e) => (ProtectList::empty(), Some(e.to_string())),
        },
        None => (ProtectList::empty(), None),
    };

    let mut candidates_by_key: HashMap<String, (String, PathBuf, CandidateAgentUnit, u64)> =
        HashMap::new();
    let mut observed: Vec<ObservedExternal> = Vec::new();
    // Keys this pass identified but could not measure completely (a
    // bounded fold that hit its entry cap or an unreadable
    // subdirectory). Never entered into `observed`, so never diffed
    // against history; passed to `observe_and_annotate_external` so its
    // owned sweep does not tombstone them either -- a folder going
    // unreadable for one pass is coverage shrinking, not the unit
    // disappearing (`.oh/guardrails/coverage-changes-are-not-storage-
    // changes.md`), mirroring `external.rs`'s `protected_keys`.
    let mut incomplete_keys: HashSet<String> = HashSet::new();
    let mut covered_roots: Vec<PathBuf> = Vec::new();

    let adapters = registry::Registry::with_builtins();
    // The identification cache is the reason an unchanged pass costs
    // zero header reads. Without a store there is nowhere to keep it, so
    // it is disabled and every derivation reads live -- correct, just not
    // free.
    let cache = match swamp_dir {
        Some(dir) => IdentificationCache::load(dir),
        None => IdentificationCache::disabled(),
    };
    // The container cache is the reason an unchanged pass costs a `stat`
    // per container rather than a `stat` per session file. Same terms as
    // the derivation cache above: no store, no reuse.
    // The container cache turns an unchanged container from a listing
    // plus a `stat` per session file into nothing at all -- but only
    // where this pass's event coverage says nothing under it moved. With
    // no coverage it still *records* what this pass identifies, so the
    // next pass has something to replay.
    let containers = match swamp_dir {
        Some(dir) => ContainerCache::load(dir, coverage.clone()),
        None => ContainerCache::disabled(),
    }
    .with_known_worktrees(project_worktrees);
    let ctx = IdentifyCtx::with_containers(observed_at, &cache, &containers);

    // Authorized scope only: a tool home the user excluded, or whose
    // detector is disabled, or that lies outside an explicit command
    // root, is not discovered at all
    // (`.oh/guardrails/discovery-consumes-effective-scope.md`).
    for (tool_id, home) in authorized_tool_homes(scope, &adapters) {
        let tool_name = tool_name_for(&tool_id, scope);
        let Some(adapter) = adapters.get(&tool_id) else {
            continue;
        };
        let mut units = adapter.identify(&home, &ctx);
        // One resolution path for identified and replayed units alike:
        // the declared path live, then folder-name inference against
        // this pass's known worktrees (`ContainerCache::finish_link`).
        for cand in units.iter_mut() {
            containers.finish_link(cand);
        }
        covered_roots.push(home.clone());
        let device = device_of(&home);
        for cand in units {
            let key = unit_key(&tool_id, cand.category(), device, cand.path());
            if cand.complete() {
                observed.push(ObservedExternal {
                    key: key.clone(),
                    detector_id: tool_id.clone(),
                    category: cand.category().key_str(),
                    device,
                    path: cand.path().display().to_string(),
                    bytes: cand.bytes(),
                    hardlinked: true,
                });
            } else {
                incomplete_keys.insert(key.clone());
            }
            candidates_by_key.insert(key, (tool_name.clone(), home.clone(), cand, device));
        }
    }

    // Project-local units (#96, Aider): materially different shape from
    // every other tool in this catalog -- attached to each *known
    // project worktree root* the caller supplies, never derived from a
    // `crate::locations` detector home. Which adapters have them is the
    // adapter's own declared capability, not a tool-id match here.
    //
    // Skipped entirely when the tool's detector is disabled, so
    // disabling a detector always turns off everything it would
    // otherwise identify, home-level or project-local alike. "Enabled"
    // is decided by the authorized scope, not by reading the detector's
    // own status: an excluded home, a disabled detector, or an
    // explicit-root invocation that does not reach it all mean the same
    // thing here -- no units.
    for adapter in adapters.adapters() {
        if !adapter.capabilities().project_local_units {
            continue;
        }
        let tool_id = adapter.id();
        if !scope.detector_enabled(tool_id) {
            continue;
        }
        let tool_name = tool_name_for(tool_id, scope);
        for wt_path in project_worktrees {
            covered_roots.push(wt_path.clone());
            let device = device_of(wt_path);
            for cand in adapter.project_local_units(wt_path, &ctx) {
                let key = unit_key(tool_id, cand.category(), device, cand.path());
                if cand.complete() {
                    observed.push(ObservedExternal {
                        key: key.clone(),
                        detector_id: tool_id.to_string(),
                        category: cand.category().key_str(),
                        device,
                        path: cand.path().display().to_string(),
                        bytes: cand.bytes(),
                        hardlinked: true,
                    });
                } else {
                    incomplete_keys.insert(key.clone());
                }
                candidates_by_key.insert(key, (tool_name.clone(), wt_path.clone(), cand, device));
            }
        }
    }

    // The cache is persisted on the same terms as the growth history:
    // `observe: false` is a read-only annotation pass and leaves the
    // store untouched.
    if let Some(dir) = swamp_dir
        && observe
    {
        cache.save(dir, observed_at)?;
        containers.save(dir, observed_at)?;
    }

    // This observation owns only agent-family rows, and only under the
    // tool homes / worktrees it actually identified this pass. An
    // external observation running before or after it in the same store
    // can no longer tombstone these rows, nor these those
    // (`.oh/guardrails/history-sweeps-are-owned.md`).
    let ownership =
        crate::growth::ObservationOwnership::new(crate::growth::KeyFamily::Agent, covered_roots);
    let annotations: HashMap<String, (Option<i64>, u32)> = match swamp_dir {
        Some(dir) if observe => observe_and_annotate_external(
            dir,
            &observed,
            &incomplete_keys,
            &ownership,
            observed_at,
            retention_days,
            since_secs,
        )?,
        Some(dir) => {
            let keys: Vec<String> = observed.iter().map(|o| o.key.clone()).collect();
            annotate_readonly_external(dir, &keys, observed_at, retention_days, since_secs)?
        }
        None => HashMap::new(),
    };

    let mut units = Vec::with_capacity(candidates_by_key.len());
    for (key, (tool_name, tool_home, mut cand, _device)) in candidates_by_key {
        // Extract the detector_id back out of the key rather than
        // threading it separately; the key's first field always is it.
        let tool_id = key.split('\u{1}').next().unwrap_or_default().to_string();
        // An `Unverified` tool is one whose modeled layout this catalog
        // could not confirm against that tool's own source or
        // documentation (`crate::agents::matrix`). Identification still
        // runs -- knowing roughly where the bytes are is useful -- but
        // nothing is offered: no action, and no project linkage, because
        // both would be claims resting on the layout we just said we
        // could not verify. One place, so it cannot be forgotten in an
        // adapter (`docs/agent-storage.md`'s support matrix).
        if matrix::support_for(&tool_id) == Some(matrix::SupportLevel::Unverified) {
            cand.withdraw_for_unverified_layout(&tool_name);
        }
        // An incomplete candidate was never pushed into `observed`
        // (`.oh/guardrails/coverage-changes-are-not-storage-changes.md`:
        // a bounded fold's partial total must never anchor a
        // growth/regrowth delta), so it has no fresh entry in
        // `annotations` either. `growth_bytes` stays `None` rather than
        // "reset to a false zero"; `regrowth_count` is read back from
        // the store's last complete observation instead of defaulting
        // to 0, exactly as a protected external unit does
        // (`external.rs`'s `protected_keys` fallback).
        let (growth_bytes, regrowth_count) = if cand.complete() {
            annotations.get(&key).copied().unwrap_or((None, 0))
        } else {
            let stored_regrowth = swamp_dir
                .and_then(|dir| {
                    crate::growth::peek_external_current(dir, &key)
                        .ok()
                        .flatten()
                })
                .map(|(_, r)| r)
                .unwrap_or(0);
            (None, stored_regrowth)
        };
        let default_protected = cand.category().default_protected();
        // Both directions (`protection_conflict`): a unit beneath a
        // protected path, *and* a unit containing one. The latter is the
        // review's `protected_descendant_must_prevent_parent_cache_proposal`
        // counterexample -- protecting `debug/log.txt` must stop `debug/`
        // being proposed, or the protection means nothing.
        let human_protected = protected_paths.conflict(cand.path()).or_else(|| {
            cand.members()
                .iter()
                .find_map(|m| protected_paths.conflict(&m.path))
        });
        let (protected, protect_reason) = if let Some(why) = &protection_unknown {
            (true, Some(format!("protection state unknown: {why}")))
        } else if cand.protected() {
            (true, cand.protect_reason().map(str::to_string))
        } else if default_protected {
            (
                true,
                Some(format!(
                    "{} is protected by default (credentials/config/skills/automation)",
                    cand.category().label()
                )),
            )
        } else if let Some(reason) = human_protected {
            (true, Some(reason))
        } else {
            (false, None)
        };
        // Activity evidence (#54): the adapter already recorded
        // `mtime_max` while folding this unit's members; turn it into
        // the shared contract's fact rather than a second stat pass.
        let evidence = vec![crate::activity::modification_evidence_during(
            cand.mtime_max(),
            observed_at,
            crate::entities::now(),
        )];
        let parts = cand.into_parts();
        units.push(AgentUnit {
            id: unit_id(&tool_id, parts.category, &parts.relative_path),
            tool_id,
            tool_name,
            tool_home,
            category: parts.category,
            relative_path: parts.relative_path,
            path: parts.path,
            members: parts.members,
            bytes: parts.bytes,
            hardlinked: true,
            complete: parts.complete,
            growth_bytes,
            regrowth_count,
            observed_at,
            mtime_max: parts.mtime_max,
            protected,
            protect_reason,
            project_link: parts.project_link,
            action: parts.action,
            note: parts.note,
            evidence,
        });
    }
    units.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.id.cmp(&b.id)));
    Ok(units)
}

/// Re-identifies `tool_id`'s units under `home` from the **live**
/// filesystem, for an execution-time recheck.
///
/// The cache is [`IdentificationCache::disabled`], so nothing here can
/// be answered from a previous pass: an approval is spent against what
/// is on disk now, which is the whole point of a recheck
/// (`.oh/guardrails/execution-sinks-recheck-live-state.md`). An adapter
/// whose storage is project-local is re-identified against the worktree
/// root the plan recorded as its `tool_home`, exactly as it was
/// identified.
///
/// Replaces the second fourteen-arm `match tool_id` that used to live in
/// `crate::actions`, which could (and did) drift from the first.
pub fn reidentify_for_tool(
    tool_id: &str,
    home: &Path,
    observed_at: u64,
) -> Option<Vec<CandidateAgentUnit>> {
    let registry = registry::Registry::with_builtins();
    let adapter = registry.get(tool_id)?;
    let cache = IdentificationCache::disabled();
    let ctx = IdentifyCtx::new(observed_at, &cache);
    let caps = adapter.capabilities();
    let mut units = adapter.reidentify(home, &ctx);
    if caps.project_local_units {
        // The plan recorded the worktree root as `tool_home` for a
        // project-local unit, so the same path serves both halves.
        units.extend(adapter.project_local_units(home, &ctx));
    }
    Some(units)
}

/// Sum of every unit's bytes, for a tool/category total. Counted once
/// per unit regardless of member count (mirrors `external::total_bytes`).
pub fn total_bytes(units: &[AgentUnit]) -> u64 {
    units.iter().map(|u| u.bytes).sum()
}

/// The shared assertions behind every adapter's five required tests
/// (`.oh/guardrails/agent-adapter-test-contract.md`). They live here so
/// the *contract* is one reviewed implementation rather than fifteen
/// near-copies that can each drift, and so a new adapter's five tests
/// cannot be satisfied by five empty function bodies.
#[cfg(test)]
pub(crate) mod contract {
    use super::*;

    /// Runs `identify` and returns what it produced plus the work it
    /// did, so a test can assert on bytes read rather than trusting a
    /// comment.
    /// Scoped, not global: these tests run in parallel, and a global
    /// counter read here is a race against every other adapter's
    /// fixture (`crate::work_counters`).
    pub fn measured<T>(f: impl FnOnce() -> T) -> (T, crate::work_counters::WorkCounters) {
        crate::work_counters::measured(f)
    }

    /// Every unit in a default-protected category arrives protected, and
    /// carries a stated reason. This is what `AgentUnitBuilder::new`
    /// guarantees and what a `CandidateAgentUnit { .. }` literal could
    /// silently omit.
    pub fn protection_defaults_hold(units: &[CandidateAgentUnit]) {
        let mut checked = 0usize;
        for u in units {
            if u.category().default_protected() {
                checked += 1;
                assert!(
                    u.protected(),
                    "{} is in the default-protected category {} and arrived unprotected",
                    u.relative_path(),
                    u.category().label()
                );
                assert!(
                    u.protect_reason().is_some(),
                    "{} is protected with no stated reason",
                    u.relative_path()
                );
            }
            if u.protected() {
                assert_ne!(
                    u.action(),
                    AgentActionCapability::SessionRemoval,
                    "{} is protected yet offers session removal",
                    u.relative_path()
                );
            }
        }
        assert!(
            checked > 0,
            "this fixture produced no default-protected unit, so the assertion proved nothing; \
             give the fixture a config/credentials file, or state in the test why this adapter \
             has none"
        );
    }

    /// The form of [`protection_defaults_hold`] for an adapter that
    /// genuinely models no default-protected category (a log directory,
    /// an editor profile whose config lives elsewhere). It asserts that
    /// claim rather than assuming it, checks every *individually*
    /// protected unit still carries a reason and offers no session
    /// removal, and then exercises the builder's own guarantee through
    /// `tool_id`'s construction path so the test is never vacuous.
    pub fn protection_defaults_hold_with_no_protected_category(
        units: &[CandidateAgentUnit],
        tool_id: &str,
    ) {
        for u in units {
            assert!(
                !u.category().default_protected(),
                "{} is in the default-protected category {}, so this adapter does have one and \
                 should use `protection_defaults_hold`",
                u.relative_path(),
                u.category().label()
            );
            if u.protected() {
                assert!(
                    u.protect_reason().is_some(),
                    "{} is protected with no stated reason",
                    u.relative_path()
                );
                assert_ne!(
                    u.action(),
                    AgentActionCapability::SessionRemoval,
                    "{} is protected yet offers session removal",
                    u.relative_path()
                );
            }
        }
        let built = AgentUnitBuilder::new(
            tool_id,
            AgentCategory::ProtectedConfig,
            PathBuf::from("/nonexistent/fixture/credentials.json"),
        )
        .build();
        protection_defaults_hold(&[built]);
    }

    /// No field of any unit carries file *content*. The fixture seeds a
    /// canary into a session body; identification may read a header and
    /// must never carry it out.
    pub fn no_content_leak(units: &[CandidateAgentUnit], canary: &str) {
        let debug = format!("{units:?}");
        assert!(
            !debug.contains(canary),
            "content leaked into an AgentUnit field: {debug}"
        );
        for u in units {
            let json = serde_json::to_string(&super::AgentUnit {
                tool_id: "t".into(),
                tool_name: "t".into(),
                tool_home: PathBuf::from("/"),
                category: u.category(),
                id: "id".into(),
                relative_path: u.relative_path().to_string(),
                path: u.path.clone(),
                members: u.members().to_vec(),
                bytes: u.bytes(),
                hardlinked: true,
                complete: u.complete(),
                growth_bytes: None,
                regrowth_count: 0,
                observed_at: 0,
                mtime_max: u.mtime_max(),
                protected: u.protected(),
                protect_reason: u.protect_reason().map(str::to_string),
                project_link: u.project_link().clone(),
                action: u.action(),
                note: u.note.clone(),
                evidence: Vec::new(),
            })
            .expect("serialize");
            assert!(!json.contains(canary), "content leaked into JSON: {json}");
        }
    }

    /// Project linkage is either read from metadata the tool itself
    /// wrote, or explicitly unresolved. `forbidden` is a string that
    /// appears only in a *directory or file name* of the fixture, never
    /// in declared metadata: a `Linked`/`NotAProject` state naming it
    /// means the adapter guessed from a basename.
    pub fn linkage_is_declared_or_explicit(units: &[CandidateAgentUnit], forbidden: &str) {
        for u in units {
            match &u.project_link() {
                ProjectLinkState::Linked { source, .. } => {
                    assert_eq!(
                        *source,
                        LinkSource::Declared,
                        "{} claims a link from something other than declared metadata",
                        u.relative_path()
                    );
                }
                ProjectLinkState::Unresolved { reason } => {
                    assert!(
                        !reason.trim().is_empty(),
                        "{} is Unresolved with no stated reason",
                        u.relative_path()
                    );
                }
                _ => {}
            }
            let rendered = format!("{:?}", u.project_link());
            assert!(
                !rendered.contains(forbidden),
                "{} resolved a project from the basename {forbidden:?}: {rendered}",
                u.relative_path()
            );
        }
    }

    /// Identification read no more than `reads` capped headers' worth of
    /// content. The bound is the shared cap, which is what makes "we
    /// never read a transcript" a number instead of a promise.
    pub fn within_header_cap(counters: crate::work_counters::WorkCounters, reads: u64) {
        let cap = reads * bounded_io::MAX_HEADER_BYTES as u64;
        assert!(
            counters.header_bytes_read <= cap,
            "identification read {} bytes, above {reads} x the {} byte cap",
            counters.header_bytes_read,
            bounded_io::MAX_HEADER_BYTES
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_builder_protects_a_config_category_without_being_asked() {
        // The whole point of the builder: a literal can omit
        // `protected`, a constructor cannot.
        let u = AgentUnitBuilder::new(
            "t",
            AgentCategory::ProtectedConfig,
            PathBuf::from("/home/.credentials.json"),
        )
        .build();
        assert!(u.protected());
        assert!(u.protect_reason().is_some());
        let c =
            AgentUnitBuilder::new("t", AgentCategory::Caches, PathBuf::from("/home/cache")).build();
        assert!(!c.protected());
    }

    #[test]
    fn lifting_a_default_protection_records_a_reason() {
        let u = AgentUnitBuilder::new(
            "t",
            AgentCategory::ProtectedConfig,
            PathBuf::from("/home/x"),
        )
        .unprotect_with_reason("fixture")
        .build();
        assert!(!u.protected());
        assert!(
            u.protect_reason().is_some_and(|r| r.contains("fixture")),
            "an unprotect must leave its reason behind"
        );
    }

    #[test]
    fn an_unchanged_file_is_derived_once_and_then_cached() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("s.jsonl");
        std::fs::write(&f, b"{\"cwd\":\"/x\"}\nBODY\n").unwrap();
        let cache = IdentificationCache::disabled();
        // Disabled: every call reads.
        let ctx = IdentifyCtx::new(1, &cache);
        let (_, counted) = crate::work_counters::measured(|| {
            for _ in 0..3 {
                assert_eq!(
                    ctx.derived("t", "cwd", &f, 4096, &|s| Some(
                        s.lines().next()?.to_string()
                    )),
                    Some("{\"cwd\":\"/x\"}".to_string())
                );
            }
        });
        assert!(counted.header_bytes_read > 0);

        // Enabled: the first call reads, the rest do not.
        let store = tempfile::tempdir().unwrap();
        let cache = IdentificationCache::load(store.path());
        let ctx = IdentifyCtx::new(1, &cache);
        let _ = ctx.derived("t", "cwd", &f, 4096, &|s| {
            Some(s.lines().next()?.to_string())
        });
        let (_, counted) = crate::work_counters::measured(|| {
            for _ in 0..3 {
                let _ = ctx.derived("t", "cwd", &f, 4096, &|s| {
                    Some(s.lines().next()?.to_string())
                });
            }
        });
        assert_eq!(
            counted.header_bytes_read, 0,
            "a cached derivation must not re-read the file"
        );

        // A rewritten file invalidates it: a stale answer is worse than
        // a slow one.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&f, b"{\"cwd\":\"/y\"}\nBODY\n").unwrap();
        let (got, counted) = crate::work_counters::measured(|| {
            ctx.derived("t", "cwd", &f, 4096, &|s| {
                Some(s.lines().next()?.to_string())
            })
        });
        assert_eq!(got, Some("{\"cwd\":\"/y\"}".to_string()));
        assert!(
            counted.header_bytes_read > 0,
            "a changed file must be re-read"
        );
    }

    #[test]
    fn a_same_second_same_size_rewrite_invalidates_the_cached_derivation() {
        // The hole two independent adversarial passes found, with no
        // sleep: a session re-linked to a different project of the same
        // path length, rewritten within the same wall-clock second. A
        // `(size, mtime_secs)` fingerprint cannot see it and serves the
        // old project; the whole cache is then a silent source of wrong
        // attribution. This test has no `sleep` on purpose -- adding one
        // is what hid it.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("s.jsonl");
        let store = tempfile::tempdir().unwrap();
        let cache = IdentificationCache::load(store.path());
        let ctx = IdentifyCtx::new(1, &cache);
        let cwd = |s: &str| {
            Some(
                serde_json::from_str::<serde_json::Value>(s)
                    .ok()?
                    .get("cwd")?
                    .as_str()?
                    .to_string(),
            )
        };
        std::fs::write(&f, b"{\"cwd\":\"/aaa/one\"}\n").unwrap();
        assert_eq!(
            ctx.derived("t", "cwd", &f, 4096, &cwd),
            Some("/aaa/one".to_string())
        );
        // Same byte length, same second.
        std::fs::write(&f, b"{\"cwd\":\"/bbb/two\"}\n").unwrap();
        assert_eq!(
            ctx.derived("t", "cwd", &f, 4096, &cwd),
            Some("/bbb/two".to_string()),
            "a same-size rewrite in the same second must not be answered from the cache"
        );
    }

    #[test]
    fn a_replaced_file_is_a_different_file_however_its_timestamps_look() {
        // A rename-over preserves mtime. The inode does not.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("s.jsonl");
        let store = tempfile::tempdir().unwrap();
        let cache = IdentificationCache::load(store.path());
        let ctx = IdentifyCtx::new(1, &cache);
        let first = |s: &str| Some(s.trim().to_string());
        std::fs::write(&f, b"AAAA\n").unwrap();
        assert_eq!(ctx.derived("t", "k", &f, 64, &first), Some("AAAA".into()));
        let replacement = dir.path().join("tmp");
        std::fs::write(&replacement, b"BBBB\n").unwrap();
        // Copy the original's mtime onto the replacement, then swap it
        // in: size and mtime now match the cached fingerprint exactly.
        let mtime = std::fs::metadata(&f).unwrap().modified().unwrap();
        std::fs::File::options()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        std::fs::rename(&replacement, &f).unwrap();
        assert_eq!(
            ctx.derived("t", "k", &f, 64, &first),
            Some("BBBB".into()),
            "a replaced inode must invalidate the cached derivation"
        );
    }

    #[test]
    fn an_absent_derivation_is_cached_too() {
        // Otherwise "this session declares no cwd" costs a read forever.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("s.jsonl");
        std::fs::write(&f, b"not json\n").unwrap();
        let store = tempfile::tempdir().unwrap();
        let cache = IdentificationCache::load(store.path());
        let ctx = IdentifyCtx::new(1, &cache);
        assert_eq!(ctx.derived("t", "cwd", &f, 4096, &|_| None), None);
        let (got, counted) =
            crate::work_counters::measured(|| ctx.derived("t", "cwd", &f, 4096, &|_| None));
        assert_eq!(got, None);
        assert_eq!(counted.header_bytes_read, 0);
    }

    #[test]
    fn the_cache_round_trips_through_the_parquet_table() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("s.jsonl");
        std::fs::write(&f, b"{\"cwd\":\"/x\"}\n").unwrap();
        let store = tempfile::tempdir().unwrap();
        {
            let cache = IdentificationCache::load(store.path());
            let ctx = IdentifyCtx::new(1, &cache);
            let _ = ctx.derived("t", "cwd", &f, 4096, &|s| Some(s.trim().to_string()));
            cache.save(store.path(), 1).unwrap();
        }
        let cache = IdentificationCache::load(store.path());
        let ctx = IdentifyCtx::new(2, &cache);
        let (got, counted) = crate::work_counters::measured(|| {
            ctx.derived("t", "cwd", &f, 4096, &|s| Some(s.trim().to_string()))
        });
        assert_eq!(got, Some("{\"cwd\":\"/x\"}".to_string()));
        assert_eq!(
            counted.header_bytes_read, 0,
            "a cache that does not survive a restart closes nothing"
        );
    }

    #[test]
    fn a_disabled_cache_never_writes_the_store() {
        let store = tempfile::tempdir().unwrap();
        IdentificationCache::disabled()
            .save(store.path(), 1)
            .unwrap();
        assert!(
            !store.path().join("associations").exists(),
            "an execution-time recheck must not write an identification cache"
        );
    }

    #[test]
    fn category_default_protection_is_config_only() {
        for c in [
            AgentCategory::Sessions,
            AgentCategory::ArchivedSessions,
            AgentCategory::Attachments,
            AgentCategory::Checkpoints,
            AgentCategory::Caches,
            AgentCategory::Logs,
            AgentCategory::ManagedWorktrees,
            AgentCategory::Plugins,
            AgentCategory::Unclassified,
        ] {
            assert!(
                !c.default_protected(),
                "{c:?} must not be protected by category default"
            );
        }
        assert!(AgentCategory::ProtectedConfig.default_protected());
        assert!(AgentCategory::ProtectedDatabases.default_protected());
    }

    #[test]
    fn category_key_strings_never_collide_with_storage_category() {
        // Defensive: the growth store's key family is shared with
        // `external.rs`'s top-level detector locations, which use
        // `StorageCategory`'s own kebab strings; the "agent:" prefix
        // must make collision structurally impossible.
        let storage_strs = [
            "installation",
            "downloads",
            "cache",
            "local-state",
            "environments",
            "build-output",
            "models",
            "unclassified",
        ];
        for c in [
            AgentCategory::Sessions,
            AgentCategory::ArchivedSessions,
            AgentCategory::Attachments,
            AgentCategory::Checkpoints,
            AgentCategory::Caches,
            AgentCategory::Logs,
            AgentCategory::ManagedWorktrees,
            AgentCategory::Plugins,
            AgentCategory::ProtectedConfig,
            AgentCategory::ProtectedDatabases,
            AgentCategory::Unclassified,
        ] {
            assert!(!storage_strs.contains(&c.key_str().as_str()));
            assert!(c.key_str().starts_with("agent:"));
        }
    }

    #[test]
    fn protect_add_list_remove_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("some-session.jsonl");
        std::fs::write(&target, b"x").unwrap();
        protect_add(dir.path(), &target).unwrap();
        let listed = protect_list(dir.path()).unwrap();
        assert_eq!(listed.len(), 1);
        assert!(
            load_protect(dir.path())
                .unwrap()
                .conflict(&target)
                .is_some()
        );
        // Idempotent add.
        protect_add(dir.path(), &target).unwrap();
        assert_eq!(protect_list(dir.path()).unwrap().len(), 1);
        protect_remove(dir.path(), &target).unwrap();
        assert!(protect_list(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn protect_survives_reload_and_blocks_by_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        protect_add(dir.path(), &home).unwrap();
        let reloaded = load_protect(dir.path()).unwrap();
        let nested = home.join("projects/x/session.jsonl");
        assert!(reloaded.conflict(&nested).is_some());
    }

    #[test]
    fn folded_bytes_bounds_a_pathological_directory() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..50 {
            std::fs::write(dir.path().join(format!("f{i}")), b"12345").unwrap();
        }
        let (bytes, _mtime, truncated) = folded_bytes(dir.path(), 10);
        assert!(
            truncated,
            "must report truncation, not a silently short total"
        );
        assert!(bytes > 0);
        let (bytes_full, _, truncated_full) = folded_bytes(dir.path(), 1000);
        assert!(!truncated_full);
        assert_eq!(bytes_full, 50 * 5);
    }

    #[test]
    fn folded_bytes_handles_a_plain_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("session.jsonl");
        std::fs::write(&f, b"0123456789").unwrap();
        let (bytes, _mtime, truncated) = folded_bytes(&f, 100);
        assert_eq!(bytes, 10);
        assert!(!truncated);
    }
}
