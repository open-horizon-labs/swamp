//! Report contract: the shape every later slice fills in.
//!
//! This module defines types and rendering only. Discovery, attribution,
//! growth, and Docker joins are later slices (R2-R5); `report()` here is a
//! stub entry point that returns an empty report so the golden test can
//! exercise the real contract shape before any discovery logic exists.

use crate::entities::Confidence;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The text renderer lives in `render.rs`; re-exported here so existing
/// `report::render_text` call sites keep working.
pub use crate::render::render_text;

/// Where a fact came from (a tool invocation, a filesystem walk, ...).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Source {
    pub tool: String,
}

impl Source {
    pub fn new(tool: impl Into<String>) -> Self {
        Self { tool: tool.into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArtifactKind {
    BuildOutput,
    DependencyTree,
    Git,
    /// A rebuildable tool/download cache found *inside* a checkout or
    /// worktree (e.g. `.cache`, `.parcel-cache`). A cache of the same
    /// shape found *outside* every checkout is not an artifact row at
    /// all; it becomes an `UnownedRow` with reason `SharedCache` instead.
    Cache,
    /// Bytes under a worktree that git tracks: authored work, in the
    /// index, recoverable from a remote. This is what "source" means.
    /// At most one `Source` row per worktree.
    Source,
    /// Non-artifact bytes under a worktree that a gitignore rule
    /// matches. Outside version control entirely: generated output
    /// (rebuildable) or private data (irrecoverable), and git cannot
    /// bring any of it back. Split out from `Source` because calling
    /// ignored bytes "source" is a lie -- see `TrackState`.
    Ignored,
    /// Non-artifact bytes under a worktree that are in no index and
    /// matched by no ignore rule: in the worktree and in no version
    /// control at all.
    Untracked,
    DockerImage,
    DockerBuildCache,
    DockerVolume,
    Loose,
    Unknown,
}

impl ArtifactKind {
    /// True for the three kinds that stand for "everything under this
    /// worktree that is not a classified artifact", split by git
    /// tracking state. They share one path (the worktree root), are
    /// aggregates over many files rather than one folded directory, and
    /// every place that special-cased `Source` means all three.
    pub fn is_worktree_remainder(&self) -> bool {
        matches!(
            self,
            ArtifactKind::Source | ArtifactKind::Ignored | ArtifactKind::Untracked
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum WorktreeKind {
    Main,
    Linked,
    /// A main checkout that is not the first-seen main checkout of its
    /// project: another clone of the same remote, discovered under this
    /// root. Grouped into the same [`ProjectRow`] as every other
    /// checkout/worktree of that project (via normalized remote URL when
    /// one is known), listed alongside `Main` and `Linked` rows rather
    /// than becoming a second project.
    Clone,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum UnownedReason {
    OutsideAnyCheckout,
    OwnedByNothing,
    InconclusiveEvidence,
    /// Outside every discovered checkout/worktree and not a recognized
    /// shared cache: nothing claims this path.
    NoContainingRepo,
    /// Outside every discovered checkout/worktree, but its basename/kind
    /// is a cache shape shared across projects (`.cache`,
    /// `.cargo-registry`, `.npm`, `.pnpm-store`, a top-level `.gradle`).
    /// Counted exactly once here, never duplicated into any project.
    SharedCache,
    /// The walk could not read this path (permissions). Bytes are
    /// recorded as 0; the row exists so the count is visible instead of
    /// silently dropped.
    PermissionDenied,
    /// A Docker object (image/build-cache/volume) with no explicit join
    /// evidence: no matching compose-project label, no label whose value
    /// is a path inside a discovered worktree, and no
    /// `org.opencontainers.image.source` matching a project's git remote.
    /// Name similarity to a project is never evidence.
    DockerNoJoin,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Summary {
    pub projects: usize,
    pub worktrees: usize,
    pub artifacts: usize,
    /// Keyed by ecosystem tag (`rs`, `js`, …); `other` for artifacts no
    /// ecosystem claims (`.cache`, `.git`).
    pub by_type: std::collections::BTreeMap<String, TypeSummary>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TypeSummary {
    pub name: String,
    pub projects: usize,
    pub artifacts: usize,
    pub bytes: u64,
    pub growth_bytes: Option<i64>,
}

/// Builds [`Summary`] from annotated project rows.
pub fn summarize(projects: &[ProjectRow]) -> Summary {
    let mut s = Summary {
        projects: projects.len(),
        ..Default::default()
    };
    for p in projects {
        for tag in &p.ecosystems {
            let e = s.by_type.entry(tag.clone()).or_default();
            e.name = crate::ecosystem::name_for(tag).unwrap_or(tag).to_string();
            e.projects += 1;
        }
        for wt in &p.worktrees {
            s.worktrees += 1;
            for a in &wt.artifacts {
                if a.kind.is_worktree_remainder() || a.kind == ArtifactKind::Git {
                    continue;
                }
                s.artifacts += 1;
                let key = a.ecosystem.clone().unwrap_or_else(|| "other".into());
                let e = s.by_type.entry(key.clone()).or_default();
                if e.name.is_empty() {
                    e.name = crate::ecosystem::name_for(&key)
                        .unwrap_or("other")
                        .to_string();
                }
                e.artifacts += 1;
                e.bytes += a.bytes;
                if let Some(g) = a.growth_bytes {
                    e.growth_bytes = Some(e.growth_bytes.unwrap_or(0) + g);
                }
            }
        }
    }
    s
}

fn yes() -> bool {
    true
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRow {
    pub kind: ArtifactKind,
    pub path: PathBuf,
    pub bytes: u64,
    /// Newest file mtime inside the unit (secs since epoch): how long ago
    /// this artifact was last written. Zero when not recorded (Source
    /// rows, Docker rows, stores written before this field).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub mtime_max: u64,
    /// Ecosystem tag that generates this artifact (`rs` for `target`,
    /// `js` for `node_modules`), see `ecosystem::artifact_ecosystem`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ecosystem: Option<String>,
    /// Whether this unit contains hardlinked files (`nlink > 1`). A unit
    /// without them can be re-sized from its stored per-directory rows,
    /// because summing those rows counts every byte exactly once. A unit
    /// with them still gets current directory allocations, but its unique-byte
    /// count is retained as stale until reconciliation. `true` is conservative
    /// when nobody has measured: allocation sums are not unique-byte totals.
    #[serde(default = "yes")]
    pub hardlinked: bool,
    /// True when unique-byte totals await reconciliation; directory allocations are current.
    #[serde(default)]
    pub dedup_stale: bool,
    /// Current path allocations (hardlinks may count more than once).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allocated_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allocated_growth_bytes: Option<i64>,
    /// Bytes with hardlinks deduplicated *within this row only* (a
    /// deterministic per-row figure), unlike `bytes`, where a hardlinked
    /// inode is charged to whichever row the full walk saw first. The
    /// incremental path applies `new_local - old_local` to `bytes` so a
    /// re-sized row never re-charges inodes another row already holds
    /// (#29). Zero means "not recorded"; readers fall back to `bytes`.
    #[serde(default)]
    pub local_bytes: u64,
    /// git tracking status of this path: tracked / ignored / untracked.
    /// Untracked content is in no version control and under no ignore
    /// rule — it exists only here. Filled by `annotate_tracking`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<crate::ignore::TrackState>,
    pub growth_bytes: Option<i64>,
    pub regrowth_count: u32,
    pub observed_at: u64,
    pub confidence: Confidence,
    pub source: Source,
    /// Set for a joined row whose evidence needed a tie-break, e.g. a
    /// `compose_ambiguous=<n>` note when several worktrees of one project
    /// contain a compose file naming the same project.
    #[serde(default)]
    pub note: Option<String>,
    /// Docker detail (#33), set only for a joined Docker row. See the
    /// matching fields on [`UnownedRow`].
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub containers: Vec<String>,
    #[serde(default)]
    pub shared_with: Vec<String>,
    #[serde(default)]
    pub dangling: bool,
    /// Decision evidence (#53): activity, consumer, current-use, recovery
    /// and reclaimability facts for this unit. Populated from the same
    /// folded-walk stats and existing enrichment already gathered for
    /// this row -- never a second per-file pass. Empty (and omitted from
    /// JSON) for a row no evidence source has populated yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<crate::evidence::Evidence>,
}

/// One directory measurement within a worktree, including folded artifact
/// interiors. Interior rows support incremental sizing without retaining files;
/// artifact boundaries prevent their totals being added twice to source rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirRollup {
    pub worktree_id: String,
    /// git tracking status of this directory (see `ArtifactRow::track`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<crate::ignore::TrackState>,
    /// Relative to the worktree root. The worktree root itself is `""`.
    pub rel_path: String,
    /// `None` only for the worktree root's own row.
    pub parent_rel_path: Option<String>,
    /// This directory's own files plus every descendant Source
    /// directory's `allocated_total` (bottom-up sum). Does not include
    /// bytes folded into an artifact row directly beneath it.
    pub allocated_total: u64,
    /// Allocated bytes of the files directly inside this directory only.
    pub own_allocated: u64,
    pub file_count: u32,
    /// Files + subdirectories + symlinks directly inside this directory.
    pub entry_count: u32,
    pub symlink_count: u32,
    /// Newest mtime among this directory's own direct entries, in
    /// minutes since the Unix epoch.
    pub mod_time_min: i32,
    pub complete: bool,
    #[serde(default)]
    pub growth_bytes: Option<i64>,
}

/// R4c: one large file (>= `large_file_min_bytes`) found under a
/// worktree's `Source` tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRow {
    pub worktree_id: String,
    /// Relative to the worktree root.
    pub rel_path: String,
    pub allocated: u64,
    pub mod_time_min: i32,
    #[serde(default)]
    pub growth_bytes: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeRow {
    pub worktree_id: String,
    pub path: PathBuf,
    pub kind: WorktreeKind,
    pub artifacts: Vec<ArtifactRow>,
    pub signals: Vec<Signal>,
    /// Current branch (`None` for a detached HEAD). Used for GitHub
    /// enrichment and shown in the `--view worktrees` printer.
    #[serde(default)]
    pub branch: Option<String>,
    /// GitHub facts for `branch`, when the remote is on github.com and
    /// enrichment ran. `None` when the remote isn't GitHub (never
    /// queried; not the same as `Unknown`, which means GitHub was asked
    /// and couldn't answer).
    #[serde(default)]
    pub github: Option<crate::github::GithubFacts>,
    /// Composite `merge-complete` fact -- always emitted with its terms.
    /// `None` when `github` is `None` (nothing to compose from).
    #[serde(default)]
    pub merge_complete: Option<crate::github::MergeComplete>,
    /// `now - max(last commit time, newest mtime observed in Source)`,
    /// in seconds. `None` when neither could be established (no commits
    /// and nothing readable under the worktree).
    #[serde(default)]
    pub idle_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRow {
    pub project_id: String,
    pub name: String,
    pub worktrees: Vec<WorktreeRow>,
    /// Ecosystem tags detected at the main checkout root (`rs`, `js`,
    /// `py`, …), see `ecosystem::ECOSYSTEMS`. A project can be several.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ecosystems: Vec<String>,
    /// Normalized `origin` remote URL shared by this project's
    /// checkouts, when at least one of them has one configured. `None`
    /// when no discovered checkout/worktree of this project has an
    /// `origin` remote (identity then falls back to object-store id).
    #[serde(default)]
    pub remote: Option<String>,
}

/// Boundary of a filesystem measurement, not an ownership claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnownedMeasurement {
    Direct,
    Subtree,
    /// Older measurement: sharing is known but direct/subtree shape is not.
    Hardlinked,
    DirectShared,
    SubtreeShared,
    /// Refreshed path allocation; unique charge is not reconciled.
    DirectEstimate,
    SubtreeEstimate,
}

impl UnownedMeasurement {
    pub(crate) fn folded(self) -> bool {
        matches!(
            self,
            Self::Subtree | Self::SubtreeShared | Self::SubtreeEstimate
        )
    }
    pub fn unique_needs_reconciliation(self) -> bool {
        matches!(self, Self::DirectEstimate | Self::SubtreeEstimate)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnownedRow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measurement: Option<UnownedMeasurement>,
    pub path_or_object: String,
    pub bytes: u64,
    pub reason: UnownedReason,
    /// Set for a `DockerNoJoin` image/volume: the object's `SharedSize`,
    /// shown alongside `bytes` (its `UniqueSize`) so the row's total
    /// footprint is visible even though only the unique bytes count.
    #[serde(default)]
    pub shared_bytes: Option<u64>,
    /// Set when an `org.opencontainers.image.source` label was read but
    /// matched no discovered project's git remote: the base image's
    /// source repo, recorded for visibility, never used to attribute.
    #[serde(default)]
    pub note: Option<String>,
    /// Set for a `DockerNoJoin` row: which Docker object kind it is
    /// (`"image"`, `"build-cache"`, `"volume"`), so the renderer can
    /// fold every unjoined Docker object into one summary line per kind
    /// instead of one line per object (a real `~/src` scan can have
    /// hundreds of unjoined build-cache entries).
    #[serde(default)]
    pub docker_kind: Option<String>,
    /// Docker detail (#33), set only for a Docker row: creation
    /// timestamp (image `CreatedAt`/volume inspect `CreatedAt`).
    #[serde(default)]
    pub created_at: Option<String>,
    /// Containers referencing this image/volume, formatted `"name
    /// (state[, finished <finished_at>])"`. Facts only, never a verdict
    /// about whether the object is safe to remove.
    #[serde(default)]
    pub containers: Vec<String>,
    /// Other image references sharing >=1 layer digest with this image;
    /// empty for volumes/build-cache.
    #[serde(default)]
    pub shared_with: Vec<String>,
    /// True for an image with no `RepoTags` at all.
    #[serde(default)]
    pub dangling: bool,
    /// Recovery/reclaimability evidence (#58): an unjoined Docker object
    /// still gets a real per-object recovery assessment -- "no project
    /// claims it" is a consumer fact, not a reason to skip its own
    /// recovery/reclaimability facts. Populated at the same join site as
    /// `ArtifactRow::evidence` for a joined Docker row.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<crate::evidence::Evidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniqueEstimate {
    #[serde(default)]
    pub sharing: Option<crate::sharing::SharingSummary>,
    /// Allocated regular-file bytes across observed roots and units, excluding
    /// Docker. Deduplicated by device/inode, not shared filesystem extents.
    /// Not a sum of the per-root charges, and not a reclaimability estimate.
    pub bytes: u64,
    pub reconciled_at: u64,
    pub needs_reconciliation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reconciliation {
    #[serde(default)]
    pub unique_estimate: Option<UniqueEstimate>,
    pub attributed: u64,
    pub unowned: u64,
    pub walked_total: u64,
    pub du_total: Option<u64>,
    /// Bytes of Docker objects joined to a project/worktree. Tracked
    /// separately from `attributed`/`walked_total`: Docker objects are
    /// not on the walked filesystem.
    pub docker_attributed: u64,
    /// Bytes of Docker objects with no join evidence (`DockerNoJoin`).
    /// Tracked separately from `unowned`/`walked_total` for the same
    /// reason.
    pub docker_unowned: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub observed_at: u64,
    pub root: PathBuf,
    pub projects: Vec<ProjectRow>,
    pub unowned: Vec<UnownedRow>,
    pub reconciliation: Reconciliation,
    /// Byte history per artifact row over the growth window, sampled into
    /// equal buckets, keyed by `growth::series_key`. Read from the
    /// reverse-delta store; empty when there is no store.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub series_by_key: std::collections::HashMap<String, Vec<Option<u64>>>,
    /// Sum of every row's series per bucket: the whole root over time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub total_series: Vec<Option<u64>>,
    /// Seconds each series spans (the effective growth window).
    #[serde(default)]
    pub series_window_secs: u64,
    /// Coverage notes that are not per-row facts, e.g. "docker:
    /// unavailable (...)" when the daemon could not be reached.
    #[serde(default)]
    pub notes: Vec<String>,
    /// R4c: per-worktree directory drill-down, populated only when the
    /// caller asked for it (`report_with(.., include_dirs: true)`) so the
    /// default report payload stays small.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirs_by_worktree: Option<std::collections::HashMap<String, Vec<DirRollup>>>,
    /// R4c: per-worktree large-file rows, same opt-in as `dirs_by_worktree`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_by_worktree: Option<std::collections::HashMap<String, Vec<FileRow>>>,
    /// Scheduled-observation status line for the report header (item 5 of
    /// #31): "last scheduled run 12m ago (full, 4.2 s)" when a schedule
    /// exists, or a "no schedule (...)" suggestion when it does not. Only
    /// populated when a store directory was supplied.
    #[serde(default)]
    pub schedule_line: Option<String>,
    /// Per-ecosystem rollup across the root: how many projects wear the
    /// tag, and the bytes/growth of the artifacts that ecosystem generates.
    #[serde(default)]
    pub summary: Summary,
    /// Set only when this call ran live GitHub enrichment (`enrich:
    /// true` -- `swamp observe`'s full walk, or `report --enrich`).
    /// `None` for a plain `report` call, which reads `enrich.parquet`
    /// as-is and never shells out to `gh`.
    #[serde(default)]
    pub github_enrichment: Option<GithubEnrichmentSummary>,
    /// Nested Cargo/build-artifact facts. These are identification units
    /// inside existing artifact rows; their physical bytes are not added to
    /// reconciliation totals a second time. Each unit's own `guidance`
    /// field (serialized as `"cleanup"`) carries the cargo-cleanup
    /// recommendation computed once at observe time
    /// (`attach_cargo_guidance`) -- R18a removed the `serialize_with`
    /// hook that used to compute it fresh from a live clock at
    /// serialization time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nested_artifacts: Vec<crate::artifact::NestedArtifact>,
    /// The store this report was produced against, when there was one.
    ///
    /// Carried so a later decision can consult *live* state rather than
    /// a snapshot taken when the report was built. `propose` uses it to
    /// reload human keep/protect intent at proposal time: the PR #123
    /// review found protection was checked against a caller-supplied
    /// list that an unreadable protect file silently emptied
    /// (`.oh/guardrails/protection-fails-closed.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_dir: Option<PathBuf>,
}

/// Live GitHub enrichment stats for one `report_full(.., enrich: true)`
/// call: how many `gh api graphql` calls it made (one per `(owner,
/// repo)` that needed a refresh) and how long that took, independent of
/// the rest of the report's wall time.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GithubEnrichmentSummary {
    pub calls_made: u32,
    pub worktrees_enriched: u32,
    pub elapsed_secs: f64,
}

/// R2 discovers projects (checkouts and linked worktrees) under `root`
/// and fills `ProjectRow`/`WorktreeRow`. R3 then classifies artifact
/// directories at discovery, attributes each to its nearest containing
/// worktree, and folds the rest of every worktree into one `Source` row.
/// Growth and full Docker joins remain later slices (R4-R5); the one
/// Docker fact this pass understands is "no compose-project label
/// matches a discovered project name", which lands as an `UnownedRow`
/// with reason `OwnedByNothing` rather than being attributed anywhere.
pub fn report(root: &Path, docker_facts: Option<&Path>) -> Result<Report> {
    report_with(root, docker_facts, false, None, None)
}

/// Whether `path` sits on a filesystem whose extents can be shared with
/// a clone or a snapshot *outside* the unit being measured (APFS). On
/// such a volume, removing a unit does not necessarily free its
/// allocated blocks: a cloned copy elsewhere, or a Time Machine local
/// snapshot, can retain every extent. This pass does not query that
/// sharing (there is no bounded way to), so the honest answer is a bound
/// -- see `reclaimability::apfs_clone_or_snapshot_bound`.
///
/// One `statfs` per row, the same bounded per-unit read
/// `activity::access_time_evidence` already makes; never a per-file
/// pass. A failed `statfs` answers `None`, which keeps the existing
/// exact figure rather than inventing uncertainty.
/// Every read is `fs_gate::sys::volume_info`'s one `statfs(2)`; there is
/// no raw `libc::statfs` here (`.oh/architecture.md`, "Capability
/// gates" -- `libc` may be named only inside `fs_gate`).
#[cfg(target_os = "macos")]
fn shared_extent_filesystem(path: &Path) -> Option<&'static str> {
    crate::fs_gate::sys::volume_info(path)
        .ok()
        .filter(|v| v.is_apfs())
        .map(|_| "APFS")
}

/// Linux: the filesystems whose allocated blocks are not the space
/// removing a file returns (owner decision (c), 2026-09-22). Btrfs,
/// bcachefs and XFS share extents through reflinks and snapshots; Btrfs,
/// bcachefs and ZFS compress and deduplicate below the file; overlayfs
/// reports a merged view over layers the unit does not own. On each, a
/// sum of `st_blocks * 512` is an **upper bound** on what a removal
/// frees, and is labelled as one. ext4 and tmpfs share nothing and keep
/// the exact figure.
///
/// `statfs(2)`'s `f_type` magic numbers are from `linux/magic.h` (ZFS's
/// from OpenZFS, which is out of tree). A failed `statfs` answers
/// `None`, as on macOS.
#[cfg(target_os = "linux")]
fn shared_extent_filesystem(path: &Path) -> Option<&'static str> {
    let f_type = crate::fs_gate::sys::volume_info(path).ok()?.type_magic;
    crate::reclaimability::shared_extent_filesystem_for_magic(f_type)
}

/// No copy-on-write extent sharing is claimed on a platform where this
/// pass has no bounded way to establish it; the exact figure stands.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn shared_extent_filesystem(_path: &Path) -> Option<&'static str> {
    None
}

/// Populates every `ArtifactRow.evidence` (#53) from facts this report
/// pass already has in hand -- `mtime_max`, `hardlinked`/`dedup_stale`,
/// the row's own kind and its worktree's path -- never a new per-file
/// walk. Called exactly once, from `bus::run_report`, so every report
/// (single- or multi-root, CLI/TUI/JSON) carries the same evidence.
/// Existing evidence a row already carries (e.g. the Docker-join
/// consumer fact attached at construction in `join_docker`) is
/// preserved; this only appends.
pub fn attach_decision_evidence(report: &mut Report) {
    let observed_at = report.observed_at;
    for project in &mut report.projects {
        for wt in &mut project.worktrees {
            let wt_path = wt.path.clone();
            for a in &mut wt.artifacts {
                // A Docker row's "path" is a repo tag, image id or volume
                // name: an object the daemon owns, not a filesystem path.
                // Nothing here that stats a path may run for it -- that is
                // the provenance discipline
                // `reviewer_counterexamples_123`'s
                // `docker_reclaimability_must_not_claim_filesystem_provenance`
                // pins.
                let is_docker_object = matches!(
                    a.kind,
                    ArtifactKind::DockerImage
                        | ArtifactKind::DockerVolume
                        | ArtifactKind::DockerBuildCache
                );

                // Activity (#54): the folded walk's own newest-child-mtime
                // stat, already recorded on every row.
                a.evidence
                    .push(crate::activity::modification_evidence_during(
                        a.mtime_max,
                        observed_at,
                        crate::entities::now(),
                    ));

                // Activity (#54), the second filesystem source: this
                // unit's own anchor path's access time. One extra `stat`
                // plus one `statfs` for this row, never a per-file pass.
                // On a `noatime`/`relatime` mount the fact *is* that
                // atime cannot support a recent-access claim, which
                // `access_time_evidence` reports as `Unavailable` with
                // the mount option named -- omitting the row instead
                // would leave "was this opened" looking unasked rather
                // than unanswerable.
                if !is_docker_object {
                    a.evidence
                        .push(crate::activity::access_time_evidence(&a.path, observed_at));
                }

                // Reclaimability (#59): allocated bytes are already
                // known; whether they are uniquely this row's is bounded,
                // not asserted, when the row is flagged hardlinked, its
                // dedup is stale, or the volume itself can share extents
                // with a clone or snapshot outside this unit.
                // A Docker object's bytes came from the daemon
                // (`docker system df -v`): a layer-shared, logical
                // number that no directory walk measured. Giving it
                // filesystem provenance and an exact reclaimable equal
                // to its allocation was the PR #123 review's
                // docker_reclaimability counterexample.
                if is_docker_object {
                    a.evidence.extend(
                        crate::reclaimability::DockerByteAccounting::for_object(a.bytes).evidence(),
                    );
                } else {
                    let acc = if a.hardlinked || a.dedup_stale {
                        crate::reclaimability::hardlink_unresolved_bound(a.bytes)
                    } else if let Some(fs) = shared_extent_filesystem(&a.path) {
                        // On APFS (and Btrfs, ZFS, XFS, bcachefs,
                        // overlayfs) an extent can be retained by a clone,
                        // a snapshot, a reflink or a lower layer this pass
                        // never queried, so the allocated bytes are a
                        // ceiling on what removal frees, not the amount.
                        crate::reclaimability::shared_extent_bound(a.bytes, fs)
                    } else {
                        crate::reclaimability::exclusive_allocation(a.bytes)
                    };
                    a.evidence
                        .extend(crate::reclaimability::accounting_evidence(
                            &acc,
                            crate::evidence::EvidenceSource::FilesystemMetadata {
                                detail: "folded directory allocation".into(),
                            },
                        ));
                }

                // Recovery (#58): only for kinds this pass can source
                // without guessing. A worktree appearing in this report at
                // all means its own source is present this pass.
                let recovery = match a.kind {
                    ArtifactKind::BuildOutput => {
                        Some(crate::recovery::build_output_recovery(true, &wt_path))
                    }
                    ArtifactKind::DependencyTree => {
                        let lockfile = [
                            "Cargo.lock",
                            "package-lock.json",
                            "pnpm-lock.yaml",
                            "go.sum",
                        ]
                        .iter()
                        .map(|name| wt_path.join(name))
                        .find(|p| crate::fs_gate::exists(p));
                        Some(crate::recovery::dependency_tree_recovery(
                            lockfile.as_deref(),
                        ))
                    }
                    ArtifactKind::Cache => Some(crate::recovery::cache_without_signal_recovery(
                        "no lockfile/source signal collected for this cache in this pass",
                    )),
                    // Docker recovery (#58): every Docker row reaching
                    // this loop already lives in `wt.artifacts`, which
                    // only ever holds a *joined* Docker row (an unjoined
                    // one becomes an `UnownedRow` instead, a separate
                    // list this loop never sees) -- so the worktree this
                    // row is being iterated under is exactly the project
                    // it was joined to.
                    ArtifactKind::DockerImage => {
                        // `a.path` is the image's repo tag when one was
                        // recorded, or its bare image id when dangling
                        // (see `join_docker_facts`'s `candidate.reference`
                        // construction) -- `a.dangling` is the row's own
                        // recorded fact distinguishing the two, never
                        // guessed from the string's shape.
                        let repo_tag = if a.dangling { None } else { a.path.to_str() };
                        Some(crate::recovery::docker_image_recovery(repo_tag, true))
                    }
                    ArtifactKind::DockerBuildCache => {
                        Some(crate::recovery::docker_build_cache_recovery(true))
                    }
                    ArtifactKind::DockerVolume => Some(crate::recovery::docker_volume_recovery(
                        &a.path.display().to_string(),
                    )),
                    _ => None,
                };
                if let Some(r) = recovery {
                    // Carry the assessment's own smallest-useful
                    // follow-up check into the fact's `note` (#60: the
                    // TUI/CLI evidence-line renderer already prints
                    // `note`) -- otherwise `RecoveryAssessment`'s richer
                    // fields never survive past this bare `Evidence`.
                    let ev = match &r.follow_up_check {
                        Some(check) => r.evidence.with_note(format!("check: {check}")),
                        None => r.evidence,
                    };
                    a.evidence.push(ev);
                }
            }
        }
    }
    attach_nested_decision_evidence(report);
    attach_cargo_guidance(report);
}

/// Computes every `NestedArtifact`'s cargo-cleanup
/// `crate::cargo_cleanup::Guidance` exactly once, from
/// `report.observed_at` (never a fresh `crate::entities::now()` read) --
/// R18a. Called once per observe pass, alongside
/// [`attach_nested_decision_evidence`]: before this, the guidance shown
/// in `swamp report --json` (and compared in this crate's own tests)
/// was computed lazily at *serialization* time by a
/// `#[serde(serialize_with = ...)]` hook that called
/// `crate::entities::now()` directly, so serializing the same `Report`
/// twice a wall-clock second apart produced two different
/// `modified_age_secs` values -- a real "`swamp report --json` is not a
/// pure read" bug, not just a test flake.
fn attach_cargo_guidance(report: &mut Report) {
    let observed_at = report.observed_at;
    for unit in &mut report.nested_artifacts {
        unit.guidance = crate::cargo_cleanup::guidance_at(unit, observed_at);
    }
}

/// The nested build-artifact units' half of [`attach_decision_evidence`].
///
/// `CHANGELOG.md` claimed evidence was "attached to ... nested
/// build-artifact units". It was not:
/// `NestedArtifact::decision_evidence` was written only as `Vec::new()`,
/// this function's caller iterated `projects[].worktrees[].artifacts`
/// and never touched `report.nested_artifacts`,
/// `skip_serializing_if = "Vec::is_empty"` hid the empty vector from
/// `--view rust` JSON, and no test existed. The 2026-09-22 re-review
/// found it, and found it because `computed-but-not-delivered` was the
/// one guardrail in `.oh/guardrails/` whose frontmatter said
/// `audit: none`.
///
/// Every fact here comes from something the pass already recorded --
/// `mtime_max`, `physical_bytes`/`bytes`, the unit's own `role` and
/// `coverage`. No new traversal, no new `stat`: a nested unit's facts
/// are a projection of the Cargo inspection that produced it.
fn attach_nested_decision_evidence(report: &mut Report) {
    let observed_at = report.observed_at;
    nested_decision_evidence(&mut report.nested_artifacts, observed_at, false);
}

/// The evidence projection for nested units, wherever they were
/// identified: inside a project container (`in_shared_store: false`) or
/// inside a machine-wide store the external observation measured
/// (`true`, `crate::build_stores`).
///
/// A unit a manager reported (`reported_by`, a BuildKit record) gets the
/// manager's provenance throughout -- its times are the daemon's, its
/// bytes are the daemon's logical figure, and nothing here names
/// `FilesystemMetadata` for them (PR #123's test 10, for nested units).
pub(crate) fn nested_decision_evidence(
    units: &mut [crate::artifact::NestedArtifact],
    observed_at: u64,
    in_shared_store: bool,
) {
    use crate::artifact::{ArtifactRole, Membership, RoleFamily};
    for unit in units.iter_mut() {
        if let Some(manager) = unit.reported_by.clone() {
            // Activity: the record's creation time is the manager's own
            // record, never a filesystem age; its last use is a separate
            // manager fact, carried as the adapter recorded it.
            unit.decision_evidence
                .push(crate::activity::tool_reported_use_evidence(
                    manager.clone(),
                    "the daemon's record of when this entry was created",
                    (unit.mtime_max > 0).then_some(unit.mtime_max),
                    observed_at,
                ));
            let last_used = unit
                .producer_evidence
                .iter()
                .find(|e| e.source == crate::build_adapters::LAST_USED_EVIDENCE)
                .map(|e| e.detail.clone());
            unit.decision_evidence
                .push(crate::activity::docker_last_used_evidence(
                    last_used.as_deref(),
                    observed_at,
                ));
            unit.decision_evidence.extend(
                crate::reclaimability::DockerByteAccounting::for_object(unit.bytes).evidence(),
            );
            let recovery = crate::recovery::docker_build_cache_recovery(false);
            let ev = match &recovery.follow_up_check {
                Some(check) => recovery.evidence.with_note(format!("check: {check}")),
                None => recovery.evidence,
            };
            unit.decision_evidence.push(ev);
            continue;
        }

        // Activity (#54): the newest recorded modification among this
        // unit's measured children. Labelled modification, never "last
        // used".
        unit.decision_evidence
            .push(crate::activity::modification_evidence_during(
                unit.mtime_max,
                observed_at,
                crate::entities::now(),
            ));

        // Activity (#54), tool-reported: a `.fingerprint` entry's own
        // mtime is Cargo's record of when it last built that unit --
        // a *tool-reported build time*, not a filesystem age, and kept
        // as a separate fact beside the modification one.
        if unit.relative_path.contains(".fingerprint") {
            unit.decision_evidence
                .push(crate::activity::tool_reported_use_evidence(
                    "cargo",
                    "the mtime of this unit's own .fingerprint entry: when Cargo last recorded a \
                     build for it, which is not the same as when a human last used the output",
                    (unit.mtime_max > 0).then_some(unit.mtime_max),
                    observed_at,
                ));
        }

        // Current use (#55): a writer's lock file the adapter found
        // present this pass. Probed read-only; absence is never "idle".
        if let Some(lock) = unit.writer_lock.clone() {
            unit.decision_evidence
                .push(crate::occupancy::manager_lock_evidence(
                    unit.adapter.clone().unwrap_or_default(),
                    &lock,
                ));
        }

        // Reclaimability (#59). A container node's `physical_bytes` is
        // zero by construction and its `physical_total` is a display
        // aggregate, so the honest number for a group is its aggregate
        // with the reason it is not exact; a leaf charges its own
        // physical bytes. `Membership::Unknown` means the subgroup
        // charge was never estimated, which is a bound of `0..N`, not a
        // measured zero.
        let accounting = match unit.membership {
            Membership::Unknown => crate::reclaimability::hardlink_unresolved_bound(
                unit.physical_total.max(unit.bytes),
            ),
            _ if unit.physical_bytes == 0 && unit.physical_total > 0 => {
                crate::reclaimability::hardlink_unresolved_bound(unit.physical_total)
            }
            _ => crate::reclaimability::exclusive_allocation(unit.physical_bytes),
        };
        unit.decision_evidence
            .extend(crate::reclaimability::accounting_evidence(
                &accounting,
                crate::evidence::EvidenceSource::FilesystemMetadata {
                    detail: "nested build-artifact inspection".into(),
                },
            ));

        // Recovery (#58), by what the unit *is*. A unit inside a project
        // container is build output whose source is present in this
        // report by construction. An installation is a reinstall; an
        // archive or a device's state is not regenerated by anything; a
        // shared store entry is a re-download from wherever it came
        // from, which the entry itself does not record. Anything the
        // inspection could not classify says so.
        let recovery = match (unit.role.family(), &unit.role) {
            (_, ArtifactRole::Unknown) => crate::recovery::cache_without_signal_recovery(
                "the nested unit's role was not established by this pass",
            ),
            (RoleFamily::Installations, _) => crate::recovery::toolchain_installation_recovery(
                unit.adapter.as_deref().unwrap_or("its manager"),
                unit.variant
                    .version
                    .as_deref()
                    .or(unit.variant.toolchain.as_deref()),
            ),
            (RoleFamily::State, _) => crate::recovery::mutable_environment_recovery(
                unit.role.label(),
                &unit.path.display().to_string(),
            ),
            (RoleFamily::SharedStore, _) | (RoleFamily::Dependencies, _) if in_shared_store => {
                crate::recovery::cache_without_signal_recovery(
                    "an entry in a shared store: whether it can be fetched again depends on the \
                     registry or source it came from, which the entry does not record",
                )
            }
            _ if in_shared_store => crate::recovery::cache_without_signal_recovery(
                "a machine-wide cache entry: which project's build would regenerate it is not \
                 recorded in the store",
            ),
            _ => crate::recovery::build_output_recovery(true, &unit.path),
        };
        let ev = match &recovery.follow_up_check {
            Some(check) => recovery.evidence.with_note(format!("check: {check}")),
            None => recovery.evidence,
        };
        unit.decision_evidence.push(ev);
    }
}

/// Same as [`report`], optionally running `du -skPx` on the root as an
/// independent oracle for `reconciliation.du_total`, and optionally
/// observing into the reverse-delta growth store (`growth.rs`).
///
/// `store_dir` is the top-level `${SWAMP_DIR}`-style directory; when
/// `None`, nothing is persisted and every `growth_bytes`/`regrowth_count`
/// stays at the R3 default (`None`/`0`) -- a read-only report, which is
/// what the golden test and `--no-observe` want. `since_override`
/// overrides the store's configured `since` setting for this call only
/// (`--since`). Kept at its existing 5-argument shape so callers outside
/// this slice do not need to change; use [`report_with_dirs`] for the
/// R4c `--dirs` opt-in.
pub fn report_with(
    root: &Path,
    docker_facts: Option<&Path>,
    verify_du: bool,
    store_dir: Option<&Path>,
    since_override: Option<&str>,
) -> Result<Report> {
    report_full(
        root,
        docker_facts,
        verify_du,
        store_dir,
        since_override,
        true,
        false,
        false,
    )
}

/// Same as [`report_with`], with explicit control over whether this call
/// *persists* a new observation into the growth store (`observe = true`,
/// the CLI's default) or only *reads* it (`observe = false`, `--no-observe`).
///
/// A read-only call still computes `growth_bytes`/`regrowth_count` from
/// whatever history the store already has for each row: growth is a
/// property of the store, not of whether this particular call wrote to
/// it. Only a store with no prior observation of a row leaves that row's
/// growth at `None`, exactly as it would immediately after `observe:
/// true`'s own first-ever observation.
pub fn report_with_observe(
    root: &Path,
    docker_facts: Option<&Path>,
    verify_du: bool,
    store_dir: Option<&Path>,
    since_override: Option<&str>,
    observe: bool,
) -> Result<Report> {
    report_full(
        root,
        docker_facts,
        verify_du,
        store_dir,
        since_override,
        observe,
        false,
        false,
    )
}

/// Same as [`report_with`], with `include_dirs` (R4c) additionally
/// populating `dirs_by_worktree` / `files_by_worktree` on the returned
/// report when `true`. When `false` they stay `None` so the default
/// report payload stays small. Always observes (persists), like
/// `report_with`; combine with [`report_with_observe`]'s `observe: bool`
/// through [`report_full`] directly if a caller ever needs both knobs at
/// once (none currently does).
pub fn report_with_dirs(
    root: &Path,
    docker_facts: Option<&Path>,
    verify_du: bool,
    store_dir: Option<&Path>,
    since_override: Option<&str>,
    include_dirs: bool,
) -> Result<Report> {
    report_full(
        root,
        docker_facts,
        verify_du,
        store_dir,
        since_override,
        true,
        include_dirs,
        false,
    )
}

/// The actual implementation behind [`report_with`], [`report_with_observe`],
/// [`report_with_dirs`], and [`report_with_enrich`]: `observe` controls
/// whether this call persists a new observation into the growth store or
/// only reads it, `include_dirs` (R4c) controls whether
/// `dirs_by_worktree`/`files_by_worktree` are populated on the returned
/// report, and `enrich` (#35) controls whether GitHub facts are
/// refreshed live or read as-is from `enrich.parquet`. `pub` (rather
/// than the other four's convenience wrapper shape) because the CLI's
/// `--dirs`/`--no-observe`/`--enrich` are independent flags and a caller
/// may need more than one at once.
///
/// Always takes the full-walk path (never tries FSEvents); see
/// [`report_full_mode`] for the `--full`-aware entry point R4b (#29) adds.
#[allow(clippy::too_many_arguments)]
pub fn report_full(
    root: &Path,
    docker_facts: Option<&Path>,
    verify_du: bool,
    store_dir: Option<&Path>,
    since_override: Option<&str>,
    observe: bool,
    include_dirs: bool,
    enrich: bool,
) -> Result<Report> {
    report_full_mode(
        root,
        docker_facts,
        verify_du,
        store_dir,
        since_override,
        observe,
        include_dirs,
        enrich,
        true,
    )
}

/// Same as [`report_full`], with one more knob: `force_full` (`--full`)
/// skips the FSEvents-driven incremental attempt entirely, same as a
/// refusal would, and still re-anchors the stored event id for the next
/// call. When `store_dir` is `None` this is identical to `report_full`
/// (no store, no FSEvents replay, no incremental path is possible).
#[allow(clippy::too_many_arguments)]
pub fn report_full_mode(
    root: &Path,
    docker_facts: Option<&Path>,
    verify_du: bool,
    store_dir: Option<&Path>,
    since_override: Option<&str>,
    observe: bool,
    include_dirs: bool,
    enrich: bool,
    force_full: bool,
) -> Result<Report> {
    report_full_mode_with_source(
        root,
        docker_facts,
        verify_du,
        store_dir,
        since_override,
        observe,
        include_dirs,
        enrich,
        force_full,
        crate::fs_events::platform_source().as_ref(),
    )
}

/// Same as [`report_full_mode`], with the [`crate::fs_events::FsEventsSource`]
/// supplied explicitly. Exposed so integration tests can exercise the
/// FSEvents-driven incremental path end to end (full `Report`, docker
/// join, signals, and all) with canned event batches instead of the live
/// `fseventsd`; production callers use [`report_full_mode`], which
/// resolves the real platform source.
#[allow(clippy::too_many_arguments)]
pub fn report_full_mode_with_source(
    root: &Path,
    docker_facts: Option<&Path>,
    verify_du: bool,
    store_dir: Option<&Path>,
    since_override: Option<&str>,
    observe: bool,
    include_dirs: bool,
    enrich: bool,
    force_full: bool,
    fs_events_source: &dyn crate::fs_events::FsEventsSource,
) -> Result<Report> {
    report_full_mode_with_exclusions(
        root,
        docker_facts,
        verify_du,
        store_dir,
        since_override,
        observe,
        include_dirs,
        enrich,
        force_full,
        fs_events_source,
        &[],
    )
}

/// Same as [`report_full_mode_with_source`], with a set of subtrees to
/// prune from this walk (#42 -- `scope::EffectiveScope::pruned_subtrees`,
/// filtered to `root`). [`report_scope`] is the one caller that has scope
/// exclusions to enforce; every other caller goes through
/// [`report_full_mode_with_source`] with none.
#[allow(clippy::too_many_arguments)]
pub fn report_full_mode_with_exclusions(
    root: &Path,
    docker_facts: Option<&Path>,
    verify_du: bool,
    store_dir: Option<&Path>,
    since_override: Option<&str>,
    observe: bool,
    include_dirs: bool,
    enrich: bool,
    force_full: bool,
    fs_events_source: &dyn crate::fs_events::FsEventsSource,
    pruned_subtrees: &[PathBuf],
) -> Result<Report> {
    report_full_mode_scoped(
        root,
        docker_facts,
        verify_du,
        store_dir,
        since_override,
        observe,
        include_dirs,
        enrich,
        force_full,
        fs_events_source,
        pruned_subtrees,
        true,
    )
}

/// [`report_full_mode_with_exclusions`] with the Docker probe gated on
/// the authorized scope. `report_scope_with_parts` is the one caller
/// that knows the scope; every other entry point keeps `true`, since a
/// scope-less single-root call has nothing to consult and its behavior
/// must not change (the 2026-09-22 re-review's CE6).
///
/// `pub`, not `pub(crate)`: an integration test exercising the build
/// adapters over a disposable fixture must be able to say
/// `docker_in_scope: false` explicitly, rather than inherit whatever
/// Docker daemon happens to be running on the machine the test executes
/// on (stack/26's build_adapter_history root-cause -- see that test
/// file's `observe` helper). Every existing caller keeps calling
/// `report_full_mode_with_source`/`_with_exclusions`, which still hand
/// this `true` unconditionally.
#[allow(clippy::too_many_arguments)]
pub fn report_full_mode_scoped(
    root: &Path,
    docker_facts: Option<&Path>,
    verify_du: bool,
    store_dir: Option<&Path>,
    since_override: Option<&str>,
    observe: bool,
    include_dirs: bool,
    enrich: bool,
    force_full: bool,
    fs_events_source: &dyn crate::fs_events::FsEventsSource,
    pruned_subtrees: &[PathBuf],
    docker_in_scope: bool,
) -> Result<Report> {
    report_full_mode_scoped_tracked(
        root,
        docker_facts,
        verify_du,
        store_dir,
        since_override,
        observe,
        include_dirs,
        enrich,
        force_full,
        fs_events_source,
        pruned_subtrees,
        docker_in_scope,
        None,
    )
    .map(|(r, _)| r)
}

/// [`report_full_mode_scoped`] plus this root's trusted event window --
/// the replay's unfiltered change list and the observation time it
/// replays from, or `None` when this walk earned no window.
///
/// Only `report_scope_with_parts` wants it: it is what turns the unit
/// families' stored measurements from "probably still right" into
/// "shown unchanged by this pass's own replay"
/// (`crate::fs_events::EventCoverage`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn report_full_mode_scoped_tracked(
    root: &Path,
    docker_facts: Option<&Path>,
    verify_du: bool,
    store_dir: Option<&Path>,
    since_override: Option<&str>,
    observe: bool,
    include_dirs: bool,
    enrich: bool,
    force_full: bool,
    fs_events_source: &dyn crate::fs_events::FsEventsSource,
    pruned_subtrees: &[PathBuf],
    docker_in_scope: bool,
    observed_at: Option<u64>,
) -> Result<(Report, Option<crate::fs_events::TrustedWindow>)> {
    // Store topology, replay paths, and report paths under one canonical
    // representation. This is essential when one invocation uses a symlink
    // alias and the next uses its canonical spelling: FSEvents is canonical,
    // while a caller-form topology would otherwise make incremental replay
    // compare different path namespaces.
    let root = crate::fs_gate::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    // Exclusion patterns were resolved against the pre-canonicalization
    // root; canonicalize them the same way so a symlinked root's pruned
    // subtrees still match what the walker actually sees.
    let pruned_subtrees: Vec<PathBuf> = pruned_subtrees
        .iter()
        .map(|p| crate::fs_gate::canonicalize(p).unwrap_or_else(|_| p.clone()))
        .collect();
    // The pipeline is consumers on the event bus (ADR 001); this function
    // only translates its arguments into the run context.
    let mut ctx = crate::bus::ctx_for_excluding(
        &root,
        docker_facts,
        verify_du,
        store_dir,
        since_override,
        observe,
        include_dirs,
        enrich,
        force_full,
        fs_events_source,
        &pruned_subtrees,
    );
    ctx.docker_in_scope = docker_in_scope;
    // R20: one scope observation, one timestamp. Every root's rows,
    // growth window and sparkline history are computed at the scope's
    // `observed_at`, which `runs.parquet` records -- so a read derives
    // exactly what the pass showed.
    if let Some(observed_at) = observed_at {
        ctx.observed_at = observed_at;
    }
    let mut report = crate::bus::run_report(&ctx)?;
    report.store_dir = store_dir.map(Path::to_path_buf);
    let window = ctx.event_window.lock().unwrap().clone();
    Ok((report, window))
}

/// Fills `ecosystems` on a project that has none yet and `ecosystem` on
/// every artifact row that has none yet (`ecosystem::artifact_ecosystem_at`).
/// Idempotent, so it runs twice per pipeline: once from the growth
/// consumer, *before* the artifact history is written (R15 item 3: the
/// current-artifact table stores `ecosystem`, and the history is the
/// stage before tracking), and again from [`annotate_tracking`], where
/// it used to live, for any row a later stage added.
pub fn annotate_artifact_ecosystems(projects: &mut [ProjectRow]) {
    let mut listed: std::collections::HashMap<PathBuf, Vec<String>> =
        std::collections::HashMap::new();
    annotate_artifact_ecosystems_listing(projects, &mut listed);
}

/// [`annotate_artifact_ecosystems`] with the per-parent listings it
/// took (or reused) exposed: one `read_dir` per distinct parent, never
/// one per artifact, and none at all for a nested artifact's history
/// shadow row (`source.tool == "cargo.layout"`: pushed by the growth
/// consumer for the history store and removed again before assembly --
/// its ecosystem is never rendered, and its parent is the interior of a
/// build directory with thousands of entries).
pub fn annotate_artifact_ecosystems_listing(
    projects: &mut [ProjectRow],
    listed: &mut std::collections::HashMap<PathBuf, Vec<String>>,
) {
    for p in projects.iter_mut() {
        if p.ecosystems.is_empty() {
            let root = p
                .worktrees
                .iter()
                .find(|w| w.kind == WorktreeKind::Main)
                .or(p.worktrees.first())
                .map(|w| w.path.clone());
            if let Some(root) = root {
                p.ecosystems = crate::ecosystem::detect(&root);
            }
        }
        for wt in p.worktrees.iter_mut() {
            for a in wt.artifacts.iter_mut() {
                if a.ecosystem.is_none()
                    && !a.kind.is_worktree_remainder()
                    && a.kind != ArtifactKind::Git
                    && a.source.tool != "cargo.layout"
                    && let Some(name) = a.path.file_name().and_then(|n| n.to_str())
                {
                    a.ecosystem = match a.path.parent() {
                        Some(parent) => {
                            let names = listed
                                .entry(parent.to_path_buf())
                                .or_insert_with(|| crate::ecosystem::dir_names(parent));
                            crate::ecosystem::artifact_ecosystem_among(
                                parent,
                                names,
                                &p.ecosystems,
                                name,
                            )
                        }
                        None => crate::ecosystem::artifact_ecosystem(&p.ecosystems, name),
                    }
                    .map(String::from);
                }
            }
        }
    }
}

/// Fills `track` on every artifact row and top-level Source directory:
/// one exclude stack per worktree, one lookup per row. `.git` rows carry
/// no status (git's own store is not content it tracks). Cheap relative
/// to the walk and needed by every surface, so it runs on every report.
pub fn annotate_tracking(
    projects: &mut [ProjectRow],
    dirs: Option<&mut std::collections::HashMap<String, Vec<DirRollup>>>,
) {
    let mut dirs = dirs;
    annotate_artifact_ecosystems(projects);
    for p in projects.iter_mut() {
        for wt in p.worktrees.iter_mut() {
            let Some(lens) = crate::ignore::IgnoreLens::open(&wt.path) else {
                continue;
            };
            for a in wt.artifacts.iter_mut() {
                if a.kind == ArtifactKind::Git || a.source.tool.starts_with("docker") {
                    continue;
                }
                // A split remainder row already carries the state it was
                // split by; asking the lens about its path would ask
                // about the worktree root and call all three "tracked".
                if a.track.is_some() {
                    continue;
                }
                let rel = a
                    .path
                    .strip_prefix(&wt.path)
                    .map(|r| r.display().to_string())
                    .unwrap_or_default();
                a.track = Some(lens.status(&rel, true));
            }
            if let Some(map) = dirs.as_deref_mut()
                && let Some(rows) = map.get_mut(&wt.worktree_id)
            {
                for d in rows.iter_mut().filter(|d| !d.rel_path.contains('/')) {
                    d.track = Some(lens.status(&d.rel_path, true));
                }
            }
        }
    }
}

/// (worktree_id, rel_path) of every folded artifact row, for
/// `aggregate_dir_totals` and for keeping interior rows out of the report.
pub(crate) fn artifact_roots(
    projects: &[ProjectRow],
) -> std::collections::HashSet<(String, String)> {
    let mut out = std::collections::HashSet::new();
    for p in projects {
        for wt in &p.worktrees {
            for a in &wt.artifacts {
                if a.kind.is_worktree_remainder() || a.source.tool.starts_with("docker") {
                    continue;
                }
                if let Ok(rel) = a.path.strip_prefix(&wt.path) {
                    out.insert((wt.worktree_id.clone(), rel.display().to_string()));
                }
            }
        }
    }
    out
}

/// Whether a directory row lies at or under one of `roots` in its worktree.
pub(crate) fn dir_inside_artifact(
    d: &DirRollup,
    roots: &std::collections::HashSet<(String, String)>,
) -> bool {
    let mut rel = d.rel_path.as_str();
    loop {
        if roots.contains(&(d.worktree_id.clone(), rel.to_string())) {
            return true;
        }
        match rel.rfind('/') {
            Some(i) => rel = &rel[..i],
            None => return false,
        }
    }
}

/// Rolls `own_allocated` up into `allocated_total` bottom-up: deepest
/// directories (most path separators) are folded into their parent's
/// running total first, so a parent's `allocated_total` always includes
/// every descendant Source directory's total by the time it is visited.
/// Deliberately excludes bytes folded into a classified artifact row
/// directly beneath a directory -- those stay one `ArtifactRow`, never
/// decomposed into `DirRollup`s, per the folding contract this issue
/// requires.
/// Rolls each directory's own bytes up into its ancestors' totals.
/// `artifact_roots` (worktree_id, rel_path) are folded units: their
/// interior rows roll up into the unit's root row, and the root row rolls
/// no further, so a Source directory's total never absorbs an artifact.
pub(crate) fn aggregate_dir_totals(
    dirs: &mut [DirRollup],
    artifact_roots: &std::collections::HashSet<(String, String)>,
) {
    let mut totals: std::collections::HashMap<(String, String), u64> =
        std::collections::HashMap::new();
    // Completeness rolls up with the bytes: a directory is complete only
    // if every measured descendant was. An unreadable directory deep in a
    // tree is otherwise invisible from every ancestor's row.
    let mut complete: std::collections::HashMap<(String, String), bool> =
        std::collections::HashMap::new();
    for d in dirs.iter() {
        totals.insert((d.worktree_id.clone(), d.rel_path.clone()), d.own_allocated);
        complete.insert((d.worktree_id.clone(), d.rel_path.clone()), d.complete);
    }
    let mut order: Vec<usize> = (0..dirs.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(dirs[i].rel_path.matches('/').count()));
    for &i in &order {
        let key = (dirs[i].worktree_id.clone(), dirs[i].rel_path.clone());
        if artifact_roots.contains(&key) {
            continue;
        }
        let value = *totals.get(&key).unwrap_or(&0);
        let whole = *complete.get(&key).unwrap_or(&true);
        if let Some(parent_rel) = dirs[i].parent_rel_path.clone() {
            let pkey = (dirs[i].worktree_id.clone(), parent_rel);
            *totals.entry(pkey.clone()).or_insert(0) += value;
            if !whole && let Some(c) = complete.get_mut(&pkey) {
                *c = false;
            }
        }
    }
    for d in dirs.iter_mut() {
        let key = (d.worktree_id.clone(), d.rel_path.clone());
        d.allocated_total = *totals.get(&key).unwrap_or(&d.own_allocated);
        d.complete = *complete.get(&key).unwrap_or(&d.complete);
    }
}

/// `${SWAMP_DIR}`, or the platform's per-user data directory.
///
/// Reached through [`crate::platform::data_dir`] rather than repeated
/// here: two spellings of the store path is how an enrichment cache
/// ends up somewhere the store is not, and the Linux answer honours
/// `$XDG_DATA_HOME` while the macOS one does not.
///
/// A fallback only for GitHub enrichment's cache when no `store_dir`
/// was supplied (see the call site in `report_with`). `None` when there
/// is no home to derive one from: enrichment then runs uncached, which
/// is slower and correct, rather than caching into the current
/// directory.
pub(crate) fn default_github_cache_dir() -> Option<PathBuf> {
    crate::platform::data_dir().ok()
}

/// Renders a `PrStatus` for the `pull_request` signal row and the
/// `--view worktrees` printer, e.g. `PR #123 open (approved)`,
/// `PR #98 merged`, `no PR`, `unknown`.
pub(crate) fn render_pr_status(status: &crate::github::PrStatus) -> String {
    use crate::github::{PrState, PrStatus, ReviewDecision};
    match status {
        PrStatus::None => "no PR".to_string(),
        PrStatus::Unknown => "unknown".to_string(),
        PrStatus::Some(pr) => {
            let state = match pr.state {
                PrState::Open => "open",
                PrState::Closed => "closed",
                PrState::Merged => "merged",
            };
            let decision = match pr.review_decision {
                ReviewDecision::Approved => Some("approved"),
                ReviewDecision::ChangesRequested => Some("changes requested"),
                ReviewDecision::ReviewRequired => Some("review required"),
                ReviewDecision::None | ReviewDecision::Unknown => None,
            };
            let draft = if pr.draft { " draft" } else { "" };
            match decision {
                Some(d) => format!("PR #{}{draft} {state} ({d})", pr.number),
                None => format!("PR #{}{draft} {state}", pr.number),
            }
        }
    }
}

/// Normalizes a git remote URL for comparison: strips a trailing `.git`,
/// collapses the `git@host:path` scp-like ssh form and any `scheme://`
/// form down to `host/path`, and lowercases the result.
pub(crate) fn normalize_remote(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let stripped = url.strip_suffix(".git").unwrap_or(url);
    if let Some(rest) = stripped.strip_prefix("git@") {
        return Some(rest.replacen(':', "/", 1).to_lowercase());
    }
    if let Some(idx) = stripped.find("://") {
        let rest = &stripped[idx + 3..];
        let rest = rest.rsplit('@').next().unwrap_or(rest);
        return Some(rest.to_lowercase());
    }
    Some(stripped.to_lowercase())
}

/// Result of joining Docker facts to discovered projects/worktrees.
pub(crate) struct DockerJoinResult {
    pub(crate) rows_by_worktree: std::collections::HashMap<String, Vec<ArtifactRow>>,
    pub(crate) unowned: Vec<UnownedRow>,
    pub(crate) attributed_bytes: u64,
    pub(crate) unowned_bytes: u64,
}

/// One candidate Docker object (image, build-cache entry, or volume)
/// being joined, in a shape common to all three.
struct JoinCandidate {
    reference: String,
    labels: std::collections::HashMap<String, String>,
    unique_bytes: u64,
    shared_bytes: u64,
    kind: ArtifactKind,
    created_at: Option<String>,
    containers: Vec<String>,
    shared_with: Vec<String>,
    dangling: bool,
    /// Daemon-sourced decision evidence built where the raw
    /// `docker::DockerFacts` are still in hand (#54's tool-reported use,
    /// #55's running-container occupancy). Collected here because the
    /// `ArtifactRow`/`UnownedRow` this candidate becomes keeps only
    /// pre-formatted `Vec<String>` container labels -- by then the
    /// `ContainerRef` states and the build cache's own `last_used`
    /// string are gone. Every fact here names the Docker daemon as its
    /// source and rides only on Docker rows, never on a filesystem row.
    docker_evidence: Vec<crate::evidence::Evidence>,
}

enum JoinOutcome {
    Joined {
        worktree_id: String,
        confidence: Confidence,
        /// The join rule that matched, used as the artifact row's
        /// `Source.tool` (`docker.<rule>`) so different evidence stays
        /// distinguishable after the fact.
        rule: &'static str,
        /// Set when the rule needed a tie-break, e.g.
        /// `compose_ambiguous=<n>` when several worktrees of one project
        /// each contain a compose file naming the same project.
        note: Option<String>,
    },
    Unowned {
        note: Option<String>,
    },
}

/// Applies the R5 join rules plus the #28 compose-file-name rule,
/// explicit evidence only:
/// 1. `com.docker.compose.project` label == a discovered project name:
///    join that project's main worktree, High confidence.
/// 2. Otherwise, `com.docker.compose.project` label == a candidate name
///    (`name:` value or containing directory basename) read from a
///    compose file inside a discovered worktree: join that worktree,
///    High confidence, rule `compose_file_name`. A compose project name
///    frequently differs from the repo name (e.g. `hiphi-staging` for a
///    `hiphi-relay`/`hiphi-authorizer` checkout), which is exactly the
///    case rule 1 misses. If several worktrees of one project each
///    contain a matching compose file, a `com.docker.compose.project.working_dir`
///    label pointing at one of them breaks the tie; otherwise the
///    project's Main worktree is used and a `compose_ambiguous=<n>` note
///    is attached. If the label matches neither a discovered project
///    name nor any compose-file candidate, it is recorded as a
///    `compose_project=<name>` note instead of being silently dropped --
///    a later slice can group by it even when this slice can't
///    attribute it.
/// 3. Any label whose value is an absolute path inside a discovered
///    worktree (this covers `com.docker.compose.project.working_dir` as
///    well as any other path-shaped label): join that worktree, High
///    confidence.
/// 4. `org.opencontainers.image.source`, normalized, matches a project's
///    normalized git remote: join that project's main worktree, Medium
///    confidence. A non-matching source is recorded as a
///    `base_image_source=<url>` note, never used to attribute -- this is
///    the base-image trap: an image can be named exactly like a project
///    (or even be that project's own published image) while its
///    `image.source` legitimately points at an upstream base image
///    (e.g. `linuxcontainers/alpine`) it was built FROM, not the project
///    that built it.
/// 5. Otherwise: unowned, reason `DockerNoJoin`. Name similarity to a
///    project is never evidence for any of these rules.
fn join_one(
    candidate: &JoinCandidate,
    projects: &[ProjectRow],
    worktree_paths: &[(PathBuf, String)],
    project_remotes: &std::collections::HashMap<String, String>,
    compose_index: &std::collections::HashMap<String, Vec<String>>,
) -> JoinOutcome {
    fn main_worktree_id(project: &ProjectRow) -> Option<String> {
        project
            .worktrees
            .iter()
            .find(|w| w.kind == WorktreeKind::Main)
            .or_else(|| project.worktrees.first())
            .map(|w| w.worktree_id.clone())
    }

    fn project_for_worktree<'a>(
        projects: &'a [ProjectRow],
        worktree_id: &str,
    ) -> Option<&'a ProjectRow> {
        projects
            .iter()
            .find(|p| p.worktrees.iter().any(|w| w.worktree_id == worktree_id))
    }

    let mut notes: Vec<String> = Vec::new();

    if let Some(project_label) = candidate.labels.get("com.docker.compose.project") {
        // Rule 1: label matches a discovered project's own name.
        if let Some(worktree_id) = projects
            .iter()
            .find(|p| &p.name == project_label)
            .and_then(main_worktree_id)
        {
            return JoinOutcome::Joined {
                worktree_id,
                confidence: Confidence::High,
                rule: "compose_project_label",
                note: None,
            };
        }

        // Rule 2 (#28): label matches a compose file's `name:` value or
        // directory basename found inside a discovered worktree.
        if let Some(worktree_ids) = compose_index.get(project_label)
            && !worktree_ids.is_empty()
        {
            // The working_dir label is where compose was invoked from
            // (e.g. a `deploy/` subdirectory), not necessarily the
            // worktree root itself, so this matches by containment
            // (deepest worktree root that contains it) rather than exact
            // equality -- the same rule rule 3 below uses for path-shaped
            // labels in general.
            let working_dir = candidate
                .labels
                .get("com.docker.compose.project.working_dir")
                .map(Path::new);
            let tie_broken = working_dir.and_then(|wd| {
                worktree_paths
                    .iter()
                    .filter(|(p, pid)| wd.starts_with(p) && worktree_ids.contains(pid))
                    .max_by_key(|(p, _)| p.as_os_str().len())
                    .map(|(_, pid)| pid)
            });

            if let Some(worktree_id) = tie_broken {
                return JoinOutcome::Joined {
                    worktree_id: worktree_id.clone(),
                    confidence: Confidence::High,
                    rule: "compose_file_name",
                    note: None,
                };
            }

            let mut unique_ids: Vec<&String> = worktree_ids.iter().collect();
            unique_ids.sort();
            unique_ids.dedup();

            if let [only] = unique_ids.as_slice() {
                return JoinOutcome::Joined {
                    worktree_id: (*only).clone(),
                    confidence: Confidence::High,
                    rule: "compose_file_name",
                    note: None,
                };
            }

            if let Some(main_worktree_id) = unique_ids
                .first()
                .and_then(|id| project_for_worktree(projects, id))
                .and_then(main_worktree_id)
            {
                return JoinOutcome::Joined {
                    worktree_id: main_worktree_id,
                    confidence: Confidence::High,
                    rule: "compose_file_name",
                    note: Some(format!("compose_ambiguous={}", unique_ids.len())),
                };
            }
        }

        notes.push(format!("compose_project={project_label}"));
    }

    for value in candidate.labels.values() {
        if !value.starts_with('/') {
            continue;
        }
        let value_path = Path::new(value);
        if let Some((_, worktree_id)) = worktree_paths
            .iter()
            .filter(|(p, _)| value_path.starts_with(p))
            .max_by_key(|(p, _)| p.as_os_str().len())
        {
            return JoinOutcome::Joined {
                worktree_id: worktree_id.clone(),
                confidence: Confidence::High,
                rule: "working_dir_path",
                note: None,
            };
        }
    }

    if let Some(source) = candidate.labels.get("org.opencontainers.image.source") {
        let joined = normalize_remote(source).and_then(|normalized| {
            project_remotes
                .iter()
                .find(|(_, r)| **r == normalized)
                .and_then(|(project_id, _)| projects.iter().find(|p| &p.project_id == project_id))
                .and_then(main_worktree_id)
        });
        match joined {
            Some(worktree_id) => {
                return JoinOutcome::Joined {
                    worktree_id,
                    confidence: Confidence::Medium,
                    rule: "image_source",
                    note: None,
                };
            }
            None => notes.push(format!("base_image_source={source}")),
        }
    }

    let note = if notes.is_empty() {
        None
    } else {
        Some(notes.join(", "))
    };
    JoinOutcome::Unowned { note }
}

pub(crate) fn join_docker_facts(
    facts: &crate::docker::DockerFacts,
    projects: &[ProjectRow],
    worktree_paths: &[(PathBuf, String)],
    project_remotes: &std::collections::HashMap<String, String>,
    compose_index: &std::collections::HashMap<String, Vec<String>>,
) -> DockerJoinResult {
    let mut result = DockerJoinResult {
        rows_by_worktree: std::collections::HashMap::new(),
        unowned: Vec::new(),
        attributed_bytes: 0,
        unowned_bytes: 0,
    };

    let mut cache_notes: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    fn format_container(c: &crate::docker::ContainerRef) -> String {
        match &c.finished_at {
            Some(finished) => format!("{} ({}, finished {finished})", c.name, c.state),
            None => format!("{} ({})", c.name, c.state),
        }
    }

    let observed_at = crate::entities::now();

    let mut candidates: Vec<JoinCandidate> = Vec::new();
    for image in &facts.images {
        candidates.push(JoinCandidate {
            reference: image
                .repo_tags
                .first()
                .cloned()
                .unwrap_or_else(|| image.id.clone()),
            labels: image.labels.clone(),
            unique_bytes: image.unique_bytes,
            shared_bytes: image.shared_bytes,
            kind: ArtifactKind::DockerImage,
            created_at: image.created_at.clone(),
            containers: image.containers.iter().map(format_container).collect(),
            shared_with: image.shared_with.clone(),
            dangling: image.dangling,
            // Current use (#55): a *running* container is a live
            // consumer of this image. No running container is not proof
            // of no consumer, and no container reference at all is
            // `Unknown` rather than a known "nothing uses this" --
            // `docker_running_container_evidence` draws both
            // distinctions.
            docker_evidence: vec![crate::occupancy::docker_running_container_evidence(
                &image.containers,
            )],
        });
    }
    for cache in &facts.build_cache {
        let mut cache_note_bits = Vec::new();
        if let Some(last_used) = &cache.last_used {
            cache_note_bits.push(format!("last_used={last_used}"));
        }
        if let Some(count) = cache.usage_count {
            cache_note_bits.push(format!("usage_count={count}"));
        }
        cache_note_bits.push(format!("in_use={}", cache.in_use));
        cache_note_bits.push(format!("shared={}", cache.shared));
        candidates.push(JoinCandidate {
            reference: cache.id.clone(),
            labels: std::collections::HashMap::new(),
            unique_bytes: cache.bytes,
            shared_bytes: 0,
            kind: ArtifactKind::DockerBuildCache,
            created_at: None,
            containers: Vec::new(),
            shared_with: Vec::new(),
            dangling: false,
            // Activity (#54): the daemon's own `last_used` for this
            // cache entry, kept as a `ToolReportedUse` fact with Docker
            // named as the source -- never flattened into the
            // filesystem-mtime `Modified` fact
            // `attach_decision_evidence` attaches separately. A build
            // cache entry with no `last_used` is `Unknown`, never a
            // fabricated timestamp.
            docker_evidence: vec![crate::activity::docker_last_used_evidence(
                cache.last_used.as_deref(),
                observed_at,
            )],
        });
        // Build-cache detail has no per-object join label to carry it on,
        // so it rides along as a note on the candidate's eventual row
        // instead of a dedicated field -- see the push sites below, which
        // attach `cache_note_bits` via a side table keyed by reference.
        cache_notes.insert(cache.id.clone(), cache_note_bits.join(", "));
    }
    for volume in &facts.volumes {
        candidates.push(JoinCandidate {
            reference: volume.name.clone(),
            labels: volume.labels.clone(),
            unique_bytes: volume.bytes,
            shared_bytes: 0,
            kind: ArtifactKind::DockerVolume,
            created_at: volume.created_at.clone(),
            containers: volume.containers.iter().map(format_container).collect(),
            shared_with: Vec::new(),
            dangling: false,
            // Current use (#55), same question as an image: a volume a
            // running container has mounted is being written to right
            // now.
            docker_evidence: vec![crate::occupancy::docker_running_container_evidence(
                &volume.containers,
            )],
        });
    }

    for candidate in candidates {
        match join_one(
            &candidate,
            projects,
            worktree_paths,
            project_remotes,
            compose_index,
        ) {
            JoinOutcome::Joined {
                worktree_id,
                confidence,
                rule,
                note,
            } => {
                result.attributed_bytes += candidate.unique_bytes;
                let source = if rule == "compose_file_name" {
                    Source::new(format!("docker.{rule}"))
                } else {
                    Source::new("docker.system_df")
                };
                let note = match (note, cache_notes.get(&candidate.reference)) {
                    (Some(n), Some(cache_note)) => Some(format!("{n}, {cache_note}")),
                    (Some(n), None) => Some(n),
                    (None, Some(cache_note)) => Some(cache_note.clone()),
                    (None, None) => None,
                };
                // Consumer evidence (#57): normalizes this already-decided
                // Docker join into the shared contract rather than
                // re-deriving it -- see
                // `external_associations::docker_join_evidence`.
                let mut evidence = vec![crate::external_associations::docker_join_evidence(
                    Some(&worktree_id),
                    rule,
                )];
                evidence.extend(candidate.docker_evidence);
                result
                    .rows_by_worktree
                    .entry(worktree_id)
                    .or_default()
                    .push(ArtifactRow {
                        kind: candidate.kind,
                        path: PathBuf::from(candidate.reference),
                        bytes: candidate.unique_bytes,
                        mtime_max: 0,
                        ecosystem: None,
                        hardlinked: false,
                        dedup_stale: false,
                        allocated_bytes: None,
                        allocated_growth_bytes: None,
                        local_bytes: 0,
                        track: None,
                        growth_bytes: None,
                        regrowth_count: 0,
                        observed_at,
                        confidence,
                        source,
                        note,
                        created_at: candidate.created_at,
                        containers: candidate.containers,
                        shared_with: candidate.shared_with,
                        dangling: candidate.dangling,
                        evidence,
                    });
            }
            JoinOutcome::Unowned { note } => {
                let docker_kind = match candidate.kind {
                    ArtifactKind::DockerImage => "image",
                    ArtifactKind::DockerBuildCache => "build-cache",
                    ArtifactKind::DockerVolume => "volume",
                    _ => "unknown",
                };
                let note = match (note, cache_notes.get(&candidate.reference)) {
                    (Some(n), Some(cache_note)) => Some(format!("{n}, {cache_note}")),
                    (Some(n), None) => Some(n),
                    (None, Some(cache_note)) => Some(cache_note.clone()),
                    (None, None) => None,
                };
                result.unowned_bytes += candidate.unique_bytes;
                // Recovery (#58): "no project claims it" is a consumer
                // fact (`UnownedReason::DockerNoJoin`), not a reason to
                // skip this object's own recovery assessment -- an
                // unjoined image/volume/build-cache is exactly as real
                // as a joined one.
                let recovery = match candidate.kind {
                    ArtifactKind::DockerImage => crate::recovery::docker_image_recovery(
                        (!candidate.dangling).then_some(candidate.reference.as_str()),
                        false,
                    ),
                    ArtifactKind::DockerBuildCache => {
                        crate::recovery::docker_build_cache_recovery(false)
                    }
                    _ => crate::recovery::docker_volume_recovery(&candidate.reference),
                };
                let recovery_evidence = match &recovery.follow_up_check {
                    Some(check) => recovery.evidence.with_note(format!("check: {check}")),
                    None => recovery.evidence,
                };
                // An unjoined object's activity/current-use facts are as
                // real as a joined one's: "no project claims it" is a
                // consumer fact, not a reason to drop the daemon's own
                // running-container and last-used evidence.
                let mut evidence = vec![recovery_evidence];
                evidence.extend(candidate.docker_evidence);
                result.unowned.push(UnownedRow {
                    measurement: None,
                    path_or_object: candidate.reference,
                    bytes: candidate.unique_bytes,
                    reason: UnownedReason::DockerNoJoin,
                    shared_bytes: Some(candidate.shared_bytes),
                    note,
                    docker_kind: Some(docker_kind.to_string()),
                    created_at: candidate.created_at,
                    containers: candidate.containers,
                    shared_with: candidate.shared_with,
                    dangling: candidate.dangling,
                    evidence,
                });
            }
        }
    }

    result
}

pub fn to_json(report: &Report) -> Result<String> {
    Ok(serde_json::to_string_pretty(report)?)
}

/// Observes/reports a whole resolved [`crate::scope::EffectiveScope`]
/// coherently (#42): one call, one merged [`Report`] over every root the
/// scope actually resolved to walk, plus a [`crate::coverage::RootCoverage`]
/// row per candidate root explaining what this pass could (and could not)
/// establish about it.
///
/// Each in-scope root keeps its own physical growth store (keyed by
/// `growth::root_scoped_volume_id`, unchanged from before #42): this
/// function's contribution is a single coherent orchestration over all of
/// them, not a merged store. That keeps the hazard this issue exists to
/// close -- one root's observation silently overwriting or tombstoning
/// another's, or a scope-wide sweep treating "not walked" as "deleted" --
/// structurally impossible: two roots can never share a store to corrupt.
///
/// A `Present` root is re-checked for read access immediately before
/// walking (scope resolution and this call are never atomic: access can
/// be lost in between), and its walk's own outcome is inspected for
/// permission-denied residue before deciding `Complete` vs `Partial`. A
/// walk that fails outright (`Err`) never reaches the growth store at
/// all -- see `bus::run_report`'s `ReportCached`-gated checkpoint commit
/// -- so marking that root `Inaccessible` here never contradicts what was
/// (not) persisted for it.
#[allow(clippy::too_many_arguments)]
pub fn report_scope(
    scope: &crate::scope::EffectiveScope,
    docker_facts: Option<&Path>,
    verify_du: bool,
    store_dir: Option<&Path>,
    since_override: Option<&str>,
    observe: bool,
    include_dirs: bool,
    enrich: bool,
    force_full: bool,
) -> Result<(Report, Vec<crate::coverage::RootCoverage>)> {
    report_scope_with_source(
        scope,
        docker_facts,
        verify_du,
        store_dir,
        since_override,
        observe,
        include_dirs,
        enrich,
        force_full,
        crate::fs_events::platform_source().as_ref(),
    )
}

/// Same as [`report_scope`], with the [`crate::fs_events::FsEventsSource`]
/// supplied explicitly -- the seam adversarial multi-root tests use.
#[allow(clippy::too_many_arguments)]
pub fn report_scope_with_source(
    scope: &crate::scope::EffectiveScope,
    docker_facts: Option<&Path>,
    verify_du: bool,
    store_dir: Option<&Path>,
    since_override: Option<&str>,
    observe: bool,
    include_dirs: bool,
    enrich: bool,
    force_full: bool,
    fs_events_source: &dyn crate::fs_events::FsEventsSource,
) -> Result<(Report, Vec<crate::coverage::RootCoverage>)> {
    let (r, c, _per_root) = report_scope_with_parts(
        scope,
        docker_facts,
        verify_du,
        store_dir,
        since_override,
        observe,
        include_dirs,
        enrich,
        force_full,
        fs_events_source,
    )?;
    Ok((r, c))
}

/// Same as [`report_scope_with_source`], additionally returning every
/// present root's own single-root [`Report`] (keyed by its walked
/// path), before they were folded into the merged one -- what a
/// multi-root TUI (#51) needs so a later live refresh can replace just
/// one root's entry (`report::merge_root_report_into`/
/// `report::merge_reports`) instead of re-walking or re-merging every
/// root whenever any one of them changes.
#[allow(clippy::too_many_arguments)]
pub fn report_scope_with_parts(
    scope: &crate::scope::EffectiveScope,
    docker_facts: Option<&Path>,
    verify_du: bool,
    store_dir: Option<&Path>,
    since_override: Option<&str>,
    observe: bool,
    include_dirs: bool,
    enrich: bool,
    force_full: bool,
    fs_events_source: &dyn crate::fs_events::FsEventsSource,
) -> Result<(
    Report,
    Vec<crate::coverage::RootCoverage>,
    std::collections::HashMap<PathBuf, Report>,
)> {
    report_scope_with_parts_covered(
        scope,
        docker_facts,
        verify_du,
        store_dir,
        since_override,
        observe,
        include_dirs,
        enrich,
        force_full,
        fs_events_source,
    )
    .map(|(r, c, p, _)| (r, c, p))
}

/// The merged report, per-root coverage, each root's own report, and the
/// event coverage this pass's replays earned.
pub type ScopeWalk = (
    Report,
    Vec<crate::coverage::RootCoverage>,
    std::collections::HashMap<PathBuf, Report>,
    crate::fs_events::EventCoverage,
);

/// [`report_scope_with_parts`] plus the [`crate::fs_events::EventCoverage`]
/// this pass's replays earned, one window per root that went
/// incremental. `observe_scope` hands it to the unit families; nothing
/// else needs it, and a caller that cannot produce one gets
/// `EventCoverage::untrusted()` and no reuse.
#[allow(clippy::too_many_arguments)]
pub fn report_scope_with_parts_covered(
    scope: &crate::scope::EffectiveScope,
    docker_facts: Option<&Path>,
    verify_du: bool,
    store_dir: Option<&Path>,
    since_override: Option<&str>,
    observe: bool,
    include_dirs: bool,
    enrich: bool,
    force_full: bool,
    fs_events_source: &dyn crate::fs_events::FsEventsSource,
) -> Result<ScopeWalk> {
    use crate::coverage::{RegionStatus, RootCoverage};
    use crate::scope::RootStatus;

    // Asked once for the whole scope, not once per root: the answer is a
    // property of the invocation's authorization, and
    // `authorized_roots()` is not free.
    let docker_authorized = scope.docker_in_scope();
    let mut per_root: std::collections::HashMap<PathBuf, Report> = std::collections::HashMap::new();

    let observed_at = crate::entities::now();
    let mut coverage: Vec<RootCoverage> = Vec::new();
    let mut events = crate::fs_events::EventCoverage::untrusted();
    let mut merged = Report {
        observed_at,
        root: PathBuf::new(),
        projects: Vec::new(),
        unowned: Vec::new(),
        reconciliation: Reconciliation {
            unique_estimate: None,
            attributed: 0,
            unowned: 0,
            walked_total: 0,
            du_total: None,
            docker_attributed: 0,
            docker_unowned: 0,
        },
        series_by_key: std::collections::HashMap::new(),
        total_series: Vec::new(),
        series_window_secs: 0,
        notes: Vec::new(),
        dirs_by_worktree: include_dirs.then(std::collections::HashMap::new),
        files_by_worktree: include_dirs.then(std::collections::HashMap::new),
        schedule_line: None,
        summary: Summary::default(),
        github_enrichment: None,
        nested_artifacts: Vec::new(),
        store_dir: store_dir.map(Path::to_path_buf),
    };

    for scope_root in &scope.roots {
        match &scope_root.status {
            RootStatus::Excluded { .. } => {
                coverage.push(RootCoverage::excluded(scope_root.path.clone()));
            }
            // Folded into its parent's own walk (see
            // `scope::resolve_effective_scope`'s nested-folding pass): the
            // parent's region already accounts for this path, so it gets
            // no separate coverage row rather than a misleading "not
            // observed" one.
            RootStatus::SkippedAsNested { .. } => {}
            RootStatus::Missing => {
                coverage.push(RootCoverage::missing(scope_root.path.clone()));
            }
            RootStatus::Unreadable { reason } => {
                coverage.push(RootCoverage::inaccessible(
                    scope_root.path.clone(),
                    reason.clone(),
                ));
            }
            RootStatus::Present => {
                let path = &scope_root.path;
                // A detector location (Cargo home, rustup, Homebrew,
                // `~/Library/Caches`, `~/Library/Developer`, ...) never
                // gets the ordinary project walk: it is measured only as
                // an external unit, by `external::discover_and_measure`,
                // over in `observe_scope` (#R13 item B, "project roots
                // vs. detector locations"). This is what keeps a Git
                // checkout planted inside a detector's home from ever
                // becoming a project, and what keeps an unchanged
                // `observe` from re-walking every detector home's
                // contents on top of measuring it as a unit.
                if !scope_root.is_project_root() {
                    coverage.push(RootCoverage::detector_only(path.clone()));
                    continue;
                }
                // Scope resolution and this call are never atomic: redo the
                // presence/readability check right before walking so a
                // root that lost access in between is never silently
                // walked as if it were empty.
                match crate::fs_gate::probe_listable(path) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        coverage.push(RootCoverage::missing(path.clone()));
                        continue;
                    }
                    Err(e) => {
                        coverage.push(RootCoverage::inaccessible(path.clone(), e.to_string()));
                        continue;
                    }
                    Ok(_) => {}
                }
                let mut pruned: Vec<PathBuf> = scope
                    .pruned_subtrees
                    .iter()
                    .filter(|note| &note.root == path)
                    .map(|note| PathBuf::from(&note.pattern))
                    .collect();
                // External-unit-eligible locations nested under this
                // root are pruned from its ordinary walk here (#45-#49's
                // B2 gap): `crate::external::discover_and_measure`
                // measures them independently, so counting them again as
                // this root's walked/unowned bytes would double the
                // measurement. Noted on the merged report regardless of
                // this root's walk outcome, so the coverage story is
                // visible even if the walk itself later fails.
                let external_prunes: Vec<&crate::scope::ExternalPruneNote> = scope
                    .external_pruned_subtrees
                    .iter()
                    .filter(|n| &n.root == path)
                    .collect();
                for n in &external_prunes {
                    pruned.push(n.path.clone());
                    merged.notes.push(format!(
                        "[{}] {} pruned from walk: measured as external unit ({})",
                        path.display(),
                        n.path.display(),
                        n.detector_id
                    ));
                }
                let r = report_full_mode_scoped_tracked(
                    path,
                    docker_facts,
                    verify_du,
                    store_dir,
                    since_override,
                    observe,
                    include_dirs,
                    enrich,
                    force_full,
                    fs_events_source,
                    &pruned,
                    docker_authorized,
                    Some(observed_at),
                );
                let r = match r {
                    Ok((r, window)) => {
                        if let Some((changed, since)) = window {
                            // FSEvents answers in canonical paths, and
                            // so must the window's root: a unit reached
                            // through a symlinked alias would otherwise
                            // never match it.
                            let canonical =
                                crate::fs_gate::canonicalize(path).unwrap_or_else(|_| path.clone());
                            events.trust(canonical, changed, since);
                        }
                        r
                    }
                    Err(e) => {
                        // The walk failed outright: `bus::run_report`'s
                        // checkpoint only commits on `ReportCached`, so no
                        // consumer -- including the growth store -- wrote
                        // anything for this root from this attempt.
                        coverage.push(RootCoverage::inaccessible(path.clone(), e.to_string()));
                        continue;
                    }
                };
                let unreadable_paths = r
                    .unowned
                    .iter()
                    .filter(|u| u.reason == UnownedReason::PermissionDenied)
                    .count();
                let status = if unreadable_paths > 0 {
                    RegionStatus::Partial {
                        reason: format!("{unreadable_paths} path(s) unreadable during this walk"),
                    }
                } else {
                    RegionStatus::Complete
                };
                let mode = r
                    .notes
                    .iter()
                    .find_map(|n| n.strip_prefix("fsevents: mode="))
                    .and_then(|s| s.split(' ').next())
                    .unwrap_or("full")
                    .to_string();
                coverage.push(RootCoverage {
                    path: path.clone(),
                    status,
                    walked_total: r.reconciliation.walked_total,
                    projects: r.projects.len(),
                    mode,
                });
                per_root.insert(path.clone(), r.clone());
                merge_root_report_into(&mut merged, r);
            }
        }
    }
    Ok((merged, coverage, per_root, events))
}

/// One scope observation's whole result: the merged report, per-root
/// coverage, each root's own report, and the external and agent units
/// discovered *in the same pass*.
pub struct ScopeObservation {
    pub merged: Report,
    pub coverage: Vec<crate::coverage::RootCoverage>,
    pub per_root: std::collections::HashMap<PathBuf, Report>,
    pub external_units: Vec<crate::external::ExternalUnit>,
    pub agent_units: Vec<crate::agents::AgentUnit>,
    /// One row per authorized unit root: whether this pass's own
    /// FSEvents replay covered it (so its units could be replayed) or
    /// why it could not. Empty when no detector resolved a root.
    pub unit_root_coverage: Vec<crate::coverage::UnitRootCoverage>,
    /// The identified interiors of the machine-wide build stores among
    /// `external_units` -- a Maven repository's artifact versions, a Go
    /// module cache's modules, a DerivedData project folder's products
    /// (`crate::build_stores`). Each unit's `container_id` names its
    /// store; a unit belongs to the external unit whose path it lies
    /// under. Empty when external units were not observed.
    pub store_interiors: Vec<crate::artifact::NestedArtifact>,
}

/// Which parts of a scope this observation covers.
///
/// A part left out is a part not *observed*, which also means not swept:
/// `growth::ObservationOwnership` only tombstones inside the region an
/// observation actually covered, so asking for fewer parts can never
/// invent a disappearance. That is what makes this a cost decision
/// rather than a correctness one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservationParts {
    pub external: bool,
    pub agents: bool,
}

impl ObservationParts {
    /// Everything: the walk plus both unit families.
    pub const ALL: Self = Self {
        external: true,
        agents: true,
    };
    /// The walk only.
    pub const WALK_ONLY: Self = Self {
        external: false,
        agents: false,
    };
    pub const EXTERNAL: Self = Self {
        external: true,
        agents: false,
    };
    pub const AGENTS: Self = Self {
        external: false,
        agents: true,
    };
}

/// The one entry point that observes a scope completely: walk, external
/// units and agent units, in a single pass with a single ownership.
///
/// Two review findings meet here.
///
/// *History ownership*: external and agent discovery share one current
/// table, and running them as separate passes in an order nobody
/// declared is what let each tombstone the other's rows and invent
/// regrowth. Running them from one place means the ordering question
/// does not arise, and each still carries its own
/// `growth::ObservationOwnership` so it could not matter even if it did.
///
/// *TUI staleness*: `finish_startup` was the only production caller that
/// refreshed the TUI's external/agent unit vectors, so agent storage
/// could be arbitrarily stale while the header said the report was
/// live. A refresh that returns all three together cannot update one
/// without the others.
///
/// `want` says which parts this observation covers. A caller that will
/// not show external or agent units does not pay for them -- and,
/// because a part not observed is a part not swept, skipping one can
/// never tombstone anything either. `base` lets a caller that already
/// has a report (the CLI's explicit-root path, which walks one root
/// rather than the configured scope) hand it in instead of walking
/// again: that is the whole point of this function owning discovery, so
/// there is nowhere else to run a second pass from
/// (`.oh/guardrails/discovery-owned-by-report-pipeline.md`).
#[allow(clippy::too_many_arguments)]
pub fn observe_scope(
    scope: &crate::scope::EffectiveScope,
    want: ObservationParts,
    base: Option<Report>,
    docker_facts: Option<&Path>,
    verify_du: bool,
    store_dir: Option<&Path>,
    since_override: Option<&str>,
    observe: bool,
    include_dirs: bool,
    enrich: bool,
    force_full: bool,
    fs_events_source: &dyn crate::fs_events::FsEventsSource,
    retention_days: u64,
    since_secs: u64,
) -> Result<ScopeObservation> {
    let owns_root_coverage = base.is_none();
    if observe && let Some(dir) = store_dir {
        crate::growth::invalidate_unique_estimate(dir, &scope_snapshot_key(scope))?;
    }
    // Every authorized external/agent root's own replay window, taken
    // **before** the walk so each cursor is read at its previous pass's
    // value rather than one this pass has just written -- a root that is
    // also a scan root would otherwise be asked about a window it opened
    // itself.
    //
    // This is what makes the event gate do anything at all in a default
    // install: scan roots are project directories, and `~/.claude`,
    // `~/.cargo` and `~/Library/Caches/...` are not under them, so
    // before this the only windows in existence could never reach the
    // units they were supposed to vouch for
    // (`.oh/sessions/2026-09-22-event-gated-reuse.md` §3).
    let trace = std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty());
    let phase = std::time::Instant::now();
    let unit_roots = scope.authorized_unit_roots();
    if trace {
        eprintln!("[xtrace] authorized_unit_roots: {:?}", phase.elapsed());
    }
    let unit_replay = crate::growth::replay_unit_roots(
        store_dir,
        &unit_roots,
        crate::entities::now(),
        force_full,
        fs_events_source,
    );

    // A caller that already walked (the CLI's explicit-root path) hands
    // its report in rather than walking a second time. It has no
    // per-root coverage to contribute, which is honest: coverage
    // describes the scope this function walked, and it did not walk one.
    if trace {
        eprintln!("[trace] observe: unit-root replay: {:?}", phase.elapsed());
    }
    let phase = std::time::Instant::now();
    let (merged, coverage, mut per_root, events) = match base {
        // A handed-in report brings no *walk* window with it. The unit
        // roots' own cursors are independent evidence -- they were
        // replayed above, not derived from the walk -- so they still
        // apply; the report this caller handed in simply contributes
        // none of its own.
        Some(r) => (
            r,
            Vec::new(),
            std::collections::HashMap::new(),
            crate::fs_events::EventCoverage::untrusted(),
        ),
        None => report_scope_with_parts_covered(
            scope,
            docker_facts,
            verify_du,
            store_dir,
            since_override,
            observe,
            include_dirs,
            enrich,
            force_full,
            fs_events_source,
        )?,
    };
    if trace {
        eprintln!("[trace] observe: project roots: {:?}", phase.elapsed());
    }
    let phase = std::time::Instant::now();
    let mut events = events;
    events.merge(unit_replay.coverage.clone());
    let observed_at = merged.observed_at;
    let mut merged = merged;
    let pass = pass::DiscoveryPass::begin();
    let mut external_ok = true;
    let (mut external_units, store_interiors) = if want.external {
        let measured = crate::external::observe_external(
            &pass,
            scope,
            store_dir,
            observe,
            observed_at,
            retention_days,
            since_secs,
            &events,
        );
        external_ok = measured.is_ok();
        let observation = measured.unwrap_or_default();
        let mut units = observation.units;
        crate::consumer_wiring::attach_associations(&mut merged, &mut units, store_dir);
        (units, observation.interiors)
    } else {
        (Vec::new(), Vec::new())
    };
    let _ = &mut external_units;
    if trace {
        eprintln!("[trace] observe: external units: {:?}", phase.elapsed());
    }
    let phase = std::time::Instant::now();
    // Aider's per-repository units need every known worktree root; the
    // walk above already produced them, so this costs no extra walk.
    let project_worktrees: Vec<PathBuf> = merged
        .projects
        .iter()
        .flat_map(|p| p.worktrees.iter())
        .map(|wt| wt.path.clone())
        .collect();
    let mut agents_ok = true;
    let agent_units = if want.agents {
        let measured = crate::agents::discover_and_measure_in(
            &pass,
            scope,
            &project_worktrees,
            store_dir,
            observe,
            observed_at,
            retention_days,
            since_secs,
            &events,
        );
        agents_ok = measured.is_ok();
        measured.unwrap_or_default()
    } else {
        Vec::new()
    };

    // The cursors advance only for a pass that observed **both** unit
    // families and persisted them. Three refusals in one condition:
    //
    // * `observe` false: this pass wrote no rows, so the next pass must
    //   keep comparing against the rows the last observing pass wrote.
    // * a family this pass did not cover: its rows are still the
    //   previous pass's. Advancing would move the window past changes
    //   that family never measured; the `observed_at` guard would then
    //   refuse to reuse those rows, so the cost of advancing is a
    //   guaranteed re-measurement and the cost of not advancing is a
    //   slightly wider replay. Wider wins.
    // * a family that erred after measuring: same reasoning, and this is
    //   the `ReportCached` gate the walk's own checkpoint has -- a pass
    //   that failed to persist never gets to vouch for what it measured.
    if trace {
        eprintln!("[trace] observe: agent units: {:?}", phase.elapsed());
    }
    let phase = std::time::Instant::now();
    let unit_root_coverage: Vec<crate::coverage::UnitRootCoverage> = unit_replay
        .outcomes
        .iter()
        .map(|(path, reason)| crate::coverage::UnitRootCoverage {
            path: path.clone(),
            event_covered: reason == "incremental",
            reason: reason.clone(),
        })
        .collect();
    if std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty()) {
        for c in &unit_root_coverage {
            eprintln!(
                "[xtrace-cov] {} covered={} reason={}",
                c.path.display(),
                c.event_covered,
                c.reason
            );
        }
    }
    // One line on the report saying why the unit families cost what they
    // cost this pass: how many authorized roots this pass's own replay
    // could vouch for, and the named refusal behind each one it could
    // not. A coverage fact, never a storage fact -- a root that loses
    // its window re-measures, which changes cost and nothing else.
    if !unit_root_coverage.is_empty() {
        let covered = unit_root_coverage
            .iter()
            .filter(|c| c.event_covered)
            .count();
        let mut refused: Vec<String> = unit_root_coverage
            .iter()
            .filter(|c| !c.event_covered)
            .map(|c| format!("{} ({})", c.path.display(), c.reason))
            .collect();
        refused.sort();
        let tail = if refused.is_empty() {
            String::new()
        } else {
            format!(", re-measured: {}", refused.join(", "))
        };
        merged.notes.push(format!(
            "unit roots: {covered}/{} event-covered{tail}",
            unit_root_coverage.len()
        ));
    }
    if observe && want.external && want.agents && external_ok && agents_ok {
        unit_replay.commit()?;
    }
    if trace {
        eprintln!("[trace] observe: commit replay: {:?}", phase.elapsed());
    }
    let phase = std::time::Instant::now();

    // Reconciliation is deliberately opt-in. Normal refreshes never visit
    // unchanged roots to recover sharing identities that folded rows discard.
    let prior_unique = store_dir
        .and_then(|dir| crate::growth::read_run_row(dir, &scope_snapshot_key(scope)))
        .and_then(|r| {
            r.unique_bytes
                .zip(r.unique_reconciled_at)
                .map(|(bytes, at)| (bytes, at, r.sharing))
        })
        .map(|(bytes, reconciled_at, sharing)| UniqueEstimate {
            sharing,
            bytes,
            reconciled_at,
            needs_reconciliation: true,
        });
    merged.reconciliation.unique_estimate = prior_unique;
    if force_full
        && owns_root_coverage
        && want == ObservationParts::ALL
        && external_ok
        && agents_ok
        && coverage.iter().all(|c| {
            matches!(
                c.status,
                crate::coverage::RegionStatus::Complete
                    | crate::coverage::RegionStatus::Excluded
                    | crate::coverage::RegionStatus::DetectorOnly
                    | crate::coverage::RegionStatus::Missing
            )
        })
        && agent_units.iter().all(|u| u.complete)
        && external_units.iter().all(|u| u.note.is_none())
    {
        let mut paths: Vec<PathBuf> = per_root.keys().cloned().collect();
        paths.extend(external_units.iter().map(|u| u.path.clone()));
        for unit in &agent_units {
            if unit.members.is_empty() {
                paths.push(unit.path.clone());
            } else {
                paths.extend(unit.members.iter().map(|m| m.path.clone()));
            }
        }
        let excluded: Vec<PathBuf> = scope
            .pruned_subtrees
            .iter()
            .map(|p| PathBuf::from(&p.pattern))
            .collect();
        let mut containers = paths.clone();
        for project in &merged.projects {
            for worktree in &project.worktrees {
                containers.push(worktree.path.clone());
                containers.extend(
                    worktree
                        .artifacts
                        .iter()
                        .filter(|a| !a.source.tool.starts_with("docker"))
                        .map(|a| a.path.clone()),
                );
            }
        }
        if let Some((bytes, sharing)) =
            crate::walk::reconcile_shared_bytes(&paths, &excluded, &containers)
        {
            merged.reconciliation.unique_estimate = Some(UniqueEstimate {
                sharing: Some(sharing),
                bytes,
                reconciled_at: observed_at,
                needs_reconciliation: false,
            });
        }
    }

    for report in per_root.values_mut() {
        report.reconciliation.unique_estimate = merged.reconciliation.unique_estimate.clone();
    }
    let observation = ScopeObservation {
        merged,
        coverage,
        per_root,
        external_units,
        agent_units,
        unit_root_coverage,
        store_interiors,
    };

    // R12: persist the scope-wide tables `swamp report` reads instead
    // of walking. Gated exactly like `unit_replay.commit()` above and
    // for the same reason -- only a pass that persisted (`observe`),
    // covered both unit families (`want == ALL`, so no table is missing
    // a family the caller never asked for) and did not error measuring
    // either one gets to replace what the last full observation wrote.
    if observe
        && want == ObservationParts::ALL
        && external_ok
        && agents_ok
        && let Some(store_dir) = store_dir
    {
        let key = scope_snapshot_key(scope);
        // R15 item 2/~10 of the JSON-in-the-store decomposition:
        // `projects.parquet`/`worktrees.parquet`/`worktree_facts.parquet`.
        // Same gate, same already-merged `projects` tree -- no second
        // pass.
        crate::growth::write_project_worktree_tables(
            store_dir,
            &key,
            &observation.merged.projects,
            observation.merged.observed_at,
        )?;
        // R18a-3b: `artifact_shape.parquet`/`artifact_shape_lists.
        // parquet` -- the last fields a project's worktree's artifact
        // list needed, from this same already-merged `projects` tree.
        // After this, the old JSON render-cache row (now deleted) has
        // nothing left it would have needed to carry.
        crate::growth::write_artifact_shape_table(
            store_dir,
            &key,
            &observation.merged.projects,
            observation.merged.observed_at,
        )?;
        // R16 item 1 of this slice: `external_units.parquet`/
        // `agent_units.parquet`/`unit_consumers.parquet`, typed columns
        // for the unit families this same pass already discovered and
        // measured (`observation.external_units`/`.agent_units`) -- no
        // second discovery pass.
        crate::growth::write_unit_tables(
            store_dir,
            &key,
            &observation.external_units,
            &observation.agent_units,
        )?;
        // R16 item 2 of this slice: `nested_artifacts.parquet`, from the
        // same pass's two already-measured `NestedArtifact` lists.
        crate::growth::write_nested_artifact_table(
            store_dir,
            &key,
            &observation.merged.nested_artifacts,
            &observation.store_interiors,
        )?;
        // R16 item 3 of this slice: `evidence.parquet`, one row per
        // `crate::evidence::Evidence` across every entity kind this same
        // pass already carries it on -- an `ArtifactRow` (keyed by
        // `artifact_row_key`), an `ExternalUnit`/`AgentUnit` (keyed by
        // the id the unit tables use) and a `NestedArtifact`'s
        // `decision_evidence` (keyed by its own id, from either list).
        let mut evidence_entities: Vec<(String, &[crate::evidence::Evidence])> = Vec::new();
        for p in &observation.merged.projects {
            for wt in &p.worktrees {
                for a in &wt.artifacts {
                    if a.evidence.is_empty() {
                        continue;
                    }
                    let artifact_key = crate::growth::artifact_row_key(
                        &p.project_id,
                        &wt.worktree_id,
                        &wt.path,
                        a,
                    );
                    evidence_entities.push((
                        crate::growth::evidence_row_key("artifact", &artifact_key),
                        &a.evidence,
                    ));
                }
            }
        }
        for u in &observation.external_units {
            if u.evidence.is_empty() {
                continue;
            }
            let id = crate::growth::external_unit_table_id(
                &u.detector_id,
                crate::external::category_str(u.category),
                &u.path,
            );
            evidence_entities.push((
                crate::growth::evidence_row_key("external-unit", &id),
                &u.evidence,
            ));
        }
        for u in &observation.agent_units {
            if u.evidence.is_empty() {
                continue;
            }
            evidence_entities.push((
                crate::growth::evidence_row_key("agent-unit", &u.id),
                &u.evidence,
            ));
        }
        for n in observation
            .merged
            .nested_artifacts
            .iter()
            .chain(observation.store_interiors.iter())
        {
            if n.decision_evidence.is_empty() {
                continue;
            }
            evidence_entities.push((
                crate::growth::evidence_row_key("nested-artifact", &n.id),
                &n.decision_evidence,
            ));
        }
        crate::growth::write_evidence_table(store_dir, &key, &evidence_entities)?;
        // Coverage per root, carrying each walked root's own walk totals
        // (R20: `Report.reconciliation` is their sum, computed on read).
        let reconciliation_by_root: std::collections::HashMap<PathBuf, Reconciliation> =
            observation
                .per_root
                .iter()
                .map(|(root, r)| (root.clone(), r.reconciliation.clone()))
                .collect();
        crate::growth::write_coverage_table(
            store_dir,
            &key,
            &observation.coverage,
            &observation.unit_root_coverage,
            &reconciliation_by_root,
            observation.merged.observed_at,
        )?;
        // The run's own diagnostics: what the consumers noted while
        // walking (fsevents mode, daemon reachability, ...).
        crate::growth::write_notes_table(
            store_dir,
            &key,
            &observation.merged.notes,
            observation.merged.observed_at,
        )?;
        // R20: per walked root, the two fact families the walk's own
        // checkpoint could not write -- the Docker rows the gate joined
        // after it, and the tracking state of top-level directories.
        // Everything else a read needs is already in the volume's
        // tables.
        for (root, r) in &observation.per_root {
            let vol = crate::growth::volume_store_dir(store_dir, root);
            let docker_rows: Vec<UnownedRow> = r
                .unowned
                .iter()
                .filter(|u| u.reason == UnownedReason::DockerNoJoin)
                .cloned()
                .collect();
            crate::growth::write_docker_unowned(&vol, &docker_rows)?;
            if let Some(dirs) = &r.dirs_by_worktree {
                crate::growth::write_dir_tracks(&vol, dirs)?;
            }
        }
        // The run row: the window the pass used (so a read derives the
        // same growth/series), the live-enrichment counters, the
        // scheduler status it saw -- and the "observed at all" marker.
        let config = crate::growth::load_config(store_dir);
        let history_since_secs = since_override
            .and_then(crate::growth::parse_duration_secs)
            .or_else(|| crate::growth::parse_duration_secs(&config.since))
            .unwrap_or(86_400);
        crate::growth::write_run_row(
            store_dir,
            &key,
            &crate::growth::RunFacts {
                unique_estimate: observation.merged.reconciliation.unique_estimate.as_ref(),
                observed_at: observation.merged.observed_at,
                since_secs: history_since_secs,
                retention_days: config.retention_days,
                include_dirs,
                github_enrichment: observation.merged.github_enrichment.as_ref(),
                schedule_line: observation.merged.schedule_line.as_deref(),
            },
        )?;
    }

    if trace {
        eprintln!("[trace] observe: scope tables: {:?}", phase.elapsed());
    }
    Ok(observation)
}

/// Folds one already-fully-formed single-root [`Report`] `r` into a
/// running multi-root accumulator `merged` -- the reusable half of
/// [`report_scope_with_source`]'s per-root merge, factored out (#51) so
/// a live TUI refresh can rebuild its aggregate report by re-folding a
/// per-root cache (`reports_by_root`) without re-deriving this
/// accumulation logic. `r.root` (already set to the walked path by
/// `report_full_mode_with_exclusions`) is used to prefix `r`'s notes and
/// as `merged.root`'s fallback, exactly as `report_scope_with_source`
/// used its own `path` variable before this extraction -- calling this
/// twice for the same root is safe only if the caller first removes
/// that root's *previous* contribution (a fresh `Report::default()`-like
/// accumulator, or a full re-fold over every root's latest cached
/// report, never an in-place double-add of the same root).
pub fn merge_root_report_into(merged: &mut Report, r: Report) {
    let path = r.root.clone();
    if let Some(mut estimate) = r.reconciliation.unique_estimate.clone() {
        // A refresh merge is not an inode reconciliation. Carry one scope
        // estimate, never sum the copies attached to its root reports.
        estimate.needs_reconciliation = true;
        merged.reconciliation.unique_estimate = Some(estimate);
    }
    merge_summary_into(&mut merged.summary, &r.summary);

    if merged.root.as_os_str().is_empty() {
        merged.root = path.clone();
    }
    merged.projects.extend(r.projects);
    merged.unowned.extend(r.unowned);
    merged.reconciliation.attributed += r.reconciliation.attributed;
    merged.reconciliation.unowned += r.reconciliation.unowned;
    merged.reconciliation.walked_total += r.reconciliation.walked_total;
    merged.reconciliation.docker_attributed += r.reconciliation.docker_attributed;
    merged.reconciliation.docker_unowned += r.reconciliation.docker_unowned;
    merged.reconciliation.du_total =
        match (merged.reconciliation.du_total, r.reconciliation.du_total) {
            (None, None) => None,
            (x, y) => Some(x.unwrap_or(0) + y.unwrap_or(0)),
        };
    merge_series_into(
        &mut merged.series_by_key,
        &mut merged.total_series,
        &mut merged.series_window_secs,
        r.series_by_key,
        r.total_series,
        r.series_window_secs,
    );
    for note in r.notes {
        merged.notes.push(format!("[{}] {note}", path.display()));
    }
    if let Some(dbw) = r.dirs_by_worktree {
        merged
            .dirs_by_worktree
            .get_or_insert_with(std::collections::HashMap::new)
            .extend(dbw);
    }
    if let Some(fbw) = r.files_by_worktree {
        merged
            .files_by_worktree
            .get_or_insert_with(std::collections::HashMap::new)
            .extend(fbw);
    }
    if merged.schedule_line.is_none() {
        merged.schedule_line = r.schedule_line;
    }
    match (&mut merged.github_enrichment, r.github_enrichment) {
        (slot @ None, Some(g)) => *slot = Some(g),
        (Some(acc), Some(g)) => {
            acc.calls_made += g.calls_made;
            acc.worktrees_enriched += g.worktrees_enriched;
            acc.elapsed_secs += g.elapsed_secs;
        }
        _ => {}
    }
    // The daemon answers once for the whole machine, and every root's
    // pass carries the same BuildKit records: one copy each, by the
    // record's stable id. Filesystem units belong to exactly one root and
    // are kept as they are.
    for u in r.nested_artifacts {
        if u.reported_by.is_some() && merged.nested_artifacts.iter().any(|m| m.id == u.id) {
            continue;
        }
        merged.nested_artifacts.push(u);
    }
}

/// Rebuilds one merged multi-root [`Report`] from scratch given every
/// root's latest single-root report, in a stable order -- what a TUI
/// live refresh calls after replacing one root's entry in its own
/// `reports_by_root` cache, so a change to one root can never leave
/// another root's rows stale, duplicated, or dropped: every rebuild
/// starts from an empty accumulator and re-folds every cached report.
pub fn merge_reports(
    roots_in_order: &[PathBuf],
    reports_by_root: &std::collections::HashMap<PathBuf, Report>,
) -> Report {
    let observed_at = reports_by_root
        .values()
        .map(|r| r.observed_at)
        .max()
        .unwrap_or_else(crate::entities::now);
    let mut merged = Report {
        observed_at,
        // Every per-root report of one scope shares one store.
        store_dir: reports_by_root.values().find_map(|r| r.store_dir.clone()),
        root: PathBuf::new(),
        projects: Vec::new(),
        unowned: Vec::new(),
        reconciliation: Reconciliation {
            unique_estimate: None,
            attributed: 0,
            unowned: 0,
            walked_total: 0,
            du_total: None,
            docker_attributed: 0,
            docker_unowned: 0,
        },
        series_by_key: std::collections::HashMap::new(),
        total_series: Vec::new(),
        series_window_secs: 0,
        notes: Vec::new(),
        dirs_by_worktree: None,
        files_by_worktree: None,
        schedule_line: None,
        summary: Summary::default(),
        github_enrichment: None,
        nested_artifacts: Vec::new(),
    };
    // `dirs_by_worktree`/`files_by_worktree` are `Some` only when the
    // caller asked for them (`include_dirs`); infer that from whether
    // *any* per-root report carried them, so the merged report matches
    // what a single `report_scope` call with the same flag would give.
    let include_dirs = reports_by_root
        .values()
        .any(|r| r.dirs_by_worktree.is_some() || r.files_by_worktree.is_some());
    if include_dirs {
        merged.dirs_by_worktree = Some(std::collections::HashMap::new());
        merged.files_by_worktree = Some(std::collections::HashMap::new());
    }
    for root in roots_in_order {
        if let Some(r) = reports_by_root.get(root) {
            merge_root_report_into(&mut merged, r.clone());
        }
    }
    // Consumer/association facts (#56/#57) were attached to the *merged*
    // report of the previous pass, so the per-root reports this re-fold
    // reads never carried them and every refresh silently erased them
    // (the PR #123 review's consumer_evidence_must_survive_a_per_root_report_refresh).
    // Re-attach from the persisted current-state tables: the
    // declaration/lockfile caches make an unchanged worktree a table
    // lookup, not a re-read. A merge has no external units to join, so
    // only the project side runs: nothing here reaches a subprocess. The
    // TUI calls this on its observation workers, never its event thread.
    if let Some(dir) = merged.store_dir.clone() {
        crate::consumer_wiring::attach_project_associations(&mut merged, Some(&dir));
    }
    merged
}

/// Merges one root's already-bus-computed `Summary` into a running total.
/// Deliberately *not* a call to `summarize` (which stays a bus-pipeline
/// stage per `all_report_paths_through_bus`/`event_bus_pluggable_consumers`
/// -- see `docs/ADRs/001-event-bus-report-pipeline.md`): `report_scope`
/// combines already-fully-formed single-root reports, it does not reach
/// into the bus's own stages to recompute one from scratch.
/// Folds one root's sparkline history into a running multi-root total:
/// per-key series are simply unioned (keys are per-entity); the total is
/// summed bucket by bucket when the windows line up, otherwise the first
/// root's total is kept (a length mismatch is never silently
/// interleaved). One function for both the observe-time merge
/// (`merge_root_report_into`) and the read-time derivation
/// (`growth::derive_report_views`), so the two agree by construction.
pub(crate) fn merge_series_into(
    series_by_key: &mut std::collections::HashMap<String, Vec<Option<u64>>>,
    total_series: &mut Vec<Option<u64>>,
    series_window_secs: &mut u64,
    add_by_key: std::collections::HashMap<String, Vec<Option<u64>>>,
    add_total: Vec<Option<u64>>,
    add_window_secs: u64,
) {
    series_by_key.extend(add_by_key);
    if total_series.is_empty() {
        *total_series = add_total;
        *series_window_secs = add_window_secs;
    } else if total_series.len() == add_total.len() {
        for (a, b) in total_series.iter_mut().zip(add_total.iter()) {
            *a = match (*a, *b) {
                (None, None) => None,
                (x, y) => Some(x.unwrap_or(0) + y.unwrap_or(0)),
            };
        }
    }
}

/// A folded artifact's own measured directory row is what its
/// `allocated_bytes`/`allocated_growth_bytes` show (R4c): the row keyed
/// by the artifact's `(worktree_id, rel_path)` among `roots`. Shared by
/// the observe pass (`consumers/growth.rs`, from the walk's rows) and
/// the read-time derivation (from `dirs.parquet`).
pub(crate) fn attach_allocated_from_dirs(
    projects: &mut [ProjectRow],
    dirs: &[DirRollup],
    roots: &std::collections::HashSet<(String, String)>,
) {
    let measured: std::collections::HashMap<(&str, &str), &DirRollup> = dirs
        .iter()
        .map(|row| ((row.worktree_id.as_str(), row.rel_path.as_str()), row))
        .collect();
    for project in projects.iter_mut() {
        for wt in &mut project.worktrees {
            for artifact in &mut wt.artifacts {
                let rel = artifact
                    .path
                    .strip_prefix(&wt.path)
                    .unwrap_or(&artifact.path)
                    .to_string_lossy();
                if !roots.contains(&(wt.worktree_id.clone(), rel.to_string())) {
                    continue;
                }
                if let Some(dir) = measured.get(&(wt.worktree_id.as_str(), rel.as_ref())) {
                    artifact.allocated_bytes = Some(dir.allocated_total);
                    artifact.allocated_growth_bytes = dir.growth_bytes;
                }
            }
        }
    }
}

/// One order for the drill-down rows however they were gathered (the
/// walk's rows arrive in traversal order; `dirs.parquet`'s in table
/// order): by relative path, which also puts every parent before its
/// children. Applied by the observe pass and the read-time derivation
/// alike, so both give the same `Report`.
pub(crate) fn sort_drill_down(
    dirs_by_worktree: &mut std::collections::HashMap<String, Vec<DirRollup>>,
    files_by_worktree: &mut std::collections::HashMap<String, Vec<FileRow>>,
) {
    for rows in dirs_by_worktree.values_mut() {
        rows.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    }
    for rows in files_by_worktree.values_mut() {
        rows.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    }
}

fn merge_summary_into(acc: &mut Summary, add: &Summary) {
    acc.projects += add.projects;
    acc.worktrees += add.worktrees;
    acc.artifacts += add.artifacts;
    for (tag, t) in &add.by_type {
        let e = acc.by_type.entry(tag.clone()).or_default();
        if e.name.is_empty() {
            e.name = t.name.clone();
        }
        e.projects += t.projects;
        e.artifacts += t.artifacts;
        e.bytes += t.bytes;
        if let Some(g) = t.growth_bytes {
            e.growth_bytes = Some(e.growth_bytes.unwrap_or(0) + g);
        }
    }
}

// ---------------------------------------------------------------------
// R12: `swamp report` is a pure read of stored Parquet rows; `swamp
// observe` is the only scanner. `crate::growth::ReportSnapshot` is
// assembled from those tables (R18a-3b: no JSON cell backs it anymore)
// and this module reads it back -- never a filesystem walk, a `stat`,
// or a subprocess.
// ---------------------------------------------------------------------

/// Re-exported so `report::ReportSnapshot` names the one type both
/// `observe` (which builds one from a [`ScopeObservation`]) and
/// `report` (which reads one back) deal in; it is assembled by
/// `crate::growth` because that is where every other current-state
/// table lives (`unowned.parquet`, `external/folded.parquet`, ...).
pub use crate::growth::ReportSnapshot;

/// The stable key `swamp report`/`swamp observe` use to find/write a
/// scope's tables: the sorted set of this
/// scope's candidate root paths, hashed. An explicit root (a scope of
/// one, #42) and the configured multi-root scope therefore key
/// differently by construction -- neither caller has to say which one
/// it built, and re-resolving the same scope always finds what the
/// last observation of it wrote.
pub fn scope_snapshot_key(scope: &crate::scope::EffectiveScope) -> String {
    let mut paths: Vec<String> = scope
        .roots
        .iter()
        .map(|r| r.path.display().to_string())
        .collect();
    paths.sort();
    crate::entities::id_for(&paths.join("\n"))[..16].to_string()
}

/// Why `swamp report` has nothing to render for a scope: it has never
/// been observed. `swamp report` never scans to produce one --
/// `swamp observe` is the only command that does.
#[derive(Debug)]
pub struct NoObservation {
    /// The scope's own description, for the error text: its present
    /// root(s), or a note that none are present.
    pub scope_description: String,
}

impl std::fmt::Display for NoObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "no observation yet for {}; run `swamp observe`",
            self.scope_description
        )
    }
}

impl std::error::Error for NoObservation {}

fn describe_scope_for_error(scope: &crate::scope::EffectiveScope) -> String {
    let mut present: Vec<String> = scope
        .roots
        .iter()
        .filter(|r| matches!(r.status, crate::scope::RootStatus::Present))
        .map(|r| r.path.display().to_string())
        .collect();
    present.sort();
    if present.is_empty() {
        "this scope (no present root)".to_string()
    } else {
        present.join(", ")
    }
}

/// R15 item 4 / R18a-3b: overwrites `snapshot.report.projects` with the
/// tree [`crate::growth::write_project_worktree_tables`] wrote for `key`
/// -- `projects.parquet`/`worktrees.parquet`/`worktree_facts.parquet`
/// are the source for a project's/worktree's own scalars (identity,
/// ecosystems, remote, branch, GitHub facts, merge-complete, signals).
/// An artifact row's *shape* (`kind`/`path`/`track`/`confidence`/
/// `source`/`note`/`created_at`/`containers`/`shared_with`/`dangling`/
/// `allocated_bytes`/`allocated_growth_bytes`/`growth_bytes`) comes from
/// `artifact_shape.parquet`/`artifact_shape_lists.parquet`
/// (`growth::artifact_shape_rows_by_worktree`), ordered exactly as
/// `observe_scope` produced it; its facts columns (`bytes`/
/// `local_bytes`/`mtime_max`/`hardlinked`/`dedup_stale`/
/// `regrowth_count`/`observed_at`/`ecosystem`) are then overlaid from
/// the current-artifact-history table (`ArtifactTableFacts`), same as
/// before this slice. No field is left to a JSON fallback -- there is no
/// JSON cell left to fall back to. A no-op when this scope has no
/// `projects.parquet` rows yet (an older store), leaving `snapshot`
/// untouched.
fn rebuild_projects_from_tables(
    scope: &crate::scope::EffectiveScope,
    store_dir: &Path,
    key: &str,
    snapshot: &mut ReportSnapshot,
) {
    let Some(tables) = crate::growth::read_project_worktree_tables(store_dir, key) else {
        return;
    };

    let mut shape_by_worktree = crate::growth::artifact_shape_rows_by_worktree(store_dir, key);

    let roots: Vec<PathBuf> = scope.roots.iter().map(|r| r.path.clone()).collect();
    let facts = crate::growth::artifact_table_facts_for_roots(store_dir, &roots);

    let mut new_projects = Vec::with_capacity(tables.projects.len());
    for stored_project in &tables.projects {
        let mut project = crate::growth::project_row_from_stored(stored_project);
        let mut worktrees = Vec::new();
        for stored_wt in tables
            .worktrees
            .iter()
            .filter(|w| w.project_id == stored_project.project_id)
        {
            let mut wt = crate::growth::worktree_row_from_stored(stored_wt, &tables.worktree_facts);
            let worktree_path = wt.path.clone();
            let mut artifacts = shape_by_worktree
                .remove(&stored_wt.worktree_id)
                .unwrap_or_default();
            for a in artifacts.iter_mut() {
                // `artifact_shape_rows_by_worktree` returns `path` as the
                // stored relative path (it has no worktree path to join
                // against) -- join it here, before the facts lookup,
                // which strips this same prefix back off. The worktree
                // root's own row (a `Source`/`Ignored`/`Untracked`
                // remainder) stores an empty relative path;
                // `Path::join("")` appends a trailing separator
                // (`/w1/`, not `/w1`), so that case keeps
                // `worktree_path` unjoined rather than round-tripping a
                // path that differs from what was observed only by a
                // trailing slash.
                if !a.path.as_os_str().is_empty() {
                    a.path = worktree_path.join(&a.path);
                } else {
                    a.path = worktree_path.clone();
                }
                let k = crate::growth::artifact_row_key(
                    &stored_project.project_id,
                    &stored_wt.worktree_id,
                    &worktree_path,
                    a,
                );
                if let Some(f) = facts.get(&k) {
                    a.bytes = f.bytes;
                    a.local_bytes = f.local_bytes;
                    a.mtime_max = f.mtime_max;
                    a.hardlinked = f.hardlinked;
                    a.dedup_stale = f.dedup_stale;
                    a.regrowth_count = f.regrowth_count;
                    a.observed_at = f.observed_at;
                    a.ecosystem = f.ecosystem.clone();
                }
            }
            wt.artifacts = artifacts;
            worktrees.push(wt);
        }
        project.worktrees = worktrees;
        new_projects.push(project);
    }
    snapshot.report.projects = new_projects;
}

/// `swamp report`'s only read path: loads the stored snapshot for
/// `scope`'s key and returns it, or [`NoObservation`] when this exact
/// scope has never been observed. No walk, no detector I/O beyond the
/// scope resolution the caller already did to build `scope`, no
/// subprocess -- see `.oh/sessions/2026-09-24-report-is-a-pure-read.md`.
/// R16 item 1: overwrites `snapshot.external_units`/`.agent_units` with
/// `write_unit_tables`'s rows for `key` -- `external_units.parquet`/
/// `agent_units.parquet` are the source for every field of each unit
/// (R18a-2: `provenance`, `hardlinked`, `tool_home`, `relative_path`,
/// `action`, `members` are all typed columns now, no JSON merge
/// fallback) and `unit_consumers.parquet` for an external unit's
/// declared-consumer list. Each unit's `evidence` is left empty here for
/// `rebuild_evidence_from_table` to fill. A no-op when this scope has no
/// unit-table rows yet (an older store).
fn rebuild_units_from_tables(store_dir: &Path, key: &str, snapshot: &mut ReportSnapshot) {
    let Some(tables) = crate::growth::read_unit_tables(store_dir, key) else {
        return;
    };

    let mut consumers_by_unit: std::collections::HashMap<
        String,
        Vec<&crate::growth::columns::StoredUnitConsumerRow>,
    > = std::collections::HashMap::new();
    for c in &tables.consumers {
        consumers_by_unit
            .entry(c.unit_id.clone())
            .or_default()
            .push(c);
    }
    let mut members_by_unit: std::collections::HashMap<
        String,
        Vec<&crate::growth::columns::StoredAgentMemberRow>,
    > = std::collections::HashMap::new();
    for m in &tables.agent_members {
        members_by_unit
            .entry(m.unit_id.clone())
            .or_default()
            .push(m);
    }

    snapshot.external_units = tables
        .external
        .iter()
        .map(|stored| {
            let mut cs = consumers_by_unit
                .get(&stored.id)
                .cloned()
                .unwrap_or_default();
            cs.sort_by_key(|c| c.seq);
            let consumers = cs
                .into_iter()
                .map(|c| crate::external::ExternalConsumer {
                    label: c.consumer_label.clone(),
                    note: c.basis.clone(),
                })
                .collect();
            crate::growth::external_unit_from_stored(stored, consumers)
        })
        .collect();

    snapshot.agent_units = tables
        .agent
        .iter()
        .map(|stored| {
            let members = members_by_unit.get(&stored.id).cloned().unwrap_or_default();
            crate::growth::agent_unit_from_stored(stored, &members)
        })
        .collect();
}

/// R18a-2: overwrites `snapshot.report.nested_artifacts` and
/// `snapshot.store_interiors` with `nested_artifacts.parquet`'s two
/// origin-split lists for `key`, each row's `nested_artifact_lists.
/// parquet`/`nested_artifact_evidence.parquet` child rows attached.
/// Every `NestedArtifact` field is typed now -- no JSON merge fallback.
/// A no-op when this scope has no nested-artifact rows yet (an older
/// store).
fn rebuild_nested_artifacts_from_tables(
    store_dir: &Path,
    key: &str,
    snapshot: &mut ReportSnapshot,
) {
    let Some((report_rows, interior_rows)) =
        crate::growth::read_nested_artifact_table(store_dir, key)
    else {
        return;
    };
    let new_report_nested = report_rows
        .iter()
        .map(|(stored, lists, evidence)| {
            crate::growth::nested_artifact_from_stored(stored, lists, evidence)
        })
        .collect();
    let new_store_interiors = interior_rows
        .iter()
        .map(|(stored, lists, evidence)| {
            crate::growth::nested_artifact_from_stored(stored, lists, evidence)
        })
        .collect();
    snapshot.report.nested_artifacts = new_report_nested;
    snapshot.store_interiors = new_store_interiors;
}

/// R16 item 3: replaces every entity's `evidence`/`decision_evidence`
/// with `evidence.parquet`'s rows for its row key, run last so it sees
/// the final project/unit/nested-artifact lists the earlier rebuilds
/// just produced. Unlike the other R16 tables this is never a no-op on
/// an older store by row *count* (a store with the table but nothing
/// evidenced yet has zero rows too) -- `evidence_table_exists` is the
/// actual "has this store ever written this table" check.
fn rebuild_evidence_from_tables(store_dir: &Path, key: &str, snapshot: &mut ReportSnapshot) {
    if !crate::growth::evidence_table_exists(store_dir) {
        return;
    }
    let by_key = crate::growth::read_evidence_table(store_dir, key);
    for p in &mut snapshot.report.projects {
        for wt in &mut p.worktrees {
            let worktree_path = wt.path.clone();
            for a in &mut wt.artifacts {
                let artifact_key = crate::growth::artifact_row_key(
                    &p.project_id,
                    &wt.worktree_id,
                    &worktree_path,
                    a,
                );
                let row_key = crate::growth::evidence_row_key("artifact", &artifact_key);
                a.evidence = by_key.get(&row_key).cloned().unwrap_or_default();
            }
        }
    }
    for u in &mut snapshot.external_units {
        let id = crate::growth::external_unit_table_id(
            &u.detector_id,
            crate::external::category_str(u.category),
            &u.path,
        );
        let row_key = crate::growth::evidence_row_key("external-unit", &id);
        u.evidence = by_key.get(&row_key).cloned().unwrap_or_default();
    }
    for u in &mut snapshot.agent_units {
        let row_key = crate::growth::evidence_row_key("agent-unit", &u.id);
        u.evidence = by_key.get(&row_key).cloned().unwrap_or_default();
    }
    for n in snapshot
        .report
        .nested_artifacts
        .iter_mut()
        .chain(snapshot.store_interiors.iter_mut())
    {
        let row_key = crate::growth::evidence_row_key("nested-artifact", &n.id);
        n.decision_evidence = by_key.get(&row_key).cloned().unwrap_or_default();
    }
}

/// R18a-3b: no JSON cell anywhere holds a `Report` -- this assembles the
/// whole thing from typed tables, starting from an empty [`ReportSnapshot`]
/// and running every `rebuild_*_from_tables` function in the same order
/// as before (each one either fills in its own fields or is a no-op, so
/// running them against an empty starting value is exactly as safe as
/// running them against a stored one). "Has this scope ever been
/// observed" is answered by `growth::scope_observed_at`
/// (`summary.parquet` having a row for `key`), not by `projects.parquet`
/// having one: a scope with genuinely zero discovered projects (external
/// / agent units only, no git checkouts) still gets a `summary.parquet`
/// row every full observe, but never gets a `projects.parquet` row at
/// all, which would make "zero projects" indistinguishable from "never
/// observed" -- and would leave `observed_at` at its default too, since
/// that timestamp would otherwise only ever come from a `projects.parquet`
/// row.
pub fn report_scope_from_store(
    scope: &crate::scope::EffectiveScope,
    store_dir: &Path,
) -> std::result::Result<ReportSnapshot, NoObservation> {
    let key = scope_snapshot_key(scope);
    let Some(observed_at) = crate::growth::scope_observed_at(store_dir, &key) else {
        return Err(NoObservation {
            scope_description: describe_scope_for_error(scope),
        });
    };
    let mut snapshot = ReportSnapshot {
        observed_at,
        report: Report {
            observed_at,
            root: PathBuf::new(),
            projects: Vec::new(),
            unowned: Vec::new(),
            reconciliation: Reconciliation {
                unique_estimate: None,
                attributed: 0,
                unowned: 0,
                walked_total: 0,
                du_total: None,
                docker_attributed: 0,
                docker_unowned: 0,
            },
            series_by_key: std::collections::HashMap::new(),
            total_series: Vec::new(),
            series_window_secs: 0,
            notes: Vec::new(),
            dirs_by_worktree: None,
            files_by_worktree: None,
            schedule_line: None,
            summary: Summary::default(),
            github_enrichment: None,
            nested_artifacts: Vec::new(),
            // The store this call read from -- known directly from the
            // argument, never something that needs to survive a store
            // round trip (`.oh/guardrails/protection-fails-closed.md`:
            // `propose` reloads live state from this path).
            store_dir: Some(store_dir.to_path_buf()),
        },
        coverage: Vec::new(),
        external_units: Vec::new(),
        agent_units: Vec::new(),
        store_interiors: Vec::new(),
    };
    rebuild_projects_from_tables(scope, store_dir, &key, &mut snapshot);
    rebuild_units_from_tables(store_dir, &key, &mut snapshot);
    rebuild_nested_artifacts_from_tables(store_dir, &key, &mut snapshot);
    rebuild_evidence_from_tables(store_dir, &key, &mut snapshot);
    crate::growth::rebuild_coverage_and_notes_from_tables(store_dir, &key, &mut snapshot);
    // R20: last -- every derived field (summary, series, unowned, the
    // drill-down, an artifact's growth/allocation) is computed from the
    // facts rebuilt above and the volumes' history, never read from a
    // table of its own.
    crate::growth::derive_report_views(store_dir, &key, &mut snapshot);
    Ok(snapshot)
}

mod pass;
pub use pass::DiscoveryPass;
