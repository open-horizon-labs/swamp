//! R4: growth since previous observation, from a reverse-delta store.
//!
//! Reuses the existing zstd/Parquet `Store` (see `store.rs`) rather than
//! adding a second persistence layer. Layout under
//! `${SWAMP_DIR}/<root-scope-id>/`:
//!
//! - `current.parquet`: one row per known artifact key, holding its most
//!   recently observed value (including tombstones for keys that are no
//!   longer present, so a later reappearance can be counted as regrowth).
//! - `deltas/delta-<seq>.parquet`: one file per observation that changed
//!   at least one row. Each row in a delta file holds the *previous*
//!   value (before that observation) of a row that changed. Replaying
//!   deltas backwards from `current.parquet` reconstructs any past
//!   observation within the retention window.
//!
//! A row's identity is `(project_id, worktree_id, kind, rel_path)` where
//! `rel_path` is always relative to the worktree root -- never an
//! absolute path -- so the store stays portable across machines and
//! home-directory renames.
//!
//! Compaction: once the number of delta files crosses
//! [`COMPACTION_THRESHOLD`], every delta older than the retention window
//! is dropped and the remaining deltas are merged into a single file.
//! A no-change observation (no row's bytes/presence changed) appends no
//! delta file at all.

use crate::bus::WorktreeSignals;
use crate::entities::Confidence;
use crate::fs_events::{FsEventsRequest, FsEventsState};
use crate::fs_gate::{MetadataExt, read::read_owned_string, store};
use crate::git::DiscoveredWorktree;
use crate::report::{
    ArtifactKind, ArtifactRow, DirRollup, FileRow, ProjectRow, Report, Signal, Source,
    UnownedReason, UnownedRow, WorktreeRow,
};
use crate::signals::RawSignals;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub(crate) mod columns;
pub use columns::FoldedRow;
use columns::*;

pub const DEFAULT_RETENTION_DAYS: u64 = 30;
pub const DEFAULT_SINCE: &str = "24h";
/// R4c default threshold for a standalone large-file row: 1 MiB.
pub const DEFAULT_LARGE_FILE_MIN_BYTES: u64 = 1024 * 1024;
/// Default watchdog budget for one `observe` invocation.
pub const DEFAULT_OBSERVE_TIMEOUT_SEC: u64 = crate::schedule::DEFAULT_OBSERVE_TIMEOUT_SECS;
/// Delta files beyond this count trigger compaction into a single file.
const COMPACTION_THRESHOLD: usize = 20;

fn should_compact(files: &[PathBuf]) -> bool {
    if files.len() > COMPACTION_THRESHOLD {
        return true;
    }
    // Amortize repeated schema/footer costs for tiny reverse deltas, without
    // repeatedly merging large history runs on every observation.
    files.len() >= 8
        && files
            .iter()
            .try_fold(0u64, |n, p| {
                crate::fs_gate::symlink_metadata(p).map(|m| n.saturating_add(m.len()))
            })
            .is_ok_and(|bytes| bytes <= 128 * 1024)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrowthConfig {
    pub retention_days: u64,
    pub since: String,
    pub large_file_min_bytes: u64,
    /// Watchdog budget for one `observe` invocation (item 3 of #31).
    pub observe_timeout_sec: u64,
    /// The `[scan]` table: built-in defaults, includes, excludes, and
    /// disabled detectors (#41). See `crate::scope`.
    pub scan: crate::scope::ScanConfig,
}

impl Default for GrowthConfig {
    fn default() -> Self {
        Self {
            retention_days: DEFAULT_RETENTION_DAYS,
            since: DEFAULT_SINCE.to_string(),
            large_file_min_bytes: DEFAULT_LARGE_FILE_MIN_BYTES,
            observe_timeout_sec: DEFAULT_OBSERVE_TIMEOUT_SEC,
            scan: crate::scope::ScanConfig::default(),
        }
    }
}

impl GrowthConfig {
    /// The file contents that reproduce this configuration, every key
    /// written out with its meaning, so `config init` leaves something a
    /// human can edit.
    pub fn to_toml(&self) -> String {
        format!(
            "# swamp configuration. Every key is optional; these are the effective values.\n\
# How far back growth is measured by default (\"24h\", \"7d\"); --since overrides per call.\n\
since = \"{}\"\n\
# Days of observation history kept in the store before deltas are pruned.\n\
retention_days = {}\n\
# Files at least this large are tracked individually under --dirs.\n\
large_file_min_bytes = {}\n\
# Watchdog budget for one `observe` run, in seconds.\n\
observe_timeout_sec = {}\n\
{}",
            self.since,
            self.retention_days,
            self.large_file_min_bytes,
            self.observe_timeout_sec,
            self.scan.to_toml_table(),
        )
    }
}

/// Raw `config.toml` shape for `toml::from_str`. Every field optional so
/// a config naming only a subset of keys still parses; unknown top-level
/// keys are accepted (forward-compatible), but a key with the wrong
/// *type* (a `[scan]` table where `defaults` is a string, `exclude` is
/// not an array of strings, ...) is a real parse error, not silently
/// discarded -- see `load_config_checked`.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
struct RawConfig {
    since: String,
    retention_days: u64,
    large_file_min_bytes: u64,
    observe_timeout_sec: u64,
    scan: crate::scope::ScanConfig,
}

impl Default for RawConfig {
    fn default() -> Self {
        let d = GrowthConfig::default();
        Self {
            since: d.since,
            retention_days: d.retention_days,
            large_file_min_bytes: d.large_file_min_bytes,
            observe_timeout_sec: d.observe_timeout_sec,
            scan: d.scan,
        }
    }
}

impl From<RawConfig> for GrowthConfig {
    fn from(r: RawConfig) -> Self {
        Self {
            since: r.since,
            retention_days: r.retention_days,
            large_file_min_bytes: r.large_file_min_bytes,
            observe_timeout_sec: r.observe_timeout_sec,
            scan: r.scan,
        }
    }
}

/// Reads and validates `<swamp_dir>/config.toml` with a real TOML
/// parser. A missing file is `Ok(GrowthConfig::default())` -- absent is
/// not invalid. A file that exists but fails to parse (bad TOML syntax,
/// or a `[scan]` field with the wrong type, e.g. `defaults = "yes"`
/// instead of a bool) is `Err`: callers that determine scan scope from
/// this must refuse to run rather than silently falling back to
/// (broader) defaults. See `load_config` for the infallible variant used
/// deep in the report pipeline, which only ever reads the four scalar
/// keys and tolerates a malformed file the same way it always has.
pub fn load_config_checked(swamp_dir: &Path) -> Result<GrowthConfig> {
    let path = swamp_dir.join("config.toml");
    let text = match read_owned_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(GrowthConfig::default()),
        Err(e) => return Err(e).context(format!("reading {}", path.display())),
    };
    let raw: RawConfig =
        toml::from_str(&text).with_context(|| format!("invalid config at {}", path.display()))?;
    Ok(raw.into())
}

/// Infallible convenience wrapper around [`load_config_checked`] for the
/// report pipeline's internal, scalar-only readers (retention/since/
/// large-file-min-bytes/observe-timeout): a malformed file falls back to
/// defaults for these settings exactly as before real-TOML parsing was
/// added. Scope-resolving call sites (the CLI's `scope`/`report`/
/// `observe`/`ui`/`schedule` commands and `config show`/`init`) must use
/// [`load_config_checked`] instead so invalid `[scan]` config is a
/// visible, nonzero-exit error rather than a silently broadened scope
/// (#41's core requirement) -- see `crates/cli/src/main.rs`.
pub fn load_config(swamp_dir: &Path) -> GrowthConfig {
    load_config_checked(swamp_dir).unwrap_or_default()
}

/// Parses a duration like `"24h"`, `"30d"`, `"10m"`, `"45s"`, or a bare
/// number of seconds, into seconds.
pub fn parse_duration_secs(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(n) = s.parse::<u64>() {
        return Some(n);
    }
    let (num, unit) = s.split_at(s.len() - 1);
    let n: u64 = num.parse().ok()?;
    match unit {
        "s" => Some(n),
        "m" => Some(n * 60),
        "h" => Some(n * 3600),
        "d" => Some(n * 86400),
        _ => None,
    }
}

/// One row's storage identity: never an absolute path.
fn row_key(project_id: &str, worktree_id: &str, kind: &str, rel_path: &str) -> String {
    if kind.starts_with("Nested:") {
        return kind.to_string();
    }
    format!("{project_id}\u{1}{worktree_id}\u{1}{kind}\u{1}{rel_path}")
}

fn volume_dir(swamp_dir: &Path, volume_id: u64) -> PathBuf {
    swamp_dir.join(volume_id.to_string())
}

/// Returns the stable store key for one observed root.
///
/// A device is not a sufficient scope: several checkouts (and linked
/// worktrees) commonly share one volume.  The canonical root makes aliases
/// such as `/tmp/work` and `/private/tmp/work` share history while keeping
/// sibling roots independent.  Hashing keeps the existing compact directory
/// layout and avoids putting user paths into the store name.
pub fn root_scoped_volume_id(root: &Path) -> u64 {
    let canonical = crate::fs_gate::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let device = crate::fs_gate::metadata_following(&canonical)
        .map(|m| m.dev())
        .unwrap_or_default();
    let mut hasher = blake3::Hasher::new();
    hasher.update(&device.to_le_bytes());
    // The canonical path is identity data, not display text. Lossy UTF-8
    // conversion can collapse distinct non-UTF-8 roots into one store.
    hasher.update(canonical.as_os_str().as_bytes());
    let digest = hasher.finalize();
    u64::from_le_bytes(
        digest.as_bytes()[..8]
            .try_into()
            .expect("blake3 digest is 32 bytes"),
    )
}
/// The stable per-root key `git_signals.parquet`/`cargo_replay_cache*.parquet`
/// partition on: the canonicalized root's id, the same key space
/// `report::last_report_key` used before R18a-4 deleted it (this is its
/// direct replacement -- moved here because every other current-state
/// table's key derivation lives in `growth`, not `report`). Deliberately
/// *not* [`root_scoped_volume_id`]: that one folds in the device, to
/// name a distinct on-disk subdirectory per volume, which these tables
/// do not need (they are ordinary rows filtered by a string column, not
/// directories); adding the device here would also make an alias whose
/// device id changes (a remounted volume) needlessly lose its replay
/// cache, which the path-only key does not.
pub fn root_key(root: &Path) -> String {
    let root = crate::fs_gate::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    crate::entities::id_for(&root.display().to_string())[..16].to_string()
}

fn current_path(dir: &Path) -> PathBuf {
    dir.join("current.parquet")
}
fn deltas_dir(dir: &Path) -> PathBuf {
    dir.join("deltas")
}

fn list_delta_files(dir: &Path) -> Vec<PathBuf> {
    list_files_in(&deltas_dir(dir))
}

fn next_delta_path(dir: &Path) -> PathBuf {
    let existing = list_delta_files(dir);
    let next_seq = existing
        .iter()
        .filter_map(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.strip_prefix("delta-"))
                .and_then(|s| s.parse::<u64>().ok())
        })
        .max()
        .map(|n| n + 1)
        .unwrap_or(0);
    deltas_dir(dir).join(format!("delta-{next_seq:012}.parquet"))
}

/// One observation's artifact facts, flattened out of the report's
/// project/worktree tree, keyed by row identity.
struct Observed {
    key: String,
    project_id: String,
    worktree_id: String,
    kind: String,
    rel_path: String,
    bytes: u64,
    local_bytes: u64,
    mtime_max: u64,
    hardlinked: bool,
    dedup_stale: bool,
    ecosystem: Option<String>,
}

fn flatten(projects: &[ProjectRow]) -> Vec<Observed> {
    let mut out = Vec::new();
    for project in projects {
        for worktree in &project.worktrees {
            for artifact in &worktree.artifacts {
                let rel_path = artifact
                    .path
                    .strip_prefix(&worktree.path)
                    .unwrap_or(&artifact.path)
                    .to_path_buf();
                let kind = observed_kind(artifact);
                let rel_path_str = rel_path.display().to_string();
                out.push(Observed {
                    key: row_key(
                        &project.project_id,
                        &worktree.worktree_id,
                        &kind,
                        &rel_path_str,
                    ),
                    project_id: project.project_id.clone(),
                    worktree_id: worktree.worktree_id.clone(),
                    kind,
                    mtime_max: artifact.mtime_max,
                    hardlinked: artifact.hardlinked,
                    dedup_stale: artifact.dedup_stale,
                    rel_path: rel_path_str,
                    bytes: artifact.bytes,
                    ecosystem: artifact.ecosystem.clone(),
                    local_bytes: if artifact.local_bytes == 0
                        && artifact.source.tool != "cargo.layout"
                    {
                        artifact.bytes
                    } else {
                        artifact.local_bytes
                    },
                });
            }
        }
    }
    out
}

fn observed_kind(artifact: &ArtifactRow) -> String {
    if artifact.source.tool == "cargo.layout" {
        artifact
            .note
            .as_deref()
            .and_then(|n| n.strip_prefix("nested-id="))
            .map(|id| format!("Nested:{id}"))
            .unwrap_or_else(|| format!("{:?}", artifact.kind))
    } else {
        format!("{:?}", artifact.kind)
    }
}

/// Read-only counterpart to [`observe_and_annotate`]: annotates each
/// artifact row in `projects` with `growth_bytes`/`regrowth_count` from
/// whatever history the store already has, without writing a new
/// observation (no `current.parquet` update, no delta file). This is
/// what `--no-observe` and any other read-only report call use: growth
/// is a property of the store's existing observations, not of whether
/// *this* call is the one adding a new one. A row with no prior
/// observation in the store keeps `growth_bytes: None`, same as the
/// first-ever `observe_and_annotate` call would leave it.
pub fn annotate_readonly(
    swamp_dir: &Path,
    volume_id: u64,
    projects: &mut [ProjectRow],
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
) -> Result<()> {
    annotate_readonly_inner(
        swamp_dir,
        volume_id,
        projects,
        observed_at,
        retention_days,
        since_secs,
        false,
    )
}

/// [`annotate_readonly`] for a read of the observation stamped
/// `observed_at` itself (`derive_report_views`): the history points
/// that observation wrote -- stamped exactly `observed_at` -- are
/// excluded, so growth is measured against what came before it, as the
/// persisting pass measured it (its index was built before it wrote).
/// Only there: a live read at `now` must keep a previous pass's rows
/// that happen to share its second (R19, Linux CI caught it).
pub(crate) fn annotate_readonly_before(
    swamp_dir: &Path,
    volume_id: u64,
    projects: &mut [ProjectRow],
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
) -> Result<()> {
    annotate_readonly_inner(
        swamp_dir,
        volume_id,
        projects,
        observed_at,
        retention_days,
        since_secs,
        true,
    )
}

fn annotate_readonly_inner(
    swamp_dir: &Path,
    volume_id: u64,
    projects: &mut [ProjectRow],
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
    exclude_this_observation: bool,
) -> Result<()> {
    let dir = volume_dir(swamp_dir, volume_id);
    let current_file = current_path(&dir);
    if !crate::fs_gate::exists(&current_file) {
        // Store has never been observed for this volume; nothing to
        // annotate from.
        return Ok(());
    }
    let current_rows = read_rows(&current_file)?;
    let current_by_key: HashMap<String, &StoredRow> = current_rows
        .iter()
        .map(|r| {
            (
                row_key(r.project_id(), r.worktree_id(), r.kind(), r.rel_path()),
                r,
            )
        })
        .collect();

    let target_time = observed_at.saturating_sub(since_secs);
    // One pass over current + deltas for every key, not one pass per
    // artifact (that was ~300 × 4 Parquet reads per observation).
    let mut history_index = build_history_index(&dir, retention_days, observed_at)?;
    if exclude_this_observation {
        for points in history_index.values_mut() {
            points.retain(|(t, _, _)| *t < observed_at);
        }
    }
    let empty: Vec<(u64, u64, bool)> = Vec::new();
    for project in projects.iter_mut() {
        for worktree in project.worktrees.iter_mut() {
            for artifact in worktree.artifacts.iter_mut() {
                let rel_path = artifact
                    .path
                    .strip_prefix(&worktree.path)
                    .unwrap_or(&artifact.path)
                    .to_path_buf();
                let kind = observed_kind(artifact);
                let key = row_key(
                    &project.project_id,
                    &worktree.worktree_id,
                    &kind,
                    &rel_path.display().to_string(),
                );
                let history = history_index.get(&key).unwrap_or(&empty);
                artifact.growth_bytes = (!artifact.dedup_stale)
                    .then(|| growth_since(history, artifact.bytes, target_time))
                    .flatten();
                artifact.regrowth_count = current_by_key
                    .get(&key)
                    .map(|r| r.regrowth_count())
                    .unwrap_or(0);
            }
        }
    }
    Ok(())
}

/// Persists this observation's rows (current-state + delta) and
/// annotates each artifact row in `projects` with `growth_bytes` (since
/// `since_secs` ago) and `regrowth_count`.
///
/// `swamp_dir` is the top-level store root (e.g.
/// `${SWAMP_DIR}`); the caller supplies the root-scope key in `volume_id`.
///
/// `protected_worktree_ids` (#42) names worktree ids this observation
/// could not confirm one way or the other -- typically because access to
/// the worktree's path was lost between observations (see
/// `compute_unconfirmed_worktrees`). A row belonging to one of these
/// worktree ids is never tombstoned by this call even though it is
/// absent from `projects`: absence here means "not observed", not
/// "deleted". See `.oh/guardrails/coverage-changes-are-not-storage-changes.md`.
#[allow(clippy::too_many_arguments)]
pub fn observe_and_annotate(
    _stage: &crate::bus::Stage,
    swamp_dir: &Path,
    volume_id: u64,
    projects: &mut [ProjectRow],
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
    protected_worktree_ids: &HashSet<String>,
) -> Result<()> {
    let dir = volume_dir(swamp_dir, volume_id);
    crate::fs_gate::store::StoreDir::at(&dir)?.create()?;
    let mut history = ArtifactHistory::load(&dir)?;

    let observed = flatten(projects);
    let mut seen_keys: HashSet<String> = HashSet::new();
    for obs in &observed {
        seen_keys.insert(obs.key.clone());
        history.observe(obs, observed_at);
    }

    // Rows present before, absent now: tombstone them (kept in the
    // store so a later reappearance counts as regrowth), but never
    // emitted as report rows in this issue. A row whose worktree could
    // not be confirmed this pass (#42) is not claimable: its absence
    // from `seen_keys` reflects lost access, not deletion.
    let ownership = ArtifactOwnership {
        unconfirmed_worktrees: protected_worktree_ids,
    };
    for key in history.unseen_present(&seen_keys) {
        if let Some(owned) = ownership.claim(&history, &key) {
            history.tombstone(owned, observed_at);
        }
    }

    // Compute growth/regrowth for the artifacts in *this* report before
    // writing, using the pre-write history (current file on disk plus
    // any not-yet-written delta files already on disk). One index for
    // every key, not one pass over the Parquet files per artifact.
    let target_time = observed_at.saturating_sub(since_secs);
    let history_index = build_history_index(&dir, retention_days, observed_at)?;
    for project in projects.iter_mut() {
        for worktree in project.worktrees.iter_mut() {
            for artifact in worktree.artifacts.iter_mut() {
                let rel_path = artifact
                    .path
                    .strip_prefix(&worktree.path)
                    .unwrap_or(&artifact.path)
                    .to_path_buf();
                let kind = observed_kind(artifact);
                let key = row_key(
                    &project.project_id,
                    &worktree.worktree_id,
                    &kind,
                    &rel_path.display().to_string(),
                );
                let past = history_index.get(&key).cloned().unwrap_or_default();
                artifact.growth_bytes = (!artifact.dedup_stale)
                    .then(|| growth_since(&past, artifact.bytes, target_time))
                    .flatten();
                artifact.regrowth_count =
                    history.row(&key).map(|r| r.regrowth_count()).unwrap_or(0);
            }
        }
    }

    history.commit()?;

    compact_if_needed(&dir, retention_days, observed_at)?;

    Ok(())
}

/// One historical snapshot of a row's value: `(observed_at, bytes,
/// present)`, oldest first, ending with the value on disk right now
/// (before this observation's write).
/// `(observed_at, bytes, measurement_usable)` history for every key in the store,
/// from the current file plus every delta within retention, sorted by
/// time. Built once per observation.
// (observation time, last measured bytes, measurement usable at that time).
type HistoryIndex = HashMap<String, Vec<(u64, u64, bool)>>;

fn build_history_index(dir: &Path, retention_days: u64, now: u64) -> Result<HistoryIndex> {
    let retention_secs = retention_days.saturating_mul(86400);
    let horizon = now.saturating_sub(retention_secs);
    let mut index: HashMap<String, Vec<(u64, u64, bool)>> = HashMap::new();
    for row in read_rows(&current_path(dir))? {
        index
            .entry(row_key(
                row.project_id(),
                row.worktree_id(),
                row.kind(),
                row.rel_path(),
            ))
            .or_default()
            .push((
                row.observed_at(),
                row.bytes(),
                !row.present() || !row.dedup_stale(),
            ));
    }
    for delta_path in list_delta_files(dir) {
        for row in read_rows(&delta_path)? {
            if row.observed_at() < horizon {
                continue;
            }
            index
                .entry(row_key(
                    row.project_id(),
                    row.worktree_id(),
                    row.kind(),
                    row.rel_path(),
                ))
                .or_default()
                .push((
                    row.observed_at(),
                    row.bytes(),
                    !row.present() || !row.dedup_stale(),
                ));
        }
    }
    for values in index.values_mut() {
        values.sort_by_key(|(t, _, _)| *t);
    }
    Ok(index)
}

/// `growth_bytes` = bytes now minus bytes at the observation closest to
/// `target_time`, or `None` when no prior observation exists at all.
fn growth_since(history: &[(u64, u64, bool)], bytes_now: u64, target_time: u64) -> Option<i64> {
    if history.is_empty() {
        return None;
    }
    let closest = history
        .iter()
        .min_by_key(|(t, _, _)| t.abs_diff(target_time))?;
    closest.2.then_some(bytes_now as i64 - closest.1 as i64)
}

/// Merges every delta file into one, dropping deltas older than the
/// retention window, once the delta file count crosses
/// [`COMPACTION_THRESHOLD`].
fn compact_if_needed(dir: &Path, retention_days: u64, now: u64) -> Result<()> {
    let files = list_delta_files(dir);
    if !should_compact(&files) {
        return Ok(());
    }
    let retention_secs = retention_days.saturating_mul(86400);
    let horizon = now.saturating_sub(retention_secs);
    compact_artifact_deltas(dir, &files, horizon)
}

/// Prunes delta files that fall entirely outside the retention window,
/// without waiting for the compaction threshold. Exposed for callers
/// (or a future maintenance command) that want retention enforced on
/// every observation regardless of file count.
/// The oldest observation the store can still answer from, in seconds
/// before `now`: the earliest `observed_at` across the delta log and the
/// current state. `None` when the store holds no observation.
///
/// A growth window longer than this span cannot be honored — the tool
/// would be reporting "growth over a year" from three days of history —
/// so surfaces clamp their offered windows to it and say what they have.
pub fn history_span_secs(dir: &Path, now: u64) -> Option<u64> {
    let mut oldest: Option<u64> = None;
    let mut consider = |rows: Vec<StoredRow>| {
        for r in rows {
            oldest = Some(oldest.map_or(r.observed_at(), |o: u64| o.min(r.observed_at())));
        }
    };
    if let Ok(rows) = read_rows(&current_path(dir)) {
        consider(rows);
    }
    for f in list_delta_files(dir) {
        if let Ok(rows) = read_rows(&f) {
            consider(rows);
        }
    }
    oldest.map(|o| now.saturating_sub(o))
}

/// `history_span_secs` for `root`, so every surface bounds its growth windows
/// identically without mixing roots on the same device.
/// One bucketed byte history: `None` before the first observation.
pub type Series = Vec<Option<u64>>;

/// Byte history per artifact row as a step series sampled at `buckets`
/// evenly spaced times over the last `window_secs`, read straight from
/// the reverse-delta log: the current row gives the latest value, each
/// delta row the value that held until the next observation. A bucket
/// before the row's first observation is `None` (not yet observed — not
/// zero); a row recorded absent (`present == false`) is `Some(0)`. Also
/// returns the total over all rows per bucket, `None` where nothing at
/// all had been observed yet. This is what a sparkline draws — no
/// rescan, just the store.
pub fn history_series(
    dir: &Path,
    window_secs: u64,
    buckets: usize,
    now: u64,
) -> (HashMap<String, Series>, Series) {
    let buckets = buckets.max(2);
    let mut points: HashMap<String, Vec<(u64, Option<u64>)>> = HashMap::new();
    let mut push = |rows: Vec<StoredRow>| {
        for r in rows {
            let key = row_key(r.project_id(), r.worktree_id(), r.kind(), r.rel_path());
            let bytes = if !r.present() {
                Some(0)
            } else if r.dedup_stale() {
                None
            } else {
                Some(r.bytes())
            };
            points
                .entry(key)
                .or_default()
                .push((r.observed_at(), bytes));
        }
    };
    for f in list_delta_files(dir) {
        if let Ok(rows) = read_rows(&f) {
            push(rows);
        }
    }
    // Current wins ties when several observations share a second.
    if let Ok(rows) = read_rows(&current_path(dir)) {
        push(rows);
    }
    let start = now.saturating_sub(window_secs);
    let step = (window_secs.max(1) as f64) / ((buckets - 1) as f64);
    let times: Vec<u64> = (0..buckets)
        .map(|i| start + (i as f64 * step).round() as u64)
        .collect();
    let mut series: HashMap<String, Series> = HashMap::with_capacity(points.len());
    let mut total: Series = vec![None; buckets];
    let mut stale_total = vec![false; buckets];
    for (key, mut pts) in points {
        pts.sort_by_key(|(t, _)| *t);
        let mut out = Vec::with_capacity(buckets);
        for (i, t) in times.iter().enumerate() {
            let point = pts.iter().rev().find(|(pt, _)| pt <= t);
            let v = point.and_then(|(_, b)| *b);
            if point.is_some() && v.is_none() && !key.starts_with("Nested:") {
                stale_total[i] = true;
            }
            out.push(v);
            if let Some(v) = v
                && !key.starts_with("Nested:")
            {
                total[i] = Some(total[i].unwrap_or(0) + v);
            }
        }
        series.insert(key, out);
    }
    for (i, stale) in stale_total.into_iter().enumerate() {
        if stale {
            total[i] = None;
        }
    }
    (series, total)
}

/// The store key for a report row, so surfaces can look up its series.
pub fn series_key(project_id: &str, worktree_id: &str, kind: &str, rel_path: &str) -> String {
    row_key(project_id, worktree_id, kind, rel_path)
}

/// The root-scoped store directory for `root`.
pub fn volume_store_dir(swamp_dir: &Path, root: &Path) -> PathBuf {
    volume_dir(swamp_dir, root_scoped_volume_id(root))
}

pub fn history_span_for_root(store: &Path, root: &Path, now: u64) -> Option<u64> {
    history_span_secs(&store.join(root_scoped_volume_id(root).to_string()), now)
}

// ---------------------------------------------------------------------
// R4c: dirs.parquet / files.parquet -- same current + reverse-delta
// layout as the artifact rows above, keyed by (worktree_id, rel_path)
// instead of (project_id, worktree_id, kind, rel_path). The current file
// is written zstd-9 (it is read on every observation and rewritten in
// full); delta files are written zstd-3 (cheap to append, most are
// pruned well before compaction).
// ---------------------------------------------------------------------

/// The artifacts store is small and read on every observation, so it
/// gets the same level the directory base file does.
const ARTIFACT_ZSTD_LEVEL: i32 = 9;
const DIR_BASE_ZSTD_LEVEL: i32 = 9;
const DIR_DELTA_ZSTD_LEVEL: i32 = 3;

fn list_files_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = crate::fs_gate::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "parquet"))
        .filter(|p| crate::fs_gate::is_file(p))
        .collect();
    files.sort();
    files
}

fn next_seq_path(dir: &Path, prefix: &str) -> PathBuf {
    let existing = list_files_in(dir);
    let next_seq = existing
        .iter()
        .filter_map(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.strip_prefix(prefix))
                .and_then(|s| s.parse::<u64>().ok())
        })
        .max()
        .map(|n| n + 1)
        .unwrap_or(0);
    dir.join(format!("{prefix}{next_seq:012}.parquet"))
}

// --- dirs.parquet ---

fn dir_row_key(worktree_id: &str, rel_path: &str) -> String {
    format!("{worktree_id}\u{1}{rel_path}")
}

fn dirs_current_path(dir: &Path) -> PathBuf {
    dir.join("dirs.parquet")
}
fn dirs_deltas_dir(dir: &Path) -> PathBuf {
    dir.join("dirs_deltas")
}

/// Builds a `key -> [(observed_at, value)]` index over every dir row's
/// history in one pass (current file + every delta file read exactly
/// once), instead of the naive per-row approach of re-reading every file
/// on disk for every single directory. On a tree with tens of thousands
/// of directories, re-reading is the difference between a few file reads
/// and tens of thousands: this index is what keeps a second observation
/// close to the first observation's wall time.
fn build_dir_history_index(
    dir: &Path,
    current: &HashMap<String, StoredDirRow>,
    retention_days: u64,
    now: u64,
) -> Result<HashMap<String, Vec<(u64, u64)>>> {
    let retention_secs = retention_days.saturating_mul(86400);
    let horizon = now.saturating_sub(retention_secs);
    let mut index: HashMap<String, Vec<(u64, u64)>> = HashMap::new();
    for (key, row) in current {
        index
            .entry(key.clone())
            .or_default()
            .push((row.observed_at, row.allocated_total));
    }
    for delta_path in list_files_in(&dirs_deltas_dir(dir)) {
        for row in read_dir_rows(&delta_path)? {
            if row.observed_at < horizon {
                continue;
            }
            let key = dir_row_key(&row.worktree_id, &row.rel_path);
            index
                .entry(key)
                .or_default()
                .push((row.observed_at, row.allocated_total));
        }
    }
    for values in index.values_mut() {
        values.sort_by_key(|(t, _)| *t);
    }
    Ok(index)
}

fn growth_since_u64(history: &[(u64, u64)], now_val: u64, target_time: u64) -> Option<i64> {
    let closest = history
        .iter()
        .min_by_key(|(t, _)| t.abs_diff(target_time))?;
    Some(now_val as i64 - closest.1 as i64)
}

/// Read-only counterpart to [`observe_and_annotate_dirs`], mirroring
/// [`annotate_readonly`] for artifacts: annotates `growth_bytes` from
/// whatever the store already has, without writing a new observation.
pub fn annotate_readonly_dirs(
    swamp_dir: &Path,
    volume_id: u64,
    dirs: &mut [crate::report::DirRollup],
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
) -> Result<()> {
    annotate_readonly_dirs_inner(
        swamp_dir,
        volume_id,
        dirs,
        observed_at,
        retention_days,
        since_secs,
        false,
    )
}

/// See [`annotate_readonly_before`].
pub(crate) fn annotate_readonly_dirs_before(
    swamp_dir: &Path,
    volume_id: u64,
    dirs: &mut [crate::report::DirRollup],
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
) -> Result<()> {
    annotate_readonly_dirs_inner(
        swamp_dir,
        volume_id,
        dirs,
        observed_at,
        retention_days,
        since_secs,
        true,
    )
}

fn annotate_readonly_dirs_inner(
    swamp_dir: &Path,
    volume_id: u64,
    dirs: &mut [crate::report::DirRollup],
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
    exclude_this_observation: bool,
) -> Result<()> {
    let dir = volume_dir(swamp_dir, volume_id);
    let current_file = dirs_current_path(&dir);
    if !crate::fs_gate::exists(&current_file) {
        return Ok(());
    }
    let current: HashMap<String, StoredDirRow> = read_dir_rows(&current_file)?
        .into_iter()
        .map(|r| (dir_row_key(&r.worktree_id, &r.rel_path), r))
        .collect();
    let mut history_index = build_dir_history_index(&dir, &current, retention_days, observed_at)?;
    if exclude_this_observation {
        for points in history_index.values_mut() {
            points.retain(|(t, _)| *t < observed_at);
        }
    }
    let empty_history: Vec<(u64, u64)> = Vec::new();
    let target_time = observed_at.saturating_sub(since_secs);
    for row in dirs.iter_mut() {
        let key = dir_row_key(&row.worktree_id, &row.rel_path);
        let history = history_index.get(&key).unwrap_or(&empty_history);
        row.growth_bytes = growth_since_u64(history, row.allocated_total, target_time);
    }
    Ok(())
}

/// Persists this observation's dir rows (current + reverse-delta) and
/// annotates each `DirRollup` with `growth_bytes` (since `since_secs`
/// ago), the same shape as [`observe_and_annotate`] for artifacts. A
/// no-change row appends no delta, matching the artifact store's
/// contract.
pub fn observe_and_annotate_dirs(
    _stage: &crate::bus::Stage,
    swamp_dir: &Path,
    volume_id: u64,
    dirs: &mut [crate::report::DirRollup],
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
) -> Result<()> {
    let dir = volume_dir(swamp_dir, volume_id);
    store::StoreDir::at(&dir)?.create()?;
    let current_file = dirs_current_path(&dir);

    let trace = std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty());
    let t = std::time::Instant::now();
    let mut current: HashMap<String, StoredDirRow> = read_dir_rows(&current_file)?
        .into_iter()
        .map(|r| (dir_row_key(&r.worktree_id, &r.rel_path), r))
        .collect();

    if trace {
        eprintln!(
            "[trace]   dirs: read current ({} rows): {:?}",
            current.len(),
            t.elapsed()
        );
    }
    let t = std::time::Instant::now();
    let history_index = build_dir_history_index(&dir, &current, retention_days, observed_at)?;
    if trace {
        eprintln!("[trace]   dirs: history index: {:?}", t.elapsed());
    }
    let t = std::time::Instant::now();
    let empty_history: Vec<(u64, u64)> = Vec::new();

    // Rewriting the whole current file (zstd-9, tens of thousands of
    // rows) is the dominant cost of an observation where almost nothing
    // moved. Skip it when no row changed, was added, or went absent.
    let mut current_changed = false;
    let mut delta_rows: Vec<StoredDirRow> = Vec::new();
    let target_time = observed_at.saturating_sub(since_secs);

    for row in dirs.iter_mut() {
        let key = dir_row_key(&row.worktree_id, &row.rel_path);
        let history = history_index.get(&key).unwrap_or(&empty_history);
        row.growth_bytes = growth_since_u64(history, row.allocated_total, target_time);

        match current.get_mut(&key) {
            Some(prev) => {
                let changed = prev.allocated_total != row.allocated_total
                    || prev.own_allocated != row.own_allocated
                    || prev.file_count != row.file_count
                    || prev.entry_count != row.entry_count
                    || prev.symlink_count != row.symlink_count
                    || prev.mod_time_min != row.mod_time_min
                    || prev.complete != row.complete;
                if changed {
                    current_changed = true;
                    delta_rows.push(prev.clone());
                    prev.allocated_total = row.allocated_total;
                    prev.own_allocated = row.own_allocated;
                    prev.file_count = row.file_count;
                    prev.entry_count = row.entry_count;
                    prev.symlink_count = row.symlink_count;
                    prev.mod_time_min = row.mod_time_min;
                    prev.complete = row.complete;
                    prev.observed_at = observed_at;
                }
            }
            None => {
                current_changed = true;
                current.insert(
                    key,
                    StoredDirRow {
                        worktree_id: row.worktree_id.clone(),
                        rel_path: row.rel_path.clone(),
                        parent_rel_path: row.parent_rel_path.clone(),
                        allocated_total: row.allocated_total,
                        own_allocated: row.own_allocated,
                        file_count: row.file_count,
                        entry_count: row.entry_count,
                        symlink_count: row.symlink_count,
                        mod_time_min: row.mod_time_min,
                        complete: row.complete,
                        observed_at,
                    },
                );
            }
        }
    }

    if !delta_rows.is_empty() {
        let delta_path = next_seq_path(&dirs_deltas_dir(&dir), "delta-");
        write_dir_rows(&delta_path, &delta_rows, DIR_DELTA_ZSTD_LEVEL)?;
    }

    let mut current_rows: Vec<StoredDirRow> = current.into_values().collect();
    current_rows.sort_by(|a, b| (&a.worktree_id, &a.rel_path).cmp(&(&b.worktree_id, &b.rel_path)));
    if trace {
        eprintln!("[trace]   dirs: diff + annotate: {:?}", t.elapsed());
    }
    let t = std::time::Instant::now();
    if current_changed {
        write_dir_rows(&current_file, &current_rows, DIR_BASE_ZSTD_LEVEL)?;
    }
    if trace {
        eprintln!(
            "[trace]   dirs: write current (zstd-{DIR_BASE_ZSTD_LEVEL}): {:?}",
            t.elapsed()
        );
    }
    let t = std::time::Instant::now();

    compact_dir_deltas_if_needed(&dir, retention_days, observed_at)?;
    if trace {
        eprintln!("[trace]   dirs: compact: {:?}", t.elapsed());
    }
    Ok(())
}

fn compact_dir_deltas_if_needed(dir: &Path, retention_days: u64, now: u64) -> Result<()> {
    let deltas_dir_path = dirs_deltas_dir(dir);
    let files = list_files_in(&deltas_dir_path);
    if !should_compact(&files) {
        return Ok(());
    }
    let retention_secs = retention_days.saturating_mul(86400);
    let horizon = now.saturating_sub(retention_secs);
    let mut merged = Vec::new();
    for path in &files {
        for row in read_dir_rows(path)? {
            if row.observed_at >= horizon {
                merged.push(row);
            }
        }
    }
    if !merged.is_empty() {
        merged.sort_by(|a, b| {
            (&a.worktree_id, &a.rel_path, a.observed_at).cmp(&(
                &b.worktree_id,
                &b.rel_path,
                b.observed_at,
            ))
        });
        write_dir_rows(
            &next_seq_path(&deltas_dir_path, "delta-"),
            &merged,
            DIR_DELTA_ZSTD_LEVEL,
        )?;
    }
    for path in &files {
        crate::fs_gate::columns::retire(path)?;
    }
    Ok(())
}

// --- files.parquet ---

fn file_row_key(worktree_id: &str, rel_path: &str) -> String {
    format!("{worktree_id}\u{1}{rel_path}")
}

fn files_current_path(dir: &Path) -> PathBuf {
    dir.join("files.parquet")
}
fn files_deltas_dir(dir: &Path) -> PathBuf {
    dir.join("files_deltas")
}

/// Same one-pass approach as [`build_dir_history_index`], for file rows.
fn build_file_history_index(
    dir: &Path,
    current: &HashMap<String, StoredFileRow>,
    retention_days: u64,
    now: u64,
) -> Result<HashMap<String, Vec<(u64, u64)>>> {
    let retention_secs = retention_days.saturating_mul(86400);
    let horizon = now.saturating_sub(retention_secs);
    let mut index: HashMap<String, Vec<(u64, u64)>> = HashMap::new();
    for (key, row) in current {
        index
            .entry(key.clone())
            .or_default()
            .push((row.observed_at, row.allocated));
    }
    for delta_path in list_files_in(&files_deltas_dir(dir)) {
        for row in read_file_rows(&delta_path)? {
            if row.observed_at < horizon {
                continue;
            }
            let key = file_row_key(&row.worktree_id, &row.rel_path);
            index
                .entry(key)
                .or_default()
                .push((row.observed_at, row.allocated));
        }
    }
    for values in index.values_mut() {
        values.sort_by_key(|(t, _)| *t);
    }
    Ok(index)
}

/// Read-only counterpart to [`observe_and_annotate_files`], mirroring
/// [`annotate_readonly_dirs`] for large-file rows.
pub fn annotate_readonly_files(
    swamp_dir: &Path,
    volume_id: u64,
    files: &mut [crate::report::FileRow],
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
) -> Result<()> {
    annotate_readonly_files_inner(
        swamp_dir,
        volume_id,
        files,
        observed_at,
        retention_days,
        since_secs,
        false,
    )
}

/// See [`annotate_readonly_before`].
pub(crate) fn annotate_readonly_files_before(
    swamp_dir: &Path,
    volume_id: u64,
    files: &mut [crate::report::FileRow],
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
) -> Result<()> {
    annotate_readonly_files_inner(
        swamp_dir,
        volume_id,
        files,
        observed_at,
        retention_days,
        since_secs,
        true,
    )
}

fn annotate_readonly_files_inner(
    swamp_dir: &Path,
    volume_id: u64,
    files: &mut [crate::report::FileRow],
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
    exclude_this_observation: bool,
) -> Result<()> {
    let dir = volume_dir(swamp_dir, volume_id);
    let current_file = files_current_path(&dir);
    if !crate::fs_gate::exists(&current_file) {
        return Ok(());
    }
    let current: HashMap<String, StoredFileRow> = read_file_rows(&current_file)?
        .into_iter()
        .map(|r| (file_row_key(&r.worktree_id, &r.rel_path), r))
        .collect();
    let mut history_index = build_file_history_index(&dir, &current, retention_days, observed_at)?;
    if exclude_this_observation {
        for points in history_index.values_mut() {
            points.retain(|(t, _)| *t < observed_at);
        }
    }
    let empty_history: Vec<(u64, u64)> = Vec::new();
    let target_time = observed_at.saturating_sub(since_secs);
    for row in files.iter_mut() {
        let key = file_row_key(&row.worktree_id, &row.rel_path);
        let history = history_index.get(&key).unwrap_or(&empty_history);
        row.growth_bytes = growth_since_u64(history, row.allocated, target_time);
    }
    Ok(())
}

/// Same shape as [`observe_and_annotate_dirs`], for large-file rows.
pub fn observe_and_annotate_files(
    _stage: &crate::bus::Stage,
    swamp_dir: &Path,
    volume_id: u64,
    files: &mut [crate::report::FileRow],
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
) -> Result<()> {
    let dir = volume_dir(swamp_dir, volume_id);
    store::StoreDir::at(&dir)?.create()?;
    let current_file = files_current_path(&dir);

    let mut current: HashMap<String, StoredFileRow> = read_file_rows(&current_file)?
        .into_iter()
        .map(|r| (file_row_key(&r.worktree_id, &r.rel_path), r))
        .collect();

    let history_index = build_file_history_index(&dir, &current, retention_days, observed_at)?;
    let empty_history: Vec<(u64, u64)> = Vec::new();

    let mut current_changed = false;
    let mut delta_rows: Vec<StoredFileRow> = Vec::new();
    let target_time = observed_at.saturating_sub(since_secs);

    for row in files.iter_mut() {
        let key = file_row_key(&row.worktree_id, &row.rel_path);
        let history = history_index.get(&key).unwrap_or(&empty_history);
        row.growth_bytes = growth_since_u64(history, row.allocated, target_time);

        match current.get_mut(&key) {
            Some(prev) => {
                let changed =
                    prev.allocated != row.allocated || prev.mod_time_min != row.mod_time_min;
                if changed {
                    current_changed = true;
                    delta_rows.push(prev.clone());
                    prev.allocated = row.allocated;
                    prev.mod_time_min = row.mod_time_min;
                    prev.observed_at = observed_at;
                }
            }
            None => {
                current_changed = true;
                current.insert(
                    key,
                    StoredFileRow {
                        worktree_id: row.worktree_id.clone(),
                        rel_path: row.rel_path.clone(),
                        allocated: row.allocated,
                        mod_time_min: row.mod_time_min,
                        observed_at,
                    },
                );
            }
        }
    }

    if !delta_rows.is_empty() {
        let delta_path = next_seq_path(&files_deltas_dir(&dir), "delta-");
        write_file_rows(&delta_path, &delta_rows, DIR_DELTA_ZSTD_LEVEL)?;
    }

    let mut current_rows: Vec<StoredFileRow> = current.into_values().collect();
    current_rows.sort_by(|a, b| (&a.worktree_id, &a.rel_path).cmp(&(&b.worktree_id, &b.rel_path)));
    if current_changed {
        write_file_rows(&current_file, &current_rows, DIR_BASE_ZSTD_LEVEL)?;
    }

    compact_file_deltas_if_needed(&dir, retention_days, observed_at)?;
    Ok(())
}

fn compact_file_deltas_if_needed(dir: &Path, retention_days: u64, now: u64) -> Result<()> {
    let deltas_dir_path = files_deltas_dir(dir);
    let files = list_files_in(&deltas_dir_path);
    if !should_compact(&files) {
        return Ok(());
    }
    let retention_secs = retention_days.saturating_mul(86400);
    let horizon = now.saturating_sub(retention_secs);
    let mut merged = Vec::new();
    for path in &files {
        for row in read_file_rows(path)? {
            if row.observed_at >= horizon {
                merged.push(row);
            }
        }
    }
    if !merged.is_empty() {
        merged.sort_by(|a, b| {
            (&a.worktree_id, &a.rel_path, a.observed_at).cmp(&(
                &b.worktree_id,
                &b.rel_path,
                b.observed_at,
            ))
        });
        write_file_rows(
            &next_seq_path(&deltas_dir_path, "delta-"),
            &merged,
            DIR_DELTA_ZSTD_LEVEL,
        )?;
    }
    for path in &files {
        crate::fs_gate::columns::retire(path)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------
// R4b: FSEvents-driven incremental observation.
//
// Two more sidecars live in the volume dir alongside the parquet files
// above, both small JSON, both rewritten in full on every observation:
//
// - `fsevents.json`: the last observed FSEvents event id + device, so
//   the next observation knows where to replay from (`fs_events.rs`).
// - `topology.json`: the discovered checkout/worktree list (path,
//   kind, project identity) as of the last observation. Artifact/dir/
//   file *bytes* already live in the parquet files above; this sidecar
//   is the structural piece (which worktrees exist, at which paths)
//   that the parquet rows alone cannot reconstruct, since `rel_path` is
//   always relative and never carries its worktree's root back.
//
// Together they let an incremental observation skip discovery and
// attribution entirely for everything FSEvents does not implicate,
// re-walking only what changed and carrying every other row forward
// with its previously observed value untouched.
// ---------------------------------------------------------------------

/// The structural half of one discovered checkout/worktree, persisted so
/// the next observation can carry it forward without re-running
/// discovery. Bytes are never stored here; those live in the parquet
/// current-state files, keyed by `worktree_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredWorktree {
    worktree_id: String,
    project_id: String,
    project_name: String,
    path: PathBuf,
    kind: crate::report::WorktreeKind,
    remote_url: Option<String>,
}

/// `<volume>/cursors.parquet` (R18b): the FSEvents replay anchors,
/// one row per family. Replaces `fsevents.json`.
fn cursors_path(dir: &Path) -> PathBuf {
    dir.join("cursors.parquet")
}
fn topology_path(dir: &Path) -> PathBuf {
    dir.join("topology.parquet")
}

fn worktree_kind_to_label(kind: &crate::report::WorktreeKind) -> &'static str {
    match kind {
        crate::report::WorktreeKind::Main => "main",
        crate::report::WorktreeKind::Linked => "linked",
        crate::report::WorktreeKind::Clone => "clone",
    }
}

fn worktree_kind_from_label(label: &str) -> crate::report::WorktreeKind {
    match label {
        "linked" => crate::report::WorktreeKind::Linked,
        "clone" => crate::report::WorktreeKind::Clone,
        _ => crate::report::WorktreeKind::Main,
    }
}

fn read_fsevents_state(dir: &Path) -> FsEventsState {
    let mut state = FsEventsState::default();
    for r in columns::read_cursor_rows(&cursors_path(dir)).unwrap_or_default() {
        match r.family.as_str() {
            "walk" => {
                state.event_id = r.event_id;
                state.device = r.device;
                state.last_observed_at = r.observed_at;
                state.rules_version = r.rules_version.unwrap_or(0);
            }
            "unit_root" => {
                state.unit_root = Some(crate::fs_events::UnitRootCursor {
                    event_id: r.event_id,
                    device: r.device,
                    observed_at: r.observed_at,
                });
            }
            _ => {}
        }
    }
    state
}

/// The volume's replay anchors as stored (`cursors.parquet`), default
/// when there are none yet.
pub fn read_fsevents_anchor(volume_dir: &Path) -> FsEventsState {
    read_fsevents_state(volume_dir)
}

/// Replaces the volume's replay anchors wholesale with `state` -- both
/// families exactly as given. The observation paths use the two
/// merging writers below; this is for a caller that owns the whole
/// anchor (tests aging a rules version, tooling).
pub fn write_fsevents_anchor(volume_dir: &Path, state: &FsEventsState) -> Result<()> {
    write_cursor_table(volume_dir, state)
}

fn write_cursor_table(dir: &Path, state: &FsEventsState) -> Result<()> {
    store::StoreDir::at(dir)?.create()?;
    let mut rows = vec![columns::StoredCursorRow {
        family: "walk".to_string(),
        event_id: state.event_id,
        device: state.device,
        observed_at: state.last_observed_at,
        rules_version: Some(state.rules_version),
    }];
    if let Some(u) = &state.unit_root {
        rows.push(columns::StoredCursorRow {
            family: "unit_root".to_string(),
            event_id: u.event_id,
            device: u.device,
            observed_at: u.observed_at,
            rules_version: None,
        });
    }
    let path = cursors_path(dir);
    columns::write_cursor_rows(&path, &rows).with_context(|| format!("write {}", path.display()))
}

/// Publishes the walk's half of this dir's replay anchor, carrying the
/// unit-root half through untouched.
///
/// A path can be both a scan root and an authorized unit root (a tool
/// home the user put in scope), in which case the walk and
/// [`replay_unit_roots`] both own an anchor in this one file. Writing
/// `state` wholesale would let whichever finished last erase the
/// other's, and the visible symptom would be a unit that re-measures
/// every pass for no stated reason.
fn write_fsevents_state(dir: &Path, state: &FsEventsState) -> Result<()> {
    let merged = FsEventsState {
        unit_root: read_fsevents_state(dir).unit_root,
        ..state.clone()
    };
    write_cursor_table(dir, &merged)
}

/// The mirror of [`write_fsevents_state`]: publishes one unit root's
/// anchor, carrying the walk's scalars through untouched.
fn write_unit_root_cursor(dir: &Path, cursor: &crate::fs_events::UnitRootCursor) -> Result<()> {
    let merged = FsEventsState {
        unit_root: Some(cursor.clone()),
        ..read_fsevents_state(dir)
    };
    write_cursor_table(dir, &merged)
}

fn read_topology(dir: &Path) -> Option<Vec<StoredWorktree>> {
    // A persisted empty topology is a measured checkoutless root, not a
    // missing baseline. Keep it reusable on an event-covered unchanged pass.
    if !crate::fs_gate::exists(topology_path(dir)) {
        return None;
    }
    let rows = columns::read_topology_rows(&topology_path(dir)).ok()?;
    Some(
        rows.into_iter()
            .map(|r| StoredWorktree {
                worktree_id: r.worktree_id,
                project_id: r.project_id,
                project_name: r.project_name,
                path: PathBuf::from(r.path),
                kind: worktree_kind_from_label(&r.kind),
                remote_url: r.remote_url,
            })
            .collect(),
    )
}

/// Replaces `<volume>/topology.parquet` wholesale (R17 item 2 --
/// replaces `topology.json`): the structural checkout/worktree list is a
/// measurement cache like `unowned.parquet`, not history. `device` comes
/// from this volume's own `fsevents.json` walk anchor when one has been
/// written (the same device the FSEvents replay that produced this
/// topology ran against); `None` before any observation has run.
fn write_topology(dir: &Path, worktrees: &[StoredWorktree]) -> Result<()> {
    store::StoreDir::at(dir)?.create()?;
    let device = read_fsevents_state(dir).device;
    let observed_at = crate::entities::now();
    let rows: Vec<columns::StoredTopologyRow> = worktrees
        .iter()
        .map(|w| columns::StoredTopologyRow {
            worktree_id: w.worktree_id.clone(),
            project_id: w.project_id.clone(),
            project_name: w.project_name.clone(),
            path: w.path.display().to_string(),
            kind: worktree_kind_to_label(&w.kind).to_string(),
            remote_url: w.remote_url.clone(),
            device,
            observed_at,
        })
        .collect();
    columns::write_topology_rows(&topology_path(dir), &rows)
        .with_context(|| format!("write {}", topology_path(dir).display()))
}

fn unowned_reason_to_str(reason: &UnownedReason) -> &'static str {
    match reason {
        UnownedReason::OutsideAnyCheckout => "OutsideAnyCheckout",
        UnownedReason::OwnedByNothing => "OwnedByNothing",
        UnownedReason::InconclusiveEvidence => "InconclusiveEvidence",
        UnownedReason::NoContainingRepo => "NoContainingRepo",
        UnownedReason::SharedCache => "SharedCache",
        UnownedReason::PermissionDenied => "PermissionDenied",
        UnownedReason::DockerNoJoin => "DockerNoJoin",
    }
}

fn unowned_reason_from_str(s: &str) -> UnownedReason {
    match s {
        "OutsideAnyCheckout" => UnownedReason::OutsideAnyCheckout,
        "OwnedByNothing" => UnownedReason::OwnedByNothing,
        "InconclusiveEvidence" => UnownedReason::InconclusiveEvidence,
        "SharedCache" => UnownedReason::SharedCache,
        "PermissionDenied" => UnownedReason::PermissionDenied,
        "DockerNoJoin" => UnownedReason::DockerNoJoin,
        _ => UnownedReason::NoContainingRepo,
    }
}

/// Reads the volume's folded unowned/remainder rows from
/// `unowned.parquet` (#R10 item 1: a Parquet current-state table, never
/// a JSON sidecar that scales with the unowned *file* count). Returns
/// an empty vec when the table is missing or unreadable -- a cache miss,
/// rebuilt whole by the next full walk, same as `folded_rows_for`.
fn read_unowned(dir: &Path) -> Vec<UnownedRow> {
    read_unowned_family(dir, "unowned")
}

fn read_unowned_family(dir: &Path, stem: &str) -> Vec<UnownedRow> {
    try_read_unowned_family(dir, stem).unwrap_or_default()
}

fn try_read_unowned_family(dir: &Path, stem: &str) -> Result<Vec<UnownedRow>> {
    let (rows_path, lists_path, evidence_path) = unowned_family_paths(dir, stem);
    anyhow::ensure!(
        crate::fs_gate::exists(&rows_path),
        "missing unowned baseline"
    );
    let rows = columns::read_unowned_rows(&rows_path)?;
    let list_rows = columns::read_unowned_list_rows(&lists_path).unwrap_or_default();
    let mut containers_by_key: HashMap<String, Vec<&columns::StoredUnownedListRow>> =
        HashMap::new();
    let mut shared_with_by_key: HashMap<String, Vec<&columns::StoredUnownedListRow>> =
        HashMap::new();
    for r in &list_rows {
        let by_key = match r.list_kind.as_str() {
            "shared-with" => &mut shared_with_by_key,
            _ => &mut containers_by_key,
        };
        by_key.entry(r.path_or_object.clone()).or_default().push(r);
    }
    let evidence_rows = columns::read_evidence_rows(&evidence_path).unwrap_or_default();
    let mut evidence_by_key: HashMap<String, Vec<&columns::StoredEvidenceRow>> = HashMap::new();
    for r in &evidence_rows {
        evidence_by_key
            .entry(r.row_key.clone())
            .or_default()
            .push(r);
    }
    let list_values =
        |by_key: &HashMap<String, Vec<&columns::StoredUnownedListRow>>, key: &str| -> Vec<String> {
            let mut v = by_key.get(key).cloned().unwrap_or_default();
            v.sort_by_key(|r| r.seq);
            v.into_iter().map(|r| r.value.clone()).collect()
        };
    Ok(rows
        .into_iter()
        .map(|r| {
            let evidence = evidence_by_key
                .get(&r.path_or_object)
                .map(|rows| evidence_from_stored_rows(rows))
                .unwrap_or_default();
            UnownedRow {
                measurement: match r.measurement.as_deref() {
                    Some("direct") => Some(crate::report::UnownedMeasurement::Direct),
                    Some("subtree") => Some(crate::report::UnownedMeasurement::Subtree),
                    Some("hardlinked") => Some(crate::report::UnownedMeasurement::Hardlinked),
                    Some("direct-shared") => Some(crate::report::UnownedMeasurement::DirectShared),
                    Some("subtree-shared") => {
                        Some(crate::report::UnownedMeasurement::SubtreeShared)
                    }
                    Some("direct-estimate") => {
                        Some(crate::report::UnownedMeasurement::DirectEstimate)
                    }
                    Some("subtree-estimate") => {
                        Some(crate::report::UnownedMeasurement::SubtreeEstimate)
                    }
                    _ => None,
                },
                containers: list_values(&containers_by_key, &r.path_or_object),
                shared_with: list_values(&shared_with_by_key, &r.path_or_object),
                path_or_object: r.path_or_object,
                bytes: r.bytes,
                reason: unowned_reason_from_str(&r.reason),
                shared_bytes: r.shared_bytes,
                note: r.note,
                docker_kind: r.docker_kind,
                created_at: r.created_at,
                dangling: r.dangling,
                evidence,
            }
        })
        .collect())
}

/// Replaces the volume's `unowned.parquet` (+ `unowned_lists.parquet`/
/// `unowned_evidence.parquet`) wholesale: like `store_folded_rows`, this
/// is a measurement cache (no growth/regrowth semantics), so a full
/// walk's rows simply overwrite whatever was there, folded per
/// directory by `walk.rs` before this ever sees them.
fn write_unowned(dir: &Path, unowned: &[UnownedRow]) -> Result<()> {
    write_unowned_family(dir, "unowned", unowned)
}

/// R20: the `DockerNoJoin` rows the pass that walked this root's volume
/// added to its report *after* the walk's own checkpoint wrote
/// `unowned.parquet` (`consumers/gate.rs` joins the daemon's answer
/// per root). Same three-file shape as the walk's unowned rows, under
/// the `docker_unowned` stem, so `Report.unowned` derives at read time
/// as each walked root's walk rows followed by its Docker rows -- the
/// order `report::merge_root_report_into` produced.
pub fn write_docker_unowned(dir: &Path, unowned: &[UnownedRow]) -> Result<()> {
    write_unowned_family(dir, "docker_unowned", unowned)
}

fn read_docker_unowned(dir: &Path) -> Vec<UnownedRow> {
    read_unowned_family(dir, "docker_unowned")
}

fn unowned_family_paths(dir: &Path, stem: &str) -> (PathBuf, PathBuf, PathBuf) {
    (
        dir.join(format!("{stem}.parquet")),
        dir.join(format!("{stem}_lists.parquet")),
        dir.join(format!("{stem}_evidence.parquet")),
    )
}

fn write_unowned_family(dir: &Path, stem: &str, unowned: &[UnownedRow]) -> Result<()> {
    store::StoreDir::at(dir)?.create()?;
    let (rows_path, lists_path, evidence_path) = unowned_family_paths(dir, stem);
    let rows: Vec<columns::StoredUnownedRow> = unowned
        .iter()
        .map(|u| columns::StoredUnownedRow {
            measurement: u.measurement.map(|m| {
                match m {
                    crate::report::UnownedMeasurement::Direct => "direct",
                    crate::report::UnownedMeasurement::Subtree => "subtree",
                    crate::report::UnownedMeasurement::Hardlinked => "hardlinked",
                    crate::report::UnownedMeasurement::DirectShared => "direct-shared",
                    crate::report::UnownedMeasurement::SubtreeShared => "subtree-shared",
                    crate::report::UnownedMeasurement::DirectEstimate => "direct-estimate",
                    crate::report::UnownedMeasurement::SubtreeEstimate => "subtree-estimate",
                }
                .to_string()
            }),
            path_or_object: u.path_or_object.clone(),
            bytes: u.bytes,
            reason: unowned_reason_to_str(&u.reason).to_string(),
            shared_bytes: u.shared_bytes,
            note: u.note.clone(),
            docker_kind: u.docker_kind.clone(),
            created_at: u.created_at.clone(),
            dangling: u.dangling,
        })
        .collect();
    columns::write_unowned_rows(&rows_path, &rows)
        .with_context(|| format!("write {}", rows_path.display()))?;

    let mut list_rows: Vec<columns::StoredUnownedListRow> = Vec::new();
    let mut evidence_rows: Vec<columns::StoredEvidenceRow> = Vec::new();
    for u in unowned {
        for (seq, value) in u.containers.iter().enumerate() {
            list_rows.push(columns::StoredUnownedListRow {
                path_or_object: u.path_or_object.clone(),
                list_kind: "container".to_string(),
                seq: seq as u32,
                value: value.clone(),
            });
        }
        for (seq, value) in u.shared_with.iter().enumerate() {
            list_rows.push(columns::StoredUnownedListRow {
                path_or_object: u.path_or_object.clone(),
                list_kind: "shared-with".to_string(),
                seq: seq as u32,
                value: value.clone(),
            });
        }
        evidence_rows.extend(stored_evidence_rows("", &u.path_or_object, &u.evidence));
    }
    columns::write_unowned_list_rows(&lists_path, &list_rows)
        .with_context(|| format!("write {}", lists_path.display()))?;
    columns::write_evidence_rows(&evidence_path, &evidence_rows)
        .with_context(|| format!("write {}", evidence_path.display()))
}

// ---------------------------------------------------------------------
// The scope-wide JSON-encoded render cache this store used to keep is
// gone (R18a-3b: `artifact_shape.parquet` typed the last fields a
// project's worktree's artifact list needed -- see the header comment
// on `columns::StoredArtifactShapeRow`). `report::report_scope_from_store`
// assembles the whole `Report` from tables only; no cell anywhere holds
// a serialized `Report`. `ReportSnapshot` below is only ever an
// in-memory assembly of one call's result -- a return type shared by
// `report::report_scope_from_store` and `report::observe_scope`'s
// caller, never itself written to disk.
// ---------------------------------------------------------------------

/// One scope's whole rendered observation, kept exactly as
/// `report::observe_scope` produced it: the assembled [`Report`]
/// (evidence, tracking and Docker joins already attached -- nothing
/// here needs a fresh `stat` to render), its per-root coverage, and the
/// external/agent unit families discovered in the same pass. Purely an
/// in-memory container -- every field is rebuilt from its own typed
/// table by `report::report_scope_from_store`, never deserialized from
/// a stored cell.
#[derive(Debug, Clone)]
pub struct ReportSnapshot {
    pub observed_at: u64,
    pub report: Report,
    pub coverage: Vec<crate::coverage::RootCoverage>,
    pub external_units: Vec<crate::external::ExternalUnit>,
    pub agent_units: Vec<crate::agents::AgentUnit>,
    pub store_interiors: Vec<crate::artifact::NestedArtifact>,
}

// ---------------------------------------------------------------------
// coverage.parquet / series.parquet / summary.parquet / notes.parquet
// (R17 item 1 of the JSON-in-the-store decomposition -- see
// `.oh/sessions/2026-09-24-r17-tables.md`). Scope-wide, keyed by
// `scope_key` like every other scope-wide table, written by the same
// `observe_scope` call and read back by `report::report_scope_from_store`.
// ---------------------------------------------------------------------

fn coverage_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("coverage.parquet")
}
fn notes_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("notes.parquet")
}
fn runs_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("runs.parquet")
}
/// `<volume>/dir_tracks.parquet` (R20): see `columns::StoredDirTrackRow`.
fn dir_tracks_path(volume_dir: &Path) -> PathBuf {
    volume_dir.join("dir_tracks.parquet")
}

/// The bare `RegionStatus` variant tag, independent of its `reason`
/// text -- `label()` formats `Partial`/`Inaccessible` with their reason
/// inline (`"partial (...)"`), which is for display, not a round-trip
/// key; `reason` is stored in its own column instead.
fn region_status_tag(status: &crate::coverage::RegionStatus) -> &'static str {
    match status {
        crate::coverage::RegionStatus::Complete => "complete",
        crate::coverage::RegionStatus::Partial { .. } => "partial",
        crate::coverage::RegionStatus::Excluded => "excluded",
        crate::coverage::RegionStatus::Missing => "missing",
        crate::coverage::RegionStatus::Inaccessible { .. } => "inaccessible",
        crate::coverage::RegionStatus::DetectorOnly => "detector_only",
    }
}

/// Writes `coverage.parquet` for `scope_key`, replacing that scope's
/// rows wholesale from this same pass's already-computed coverage --
/// `class = "project"` for a walked scan root's [`crate::coverage::RootCoverage`],
/// `class = "detector"` for an authorized unit root's own
/// [`crate::coverage::UnitRootCoverage`].
pub fn write_coverage_table(
    swamp_dir: &Path,
    scope_key: &str,
    project_coverage: &[crate::coverage::RootCoverage],
    unit_root_coverage: &[crate::coverage::UnitRootCoverage],
    reconciliation_by_root: &HashMap<PathBuf, crate::report::Reconciliation>,
    observed_at: u64,
) -> Result<()> {
    store::StoreDir::at(swamp_dir)?.create()?;
    let path = coverage_path(swamp_dir);
    let mut rows: Vec<columns::StoredCoverageRow> = columns::read_coverage_rows(&path)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.scope_key != scope_key)
        .collect();
    for c in project_coverage {
        let rec = c
            .status
            .was_observed()
            .then(|| reconciliation_by_root.get(&c.path))
            .flatten();
        rows.push(columns::StoredCoverageRow {
            scope_key: scope_key.to_string(),
            root_path: c.path.display().to_string(),
            class: "project".to_string(),
            status: region_status_tag(&c.status).to_string(),
            reason: match &c.status {
                crate::coverage::RegionStatus::Partial { reason }
                | crate::coverage::RegionStatus::Inaccessible { reason } => Some(reason.clone()),
                _ => None,
            },
            walked_total: c.status.was_observed().then_some(c.walked_total),
            projects: c.status.was_observed().then_some(c.projects as u32),
            mode: (!c.mode.is_empty()).then(|| c.mode.clone()),
            attributed: rec.map(|r| r.attributed),
            unowned: rec.map(|r| r.unowned),
            du_total: rec.and_then(|r| r.du_total),
            docker_attributed: rec.map(|r| r.docker_attributed),
            docker_unowned: rec.map(|r| r.docker_unowned),
            cursor_family: Some("walk".to_string()),
            observed_at,
        });
    }
    for c in unit_root_coverage {
        rows.push(columns::StoredCoverageRow {
            scope_key: scope_key.to_string(),
            root_path: c.path.display().to_string(),
            class: "detector".to_string(),
            status: if c.event_covered {
                "event_covered".to_string()
            } else {
                "re_measured".to_string()
            },
            reason: Some(c.reason.clone()),
            walked_total: None,
            projects: None,
            mode: None,
            attributed: None,
            unowned: None,
            du_total: None,
            docker_attributed: None,
            docker_unowned: None,
            cursor_family: Some("unit_root".to_string()),
            observed_at,
        });
    }
    columns::write_coverage_rows(&path, &rows).with_context(|| format!("write {}", path.display()))
}

/// Rebuilds `snapshot.coverage` (the project-class rows only --
/// `UnitRootCoverage` has no `ReportSnapshot` field to rebuild into; it
/// is written for completeness and future readers) from `coverage.parquet`,
/// and, when at least one row was walked, `snapshot.report.root` from the
/// first walked row in scope-root order -- the same value
/// `report::merge_root_report_into` would have set. A no-op (leaves
/// `snapshot` untouched) when this scope has no coverage rows yet (an
/// older store).
fn rebuild_coverage_from_tables(swamp_dir: &Path, scope_key: &str, snapshot: &mut ReportSnapshot) {
    let Ok(rows) = columns::read_coverage_rows(&coverage_path(swamp_dir)) else {
        return;
    };
    let rows: Vec<columns::StoredCoverageRow> = rows
        .into_iter()
        .filter(|r| r.scope_key == scope_key)
        .collect();
    if rows.is_empty() {
        return;
    }
    let mut coverage = Vec::new();
    let mut root: Option<PathBuf> = None;
    let mut reconciliation = crate::report::Reconciliation {
        unique_estimate: None,
        attributed: 0,
        unowned: 0,
        walked_total: 0,
        du_total: None,
        docker_attributed: 0,
        docker_unowned: 0,
    };
    for r in rows.iter().filter(|r| r.class == "project") {
        // R20: `Report.reconciliation` is the sum of the walked roots'
        // own totals, exactly as `report::merge_root_report_into` sums
        // each root's report at observe time.
        reconciliation.attributed += r.attributed.unwrap_or(0);
        reconciliation.unowned += r.unowned.unwrap_or(0);
        reconciliation.walked_total += r.walked_total.unwrap_or(0);
        reconciliation.docker_attributed += r.docker_attributed.unwrap_or(0);
        reconciliation.docker_unowned += r.docker_unowned.unwrap_or(0);
        reconciliation.du_total = match (reconciliation.du_total, r.du_total) {
            (None, None) => None,
            (x, y) => Some(x.unwrap_or(0) + y.unwrap_or(0)),
        };
        let status = match r.status.as_str() {
            "complete" => crate::coverage::RegionStatus::Complete,
            "excluded" => crate::coverage::RegionStatus::Excluded,
            "missing" => crate::coverage::RegionStatus::Missing,
            "detector_only" => crate::coverage::RegionStatus::DetectorOnly,
            "partial" => crate::coverage::RegionStatus::Partial {
                reason: r.reason.clone().unwrap_or_default(),
            },
            "inaccessible" => crate::coverage::RegionStatus::Inaccessible {
                reason: r.reason.clone().unwrap_or_default(),
            },
            _ => crate::coverage::RegionStatus::Missing,
        };
        if root.is_none() && status.was_observed() {
            root = Some(PathBuf::from(&r.root_path));
        }
        coverage.push(crate::coverage::RootCoverage {
            path: PathBuf::from(&r.root_path),
            status,
            walked_total: r.walked_total.unwrap_or(0),
            projects: r.projects.unwrap_or(0) as usize,
            mode: r.mode.clone().unwrap_or_default(),
        });
    }
    snapshot.coverage = coverage;
    snapshot.report.reconciliation = reconciliation;
    if let Some(root) = root {
        snapshot.report.root = root;
    }
}

/// Writes `notes.parquet` for `scope_key`: `Report.notes`, order
/// preserved via `seq`.
pub fn write_notes_table(
    swamp_dir: &Path,
    scope_key: &str,
    notes: &[String],
    observed_at: u64,
) -> Result<()> {
    store::StoreDir::at(swamp_dir)?.create()?;
    let path = notes_path(swamp_dir);
    let mut rows: Vec<columns::StoredNoteRow> = columns::read_note_rows(&path)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.scope_key != scope_key)
        .collect();
    for (seq, note) in notes.iter().enumerate() {
        rows.push(columns::StoredNoteRow {
            scope_key: scope_key.to_string(),
            seq: seq as u32,
            note: note.clone(),
            observed_at,
        });
    }
    columns::write_note_rows(&path, &rows).with_context(|| format!("write {}", path.display()))
}

/// Rebuilds `snapshot.report.notes` from `notes.parquet`, in `seq` order.
/// A no-op when this scope has no note rows yet.
fn rebuild_notes_from_tables(swamp_dir: &Path, scope_key: &str, snapshot: &mut ReportSnapshot) {
    let Ok(rows) = columns::read_note_rows(&notes_path(swamp_dir)) else {
        return;
    };
    let mut rows: Vec<columns::StoredNoteRow> = rows
        .into_iter()
        .filter(|r| r.scope_key == scope_key)
        .collect();
    if rows.is_empty() {
        return;
    }
    rows.sort_by_key(|r| r.seq);
    snapshot.report.notes = rows.into_iter().map(|r| r.note).collect();
}

/// The one entry point `report::report_scope_from_store` calls to
/// replace `snapshot`'s coverage (and `Report.reconciliation`, the sum
/// of the walked roots' totals) and notes with what `coverage.parquet`/
/// `notes.parquet` hold for `scope_key`.
pub(crate) fn rebuild_coverage_and_notes_from_tables(
    swamp_dir: &Path,
    scope_key: &str,
    snapshot: &mut ReportSnapshot,
) {
    rebuild_coverage_from_tables(swamp_dir, scope_key, snapshot);
    rebuild_notes_from_tables(swamp_dir, scope_key, snapshot);
}

// ---------------------------------------------------------------------
// runs.parquet (R20): the last full observe pass's run facts for a scope
// key -- see `columns::StoredRunRow`. Also the "has this scope ever been
// observed" answer: a scope with zero discovered projects still gets
// exactly one row per full pass.
// ---------------------------------------------------------------------

/// The run parameters and per-run facts `observe_scope` records.
pub struct RunFacts<'a> {
    pub unique_estimate: Option<&'a crate::report::UniqueEstimate>,
    pub observed_at: u64,
    pub since_secs: u64,
    pub retention_days: u64,
    pub include_dirs: bool,
    pub github_enrichment: Option<&'a crate::report::GithubEnrichmentSummary>,
    pub schedule_line: Option<&'a str>,
}

/// Writes this scope's one `runs.parquet` row, replacing the previous
/// pass's row for `scope_key` wholesale.
pub fn write_run_row(swamp_dir: &Path, scope_key: &str, run: &RunFacts<'_>) -> Result<()> {
    store::StoreDir::at(swamp_dir)?.create()?;
    let path = runs_path(swamp_dir);
    let mut rows: Vec<columns::StoredRunRow> = columns::read_run_rows(&path)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.scope_key != scope_key)
        .collect();
    rows.push(columns::StoredRunRow {
        sharing: run.unique_estimate.and_then(|u| u.sharing.clone()),
        unique_bytes: run.unique_estimate.map(|u| u.bytes),
        unique_reconciled_at: run.unique_estimate.map(|u| u.reconciled_at),
        unique_needs_reconciliation: run.unique_estimate.map(|u| u.needs_reconciliation),
        scope_key: scope_key.to_string(),
        observed_at: run.observed_at,
        since_secs: run.since_secs,
        retention_days: run.retention_days,
        include_dirs: run.include_dirs,
        github_calls_made: run.github_enrichment.map(|g| g.calls_made),
        github_worktrees_enriched: run.github_enrichment.map(|g| g.worktrees_enriched),
        github_elapsed_secs: run.github_enrichment.map(|g| g.elapsed_secs),
        schedule_line: run.schedule_line.map(str::to_string),
    });
    columns::write_run_rows(&path, &rows).with_context(|| format!("write {}", path.display()))
}

pub(crate) fn read_run_row(swamp_dir: &Path, scope_key: &str) -> Option<columns::StoredRunRow> {
    columns::read_run_rows(&runs_path(swamp_dir))
        .ok()?
        .into_iter()
        .find(|r| r.scope_key == scope_key)
}

/// An observing pass may update some family tables before another family
/// fails. Invalidate the old aggregate first, so a cold read cannot combine
/// new rows with a supposedly current unique-byte total from the old pass.
pub(crate) fn invalidate_unique_estimate(swamp_dir: &Path, scope_key: &str) -> Result<()> {
    let path = runs_path(swamp_dir);
    let mut rows = columns::read_run_rows(&path)?;
    let mut changed = false;
    for row in &mut rows {
        if row.scope_key == scope_key && row.unique_needs_reconciliation == Some(false) {
            row.unique_needs_reconciliation = Some(true);
            changed = true;
        }
    }
    if changed {
        columns::write_run_rows(&path, &rows)?;
    }
    Ok(())
}

/// `Some(observed_at)` when `scope_key` has ever been fully observed
/// (`runs.parquet` has its row), `None` otherwise.
/// `report::report_scope_from_store` uses this as its "no observation
/// yet" gate and as the source of `Report.observed_at`.
pub(crate) fn scope_observed_at(swamp_dir: &Path, scope_key: &str) -> Option<u64> {
    read_run_row(swamp_dir, scope_key).map(|r| r.observed_at)
}

// ---------------------------------------------------------------------
// <volume>/dir_tracks.parquet (R20): see `columns::StoredDirTrackRow`.
// ---------------------------------------------------------------------

/// Writes the tracking state of this root's top-level worktree
/// directories (the ones `report::annotate_tracking` filled), replacing
/// the volume's previous rows wholesale.
pub fn write_dir_tracks(
    volume_dir: &Path,
    dirs_by_worktree: &HashMap<String, Vec<crate::report::DirRollup>>,
) -> Result<()> {
    store::StoreDir::at(volume_dir)?.create()?;
    let mut rows: Vec<columns::StoredDirTrackRow> = Vec::new();
    for dirs in dirs_by_worktree.values() {
        for d in dirs {
            if let Some(track) = d.track {
                rows.push(columns::StoredDirTrackRow {
                    worktree_id: d.worktree_id.clone(),
                    rel_path: d.rel_path.clone(),
                    track: track.label().to_string(),
                });
            }
        }
    }
    rows.sort();
    let path = dir_tracks_path(volume_dir);
    columns::write_dir_track_rows(&path, &rows).with_context(|| format!("write {}", path.display()))
}

fn read_dir_tracks(volume_dir: &Path) -> HashMap<(String, String), crate::ignore::TrackState> {
    columns::read_dir_track_rows(&dir_tracks_path(volume_dir))
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            (
                (r.worktree_id, r.rel_path),
                crate::ignore::TrackState::from_label(&r.track),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------
// R20: store = facts, report = view. Everything in a `Report` that is a
// function of the stored facts (+ history + the run's window) is
// computed here at read time, never stored: `summary` (a fold over the
// project rows), `reconciliation` (the walked roots' totals summed),
// `series_by_key`/`total_series`/`series_window_secs` (the reverse-delta
// history bucketed), `unowned` (each walked root's `unowned.parquet` +
// `docker_unowned.parquet`, in scope-root order), `dirs_by_worktree`/
// `files_by_worktree` (`dirs.parquet`/`files.parquet` with growth from
// their history and tracking from `dir_tracks.parquet`), an artifact's
// `growth_bytes`/`allocated_bytes`/`allocated_growth_bytes` (its own
// history and its measured directory). The same functions the observe
// pass used produce them, from the same `observed_at` and window
// (`runs.parquet`), so a read is byte-identical to the pass that wrote
// the facts -- `crates/core/tests/report_is_a_pure_read.rs` holds that
// line.
// ---------------------------------------------------------------------

/// Fills every derived `Report` field of `snapshot` from the fact tables.
/// Runs last in `report::report_scope_from_store`: it needs the rebuilt
/// project list and `snapshot.coverage` (the walked roots, in scope
/// order). A no-op when this scope has no `runs.parquet` row.
pub(crate) fn derive_report_views(
    swamp_dir: &Path,
    scope_key: &str,
    snapshot: &mut ReportSnapshot,
) {
    let Some(run) = read_run_row(swamp_dir, scope_key) else {
        return;
    };
    snapshot.report.reconciliation.unique_estimate = run
        .unique_bytes
        .zip(run.unique_reconciled_at)
        .map(|(bytes, reconciled_at)| crate::report::UniqueEstimate {
            sharing: run.sharing.clone(),
            bytes,
            reconciled_at,
            needs_reconciliation: run.unique_needs_reconciliation.unwrap_or(true),
        });
    let observed_at = run.observed_at;
    let roots: Vec<PathBuf> = snapshot
        .coverage
        .iter()
        .filter(|c| c.status.was_observed())
        .map(|c| c.path.clone())
        .collect();
    let artifact_roots = crate::report::artifact_roots(&snapshot.report.projects);

    let mut unowned: Vec<UnownedRow> = Vec::new();
    let mut dirs_by_worktree: HashMap<String, Vec<DirRollup>> = HashMap::new();
    let mut files_by_worktree: HashMap<String, Vec<FileRow>> = HashMap::new();
    let mut series_by_key: HashMap<String, Vec<Option<u64>>> = HashMap::new();
    let mut total_series: Vec<Option<u64>> = Vec::new();
    let mut series_window_secs = 0u64;

    for root in &roots {
        let vol = volume_store_dir(swamp_dir, root);
        let volume_id = root_scoped_volume_id(root);

        // This root's own artifacts: growth from their history. Only the
        // worktrees this volume holds rows for (every worktree has at
        // least its own `Source` row there) -- annotating another root's
        // rows against this volume's (empty) history would blank them.
        // By worktree id, not path prefix: the scope root and a worktree
        // path can differ in canonical form (`/var` vs `/private/var`).
        let held = worktree_ids_held_by_volume(&vol);
        let mut mine: Vec<ProjectRow> = snapshot
            .report
            .projects
            .iter()
            .map(|p| ProjectRow {
                worktrees: p
                    .worktrees
                    .iter()
                    .filter(|wt| held.contains(&wt.worktree_id))
                    .cloned()
                    .collect(),
                ..p.clone()
            })
            .filter(|p| !p.worktrees.is_empty())
            .collect();
        let _ = annotate_readonly_before(
            swamp_dir,
            volume_id,
            &mut mine,
            observed_at,
            run.retention_days,
            run.since_secs,
        );

        // Directory and large-file rows: the volume's current tables,
        // growth from their history, then the same shaping the observe
        // pass did (`consumers/growth.rs`): a folded artifact's own
        // measured row feeds `allocated_bytes` on the artifact, interior
        // rows stay out of the drill-down.
        let mut dirs: Vec<DirRollup> = columns::read_dir_rows(&dirs_current_path(&vol))
            .unwrap_or_default()
            .into_iter()
            .map(dir_rollup_from_stored)
            .collect();
        let _ = annotate_readonly_dirs_before(
            swamp_dir,
            volume_id,
            &mut dirs,
            observed_at,
            run.retention_days,
            run.since_secs,
        );
        crate::report::attach_allocated_from_dirs(&mut mine, &dirs, &artifact_roots);
        dirs.retain(|row| !crate::report::dir_inside_artifact(row, &artifact_roots));
        let tracks = read_dir_tracks(&vol);
        for d in dirs.iter_mut() {
            d.track = tracks
                .get(&(d.worktree_id.clone(), d.rel_path.clone()))
                .copied();
        }
        let mut files: Vec<FileRow> = columns::read_file_rows(&files_current_path(&vol))
            .unwrap_or_default()
            .into_iter()
            .map(file_row_from_stored)
            .collect();
        let _ = annotate_readonly_files_before(
            swamp_dir,
            volume_id,
            &mut files,
            observed_at,
            run.retention_days,
            run.since_secs,
        );
        for d in dirs {
            dirs_by_worktree
                .entry(d.worktree_id.clone())
                .or_default()
                .push(d);
        }
        for f in files {
            files_by_worktree
                .entry(f.worktree_id.clone())
                .or_default()
                .push(f);
        }

        // Copy this root's annotated artifact fields back by identity.
        for p in mine {
            let Some(target) = snapshot
                .report
                .projects
                .iter_mut()
                .find(|t| t.project_id == p.project_id)
            else {
                continue;
            };
            for wt in p.worktrees {
                let Some(twt) = target
                    .worktrees
                    .iter_mut()
                    .find(|t| t.worktree_id == wt.worktree_id)
                else {
                    continue;
                };
                for (a, ta) in wt.artifacts.into_iter().zip(twt.artifacts.iter_mut()) {
                    ta.growth_bytes = a.growth_bytes;
                    ta.regrowth_count = a.regrowth_count;
                    ta.allocated_bytes = a.allocated_bytes;
                    ta.allocated_growth_bytes = a.allocated_growth_bytes;
                }
            }
        }

        unowned.extend(read_unowned(&vol));
        unowned.extend(read_docker_unowned(&vol));

        // The sparkline history, exactly as `consumers/history.rs`
        // computed it for this root at observe time.
        let asked = run.since_secs;
        let hist = history_span_secs(&vol, observed_at).unwrap_or(asked);
        let window = asked.min(hist).max(60);
        let (series, total) = history_series(&vol, window, 24, observed_at);
        crate::report::merge_series_into(
            &mut series_by_key,
            &mut total_series,
            &mut series_window_secs,
            series,
            total,
            window,
        );
    }
    crate::report::sort_drill_down(&mut dirs_by_worktree, &mut files_by_worktree);

    let report = &mut snapshot.report;
    report.observed_at = observed_at;
    report.unowned = unowned;
    report.series_by_key = series_by_key;
    report.total_series = total_series;
    report.series_window_secs = series_window_secs;
    report.dirs_by_worktree = run.include_dirs.then_some(dirs_by_worktree);
    report.files_by_worktree = run.include_dirs.then_some(files_by_worktree);
    report.schedule_line = run.schedule_line.clone();
    report.github_enrichment = match (
        run.github_calls_made,
        run.github_worktrees_enriched,
        run.github_elapsed_secs,
    ) {
        (Some(calls_made), Some(worktrees_enriched), Some(elapsed_secs)) => {
            Some(crate::report::GithubEnrichmentSummary {
                calls_made,
                worktrees_enriched,
                elapsed_secs,
            })
        }
        _ => None,
    };
    report.summary = crate::report::summarize(&report.projects);
}

/// The worktree ids a volume holds current-artifact rows for: which of
/// a scope's worktrees this root's walk measured (every worktree has at
/// least its own `Source` row there).
pub fn worktree_ids_held_by_volume(volume_dir: &Path) -> HashSet<String> {
    read_rows(&current_path(volume_dir))
        .unwrap_or_default()
        .iter()
        .map(|r| r.worktree_id().to_string())
        .collect()
}

fn dir_rollup_from_stored(r: columns::StoredDirRow) -> DirRollup {
    DirRollup {
        worktree_id: r.worktree_id,
        track: None,
        rel_path: r.rel_path,
        parent_rel_path: r.parent_rel_path,
        allocated_total: r.allocated_total,
        own_allocated: r.own_allocated,
        file_count: r.file_count,
        entry_count: r.entry_count,
        symlink_count: r.symlink_count,
        mod_time_min: r.mod_time_min,
        complete: r.complete,
        growth_bytes: None,
    }
}

fn file_row_from_stored(r: columns::StoredFileRow) -> FileRow {
    FileRow {
        worktree_id: r.worktree_id,
        rel_path: r.rel_path,
        allocated: r.allocated,
        mod_time_min: r.mod_time_min,
        growth_bytes: None,
    }
}

// ---------------------------------------------------------------------
// projects.parquet / worktrees.parquet / worktree_facts.parquet (R15
// item 2/~10 of the JSON-in-the-store decomposition; see
// `.oh/sessions/2026-09-24-r14-json-decomposition.md` for item 1,
// `protect.parquet`). Scope-wide, keyed by `scope_key` like every
// other scope-wide table, written by the same `observe_scope` call and
// read back by `report::report_scope_from_store`, which uses these
// tables as the source of a `WorktreeRow`'s/`ProjectRow`'s own scalars
// and overlays the fields this slice does not migrate (an
// `ArtifactRow`'s evidence/confidence/track/containers/... -- CHUNK_R15
// item 3 names exactly which artifact-render columns move this slice;
// the rest are explicitly later slices) from the snapshot by key.
// ---------------------------------------------------------------------

fn projects_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("projects.parquet")
}

fn worktrees_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("worktrees.parquet")
}

fn worktree_facts_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("worktree_facts.parquet")
}

/// Sums one project's artifacts for `projects.parquet`'s aggregate
/// columns. Not read by any render/agent_json path today (`ProjectRow`
/// itself carries no byte totals; `render.rs` sums worktrees/artifacts
/// on the fly instead) -- computed here anyway because CHUNK_R15 names
/// these columns explicitly, and a stored rollup is real, cheap
/// information for a future summary table even before anything reads
/// it back.
fn project_totals(p: &ProjectRow) -> (u64, u64, u64, Option<i64>, u32) {
    let mut bytes = 0u64;
    let mut local_bytes = 0u64;
    let mut allocated_bytes = 0u64;
    let mut growth_bytes: Option<i64> = None;
    let mut regrowth_count = 0u32;
    for wt in &p.worktrees {
        for a in &wt.artifacts {
            bytes += a.bytes;
            local_bytes += a.local_bytes;
            allocated_bytes += a.allocated_bytes.unwrap_or(0);
            if let Some(g) = a.growth_bytes {
                growth_bytes = Some(growth_bytes.unwrap_or(0) + g);
            }
            regrowth_count += a.regrowth_count;
        }
    }
    (
        bytes,
        local_bytes,
        allocated_bytes,
        growth_bytes,
        regrowth_count,
    )
}

fn github_merged_label(m: &crate::github::MergedStatus) -> &'static str {
    match m {
        crate::github::MergedStatus::Yes { .. } => "yes",
        crate::github::MergedStatus::No => "no",
        crate::github::MergedStatus::Unknown => "unknown",
    }
}

fn github_pr_state_label(p: &crate::github::PrState) -> &'static str {
    match p {
        crate::github::PrState::Open => "open",
        crate::github::PrState::Closed => "closed",
        crate::github::PrState::Merged => "merged",
    }
}

fn github_review_decision_label(d: crate::github::ReviewDecision) -> &'static str {
    use crate::github::ReviewDecision;
    match d {
        ReviewDecision::Approved => "approved",
        ReviewDecision::ChangesRequested => "changes_requested",
        ReviewDecision::ReviewRequired => "review_required",
        ReviewDecision::None => "none",
        ReviewDecision::Unknown => "unknown",
    }
}

fn tristate_label(t: crate::github::TriState) -> &'static str {
    use crate::github::TriState;
    match t {
        TriState::Yes => "yes",
        TriState::No => "no",
        TriState::Unknown => "unknown",
    }
}

fn build_stored_worktree_row(
    scope_key: &str,
    project_id: &str,
    wt: &WorktreeRow,
    observed_at: u64,
) -> columns::StoredWorktreeRow {
    let mut row = columns::StoredWorktreeRow {
        scope_key: scope_key.to_string(),
        worktree_id: wt.worktree_id.clone(),
        project_id: project_id.to_string(),
        path: wt.path.display().to_string(),
        kind: format!("{:?}", wt.kind),
        branch: wt.branch.clone(),
        idle_secs: wt.idle_secs,
        github_default_branch: None,
        github_branch_exists_on_remote: None,
        github_unavailable_reason: None,
        github_merged_state: None,
        github_merged_at: None,
        github_merged_pr_number: None,
        github_pr_state: None,
        github_pr_number: None,
        github_pr_status: None,
        github_pr_draft: None,
        github_pr_url: None,
        github_pr_title: None,
        github_pr_review_decision: None,
        github_pr_updated_at: None,
        merge_complete_verdict: None,
        observed_at,
    };
    if let Some(g) = &wt.github {
        row.github_default_branch = g.default_branch.clone();
        row.github_branch_exists_on_remote = g.branch_exists_on_remote;
        row.github_unavailable_reason = g.unavailable_reason.clone();
        row.github_merged_state = Some(github_merged_label(&g.merged).to_string());
        if let crate::github::MergedStatus::Yes {
            merged_at,
            pr_number,
        } = &g.merged
        {
            row.github_merged_at = merged_at.clone();
            row.github_merged_pr_number = *pr_number;
        }
        match &g.pull_request {
            crate::github::PrStatus::Some(pr) => {
                row.github_pr_state = Some("some".to_string());
                row.github_pr_number = Some(pr.number);
                row.github_pr_status = Some(github_pr_state_label(&pr.state).to_string());
                row.github_pr_draft = Some(pr.draft);
                row.github_pr_url = Some(pr.url.clone());
                row.github_pr_title = Some(pr.title.clone());
                row.github_pr_review_decision =
                    Some(github_review_decision_label(pr.review_decision).to_string());
                row.github_pr_updated_at = Some(pr.updated_at.clone());
            }
            crate::github::PrStatus::None => row.github_pr_state = Some("none".to_string()),
            crate::github::PrStatus::Unknown => row.github_pr_state = Some("unknown".to_string()),
        }
    }
    if let Some(mc) = &wt.merge_complete {
        row.merge_complete_verdict = Some(tristate_label(mc.verdict).to_string());
    }
    row
}

/// Writes `projects.parquet`/`worktrees.parquet`/`worktree_facts.parquet`
/// for `scope_key`, replacing that scope's rows wholesale (same
/// per-scope-key replace semantics as `write_artifact_shape_table`).
/// Called from `observe_scope` on the same already-merged multi-root
/// `projects` tree -- no second walk.
pub fn write_project_worktree_tables(
    swamp_dir: &Path,
    scope_key: &str,
    projects: &[ProjectRow],
    observed_at: u64,
) -> Result<()> {
    store::StoreDir::at(swamp_dir)?.create()?;

    let projects_file = projects_path(swamp_dir);
    let mut project_rows: Vec<columns::StoredProjectRow> =
        columns::read_project_rows(&projects_file)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.scope_key != scope_key)
            .collect();

    let worktrees_file = worktrees_path(swamp_dir);
    let mut worktree_rows: Vec<columns::StoredWorktreeRow> =
        columns::read_worktree_rows(&worktrees_file)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.scope_key != scope_key)
            .collect();

    let facts_file = worktree_facts_path(swamp_dir);
    let mut fact_rows: Vec<columns::StoredWorktreeFactRow> =
        columns::read_worktree_fact_rows(&facts_file)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.scope_key != scope_key)
            .collect();

    for p in projects {
        let (bytes, local_bytes, allocated_bytes, growth_bytes, regrowth_count) = project_totals(p);
        project_rows.push(columns::StoredProjectRow {
            scope_key: scope_key.to_string(),
            project_id: p.project_id.clone(),
            name: p.name.clone(),
            ecosystems: p.ecosystems.join("|"),
            remote: p.remote.clone(),
            bytes,
            local_bytes,
            allocated_bytes,
            growth_bytes,
            regrowth_count,
            worktree_count: p.worktrees.len() as u32,
            observed_at,
        });

        for wt in &p.worktrees {
            worktree_rows.push(build_stored_worktree_row(
                scope_key,
                &p.project_id,
                wt,
                observed_at,
            ));
            for (seq, s) in wt.signals.iter().enumerate() {
                fact_rows.push(columns::StoredWorktreeFactRow {
                    scope_key: scope_key.to_string(),
                    worktree_id: wt.worktree_id.clone(),
                    fact_kind: "signal".to_string(),
                    name: Some(s.name.clone()),
                    value: s.value.clone(),
                    seq: seq as u32,
                });
            }
            if let Some(mc) = &wt.merge_complete {
                for (seq, t) in mc.terms.iter().enumerate() {
                    fact_rows.push(columns::StoredWorktreeFactRow {
                        scope_key: scope_key.to_string(),
                        worktree_id: wt.worktree_id.clone(),
                        fact_kind: "merge_complete_term".to_string(),
                        name: None,
                        value: t.clone(),
                        seq: seq as u32,
                    });
                }
            }
        }
    }

    columns::write_project_rows(&projects_file, &project_rows)
        .with_context(|| format!("write {}", projects_file.display()))?;
    columns::write_worktree_rows(&worktrees_file, &worktree_rows)
        .with_context(|| format!("write {}", worktrees_file.display()))?;
    columns::write_worktree_fact_rows(&facts_file, &fact_rows)
        .with_context(|| format!("write {}", facts_file.display()))?;
    Ok(())
}

/// Every stored row of `projects.parquet`/`worktrees.parquet`/
/// `worktree_facts.parquet` for `scope_key`. `None` when no table exists
/// yet (an older store, or a scope never observed under this build) --
/// same cache-miss discipline as every other scope-wide table read.
pub(crate) struct StoredProjectWorktreeTables {
    pub(crate) projects: Vec<columns::StoredProjectRow>,
    pub(crate) worktrees: Vec<columns::StoredWorktreeRow>,
    pub(crate) worktree_facts: Vec<columns::StoredWorktreeFactRow>,
}

pub(crate) fn read_project_worktree_tables(
    swamp_dir: &Path,
    scope_key: &str,
) -> Option<StoredProjectWorktreeTables> {
    let projects: Vec<columns::StoredProjectRow> =
        columns::read_project_rows(&projects_path(swamp_dir))
            .ok()?
            .into_iter()
            .filter(|r| r.scope_key == scope_key)
            .collect();
    if projects.is_empty() {
        return None;
    }
    let worktrees: Vec<columns::StoredWorktreeRow> =
        columns::read_worktree_rows(&worktrees_path(swamp_dir))
            .ok()?
            .into_iter()
            .filter(|r| r.scope_key == scope_key)
            .collect();
    let worktree_facts: Vec<columns::StoredWorktreeFactRow> =
        columns::read_worktree_fact_rows(&worktree_facts_path(swamp_dir))
            .ok()?
            .into_iter()
            .filter(|r| r.scope_key == scope_key)
            .collect();
    Some(StoredProjectWorktreeTables {
        projects,
        worktrees,
        worktree_facts,
    })
}

fn artifact_shape_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("artifact_shape.parquet")
}
fn artifact_shape_lists_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("artifact_shape_lists.parquet")
}

/// Writes `artifact_shape.parquet`/`artifact_shape_lists.parquet` for
/// `scope_key`, replacing that scope's rows wholesale from this pass's
/// already-merged `projects` tree (same per-scope-key replace semantics
/// as `write_project_worktree_tables`, called alongside it from
/// `observe_scope`) -- no second pass. See the header comment on
/// `columns::StoredArtifactShapeRow` for exactly which `ArtifactRow`
/// fields land here.
pub fn write_artifact_shape_table(
    swamp_dir: &Path,
    scope_key: &str,
    projects: &[ProjectRow],
    observed_at: u64,
) -> Result<()> {
    store::StoreDir::at(swamp_dir)?.create()?;
    let shape_file = artifact_shape_path(swamp_dir);
    let mut rows: Vec<columns::StoredArtifactShapeRow> =
        columns::read_artifact_shape_rows(&shape_file)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.scope_key != scope_key)
            .collect();
    let lists_file = artifact_shape_lists_path(swamp_dir);
    let mut list_rows: Vec<columns::StoredArtifactShapeListRow> =
        columns::read_artifact_shape_list_rows(&lists_file)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.scope_key != scope_key)
            .collect();

    for project in projects {
        for wt in &project.worktrees {
            for (seq, a) in wt.artifacts.iter().enumerate() {
                let seq = seq as u32;
                let rel_path = a
                    .path
                    .strip_prefix(&wt.path)
                    .unwrap_or(&a.path)
                    .display()
                    .to_string();
                rows.push(columns::StoredArtifactShapeRow {
                    scope_key: scope_key.to_string(),
                    project_id: project.project_id.clone(),
                    worktree_id: wt.worktree_id.clone(),
                    seq,
                    rel_path,
                    kind: format!("{:?}", a.kind),
                    track: a.track.map(|t| t.label().to_string()),
                    confidence: a.confidence.label().to_string(),
                    source_tool: a.source.tool.clone(),
                    note: a.note.clone(),
                    created_at: a.created_at.clone(),
                    dangling: a.dangling,
                    observed_at,
                });
                for (item_seq, value) in a.containers.iter().enumerate() {
                    list_rows.push(columns::StoredArtifactShapeListRow {
                        scope_key: scope_key.to_string(),
                        worktree_id: wt.worktree_id.clone(),
                        seq,
                        list_kind: "containers".to_string(),
                        item_seq: item_seq as u32,
                        value: value.clone(),
                    });
                }
                for (item_seq, value) in a.shared_with.iter().enumerate() {
                    list_rows.push(columns::StoredArtifactShapeListRow {
                        scope_key: scope_key.to_string(),
                        worktree_id: wt.worktree_id.clone(),
                        seq,
                        list_kind: "shared-with".to_string(),
                        item_seq: item_seq as u32,
                        value: value.clone(),
                    });
                }
            }
        }
    }

    columns::write_artifact_shape_rows(&shape_file, &rows)
        .with_context(|| format!("write {}", shape_file.display()))?;
    columns::write_artifact_shape_list_rows(&lists_file, &list_rows)
        .with_context(|| format!("write {}", lists_file.display()))
}

/// Rebuilds every worktree's artifact list *shape* from
/// `artifact_shape.parquet`/`artifact_shape_lists.parquet` for
/// `scope_key`, grouped by `worktree_id` and ordered by the stored
/// `seq` (Parquet row order is not guaranteed across a read). Each
/// `ArtifactRow`'s facts fields (`bytes`/`local_bytes`/`mtime_max`/
/// `hardlinked`/`dedup_stale`/`regrowth_count`/`observed_at`/
/// `ecosystem`) are left at their zero/default value here --
/// `report::rebuild_projects_from_tables` overlays those from
/// `ArtifactTableFacts` right after calling this, same as it always
/// has. `evidence` is left empty for `rebuild_evidence_from_tables`.
/// Empty (never missing) map when this scope has no artifact-shape rows
/// yet (an older store) -- the caller's existing "no rows yet" handling
/// on the empty per-worktree list already covers that case, matching
/// what an empty `old_artifacts_by_worktree` used to mean.
pub(crate) fn artifact_shape_rows_by_worktree(
    swamp_dir: &Path,
    scope_key: &str,
) -> HashMap<String, Vec<ArtifactRow>> {
    let Ok(shape_rows) = columns::read_artifact_shape_rows(&artifact_shape_path(swamp_dir)) else {
        return HashMap::new();
    };
    let Ok(list_rows) =
        columns::read_artifact_shape_list_rows(&artifact_shape_lists_path(swamp_dir))
    else {
        return HashMap::new();
    };

    let mut lists_by_worktree_seq: HashMap<
        (String, u32),
        Vec<&columns::StoredArtifactShapeListRow>,
    > = HashMap::new();
    for r in &list_rows {
        if r.scope_key != scope_key {
            continue;
        }
        lists_by_worktree_seq
            .entry((r.worktree_id.clone(), r.seq))
            .or_default()
            .push(r);
    }

    let mut by_worktree: HashMap<String, Vec<(u32, columns::StoredArtifactShapeRow)>> =
        HashMap::new();
    for r in shape_rows {
        if r.scope_key != scope_key {
            continue;
        }
        by_worktree
            .entry(r.worktree_id.clone())
            .or_default()
            .push((r.seq, r));
    }

    let mut out = HashMap::new();
    for (worktree_id, mut rows) in by_worktree {
        rows.sort_by_key(|(seq, _)| *seq);
        let artifacts = rows
            .into_iter()
            .map(|(seq, r)| {
                let mut lists = lists_by_worktree_seq
                    .get(&(worktree_id.clone(), seq))
                    .cloned()
                    .unwrap_or_default();
                lists.sort_by_key(|l| l.item_seq);
                let containers = lists
                    .iter()
                    .filter(|l| l.list_kind == "containers")
                    .map(|l| l.value.clone())
                    .collect();
                let shared_with = lists
                    .iter()
                    .filter(|l| l.list_kind == "shared-with")
                    .map(|l| l.value.clone())
                    .collect();
                ArtifactRow {
                    kind: parse_artifact_kind(&r.kind),
                    path: PathBuf::from(&r.rel_path),
                    bytes: 0,
                    mtime_max: 0,
                    ecosystem: None,
                    hardlinked: true,
                    dedup_stale: false,
                    allocated_bytes: None,
                    allocated_growth_bytes: None,
                    local_bytes: 0,
                    track: r
                        .track
                        .as_deref()
                        .map(crate::ignore::TrackState::from_label),
                    growth_bytes: None,
                    regrowth_count: 0,
                    observed_at: 0,
                    confidence: Confidence::from_label(&r.confidence),
                    source: crate::report::Source::new(r.source_tool.clone()),
                    note: r.note.clone(),
                    created_at: r.created_at.clone(),
                    containers,
                    shared_with,
                    dangling: r.dangling,
                    evidence: Vec::new(),
                }
            })
            .collect();
        out.insert(worktree_id, artifacts);
    }
    out
}

/// The current-artifact-table facts `report_scope_from_store` needs per
/// artifact row (R15 item 3): everything the extended `StoredRow` now
/// carries, keyed the same way the artifact history already keys rows
/// (`project_id`/`worktree_id`/`kind`/`rel_path`). Read across every
/// root's own volume directory, since one scope can span several.
pub struct ArtifactTableFacts {
    pub bytes: u64,
    pub local_bytes: u64,
    pub mtime_max: u64,
    pub hardlinked: bool,
    pub dedup_stale: bool,
    pub regrowth_count: u32,
    pub observed_at: u64,
    pub present: bool,
    pub ecosystem: Option<String>,
}

pub fn artifact_table_facts_for_roots(
    swamp_dir: &Path,
    roots: &[PathBuf],
) -> HashMap<String, ArtifactTableFacts> {
    let mut out = HashMap::new();
    for root in roots {
        let dir = volume_dir(swamp_dir, root_scoped_volume_id(root));
        let Ok(rows) = read_rows(&current_path(&dir)) else {
            continue;
        };
        for r in rows {
            out.insert(
                r.key(),
                ArtifactTableFacts {
                    bytes: r.bytes(),
                    local_bytes: r.local_bytes(),
                    mtime_max: r.mtime_max(),
                    hardlinked: r.hardlinked(),
                    dedup_stale: r.dedup_stale(),
                    regrowth_count: r.regrowth_count(),
                    observed_at: r.observed_at(),
                    present: r.present(),
                    ecosystem: r.ecosystem().map(|s| s.to_string()),
                },
            );
        }
    }
    out
}

fn parse_worktree_kind(s: &str) -> crate::report::WorktreeKind {
    match s {
        "Linked" => crate::report::WorktreeKind::Linked,
        "Clone" => crate::report::WorktreeKind::Clone,
        _ => crate::report::WorktreeKind::Main,
    }
}

fn parse_merged_status(
    state: Option<&str>,
    merged_at: Option<String>,
    pr_number: Option<u64>,
) -> crate::github::MergedStatus {
    match state {
        Some("yes") => crate::github::MergedStatus::Yes {
            merged_at,
            pr_number,
        },
        Some("no") => crate::github::MergedStatus::No,
        _ => crate::github::MergedStatus::Unknown,
    }
}

fn parse_pr_state(s: Option<&str>) -> crate::github::PrState {
    match s {
        Some("closed") => crate::github::PrState::Closed,
        Some("merged") => crate::github::PrState::Merged,
        _ => crate::github::PrState::Open,
    }
}

fn parse_review_decision(s: Option<&str>) -> crate::github::ReviewDecision {
    use crate::github::ReviewDecision;
    match s {
        Some("approved") => ReviewDecision::Approved,
        Some("changes_requested") => ReviewDecision::ChangesRequested,
        Some("review_required") => ReviewDecision::ReviewRequired,
        Some("none") => ReviewDecision::None,
        _ => ReviewDecision::Unknown,
    }
}

fn parse_tristate(s: Option<&str>) -> crate::github::TriState {
    use crate::github::TriState;
    match s {
        Some("yes") => TriState::Yes,
        Some("no") => TriState::No,
        _ => TriState::Unknown,
    }
}

/// Reconstructs a `WorktreeRow`'s own scalars (everything but
/// `artifacts`, left empty for the caller to fill from the artifact
/// table) from `projects.parquet`'s sibling `worktrees.parquet` row plus
/// its `worktree_facts.parquet` entries.
pub(crate) fn worktree_row_from_stored(
    stored: &columns::StoredWorktreeRow,
    facts: &[columns::StoredWorktreeFactRow],
) -> WorktreeRow {
    let mut signal_facts: Vec<&columns::StoredWorktreeFactRow> = facts
        .iter()
        .filter(|f| f.worktree_id == stored.worktree_id && f.fact_kind == "signal")
        .collect();
    signal_facts.sort_by_key(|f| f.seq);
    let signals = signal_facts
        .into_iter()
        .map(|f| crate::report::Signal {
            name: f.name.clone().unwrap_or_default(),
            value: f.value.clone(),
        })
        .collect();

    let github =
        stored
            .github_merged_state
            .as_deref()
            .map(|merged_state| crate::github::GithubFacts {
                default_branch: stored.github_default_branch.clone(),
                branch_exists_on_remote: stored.github_branch_exists_on_remote,
                merged: parse_merged_status(
                    Some(merged_state),
                    stored.github_merged_at.clone(),
                    stored.github_merged_pr_number,
                ),
                pull_request: match stored.github_pr_state.as_deref() {
                    Some("some") => crate::github::PrStatus::Some(crate::github::PullRequestInfo {
                        number: stored.github_pr_number.unwrap_or(0),
                        state: parse_pr_state(stored.github_pr_status.as_deref()),
                        draft: stored.github_pr_draft.unwrap_or(false),
                        url: stored.github_pr_url.clone().unwrap_or_default(),
                        title: stored.github_pr_title.clone().unwrap_or_default(),
                        review_decision: parse_review_decision(
                            stored.github_pr_review_decision.as_deref(),
                        ),
                        updated_at: stored.github_pr_updated_at.clone().unwrap_or_default(),
                    }),
                    Some("none") => crate::github::PrStatus::None,
                    _ => crate::github::PrStatus::Unknown,
                },
                unavailable_reason: stored.github_unavailable_reason.clone(),
            });

    let merge_complete = stored.merge_complete_verdict.as_deref().map(|verdict| {
        let mut term_facts: Vec<&columns::StoredWorktreeFactRow> = facts
            .iter()
            .filter(|f| f.worktree_id == stored.worktree_id && f.fact_kind == "merge_complete_term")
            .collect();
        term_facts.sort_by_key(|f| f.seq);
        crate::github::MergeComplete {
            verdict: parse_tristate(Some(verdict)),
            terms: term_facts.into_iter().map(|f| f.value.clone()).collect(),
        }
    });

    WorktreeRow {
        worktree_id: stored.worktree_id.clone(),
        path: PathBuf::from(&stored.path),
        kind: parse_worktree_kind(&stored.kind),
        artifacts: Vec::new(),
        signals,
        branch: stored.branch.clone(),
        github,
        merge_complete,
        idle_secs: stored.idle_secs,
    }
}

/// Reconstructs a `ProjectRow`'s own scalars (everything but
/// `worktrees`, left empty for the caller to fill) from a
/// `projects.parquet` row.
pub(crate) fn project_row_from_stored(stored: &columns::StoredProjectRow) -> ProjectRow {
    ProjectRow {
        project_id: stored.project_id.clone(),
        name: stored.name.clone(),
        worktrees: Vec::new(),
        ecosystems: if stored.ecosystems.is_empty() {
            Vec::new()
        } else {
            stored
                .ecosystems
                .split('|')
                .map(|s| s.to_string())
                .collect()
        },
        remote: stored.remote.clone(),
    }
}

/// The same row key the artifact history stores rows under
/// (`project_id`/`worktree_id`/`observed_kind`/`rel_path`), exposed so
/// `report::report_scope_from_store` can look up
/// [`ArtifactTableFacts`] for an `ArtifactRow` it already has (from the
/// snapshot) without duplicating `observed_kind`'s cargo-nested-id
/// special case.
pub(crate) fn artifact_row_key(
    project_id: &str,
    worktree_id: &str,
    worktree_path: &Path,
    artifact: &ArtifactRow,
) -> String {
    let rel_path = artifact
        .path
        .strip_prefix(worktree_path)
        .unwrap_or(&artifact.path)
        .display()
        .to_string();
    let kind = observed_kind(artifact);
    row_key(project_id, worktree_id, &kind, &rel_path)
}

// ---------------------------------------------------------------------
// external_units.parquet / agent_units.parquet / unit_consumers.parquet
// (R16 item 1). Scope-wide, keyed like `projects.parquet`; written by
// `observe_scope` alongside the other tables (same already-measured
// `ScopeObservation`, no second discovery pass) and read by
// `report::report_scope_from_store`, which replaces
// `snapshot.external_units`/`snapshot.agent_units` wholesale and
// overlays the fields this slice does not migrate (an `ExternalUnit`'s
// `provenance`; an `AgentUnit`'s `tool_home`/`relative_path`/`members`/
// `action`) from the snapshot's own list by `id` -- exactly
// `rebuild_projects_from_tables`'s pattern for artifacts. `evidence` is
// the one field on both unit types this slice does *not* overlay: it is
// replaced entirely from `evidence.parquet` (see further down), per
// CHUNK_R16 ("`report_scope_from_store` builds ... per-row evidence
// FROM THEM").
// ---------------------------------------------------------------------

fn external_units_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("external_units.parquet")
}

fn agent_units_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("agent_units.parquet")
}

fn unit_consumers_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("unit_consumers.parquet")
}

fn agent_unit_members_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("agent_unit_members.parquet")
}

/// A stable identity for an external unit independent of the growth
/// store's own device-inclusive row key (`external_row_key`, which
/// exists to detect a remounted/replaced device for byte-history
/// purposes, not to key a report-facing table): `(detector_id,
/// category, canonical path)`, hashed the same way every other stable
/// id in this codebase is minted.
pub(crate) fn external_unit_table_id(detector_id: &str, category: &str, path: &Path) -> String {
    crate::entities::id_for(&format!(
        "external-unit-table:v1:{detector_id}:{category}:{}",
        path.display()
    ))
}

const LINK_SEP: char = '\u{1}';

fn project_link_state_label(s: &crate::agents::ProjectLinkState) -> &'static str {
    use crate::agents::ProjectLinkState as P;
    match s {
        P::Linked { .. } => "linked",
        P::Unresolved { .. } => "unresolved",
        P::Missing { .. } => "missing",
        P::NotAProject { .. } => "not-a-project",
        P::Moved { .. } => "moved",
        P::Remote { .. } => "remote",
        P::Shared { .. } => "shared",
        P::NotApplicable => "not-applicable",
    }
}

/// Flattens a `ProjectLinkState`'s variant-specific detail into one
/// string (`linkage_basis`) plus, for `Linked` only, a separate
/// `project_id` -- CHUNK_R16's three-column budget for what is, on the
/// production type, up to five distinct pieces of data. `LINK_SEP`
/// joins the extra pieces `Linked`/`Moved`/`Remote` carry beyond a
/// single string; every other variant's detail is already one string.
fn project_link_state_to_columns(
    s: &crate::agents::ProjectLinkState,
) -> (Option<String>, Option<String>) {
    use crate::agents::{LinkSource, ProjectLinkState as P};
    match s {
        P::Linked {
            project_id,
            project_name,
            project_path,
            source,
            fallback_reason,
            worktree_kind,
        } => {
            let source_label = match source {
                LinkSource::Declared => "declared",
                LinkSource::Inferred => "inferred",
            };
            (
                Some(format!(
                    "{project_name}{LINK_SEP}{}{LINK_SEP}{source_label}{LINK_SEP}{worktree_kind}{LINK_SEP}{}",
                    project_path.display(),
                    fallback_reason.as_deref().unwrap_or_default()
                )),
                Some(project_id.clone()),
            )
        }
        P::Unresolved { reason } => (Some(reason.clone()), None),
        P::Missing { path } => (Some(path.display().to_string()), None),
        P::NotAProject { path } => (Some(path.display().to_string()), None),
        P::Moved { from, to } => (
            Some(format!("{}{LINK_SEP}{}", from.display(), to.display())),
            None,
        ),
        P::Remote { host, path } => (Some(format!("{host}{LINK_SEP}{}", path.display())), None),
        P::Shared { project_ids } => (Some(project_ids.join("|")), None),
        P::NotApplicable => (None, None),
    }
}

fn project_link_state_from_columns(
    state: &str,
    basis: Option<&str>,
    project_id: Option<&str>,
) -> crate::agents::ProjectLinkState {
    use crate::agents::{LinkSource, ProjectLinkState as P};
    let basis = basis.unwrap_or_default();
    match state {
        "linked" => {
            // The fifth field is path-derived and can contain LINK_SEP.
            // Split only the four structural boundaries; preserve the
            // complete fallback reason as the remainder.
            let parts: Vec<&str> = basis.splitn(5, LINK_SEP).collect();
            P::Linked {
                project_id: project_id.unwrap_or_default().to_string(),
                project_name: parts.first().copied().unwrap_or_default().to_string(),
                project_path: PathBuf::from(parts.get(1).copied().unwrap_or_default()),
                source: if parts.get(2) == Some(&"inferred") {
                    LinkSource::Inferred
                } else {
                    LinkSource::Declared
                },
                fallback_reason: parts
                    .get(4)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string()),
                worktree_kind: parts.get(3).copied().unwrap_or_default().to_string(),
            }
        }
        "missing" => P::Missing {
            path: PathBuf::from(basis),
        },
        "not-a-project" => P::NotAProject {
            path: PathBuf::from(basis),
        },
        "moved" => {
            let mut parts = basis.splitn(2, LINK_SEP);
            P::Moved {
                from: PathBuf::from(parts.next().unwrap_or_default()),
                to: PathBuf::from(parts.next().unwrap_or_default()),
            }
        }
        "remote" => {
            let mut parts = basis.splitn(2, LINK_SEP);
            P::Remote {
                host: parts.next().unwrap_or_default().to_string(),
                path: PathBuf::from(parts.next().unwrap_or_default()),
            }
        }
        "shared" => P::Shared {
            project_ids: if basis.is_empty() {
                Vec::new()
            } else {
                basis.split('|').map(str::to_string).collect()
            },
        },
        "not-applicable" => P::NotApplicable,
        _ => P::Unresolved {
            reason: basis.to_string(),
        },
    }
}

/// `Provenance`'s variant tag plus its own payload, flattened to two
/// columns (R18a: replaces the JSON merge fallback
/// `external_unit_from_stored` used to read `old.provenance` from).
fn provenance_to_columns(p: &crate::locations::Provenance) -> (&'static str, Option<String>) {
    use crate::locations::Provenance as P;
    match p {
        P::BuiltinConvention => ("builtin", None),
        P::EnvVar(name) => ("env-var", Some(name.clone())),
        P::ConfigField(field) => ("config-field", Some(field.clone())),
        P::ToolQuery(desc) => ("tool-query", Some(desc.clone())),
    }
}

fn provenance_from_columns(kind: &str, value: Option<&str>) -> crate::locations::Provenance {
    use crate::locations::Provenance as P;
    match kind {
        "env-var" => P::EnvVar(value.unwrap_or_default().to_string()),
        "config-field" => P::ConfigField(value.unwrap_or_default().to_string()),
        "tool-query" => P::ToolQuery(value.unwrap_or_default().to_string()),
        _ => P::BuiltinConvention,
    }
}

fn stored_row_from_external_unit(
    scope_key: &str,
    u: &crate::external::ExternalUnit,
) -> columns::StoredUnitRow {
    let category = crate::external::category_str(u.category);
    let (provenance_kind, provenance_value) = provenance_to_columns(&u.provenance);
    columns::StoredUnitRow {
        scope_key: scope_key.to_string(),
        id: external_unit_table_id(&u.detector_id, category, &u.path),
        source_id: u.detector_id.clone(),
        source_name: u.detector_name.clone(),
        category: category.to_string(),
        path: u.path.display().to_string(),
        bytes: u.bytes,
        complete: None,
        mtime_max: u.mtime_max,
        linkage_state: None,
        linkage_basis: None,
        project_id: None,
        protected: None,
        protect_reason: None,
        consequence: u.note.clone(),
        observed_at: u.observed_at,
        growth_bytes: u.growth_bytes,
        regrowth_count: u.regrowth_count,
        provenance_kind: Some(provenance_kind.to_string()),
        provenance_value,
        hardlinked: Some(u.hardlinked),
        tool_home: None,
        relative_path: None,
        action: None,
    }
}

fn stored_row_from_agent_unit(
    scope_key: &str,
    u: &crate::agents::AgentUnit,
) -> columns::StoredUnitRow {
    let (linkage_basis, project_id) = project_link_state_to_columns(&u.project_link);
    columns::StoredUnitRow {
        scope_key: scope_key.to_string(),
        id: u.id.clone(),
        source_id: u.tool_id.clone(),
        source_name: u.tool_name.clone(),
        category: u.category.label().to_string(),
        path: u.path.display().to_string(),
        bytes: u.bytes,
        complete: Some(u.complete),
        mtime_max: u.mtime_max,
        linkage_state: Some(project_link_state_label(&u.project_link).to_string()),
        linkage_basis,
        project_id,
        protected: Some(u.protected),
        protect_reason: u.protect_reason.clone(),
        consequence: u.note.clone(),
        observed_at: u.observed_at,
        growth_bytes: u.growth_bytes,
        regrowth_count: u.regrowth_count,
        provenance_kind: None,
        provenance_value: None,
        hardlinked: Some(u.hardlinked),
        tool_home: Some(u.tool_home.display().to_string()),
        relative_path: Some(u.relative_path.clone()),
        action: Some(u.action.label().to_string()),
    }
}

/// One `agent_unit_members.parquet` row per [`crate::agents::AgentMember`].
fn stored_agent_member_rows(
    scope_key: &str,
    unit_id: &str,
    members: &[crate::agents::AgentMember],
) -> Vec<columns::StoredAgentMemberRow> {
    members
        .iter()
        .enumerate()
        .map(|(seq, m)| columns::StoredAgentMemberRow {
            scope_key: scope_key.to_string(),
            unit_id: unit_id.to_string(),
            seq: seq as u32,
            path: m.path.display().to_string(),
            bytes: m.bytes,
            kind: m.kind.label().to_string(),
        })
        .collect()
}

fn agent_members_from_stored(
    rows: &[&columns::StoredAgentMemberRow],
) -> Vec<crate::agents::AgentMember> {
    let mut sorted: Vec<&&columns::StoredAgentMemberRow> = rows.iter().collect();
    sorted.sort_by_key(|r| r.seq);
    sorted
        .into_iter()
        .map(|r| crate::agents::AgentMember {
            path: PathBuf::from(&r.path),
            bytes: r.bytes,
            kind: crate::agents::AgentMemberKind::from_label(&r.kind)
                .unwrap_or(crate::agents::AgentMemberKind::CategoryDir),
        })
        .collect()
}

/// Writes `external_units.parquet`/`agent_units.parquet`/
/// `unit_consumers.parquet` for `scope_key`, replacing that scope's rows
/// wholesale (same per-scope-key replace semantics as
/// `write_project_worktree_tables`). Called from `observe_scope`
/// alongside the other tables, on the already-measured unit lists --
/// no second discovery pass.
pub fn write_unit_tables(
    swamp_dir: &Path,
    scope_key: &str,
    external_units: &[crate::external::ExternalUnit],
    agent_units: &[crate::agents::AgentUnit],
) -> Result<()> {
    store::StoreDir::at(swamp_dir)?.create()?;

    let ext_file = external_units_path(swamp_dir);
    let mut ext_rows: Vec<columns::StoredUnitRow> = columns::read_unit_rows(&ext_file)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.scope_key != scope_key)
        .collect();
    ext_rows.extend(
        external_units
            .iter()
            .map(|u| stored_row_from_external_unit(scope_key, u)),
    );
    columns::write_unit_rows(&ext_file, &ext_rows)
        .with_context(|| format!("write {}", ext_file.display()))?;

    let agent_file = agent_units_path(swamp_dir);
    let mut agent_rows: Vec<columns::StoredUnitRow> = columns::read_unit_rows(&agent_file)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.scope_key != scope_key)
        .collect();
    agent_rows.extend(
        agent_units
            .iter()
            .map(|u| stored_row_from_agent_unit(scope_key, u)),
    );
    columns::write_unit_rows(&agent_file, &agent_rows)
        .with_context(|| format!("write {}", agent_file.display()))?;

    let consumers_file = unit_consumers_path(swamp_dir);
    let mut consumer_rows: Vec<columns::StoredUnitConsumerRow> =
        columns::read_unit_consumer_rows(&consumers_file)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.scope_key != scope_key)
            .collect();
    for u in external_units {
        let unit_id = external_unit_table_id(
            &u.detector_id,
            crate::external::category_str(u.category),
            &u.path,
        );
        for (seq, c) in u.consumers.iter().enumerate() {
            consumer_rows.push(columns::StoredUnitConsumerRow {
                scope_key: scope_key.to_string(),
                unit_id: unit_id.clone(),
                consumer_label: c.label.clone(),
                consumer_project_id: None,
                basis: c.note.clone(),
                seq: seq as u32,
            });
        }
    }
    columns::write_unit_consumer_rows(&consumers_file, &consumer_rows)
        .with_context(|| format!("write {}", consumers_file.display()))?;

    let members_file = agent_unit_members_path(swamp_dir);
    let mut member_rows: Vec<columns::StoredAgentMemberRow> =
        columns::read_agent_member_rows(&members_file)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.scope_key != scope_key)
            .collect();
    for u in agent_units {
        member_rows.extend(stored_agent_member_rows(scope_key, &u.id, &u.members));
    }
    columns::write_agent_member_rows(&members_file, &member_rows)
        .with_context(|| format!("write {}", members_file.display()))?;

    Ok(())
}

/// Every stored `external_units.parquet`/`agent_units.parquet`/
/// `unit_consumers.parquet`/`agent_unit_members.parquet` row for
/// `scope_key`. `None` when neither unit table has rows for this scope
/// yet (an older store) -- the caller falls back to the snapshot's own
/// lists in that case.
pub(crate) struct StoredUnitTables {
    pub(crate) external: Vec<columns::StoredUnitRow>,
    pub(crate) agent: Vec<columns::StoredUnitRow>,
    pub(crate) consumers: Vec<columns::StoredUnitConsumerRow>,
    pub(crate) agent_members: Vec<columns::StoredAgentMemberRow>,
}

pub(crate) fn read_unit_tables(swamp_dir: &Path, scope_key: &str) -> Option<StoredUnitTables> {
    let external: Vec<columns::StoredUnitRow> =
        columns::read_unit_rows(&external_units_path(swamp_dir))
            .ok()?
            .into_iter()
            .filter(|r| r.scope_key == scope_key)
            .collect();
    let agent: Vec<columns::StoredUnitRow> = columns::read_unit_rows(&agent_units_path(swamp_dir))
        .ok()?
        .into_iter()
        .filter(|r| r.scope_key == scope_key)
        .collect();
    if external.is_empty() && agent.is_empty() {
        return None;
    }
    let consumers: Vec<columns::StoredUnitConsumerRow> =
        columns::read_unit_consumer_rows(&unit_consumers_path(swamp_dir))
            .ok()?
            .into_iter()
            .filter(|r| r.scope_key == scope_key)
            .collect();
    let agent_members: Vec<columns::StoredAgentMemberRow> =
        columns::read_agent_member_rows(&agent_unit_members_path(swamp_dir))
            .ok()?
            .into_iter()
            .filter(|r| r.scope_key == scope_key)
            .collect();
    Some(StoredUnitTables {
        external,
        agent,
        consumers,
        agent_members,
    })
}

/// Rebuilds one `ExternalUnit` from its stored row and its consumers.
/// Every field is read from `stored`'s own typed columns (R18a); there
/// is no JSON merge fallback -- `provenance_kind`/`hardlinked` are
/// always written together with the rest of the row, so a row that
/// exists at all always carries them. `evidence` is intentionally left
/// empty here -- the caller fills it from `evidence.parquet` by this
/// same id.
pub(crate) fn external_unit_from_stored(
    stored: &columns::StoredUnitRow,
    consumers: Vec<crate::external::ExternalConsumer>,
) -> crate::external::ExternalUnit {
    let category = crate::external::category_from_str(&stored.category)
        .unwrap_or(crate::locations::StorageCategory::Unclassified);
    let provenance = match &stored.provenance_kind {
        Some(kind) => provenance_from_columns(kind, stored.provenance_value.as_deref()),
        None => crate::locations::Provenance::BuiltinConvention,
    };
    crate::external::ExternalUnit {
        detector_id: stored.source_id.clone(),
        detector_name: stored.source_name.clone(),
        category,
        provenance,
        path: PathBuf::from(&stored.path),
        bytes: stored.bytes,
        mtime_max: stored.mtime_max,
        hardlinked: stored.hardlinked.unwrap_or(true),
        growth_bytes: stored.growth_bytes,
        regrowth_count: stored.regrowth_count,
        observed_at: stored.observed_at,
        consumers,
        note: stored.consequence.clone(),
        evidence: Vec::new(),
    }
}

/// Rebuilds one `AgentUnit` from its stored row and its
/// `agent_unit_members.parquet` rows. Every field is read from
/// `stored`'s own typed columns (`tool_home`/`relative_path`/
/// `hardlinked`/`action`) and `members` from `member_rows` (R18a); there
/// is no JSON merge fallback, same discipline as
/// `external_unit_from_stored`. `evidence` is left empty for the same
/// reason as `external_unit_from_stored`.
pub(crate) fn agent_unit_from_stored(
    stored: &columns::StoredUnitRow,
    member_rows: &[&columns::StoredAgentMemberRow],
) -> crate::agents::AgentUnit {
    let category = crate::agents::AgentCategory::from_label(&stored.category)
        .unwrap_or(crate::agents::AgentCategory::Unclassified);
    let project_link = stored
        .linkage_state
        .as_deref()
        .map(|s| {
            project_link_state_from_columns(
                s,
                stored.linkage_basis.as_deref(),
                stored.project_id.as_deref(),
            )
        })
        .unwrap_or(crate::agents::ProjectLinkState::Unresolved {
            reason: "no stored linkage state".to_string(),
        });
    let members = agent_members_from_stored(member_rows);
    crate::agents::AgentUnit {
        tool_id: stored.source_id.clone(),
        tool_name: stored.source_name.clone(),
        tool_home: stored
            .tool_home
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_default(),
        category,
        id: stored.id.clone(),
        relative_path: stored.relative_path.clone().unwrap_or_default(),
        path: PathBuf::from(&stored.path),
        members,
        bytes: stored.bytes,
        hardlinked: stored.hardlinked.unwrap_or(true),
        complete: stored.complete.unwrap_or(true),
        growth_bytes: stored.growth_bytes,
        regrowth_count: stored.regrowth_count,
        observed_at: stored.observed_at,
        mtime_max: stored.mtime_max,
        protected: stored.protected.unwrap_or(false),
        protect_reason: stored.protect_reason.clone(),
        project_link,
        action: stored
            .action
            .as_deref()
            .and_then(crate::agents::AgentActionCapability::from_label)
            .unwrap_or(crate::agents::AgentActionCapability::None),
        note: stored.consequence.clone(),
        evidence: Vec::new(),
    }
}

// ---------------------------------------------------------------------
// nested_artifacts.parquet (R16 item 2).
// ---------------------------------------------------------------------

fn nested_artifacts_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("nested_artifacts.parquet")
}

const NESTED_ORIGIN_REPORT: &str = "report";
const NESTED_ORIGIN_STORE_INTERIOR: &str = "store-interior";

fn stored_row_from_nested_artifact(
    scope_key: &str,
    origin: &str,
    n: &crate::artifact::NestedArtifact,
) -> columns::StoredNestedArtifactRow {
    let (action_capability, action_unsupported_reason) = match &n.action {
        crate::artifact::NestedActionCapability::TrashPath => (n.action.label().to_string(), None),
        crate::artifact::NestedActionCapability::InspectionOnly => {
            (n.action.label().to_string(), None)
        }
        crate::artifact::NestedActionCapability::Unsupported { reason } => {
            (n.action.label().to_string(), Some(reason.clone()))
        }
    };
    columns::StoredNestedArtifactRow {
        scope_key: scope_key.to_string(),
        origin: origin.to_string(),
        id: n.id.clone(),
        relative_path: n.relative_path.clone(),
        parent_id: n.parent_id.clone(),
        container_id: n.container_id.clone(),
        adapter: n.adapter.clone(),
        family: n.role.family().label().to_string(),
        role: n.role.label().to_string(),
        path: n.path.display().to_string(),
        membership: n.membership.label().to_string(),
        is_dir: n.is_dir,
        device: n.device,
        inode: n.inode,
        logical_bytes: n.logical_bytes,
        bytes: n.bytes,
        physical_bytes: n.physical_bytes,
        physical_total: n.physical_total,
        basis: n.basis.label().to_string(),
        mtime: Some(n.mtime_max),
        time_source: n.time_source.label().to_string(),
        coverage_supported: n.coverage.supported,
        coverage_complete: n.coverage.complete,
        action_group: n.action_group.clone(),
        present: n.present,
        growth_bytes: n.growth_bytes,
        regrowth_count: n.regrowth_count,
        action_capability,
        action_unsupported_reason,
        consequence: n.consequence.clone(),
        reported_by: n.reported_by.clone(),
        writer_lock: n.writer_lock.as_ref().map(|p| p.display().to_string()),
        variant_profile: n.variant.profile.clone(),
        variant_configuration: n.variant.configuration.clone(),
        variant_target: n.variant.target.clone(),
        variant_arch: n.variant.architecture.clone(),
        variant_package: n.variant.package.clone(),
        variant_version: n.variant.version.clone(),
        variant_toolchain: n.variant.toolchain.clone(),
        variant_features: n.variant.features.clone(),
        variant_generation: n.variant.generation.clone(),
        guidance_recommendation: Some(n.guidance.recommendation.clone()),
        guidance_modified_age_secs: n.guidance.modified_age_secs,
        guidance_consequence: Some(n.guidance.consequence.clone()),
        guidance_scope: Some(n.guidance.scope.clone()),
        guidance_check_status: Some(n.guidance.check_status.clone()),
        guidance_reason_code: Some(n.guidance.reason_code.clone()),
        guidance_message: Some(n.guidance.message.clone()),
        guidance_next_action: Some(n.guidance.next_action.clone()),
    }
}

const NESTED_LIST_KIND_COVERAGE_LIMIT: &str = "coverage-limit";
const NESTED_LIST_KIND_VARIANT_UNKNOWN: &str = "variant-unknown";
const NESTED_EVIDENCE_KIND_PRODUCER: &str = "producer";
const NESTED_EVIDENCE_KIND_CONSUMER: &str = "consumer";

/// `nested_artifact_lists.parquet` rows for one `NestedArtifact`'s two
/// list-valued fields (`coverage.limits`, `variant.unknowns`) -- R18a-2.
fn nested_artifact_list_rows(
    scope_key: &str,
    origin: &str,
    n: &crate::artifact::NestedArtifact,
) -> Vec<columns::StoredNestedArtifactListRow> {
    let mut rows = Vec::new();
    for (seq, value) in n.coverage.limits.iter().enumerate() {
        rows.push(columns::StoredNestedArtifactListRow {
            scope_key: scope_key.to_string(),
            origin: origin.to_string(),
            artifact_id: n.id.clone(),
            list_kind: NESTED_LIST_KIND_COVERAGE_LIMIT.to_string(),
            seq: seq as u32,
            value: value.clone(),
        });
    }
    for (seq, value) in n.variant.unknowns.iter().enumerate() {
        rows.push(columns::StoredNestedArtifactListRow {
            scope_key: scope_key.to_string(),
            origin: origin.to_string(),
            artifact_id: n.id.clone(),
            list_kind: NESTED_LIST_KIND_VARIANT_UNKNOWN.to_string(),
            seq: seq as u32,
            value: value.clone(),
        });
    }
    rows
}

/// `nested_artifact_evidence.parquet` rows for one `NestedArtifact`'s
/// `producer_evidence`/`consumer_evidence` (the narrower
/// `ArtifactEvidence` shape, distinct from `decision_evidence`) -- R18a-2.
fn nested_artifact_evidence_rows(
    scope_key: &str,
    origin: &str,
    n: &crate::artifact::NestedArtifact,
) -> Vec<columns::StoredNestedArtifactEvidenceRow> {
    let mut rows = Vec::new();
    for (seq, e) in n.producer_evidence.iter().enumerate() {
        rows.push(columns::StoredNestedArtifactEvidenceRow {
            scope_key: scope_key.to_string(),
            origin: origin.to_string(),
            artifact_id: n.id.clone(),
            kind: NESTED_EVIDENCE_KIND_PRODUCER.to_string(),
            seq: seq as u32,
            source: e.source.clone(),
            detail: e.detail.clone(),
            confidence: e.confidence.label().to_string(),
        });
    }
    for (seq, e) in n.consumer_evidence.iter().enumerate() {
        rows.push(columns::StoredNestedArtifactEvidenceRow {
            scope_key: scope_key.to_string(),
            origin: origin.to_string(),
            artifact_id: n.id.clone(),
            kind: NESTED_EVIDENCE_KIND_CONSUMER.to_string(),
            seq: seq as u32,
            source: e.source.clone(),
            detail: e.detail.clone(),
            confidence: e.confidence.label().to_string(),
        });
    }
    rows
}

fn nested_artifact_lists_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("nested_artifact_lists.parquet")
}

fn nested_artifact_evidence_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("nested_artifact_evidence.parquet")
}

/// Writes `nested_artifacts.parquet` (+ `nested_artifact_lists.parquet`/
/// `nested_artifact_evidence.parquet` for the list-valued fields),
/// replacing that scope's rows wholesale, from the two already-measured
/// lists this same pass produced (`Report.nested_artifacts` and
/// `ReportSnapshot.store_interiors`) -- no second pass over either.
pub fn write_nested_artifact_table(
    swamp_dir: &Path,
    scope_key: &str,
    report_nested: &[crate::artifact::NestedArtifact],
    store_interiors: &[crate::artifact::NestedArtifact],
) -> Result<()> {
    store::StoreDir::at(swamp_dir)?.create()?;
    let file = nested_artifacts_path(swamp_dir);
    let mut rows: Vec<columns::StoredNestedArtifactRow> = columns::read_nested_artifact_rows(&file)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.scope_key != scope_key)
        .collect();
    rows.extend(
        report_nested
            .iter()
            .map(|n| stored_row_from_nested_artifact(scope_key, NESTED_ORIGIN_REPORT, n)),
    );
    rows.extend(
        store_interiors
            .iter()
            .map(|n| stored_row_from_nested_artifact(scope_key, NESTED_ORIGIN_STORE_INTERIOR, n)),
    );
    columns::write_nested_artifact_rows(&file, &rows)
        .with_context(|| format!("write {}", file.display()))?;

    let lists_file = nested_artifact_lists_path(swamp_dir);
    let mut list_rows: Vec<columns::StoredNestedArtifactListRow> =
        columns::read_nested_artifact_list_rows(&lists_file)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.scope_key != scope_key)
            .collect();
    for n in report_nested {
        list_rows.extend(nested_artifact_list_rows(
            scope_key,
            NESTED_ORIGIN_REPORT,
            n,
        ));
    }
    for n in store_interiors {
        list_rows.extend(nested_artifact_list_rows(
            scope_key,
            NESTED_ORIGIN_STORE_INTERIOR,
            n,
        ));
    }
    columns::write_nested_artifact_list_rows(&lists_file, &list_rows)
        .with_context(|| format!("write {}", lists_file.display()))?;

    let evidence_file = nested_artifact_evidence_path(swamp_dir);
    let mut evidence_rows: Vec<columns::StoredNestedArtifactEvidenceRow> =
        columns::read_nested_artifact_evidence_rows(&evidence_file)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.scope_key != scope_key)
            .collect();
    for n in report_nested {
        evidence_rows.extend(nested_artifact_evidence_rows(
            scope_key,
            NESTED_ORIGIN_REPORT,
            n,
        ));
    }
    for n in store_interiors {
        evidence_rows.extend(nested_artifact_evidence_rows(
            scope_key,
            NESTED_ORIGIN_STORE_INTERIOR,
            n,
        ));
    }
    columns::write_nested_artifact_evidence_rows(&evidence_file, &evidence_rows)
        .with_context(|| format!("write {}", evidence_file.display()))
}

/// One `nested_artifacts.parquet` row plus its `nested_artifact_lists.
/// parquet`/`nested_artifact_evidence.parquet` child rows (keyed by
/// `(origin, artifact_id)`).
pub(crate) type NestedArtifactStoredWithChildren = (
    columns::StoredNestedArtifactRow,
    Vec<columns::StoredNestedArtifactListRow>,
    Vec<columns::StoredNestedArtifactEvidenceRow>,
);

/// Every stored `nested_artifacts.parquet` row for `scope_key`, split
/// back into the two lists it was written from (`origin`), each paired
/// with its child rows. `None` when there are no rows yet (an older
/// store).
pub(crate) fn read_nested_artifact_table(
    swamp_dir: &Path,
    scope_key: &str,
) -> Option<(
    Vec<NestedArtifactStoredWithChildren>,
    Vec<NestedArtifactStoredWithChildren>,
)> {
    let all: Vec<columns::StoredNestedArtifactRow> =
        columns::read_nested_artifact_rows(&nested_artifacts_path(swamp_dir))
            .ok()?
            .into_iter()
            .filter(|r| r.scope_key == scope_key)
            .collect();
    if all.is_empty() {
        return None;
    }
    let list_rows: Vec<columns::StoredNestedArtifactListRow> =
        columns::read_nested_artifact_list_rows(&nested_artifact_lists_path(swamp_dir))
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.scope_key == scope_key)
            .collect();
    let evidence_rows: Vec<columns::StoredNestedArtifactEvidenceRow> =
        columns::read_nested_artifact_evidence_rows(&nested_artifact_evidence_path(swamp_dir))
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.scope_key == scope_key)
            .collect();

    let attach = |row: columns::StoredNestedArtifactRow| {
        let lists: Vec<columns::StoredNestedArtifactListRow> = list_rows
            .iter()
            .filter(|l| l.origin == row.origin && l.artifact_id == row.id)
            .cloned()
            .collect();
        let evidence: Vec<columns::StoredNestedArtifactEvidenceRow> = evidence_rows
            .iter()
            .filter(|e| e.origin == row.origin && e.artifact_id == row.id)
            .cloned()
            .collect();
        (row, lists, evidence)
    };

    let (report, store_interior): (
        Vec<columns::StoredNestedArtifactRow>,
        Vec<columns::StoredNestedArtifactRow>,
    ) = all
        .into_iter()
        .partition(|r| r.origin == NESTED_ORIGIN_REPORT);
    Some((
        report.into_iter().map(attach).collect(),
        store_interior.into_iter().map(attach).collect(),
    ))
}

/// Rebuilds one `NestedArtifact` from its stored row and its
/// `nested_artifact_lists.parquet`/`nested_artifact_evidence.parquet`
/// child rows. Every field is read from typed storage (R18a-2); there is
/// no JSON merge fallback. `decision_evidence` is left empty for
/// `rebuild_evidence_from_table` to fill.
pub(crate) fn nested_artifact_from_stored(
    stored: &columns::StoredNestedArtifactRow,
    lists: &[columns::StoredNestedArtifactListRow],
    evidence: &[columns::StoredNestedArtifactEvidenceRow],
) -> crate::artifact::NestedArtifact {
    let role = crate::artifact::ArtifactRole::from_label(&stored.role)
        .unwrap_or(crate::artifact::ArtifactRole::Unknown);
    let basis = crate::artifact::AccountingBasis::from_label(&stored.basis)
        .unwrap_or(crate::artifact::AccountingBasis::Unknown);

    let mut coverage_limits: Vec<&columns::StoredNestedArtifactListRow> = lists
        .iter()
        .filter(|l| l.list_kind == NESTED_LIST_KIND_COVERAGE_LIMIT)
        .collect();
    coverage_limits.sort_by_key(|l| l.seq);
    let mut variant_unknowns: Vec<&columns::StoredNestedArtifactListRow> = lists
        .iter()
        .filter(|l| l.list_kind == NESTED_LIST_KIND_VARIANT_UNKNOWN)
        .collect();
    variant_unknowns.sort_by_key(|l| l.seq);

    let mut producer_evidence: Vec<&columns::StoredNestedArtifactEvidenceRow> = evidence
        .iter()
        .filter(|e| e.kind == NESTED_EVIDENCE_KIND_PRODUCER)
        .collect();
    producer_evidence.sort_by_key(|e| e.seq);
    let mut consumer_evidence: Vec<&columns::StoredNestedArtifactEvidenceRow> = evidence
        .iter()
        .filter(|e| e.kind == NESTED_EVIDENCE_KIND_CONSUMER)
        .collect();
    consumer_evidence.sort_by_key(|e| e.seq);

    let to_artifact_evidence =
        |e: &columns::StoredNestedArtifactEvidenceRow| crate::artifact::ArtifactEvidence {
            source: e.source.clone(),
            detail: e.detail.clone(),
            confidence: crate::entities::Confidence::from_label(&e.confidence),
        };

    let variant = crate::artifact::ArtifactVariant {
        profile: stored.variant_profile.clone(),
        configuration: stored.variant_configuration.clone(),
        target: stored.variant_target.clone(),
        architecture: stored.variant_arch.clone(),
        package: stored.variant_package.clone(),
        version: stored.variant_version.clone(),
        toolchain: stored.variant_toolchain.clone(),
        features: stored.variant_features.clone(),
        generation: stored.variant_generation.clone(),
        unknowns: variant_unknowns.iter().map(|l| l.value.clone()).collect(),
    };
    crate::artifact::NestedArtifact {
        id: stored.id.clone(),
        path: PathBuf::from(&stored.path),
        relative_path: stored.relative_path.clone(),
        parent_id: stored.parent_id.clone(),
        container_id: stored.container_id.clone(),
        role,
        membership: crate::artifact::Membership::from_label(&stored.membership),
        is_dir: stored.is_dir,
        device: stored.device,
        inode: stored.inode,
        logical_bytes: stored.logical_bytes,
        bytes: stored.bytes,
        physical_bytes: stored.physical_bytes,
        physical_total: stored.physical_total,
        mtime_max: stored.mtime.unwrap_or(0),
        variant,
        producer_evidence: producer_evidence
            .into_iter()
            .map(to_artifact_evidence)
            .collect(),
        consumer_evidence: consumer_evidence
            .into_iter()
            .map(to_artifact_evidence)
            .collect(),
        coverage: crate::artifact::ArtifactCoverage {
            supported: stored.coverage_supported,
            complete: stored.coverage_complete,
            limits: coverage_limits.iter().map(|l| l.value.clone()).collect(),
        },
        action_group: stored.action_group.clone(),
        present: stored.present,
        growth_bytes: stored.growth_bytes,
        regrowth_count: stored.regrowth_count,
        decision_evidence: Vec::new(),
        adapter: stored.adapter.clone(),
        basis,
        time_source: crate::artifact::TimeSource::from_label(&stored.time_source),
        action: crate::artifact::NestedActionCapability::from_label(
            &stored.action_capability,
            stored.action_unsupported_reason.clone(),
        ),
        consequence: stored.consequence.clone(),
        reported_by: stored.reported_by.clone(),
        writer_lock: stored.writer_lock.as_ref().map(PathBuf::from),
        guidance: match &stored.guidance_recommendation {
            Some(recommendation) => crate::cargo_cleanup::Guidance {
                recommendation: recommendation.clone(),
                modified_age_secs: stored.guidance_modified_age_secs,
                consequence: stored.guidance_consequence.clone().unwrap_or_default(),
                scope: stored.guidance_scope.clone().unwrap_or_default(),
                check_status: stored.guidance_check_status.clone().unwrap_or_default(),
                reason_code: stored.guidance_reason_code.clone().unwrap_or_default(),
                message: stored.guidance_message.clone().unwrap_or_default(),
                next_action: stored.guidance_next_action.clone().unwrap_or_default(),
            },
            // Never recompute from a live clock here (that is exactly
            // the bug R18a's first session fixed) -- a row written by
            // this slice's writer always carries a recommendation, so
            // this is only reachable for a row from a store older than
            // that fix, where a bare default is the honest answer.
            None => crate::cargo_cleanup::Guidance::default(),
        },
    }
}

// ---------------------------------------------------------------------
// git_signals.parquet (R18a-4). `consumers/signals.rs`'s per-root
// replay cache: what an unchanged worktree's git activity was last
// measured as, so an incremental pass ages it forward (`crate::signals
// ::age_signals`) instead of re-opening the repository. Replaces the
// git-activity slice of `last_report-<key>.json.zst`. Root-keyed
// (`root_key`, above), wholesale-replaced per root -- never scope-keyed,
// so a single-root, scope-less call (every pre-#42 entry point, plus
// every per-root pass inside a scope) reads and writes this exactly the
// same way.
// ---------------------------------------------------------------------

fn git_signals_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("git_signals.parquet")
}

fn git_signal_values_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("git_signals_values.parquet")
}

/// Writes this root's per-worktree git signals, replacing every row this
/// `root_key` owns in both tables. Called from `consumers::signals`
/// itself, once per pass, right after `by_worktree` is complete (both
/// the worktrees this pass replayed from the previous table and the
/// ones it re-walked) -- deliberately *before* `gate.rs` appends
/// `merge_complete`/`pull_request` to a `WorktreeRow`'s signals from the
/// GitHub facts this same pass fetched: those two are re-derived fresh
/// every pass regardless of replay, so storing pre-enrichment rows means
/// a replay never needs to filter them back out (the JSON cache this
/// replaces did, because it persisted the fully merged `Report`).
pub(crate) fn write_git_signals_table(
    swamp_dir: &Path,
    root_key: &str,
    observed_at: u64,
    by_worktree: &HashMap<String, WorktreeSignals>,
) -> Result<()> {
    store::StoreDir::at(swamp_dir)?.create()?;
    let file = git_signals_path(swamp_dir);
    let mut rows: Vec<columns::StoredGitSignalRow> = columns::read_git_signal_rows(&file)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.root_key != root_key)
        .collect();
    rows.extend(
        by_worktree
            .iter()
            .map(|(id, w)| columns::StoredGitSignalRow {
                root_key: root_key.to_string(),
                worktree_id: id.clone(),
                branch: w.branch.clone(),
                last_commit_age_secs: w.raw.last_commit_age_secs,
                dirty: w.raw.dirty,
                unpushed: w.raw.unpushed,
                locked: w.raw.locked,
                idle_for_secs: w.raw.idle_for_secs,
                observed_at,
            }),
    );
    columns::write_git_signal_rows(&file, &rows)
        .with_context(|| format!("write {}", file.display()))?;

    let values_file = git_signal_values_path(swamp_dir);
    let mut value_rows: Vec<columns::StoredGitSignalValueRow> =
        columns::read_git_signal_value_rows(&values_file)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.root_key != root_key)
            .collect();
    for (id, w) in by_worktree {
        for (seq, s) in w.rows.iter().enumerate() {
            value_rows.push(columns::StoredGitSignalValueRow {
                root_key: root_key.to_string(),
                worktree_id: id.clone(),
                seq: seq as u32,
                name: s.name.clone(),
                value: s.value.clone(),
            });
        }
    }
    columns::write_git_signal_value_rows(&values_file, &value_rows)
        .with_context(|| format!("write {}", values_file.display()))
}

/// One worktree's previously stored git signals: `branch` plus the raw
/// and rendered forms `crate::signals::age_signals` needs.
pub(crate) struct StoredWorktreeGitSignals {
    pub branch: Option<String>,
    pub raw: RawSignals,
    pub rows: Vec<Signal>,
}

/// This root's previous git-signals pass: `(observed_at, by_worktree)`,
/// or `None` when nothing has ever been stored for this root (a fresh
/// store, or a root observed for the first time).
pub(crate) fn read_git_signals_table(
    swamp_dir: &Path,
    root_key: &str,
) -> Option<(u64, HashMap<String, StoredWorktreeGitSignals>)> {
    let rows: Vec<columns::StoredGitSignalRow> =
        columns::read_git_signal_rows(&git_signals_path(swamp_dir))
            .ok()?
            .into_iter()
            .filter(|r| r.root_key == root_key)
            .collect();
    if rows.is_empty() {
        return None;
    }
    let observed_at = rows.iter().map(|r| r.observed_at).max()?;
    let value_rows: Vec<columns::StoredGitSignalValueRow> =
        columns::read_git_signal_value_rows(&git_signal_values_path(swamp_dir))
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.root_key == root_key)
            .collect();
    let mut by_worktree = HashMap::new();
    for r in rows {
        let mut values: Vec<&columns::StoredGitSignalValueRow> = value_rows
            .iter()
            .filter(|v| v.worktree_id == r.worktree_id)
            .collect();
        values.sort_by_key(|v| v.seq);
        let signal_rows: Vec<Signal> = values
            .into_iter()
            .map(|v| Signal {
                name: v.name.clone(),
                value: v.value.clone(),
            })
            .collect();
        by_worktree.insert(
            r.worktree_id.clone(),
            StoredWorktreeGitSignals {
                branch: r.branch,
                raw: RawSignals {
                    last_commit_age_secs: r.last_commit_age_secs,
                    dirty: r.dirty,
                    unpushed: r.unpushed,
                    locked: r.locked,
                    idle_for_secs: r.idle_for_secs,
                },
                rows: signal_rows,
            },
        );
    }
    Some((observed_at, by_worktree))
}

// ---------------------------------------------------------------------
// cargo_replay_cache.parquet / cargo_replay_cache_lists.parquet /
// cargo_replay_cache_evidence.parquet (R18a-4). `consumers/cargo.rs`'s
// per-root replay cache for `build_adapters::ContainerCache::from_previous`:
// the previous pass's `NestedArtifact` units for this root, root-keyed
// exactly like `git_signals.parquet` above, replacing the
// nested-artifacts slice of `last_report-<key>.json.zst`.
//
// This reuses `nested_artifacts.parquet`'s own row shapes and
// stored<->domain conversions (`stored_row_from_nested_artifact`,
// `nested_artifact_list_rows`, `nested_artifact_evidence_rows`,
// `nested_artifact_from_stored`) verbatim -- every `NestedArtifact`
// field this cache needs was already typed there (R16/R18a-2) -- but
// writes them to their own, root-keyed files rather than extending
// `nested_artifacts.parquet` itself. `nested_artifacts.parquet` is
// scope-keyed and wholesale-replaced exactly once, at the end of
// `observe_scope`, after every root's bus pass has already run and
// merged (`report::observe_scope`'s `write_nested_artifact_table` call);
// it is also never written at all for a plain single-root,
// scope-less call (`report_full_mode_with_source` et al., which have no
// `ObservationParts::ALL`/scope-wide unit discovery to gate it on). A
// per-root incremental pass's replay decision must be available *during*
// that same root's own bus run, for both kinds of caller -- exactly the
// "never scope-keyed" requirement `git_signals.parquet` above states --
// so this is a second, root-keyed file, not a new column on the first.
// ---------------------------------------------------------------------

const CARGO_REPLAY_ORIGIN: &str = "cargo-replay-cache";

fn cargo_replay_cache_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("cargo_replay_cache.parquet")
}

fn cargo_replay_cache_lists_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("cargo_replay_cache_lists.parquet")
}

fn cargo_replay_cache_evidence_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("cargo_replay_cache_evidence.parquet")
}

fn cargo_replay_cache_meta_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("cargo_replay_cache_meta.parquet")
}

/// Writes this root's nested-artifact replay cache, replacing every row
/// this `root_key` owns across all four files. Called from
/// `consumers::cargo` after this pass's `NestedArtifact` identification
/// finishes, so the *next* pass over this root -- unless it force-full
/// walks -- replays from exactly what this pass measured.
pub(crate) fn write_cargo_replay_cache(
    swamp_dir: &Path,
    root_key: &str,
    units: &[crate::artifact::NestedArtifact],
    observed_at: u64,
) -> Result<()> {
    store::StoreDir::at(swamp_dir)?.create()?;

    let file = cargo_replay_cache_path(swamp_dir);
    let mut rows: Vec<columns::StoredNestedArtifactRow> = columns::read_nested_artifact_rows(&file)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.scope_key != root_key)
        .collect();
    rows.extend(
        units
            .iter()
            .map(|n| stored_row_from_nested_artifact(root_key, CARGO_REPLAY_ORIGIN, n)),
    );
    columns::write_nested_artifact_rows(&file, &rows)
        .with_context(|| format!("write {}", file.display()))?;

    let lists_file = cargo_replay_cache_lists_path(swamp_dir);
    let mut list_rows: Vec<columns::StoredNestedArtifactListRow> =
        columns::read_nested_artifact_list_rows(&lists_file)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.scope_key != root_key)
            .collect();
    for n in units {
        list_rows.extend(nested_artifact_list_rows(root_key, CARGO_REPLAY_ORIGIN, n));
    }
    columns::write_nested_artifact_list_rows(&lists_file, &list_rows)
        .with_context(|| format!("write {}", lists_file.display()))?;

    let evidence_file = cargo_replay_cache_evidence_path(swamp_dir);
    let mut evidence_rows: Vec<columns::StoredNestedArtifactEvidenceRow> =
        columns::read_nested_artifact_evidence_rows(&evidence_file)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.scope_key != root_key)
            .collect();
    for n in units {
        evidence_rows.extend(nested_artifact_evidence_rows(
            root_key,
            CARGO_REPLAY_ORIGIN,
            n,
        ));
    }
    columns::write_nested_artifact_evidence_rows(&evidence_file, &evidence_rows)
        .with_context(|| format!("write {}", evidence_file.display()))?;

    let meta_file = cargo_replay_cache_meta_path(swamp_dir);
    let mut meta_rows: Vec<columns::StoredCargoReplayMetaRow> =
        columns::read_cargo_replay_meta_rows(&meta_file)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.root_key != root_key)
            .collect();
    meta_rows.push(columns::StoredCargoReplayMetaRow {
        root_key: root_key.to_string(),
        observed_at,
    });
    columns::write_cargo_replay_meta_rows(&meta_file, &meta_rows)
        .with_context(|| format!("write {}", meta_file.display()))
}

/// This root's previous nested-artifact pass: `(observed_at, units)`, or
/// `None` when this root has never been observed. Presence is decided
/// by the meta row, not by `units` being non-empty -- a root that
/// genuinely had zero nested artifacts last pass is still a real
/// previous observation, not "never observed".
pub fn read_cargo_replay_cache(
    swamp_dir: &Path,
    root_key: &str,
) -> Option<(u64, Vec<crate::artifact::NestedArtifact>)> {
    let meta_rows =
        columns::read_cargo_replay_meta_rows(&cargo_replay_cache_meta_path(swamp_dir)).ok()?;
    let observed_at = meta_rows
        .iter()
        .find(|r| r.root_key == root_key)?
        .observed_at;

    let rows: Vec<columns::StoredNestedArtifactRow> =
        columns::read_nested_artifact_rows(&cargo_replay_cache_path(swamp_dir))
            .ok()?
            .into_iter()
            .filter(|r| r.scope_key == root_key)
            .collect();
    let list_rows: Vec<columns::StoredNestedArtifactListRow> =
        columns::read_nested_artifact_list_rows(&cargo_replay_cache_lists_path(swamp_dir))
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.scope_key == root_key)
            .collect();
    let evidence_rows: Vec<columns::StoredNestedArtifactEvidenceRow> =
        columns::read_nested_artifact_evidence_rows(&cargo_replay_cache_evidence_path(swamp_dir))
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.scope_key == root_key)
            .collect();

    let units = rows
        .iter()
        .map(|r| {
            let lists: Vec<columns::StoredNestedArtifactListRow> = list_rows
                .iter()
                .filter(|l| l.artifact_id == r.id)
                .cloned()
                .collect();
            let evidence: Vec<columns::StoredNestedArtifactEvidenceRow> = evidence_rows
                .iter()
                .filter(|e| e.artifact_id == r.id)
                .cloned()
                .collect();
            nested_artifact_from_stored(r, &lists, &evidence)
        })
        .collect();
    Some((observed_at, units))
}

// ---------------------------------------------------------------------
// evidence.parquet (R16 item 3). One row per `crate::evidence::Evidence`
// entry across every entity kind that carries the #53 decision-evidence
// contract: an `ArtifactRow` (keyed by `artifact_row_key`), an
// `ExternalUnit`/`AgentUnit` (keyed by the same `id` the unit tables
// use) and a `NestedArtifact`'s `decision_evidence` (keyed by its own
// `id`). Every label function here is an exhaustive `match`, not a
// serde round-trip, matching every other `label`/`from_label` pair in
// this codebase (a new `FactKind`/`FactSubtype`/`EvidenceSource`
// variant is a compile error here, not a silently-dropped fact).
// ---------------------------------------------------------------------

fn evidence_path(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("evidence.parquet")
}

/// Whether `evidence.parquet` exists at all -- distinct from "has no
/// rows for this scope", which is a legitimate outcome once the table
/// exists (a scope whose entities happen to carry no evidence this
/// pass). An older store with no such file yet must leave every
/// entity's `evidence` exactly as the snapshot already has it (the
/// same no-op-on-an-older-store contract every other R16 table keeps),
/// not clear it.
pub(crate) fn evidence_table_exists(swamp_dir: &Path) -> bool {
    crate::fs_gate::is_file(evidence_path(swamp_dir))
}

/// `"<entity_kind>:<id>"` -- the one identity space `evidence.parquet`
/// keys every entity kind under.
pub(crate) fn evidence_row_key(entity_kind: &str, id: &str) -> String {
    format!("{entity_kind}:{id}")
}

fn fact_kind_label(k: crate::evidence::FactKind) -> &'static str {
    use crate::evidence::FactKind::*;
    match k {
        Activity => "activity",
        Consumer => "consumer",
        CurrentUse => "current-use",
        Recovery => "recovery",
        Reclaimability => "reclaimability",
    }
}

fn fact_kind_from_label(s: &str) -> crate::evidence::FactKind {
    use crate::evidence::FactKind::*;
    match s {
        "consumer" => Consumer,
        "current-use" => CurrentUse,
        "recovery" => Recovery,
        "reclaimability" => Reclaimability,
        _ => Activity,
    }
}

fn fact_subtype_label(s: crate::evidence::FactSubtype) -> &'static str {
    use crate::evidence::FactSubtype::*;
    match s {
        Modified => "modified",
        Accessed => "accessed",
        ToolReportedUse => "tool-reported-use",
        DeclaredConsumer => "declared-consumer",
        InferredConsumer => "inferred-consumer",
        Process => "process",
        OpenFile => "open-file",
        Lock => "lock",
        RunningContainer => "running-container",
        Mounted => "mounted",
        Booted => "booted",
        Rebuild => "rebuild",
        NetworkFetch => "network-fetch",
        LocalReinstall => "local-reinstall",
        TrashRecovery => "trash-recovery",
        BackupDependent => "backup-dependent",
        PotentiallyUniqueLocalState => "potentially-unique-local-state",
        UnknownPrerequisites => "unknown-prerequisites",
        LogicalBytes => "logical-bytes",
        AllocatedBytes => "allocated-bytes",
        EstimatedReclaimable => "estimated-reclaimable",
        ObservedFreed => "observed-freed",
    }
}

fn fact_subtype_from_label(s: &str) -> crate::evidence::FactSubtype {
    use crate::evidence::FactSubtype::*;
    match s {
        "accessed" => Accessed,
        "tool-reported-use" => ToolReportedUse,
        "declared-consumer" => DeclaredConsumer,
        "inferred-consumer" => InferredConsumer,
        "process" => Process,
        "open-file" => OpenFile,
        "lock" => Lock,
        "running-container" => RunningContainer,
        "mounted" => Mounted,
        "booted" => Booted,
        "rebuild" => Rebuild,
        "network-fetch" => NetworkFetch,
        "local-reinstall" => LocalReinstall,
        "trash-recovery" => TrashRecovery,
        "backup-dependent" => BackupDependent,
        "potentially-unique-local-state" => PotentiallyUniqueLocalState,
        "unknown-prerequisites" => UnknownPrerequisites,
        "logical-bytes" => LogicalBytes,
        "allocated-bytes" => AllocatedBytes,
        "estimated-reclaimable" => EstimatedReclaimable,
        "observed-freed" => ObservedFreed,
        _ => Modified,
    }
}

/// Encodes one `FactValue` as `(value_kind, value_num, value_ts,
/// value_text)`; the inverse of [`fact_value_from_columns`].
fn fact_value_to_columns(
    v: &crate::evidence::FactValue,
) -> (&'static str, Option<f64>, Option<i64>, Option<String>) {
    use crate::evidence::FactValue::*;
    match v {
        Timestamp(t) => ("timestamp", None, Some(*t as i64), None),
        Bool(b) => ("bool", Some(if *b { 1.0 } else { 0.0 }), None, None),
        Text(t) => ("text", None, None, Some(t.clone())),
        Bytes(b) => ("bytes", Some(*b as f64), None, None),
        SignedBytes(b) => ("signed-bytes", Some(*b as f64), None, None),
        Count(c) => ("count", Some(*c as f64), None, None),
        List(l) => ("list", None, None, Some(l.join("|"))),
    }
}

/// A `FactValue`'s own textual form, used for `Conflicting`'s
/// `candidates` (`conflicting_extra`, `|`-joined) -- distinct from
/// `fact_value_to_columns`'s three-column split, since several
/// candidates share one row.
fn fact_value_to_text(v: &crate::evidence::FactValue) -> String {
    use crate::evidence::FactValue::*;
    match v {
        Timestamp(t) => t.to_string(),
        Bool(b) => b.to_string(),
        Text(t) => t.clone(),
        Bytes(b) => b.to_string(),
        SignedBytes(b) => b.to_string(),
        Count(c) => c.to_string(),
        List(l) => l.join(","),
    }
}

fn fact_value_from_text(kind: &str, text: &str) -> crate::evidence::FactValue {
    use crate::evidence::FactValue::*;
    match kind {
        "timestamp" => Timestamp(text.parse().unwrap_or(0)),
        "bool" => Bool(text == "true" || text == "1"),
        "bytes" => Bytes(text.parse().unwrap_or(0)),
        "signed-bytes" => SignedBytes(text.parse().unwrap_or(0)),
        "count" => Count(text.parse().unwrap_or(0)),
        "list" => List(text.split(',').map(str::to_string).collect()),
        _ => Text(text.to_string()),
    }
}

fn fact_value_from_columns(
    kind: &str,
    value_num: Option<f64>,
    value_ts: Option<i64>,
    value_text: Option<&str>,
) -> crate::evidence::FactValue {
    use crate::evidence::FactValue::*;
    match kind {
        "timestamp" => Timestamp(value_ts.unwrap_or(0) as u64),
        "bool" => Bool(value_num.unwrap_or(0.0) != 0.0),
        "bytes" => Bytes(value_num.unwrap_or(0.0) as u64),
        "signed-bytes" => SignedBytes(value_num.unwrap_or(0.0) as i64),
        "count" => Count(value_num.unwrap_or(0.0) as u64),
        "list" => List(
            value_text
                .unwrap_or_default()
                .split('|')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
        ),
        _ => Text(value_text.unwrap_or_default().to_string()),
    }
}

fn evidence_source_to_columns(
    s: &crate::evidence::EvidenceSource,
) -> (&'static str, Option<String>) {
    use crate::evidence::EvidenceSource::*;
    match s {
        FilesystemMetadata { detail } => ("filesystem-metadata", Some(detail.clone())),
        ToolReported { tool, detail } => {
            ("tool-reported", Some(format!("{tool}{LINK_SEP}{detail}")))
        }
        ProcessQuery { tool } => ("process-query", Some(tool.clone())),
        ManagerLock { tool, path } => ("manager-lock", Some(format!("{tool}{LINK_SEP}{path}"))),
        ConfigDeclaration { path } => ("config-declaration", Some(path.clone())),
        Lockfile { ecosystem, path } => ("lockfile", Some(format!("{ecosystem}{LINK_SEP}{path}"))),
        BuildMetadata { path } => ("build-metadata", Some(path.clone())),
        DockerApi { detail } => ("docker-api", Some(detail.clone())),
        Statvfs => ("statvfs", None),
        Inferred { basis } => ("inferred", Some(basis.clone())),
    }
}

fn evidence_source_from_columns(
    kind: &str,
    detail: Option<&str>,
) -> crate::evidence::EvidenceSource {
    use crate::evidence::EvidenceSource::*;
    let detail = detail.unwrap_or_default();
    let two = || {
        let mut parts = detail.splitn(2, LINK_SEP);
        (
            parts.next().unwrap_or_default().to_string(),
            parts.next().unwrap_or_default().to_string(),
        )
    };
    match kind {
        "tool-reported" => {
            let (tool, d) = two();
            ToolReported { tool, detail: d }
        }
        "process-query" => ProcessQuery {
            tool: detail.to_string(),
        },
        "manager-lock" => {
            let (tool, path) = two();
            ManagerLock { tool, path }
        }
        "config-declaration" => ConfigDeclaration {
            path: detail.to_string(),
        },
        "lockfile" => {
            let (ecosystem, path) = two();
            Lockfile { ecosystem, path }
        }
        "build-metadata" => BuildMetadata {
            path: detail.to_string(),
        },
        "docker-api" => DockerApi {
            detail: detail.to_string(),
        },
        "statvfs" => Statvfs,
        "inferred" => Inferred {
            basis: detail.to_string(),
        },
        _ => FilesystemMetadata {
            detail: detail.to_string(),
        },
    }
}

/// Flattens one entity's `Vec<Evidence>` into ordered
/// `StoredEvidenceRow`s under `row_key`.
fn stored_evidence_rows(
    scope_key: &str,
    row_key: &str,
    evidence: &[crate::evidence::Evidence],
) -> Vec<columns::StoredEvidenceRow> {
    evidence
        .iter()
        .enumerate()
        .map(|(seq, e)| {
            let (status, value_kind, value_num, value_ts, value_text, conflicting_extra, reason) =
                match &e.status {
                    crate::evidence::FactStatus::Known(v) => {
                        let (vk, num, ts, text) = fact_value_to_columns(v);
                        ("known", Some(vk.to_string()), num, ts, text, None, None)
                    }
                    crate::evidence::FactStatus::Unknown { reason } => (
                        "unknown",
                        None,
                        None,
                        None,
                        None,
                        None,
                        Some(reason.to_string()),
                    ),
                    crate::evidence::FactStatus::Unavailable { reason } => (
                        "unavailable",
                        None,
                        None,
                        None,
                        None,
                        None,
                        Some(reason.to_string()),
                    ),
                    crate::evidence::FactStatus::Conflicting { candidates, reason } => {
                        let (vk, num, ts, text) = candidates
                            .first()
                            .map(fact_value_to_columns)
                            .unwrap_or(("text", None, None, None));
                        let extra = candidates
                            .iter()
                            .skip(1)
                            .map(fact_value_to_text)
                            .collect::<Vec<_>>()
                            .join("|");
                        (
                            "conflicting",
                            Some(vk.to_string()),
                            num,
                            ts,
                            text,
                            (!extra.is_empty()).then_some(extra),
                            Some(reason.to_string()),
                        )
                    }
                };
            let (source, source_detail) = evidence_source_to_columns(&e.source);
            columns::StoredEvidenceRow {
                scope_key: scope_key.to_string(),
                row_key: row_key.to_string(),
                seq: seq as u32,
                kind: fact_kind_label(e.kind).to_string(),
                subtype: fact_subtype_label(e.subtype).to_string(),
                status: status.to_string(),
                value_kind,
                value_num,
                value_ts,
                value_text,
                conflicting_extra,
                reason,
                source: source.to_string(),
                source_detail,
                event_at: e.event_at.map(|t| t as i64),
                observed_at: e.observed_at,
                freshness_expires_after_secs: e.freshness.expires_after_secs,
                freshness_coverage_note: e.freshness.coverage_note.clone(),
                note: e.note.clone(),
            }
        })
        .collect()
}

/// The inverse of [`stored_evidence_rows`]: `rows` must already be every
/// row for one `row_key`, in `seq` order.
fn evidence_from_stored_rows(
    rows: &[&columns::StoredEvidenceRow],
) -> Vec<crate::evidence::Evidence> {
    rows.iter()
        .map(|r| {
            let source = evidence_source_from_columns(&r.source, r.source_detail.as_deref());
            let status = match r.status.as_str() {
                "unknown" => crate::evidence::FactStatus::Unknown {
                    reason: crate::reason!("{}", r.reason.clone().unwrap_or_default()),
                },
                "unavailable" => crate::evidence::FactStatus::Unavailable {
                    reason: crate::reason!("{}", r.reason.clone().unwrap_or_default()),
                },
                "conflicting" => {
                    let vk = r.value_kind.as_deref().unwrap_or("text");
                    let mut candidates = vec![fact_value_from_columns(
                        vk,
                        r.value_num,
                        r.value_ts,
                        r.value_text.as_deref(),
                    )];
                    if let Some(extra) = &r.conflicting_extra {
                        candidates.extend(extra.split('|').map(|t| fact_value_from_text(vk, t)));
                    }
                    crate::evidence::FactStatus::Conflicting {
                        candidates,
                        reason: crate::reason!("{}", r.reason.clone().unwrap_or_default()),
                    }
                }
                _ => crate::evidence::FactStatus::Known(fact_value_from_columns(
                    r.value_kind.as_deref().unwrap_or("text"),
                    r.value_num,
                    r.value_ts,
                    r.value_text.as_deref(),
                )),
            };
            crate::evidence::Evidence {
                kind: fact_kind_from_label(&r.kind),
                subtype: fact_subtype_from_label(&r.subtype),
                status,
                source,
                observed_at: r.observed_at,
                event_at: r.event_at.map(|t| t as u64),
                freshness: crate::evidence::Freshness {
                    expires_after_secs: r.freshness_expires_after_secs,
                    coverage_note: r.freshness_coverage_note.clone(),
                },
                note: r.note.clone(),
            }
        })
        .collect()
}

/// Writes `evidence.parquet` for `scope_key`, replacing that scope's
/// rows wholesale, from every entity this same pass already carries
/// evidence on -- no second collection pass. `entities` is
/// `(row_key, evidence)` pairs; the caller assembles them from
/// `Report.projects`' artifacts, `external_units`, `agent_units` and
/// `nested_artifacts`/`store_interiors`.
pub fn write_evidence_table(
    swamp_dir: &Path,
    scope_key: &str,
    entities: &[(String, &[crate::evidence::Evidence])],
) -> Result<()> {
    store::StoreDir::at(swamp_dir)?.create()?;
    let file = evidence_path(swamp_dir);
    let mut rows: Vec<columns::StoredEvidenceRow> = columns::read_evidence_rows(&file)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.scope_key != scope_key)
        .collect();
    for (row_key, evidence) in entities {
        rows.extend(stored_evidence_rows(scope_key, row_key, evidence));
    }
    columns::write_evidence_rows(&file, &rows).with_context(|| format!("write {}", file.display()))
}

/// Every stored `evidence.parquet` row for `scope_key`, grouped by
/// `row_key` and reassembled into `Vec<Evidence>` in `seq` order.
/// Empty (never `None`) when there are no rows yet -- unlike the other
/// R16 tables, an entity with no evidence at all is indistinguishable
/// from "table not yet written" by row count alone, so the caller
/// always calls this and gets an empty map rather than falling back to
/// the snapshot (evidence is replaced, not overlaid).
pub(crate) fn read_evidence_table(
    swamp_dir: &Path,
    scope_key: &str,
) -> HashMap<String, Vec<crate::evidence::Evidence>> {
    let Ok(all) = columns::read_evidence_rows(&evidence_path(swamp_dir)) else {
        return HashMap::new();
    };
    let mut by_key: HashMap<String, Vec<&columns::StoredEvidenceRow>> = HashMap::new();
    for r in &all {
        if r.scope_key == scope_key {
            by_key.entry(r.row_key.clone()).or_default().push(r);
        }
    }
    by_key
        .into_iter()
        .map(|(k, mut rows)| {
            rows.sort_by_key(|r| r.seq);
            (k, evidence_from_stored_rows(&rows))
        })
        .collect()
}

fn parse_artifact_kind(s: &str) -> ArtifactKind {
    match s {
        "BuildOutput" => ArtifactKind::BuildOutput,
        "DependencyTree" => ArtifactKind::DependencyTree,
        "Git" => ArtifactKind::Git,
        "Cache" => ArtifactKind::Cache,
        "Source" => ArtifactKind::Source,
        "Ignored" => ArtifactKind::Ignored,
        "Untracked" => ArtifactKind::Untracked,
        "DockerImage" => ArtifactKind::DockerImage,
        "DockerBuildCache" => ArtifactKind::DockerBuildCache,
        "DockerVolume" => ArtifactKind::DockerVolume,
        "Loose" => ArtifactKind::Loose,
        _ => ArtifactKind::Unknown,
    }
}

/// Reconstructs a full [`crate::attribution::AttributionResult`] from the
/// growth store's current-state files: every artifact/dir/file row this
/// volume has ever observed and is still present, plus the last-known
/// unowned rows carried forward verbatim. The incremental caller replaces
/// those rows when an unowned directory or folded boundary was refreshed.
///
/// Every `ArtifactRow::path` here is still **relative** (the raw
/// `rel_path` from storage); the caller re-joins it against each
/// worktree's root once the topology is loaded, since this function has
/// no access to worktree roots on its own.
fn reconstruct_attribution(dir: &Path) -> Result<crate::attribution::AttributionResult> {
    let current_rows = read_rows(&current_path(dir))?;
    let mut artifacts_by_worktree: HashMap<String, Vec<ArtifactRow>> = HashMap::new();
    let mut attributed_total = 0u64;
    // Docker rows are persisted for growth history but are NOT part of the
    // filesystem walk: the report re-derives them from daemon facts every
    // time and keeps them out of `walked_total`/`attributed`. Carrying
    // them into the reconstructed attribution both double-listed them and
    // inflated the incremental totals by their unique bytes (#29 live).
    let is_docker_kind = |k: &str| matches!(k, "DockerImage" | "DockerBuildCache" | "DockerVolume");
    for row in current_rows
        .iter()
        .filter(|r| r.present() && !is_docker_kind(r.kind()) && !r.kind().starts_with("Nested:"))
    {
        attributed_total += row.bytes();
        artifacts_by_worktree
            .entry(row.worktree_id().to_string())
            .or_default()
            .push(ArtifactRow {
                kind: parse_artifact_kind(row.kind()),
                path: PathBuf::from(&row.rel_path()),
                bytes: row.bytes(),
                mtime_max: row.mtime_max(),
                ecosystem: None,
                hardlinked: row.hardlinked(),
                dedup_stale: row.dedup_stale(),
                local_bytes: row.local_bytes(),
                allocated_bytes: None,
                allocated_growth_bytes: None,
                track: None,
                growth_bytes: None,
                regrowth_count: row.regrowth_count(),
                observed_at: row.observed_at(),
                confidence: Confidence::High,
                source: Source::new("filesystem.walk"),
                note: None,
                created_at: None,
                containers: Vec::new(),
                shared_with: Vec::new(),
                dangling: false,
                evidence: Vec::new(),
            });
    }

    let dirs: Vec<DirRollup> = read_dir_rows(&dirs_current_path(dir))?
        .into_iter()
        .map(|r| DirRollup {
            worktree_id: r.worktree_id,
            track: None,
            rel_path: r.rel_path,
            parent_rel_path: r.parent_rel_path,
            allocated_total: r.allocated_total,
            own_allocated: r.own_allocated,
            file_count: r.file_count,
            entry_count: r.entry_count,
            symlink_count: r.symlink_count,
            mod_time_min: r.mod_time_min,
            complete: r.complete,
            growth_bytes: None,
        })
        .collect();
    let files: Vec<FileRow> = read_file_rows(&files_current_path(dir))?
        .into_iter()
        .map(|r| FileRow {
            worktree_id: r.worktree_id,
            rel_path: r.rel_path,
            allocated: r.allocated,
            mod_time_min: r.mod_time_min,
            growth_bytes: None,
        })
        .collect();

    let unowned = read_unowned(dir);
    let unowned_total = unowned.iter().map(|u| u.bytes).sum();
    // `attributed_total` already sums every stored artifact row,
    // including each worktree's own `Source` row (whose bytes equal that
    // worktree's root `DirRollup.allocated_total`), so it is not summed a
    // second time from `dirs`.
    let walked_total = attributed_total + unowned_total;

    Ok(crate::attribution::AttributionResult {
        artifacts_by_worktree,
        unowned,
        walked_total,
        attributed_total,
        unowned_total,
        dirs,
        files,
    })
}

/// One tracked walk's outcome: the same `(discovered, attribution)` shape
/// [`crate::walk::discover_and_attribute`] returns, plus the mode/reason
/// a caller reports in the coverage block and the `observe` log line.
pub struct TrackedWalk {
    pub changed_paths: Option<Vec<PathBuf>>,
    /// This root's trusted event window, when the replay earned one:
    /// the replay's **unfiltered** change list and the observation time
    /// it replays from.
    ///
    /// Distinct from `changed_paths`, which is the same replay narrowed
    /// to the subtrees this walk was allowed to descend into. The unit
    /// families (`crate::external`, `crate::agents`) measure paths this
    /// walk deliberately pruned -- every external location nested under
    /// a scan root is pruned from it precisely so it can be measured
    /// once, as its own unit -- so narrowing the list for them would
    /// hand out a window that cannot see a change it was asked about.
    pub event_window: Option<crate::fs_events::TrustedWindow>,
    pub discovered: Vec<DiscoveredWorktree>,
    pub attribution: crate::attribution::AttributionResult,
    /// `"incremental"` or `"full"`.
    pub mode: &'static str,
    /// `"incremental"` on success; otherwise the refusal reason (see
    /// [`crate::fs_events::RefreshRefusal::as_str`]), or `"no_stored_event_id"`
    /// / `"full_forced"` for the two non-FSEvents reasons a walk is full.
    pub reason: &'static str,
    pub changed_dirs: usize,
    /// Worktree ids this observation actually re-walked (incremental
    /// path); `None` on a full walk, meaning all of them. Downstream
    /// stages may carry forward what they computed last time for every
    /// worktree not listed.
    pub rewalked: Option<Vec<String>>,
    /// Incremental accounting: artifact roots re-sized from interior
    /// rows, artifact roots re-sized whole, Source directories re-listed
    /// in place, worktrees handed back to the walker.
    pub in_place: (usize, usize, usize, usize),
    /// Worktree ids that were present in the last observation's topology
    /// but are absent from `discovered` this pass *and could not be
    /// confirmed gone* -- the path still exists on disk but could not be
    /// read (e.g. `chmod 000`), so its absence from `discovered` reflects
    /// lost access, not deletion (#42). The growth store must not
    /// tombstone rows belonging to these worktree ids from this
    /// observation: an inaccessible worktree is a coverage gap, never a
    /// storage change. A worktree id whose path is genuinely gone
    /// (`ENOENT`) is *not* included here -- that is real deletion, and
    /// tombstoning is exactly correct for it.
    pub unconfirmed_worktree_ids: Vec<String>,
}

/// Threshold past which re-walking piecemeal costs more than a full
/// walk: more than this fraction of previously known directories
/// implicated by one replay.
const TOO_MANY_CHANGES_FRACTION: f64 = 0.20;

/// The [`RefreshRefusal::TooSoon`] floor, in seconds. Overridable via
/// `SWAMP_FSEVENTS_MIN_INTERVAL_SECS` so a test driving a canned
/// [`crate::fs_events::FsEventsSource`] -- which has no real FSEvents
/// log-persistence lag to protect against -- can set it to `0` and reach
/// the incremental path without a real `sleep`.
fn min_interval_secs() -> u64 {
    std::env::var("SWAMP_FSEVENTS_MIN_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3)
}

/// Same as [`observe_tracked`], with the [`crate::fs_events::FsEventsSource`]
/// supplied explicitly rather than resolved via [`crate::fs_events::platform_source`].
/// This is the seam integration tests use to exercise every refusal
/// reason and the incremental merge deterministically, with canned event
/// batches, so no test depends on the live `fseventsd`.
///
/// `observe` mirrors `growth::observe_and_annotate` vs `annotate_readonly`:
/// the walk itself (full or incremental) always happens, so the caller
/// gets a report of the tree's current state either way, but the
/// FSEvents/topology/unowned sidecars are only rewritten when `observe`
/// is true. A `--no-observe` read must not silently advance the stored
/// event id, or the next real observation would replay from a point it
/// never actually walked from.
///
/// This low-level entry point commits after the walk. Report pipelines must use
/// [`stage_tracked_with_source`] and commit only after their downstream writes.
#[allow(clippy::too_many_arguments)]
pub fn observe_tracked_with_source(
    stage: &crate::bus::Stage,
    swamp_dir: &Path,
    root: &Path,
    observed_at: u64,
    large_file_min_bytes: u64,
    force_full: bool,
    observe: bool,
    source: &dyn crate::fs_events::FsEventsSource,
    excluded: &[PathBuf],
) -> Result<TrackedWalk> {
    let (walk, checkpoint) = stage_tracked_with_source(
        stage,
        swamp_dir,
        root,
        observed_at,
        large_file_min_bytes,
        force_full,
        observe,
        source,
        excluded,
    )?;
    if let Some(checkpoint) = checkpoint {
        checkpoint.commit()?;
    }
    Ok(walk)
}

/// What one pass's unit-root replays earned: the event coverage the
/// authorized external/agent roots contributed, one reason code per root
/// for the report, and the cursors this pass may publish **if it
/// completes**.
///
/// # Why the cursors are staged rather than written
///
/// Advancing a cursor is a promise: it says the next pass's window may
/// start here, which is only true if this pass actually refreshed the
/// rows the next pass will want to reuse. A pass that measured and then
/// failed to persist has not earned that, so [`Self::commit`] is called
/// only on the success path -- the same `ReportCached`-gated ordering
/// [`ObservationCheckpoint`] uses for the walk's anchor. Dropping this
/// value leaves the previous cursor in place, which makes the next
/// window *wider* than necessary: the outcome of a failed pass is a
/// re-measurement, never a reuse that rests on it.
///
/// Not advancing is always the safe direction, which is why a pass that
/// covers only one unit family does not advance either (see
/// `report::observe_scope`).
#[derive(Default)]
pub struct UnitRootReplay {
    /// The windows the unit roots earned, to be merged into the walk's.
    pub coverage: crate::fs_events::EventCoverage,
    /// `(root, reason)` per root asked about: `incremental` when the
    /// root earned a window, otherwise the refusal that explains why the
    /// units under it are being measured from scratch.
    pub outcomes: Vec<(PathBuf, String)>,
    staged: Vec<(PathBuf, crate::fs_events::UnitRootCursor)>,
}

impl UnitRootReplay {
    /// Publishes every staged cursor. Called only by a pass that
    /// observed and persisted both unit families.
    pub fn commit(self) -> Result<()> {
        for (dir, cursor) in self.staged {
            write_unit_root_cursor(&dir, &cursor)?;
        }
        Ok(())
    }

    /// Whether this root earned a window this pass -- the "event-covered
    /// / replayed" fact the report renders.
    pub fn covered(&self, root: &Path) -> bool {
        self.outcomes
            .iter()
            .any(|(p, reason)| p == root && reason == "incremental")
    }
}

/// Replays one FSEvents cursor per authorized unit root (external cache
/// or agent tool home), so a default install -- where no tool home is
/// under any scan root -- can reuse stored measurements at all.
///
/// Each root's anchor lives in its own volume dir's `fsevents.json`,
/// under `unit_root`, beside (never instead of) the walk's anchor for
/// the same path. Roots on one device are replayed through a single
/// FSEvents stream and the result split per root
/// (`fs_events::FsEventsSource::replay_roots`).
///
/// Three things make the result safe to reuse a measurement on, and all
/// three are here rather than in the caller:
///
/// * `force_full` never touches the source at all, exactly as the walk's
///   own path does not -- a forced full pass has promised not to pay for
///   a replay, and "call it and discard the answer" is not that promise.
/// * The `TooSoon` floor applies unchanged: FSEvents' persisted log can
///   lag a write by longer than a whole second, so two passes in quick
///   succession get no window and honestly re-measure.
/// * A root whose device differs from the one its cursor was recorded
///   against gets no window and a `root_mismatch` reason, decided here
///   rather than left to the platform source, so it holds for every
///   source including the injected ones.
///
/// On a platform with no FSEvents the source refuses
/// (`unsupported_platform`), no root earns a window, and every unit is
/// re-measured -- the "continuity unavailable" contract from stack/09.
pub fn replay_unit_roots(
    swamp_dir: Option<&Path>,
    roots: &[PathBuf],
    observed_at: u64,
    force_full: bool,
    source: &dyn crate::fs_events::FsEventsSource,
) -> UnitRootReplay {
    use crate::fs_events::{FsEventsRequest, FsEventsState, UnitRootCursor};

    let mut out = UnitRootReplay::default();
    let Some(swamp_dir) = swamp_dir else {
        for r in roots {
            out.outcomes.push((r.clone(), "no_store".to_string()));
        }
        return out;
    };
    if force_full {
        for r in roots {
            out.outcomes.push((r.clone(), "full_forced".to_string()));
        }
        return out;
    }

    struct Staged {
        root: PathBuf,
        canonical: PathBuf,
        dir: PathBuf,
        device: Option<u64>,
        since: Option<u64>,
        too_soon: bool,
        device_mismatch: bool,
    }

    let trace = std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty());
    let staging = std::time::Instant::now();
    let mut staged: Vec<Staged> = Vec::new();
    let mut requests: Vec<FsEventsRequest> = Vec::new();
    for root in roots {
        let canonical = crate::fs_gate::canonicalize(root).unwrap_or_else(|_| root.clone());
        let dir = volume_dir(swamp_dir, root_scoped_volume_id(&canonical));
        let prev = read_fsevents_state(&dir).unit_root.unwrap_or_default();
        // A root that is not on disk has no units, nothing to replay and
        // nothing worth anchoring. It gets no row at all rather than a
        // refusal row, for the same reason a `SkippedAsNested` scan root
        // gets no coverage row: a report should not list a reason for
        // something that was never a candidate this pass. The authorized
        // scope legitimately contains such paths -- a Cargo home
        // contributes `registry/index` and `git/db` whether or not they
        // have ever been populated.
        let Some(device) = crate::fs_gate::metadata_following(&canonical)
            .map(|m| m.dev())
            .ok()
        else {
            continue;
        };
        let device_mismatch = prev.device.is_some_and(|stored| stored != device);
        let too_soon = prev
            .observed_at
            .is_some_and(|t| observed_at.saturating_sub(t) < min_interval_secs());
        requests.push(FsEventsRequest {
            root: canonical.clone(),
            since: FsEventsState {
                event_id: prev.event_id,
                device: prev.device,
                last_observed_at: prev.observed_at,
                rules_version: crate::ecosystem::RULES_VERSION,
                unit_root: None,
            },
            swamp_dir: Some(swamp_dir.to_path_buf()),
            excluded: Vec::new(),
        });
        staged.push(Staged {
            root: root.clone(),
            canonical,
            dir,
            device: Some(device),
            since: prev.observed_at,
            too_soon,
            device_mismatch,
        });
    }

    if trace {
        eprintln!(
            "[xtrace] unit-root staging: {} roots in {:?}",
            requests.len(),
            staging.elapsed()
        );
    }
    let replay = std::time::Instant::now();
    let plans = source.replay_roots(&requests);
    if trace {
        eprintln!("[xtrace] unit-root replay_roots: {:?}", replay.elapsed());
    }
    for (s, plan) in staged.into_iter().zip(plans) {
        let reason = if s.device_mismatch {
            crate::fs_events::RefreshRefusal::RootMismatch
                .as_str()
                .to_string()
        } else if s.too_soon && !plan.live {
            crate::fs_events::RefreshRefusal::TooSoon
                .as_str()
                .to_string()
        } else if plan.incremental && s.since.is_none() {
            // A replay with nothing to replay *from* answers about a
            // window whose start is unknown; a stored row cannot be
            // shown to predate it.
            crate::fs_events::RefreshRefusal::NoStoredEventId
                .as_str()
                .to_string()
        } else {
            plan.reason_str().to_string()
        };
        if reason == "incremental"
            && let Some(since) = s.since
        {
            // Two spellings, because the two unit families address their
            // units differently: `external::discover_and_measure`
            // canonicalizes every candidate, while
            // `agents::authorized_tool_homes` keeps the scope's own
            // spelling. Registering only one of them would silently
            // cover only one family whenever a root's canonical form
            // differs (`/var` vs `/private/var`, a symlinked home).
            out.coverage.trust_alias(
                s.root.clone(),
                s.canonical.clone(),
                plan.changed_dirs.clone(),
                since,
            );
            if s.canonical != s.root {
                out.coverage
                    .trust(s.canonical.clone(), plan.changed_dirs.clone(), since);
            }
        }
        out.outcomes.push((s.root.clone(), reason));
        // Nothing was learned about where to replay from next time (no
        // FSEvents on this platform at all), so there is no anchor worth
        // storing.
        if plan.current_event_id == 0 && plan.device.is_none() {
            continue;
        }
        out.staged.push((
            s.dir,
            UnitRootCursor {
                event_id: Some(plan.current_event_id),
                device: s.device.or(plan.device),
                observed_at: Some(observed_at),
            },
        ));
    }
    out
}

/// Replay state is staged until all observation consumers have persisted their
/// facts. Dropping this value on any later failure leaves the old replay anchor.
pub struct ObservationCheckpoint {
    dir: PathBuf,
    state: Option<FsEventsState>,
    topology: Vec<StoredWorktree>,
    unowned: Vec<crate::report::UnownedRow>,
    /// Linux collector: the dirty-list entries this observation's walk
    /// covered, consumed only once everything above is written.
    consume: Option<crate::continuity::Consumption>,
    /// Held from before the previous state was read until the commit (or
    /// drop): two observations of one root -- TUI, CLI, a scheduled run
    /// -- cannot interleave, so neither overwrites the other's rows with
    /// an older walk or consumes changes the other has not written.
    _lock: Option<crate::continuity::FileLock>,
}

impl ObservationCheckpoint {
    pub fn commit(self) -> Result<()> {
        write_topology(&self.dir, &self.topology)?;
        write_unowned(&self.dir, &self.unowned)?;
        // Publish the replay anchor last. This is safe replay ordering, not an
        // atomic transaction across the legacy volume-wide datasets.
        if let Some(state) = self.state {
            write_fsevents_state(&self.dir, &state)?;
        }
        // And only after that, forget the collector's entries this walk
        // covered. A crash before this line leaves them, and the next
        // observation re-walks them: redundant, never wrong.
        if let Some(c) = &self.consume {
            crate::continuity::consume(c)?;
        }
        Ok(())
    }

    /// Test hook: the crash between the history write and the
    /// consumption -- everything is written except the consumption.
    #[doc(hidden)]
    pub fn commit_without_consuming_for_test(mut self) -> Result<()> {
        self.consume = None;
        self.commit()
    }
}

/// How long an observation waits for another observation of the same
/// root to finish before giving up.
const OBSERVATION_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(600);

#[allow(clippy::too_many_arguments)]
pub fn stage_tracked_with_source(
    stage: &crate::bus::Stage,
    swamp_dir: &Path,
    root: &Path,
    observed_at: u64,
    large_file_min_bytes: u64,
    force_full: bool,
    observe: bool,
    source: &dyn crate::fs_events::FsEventsSource,
    excluded: &[PathBuf],
) -> Result<(TrackedWalk, Option<ObservationCheckpoint>)> {
    // This public lower-level entry point must be safe for direct callers;
    // never persist alias-form topology into a canonical root scope.
    let root = crate::fs_gate::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let volume_id = root_scoped_volume_id(&root);
    let dir = volume_dir(swamp_dir, volume_id);
    store::StoreDir::at(&dir)?.create()?;
    // One writer per root, from reading the previous state to committing
    // the next (see `ObservationCheckpoint::_lock`). A read-only pass
    // (`observe == false`) writes nothing and takes nothing.
    let lock = if observe {
        Some(crate::continuity::lock_wait(
            &dir.join("observation.lock"),
            true,
            OBSERVATION_LOCK_WAIT,
        )?)
    } else {
        None
    };

    // FSEvents and persisted topology use the canonical root namespace.
    let prev_state = read_fsevents_state(&dir);

    // `force_full` (`--full`, and every pre-#29 caller: `report_full`,
    // `report_with*`, every test that predates this feature) must never
    // touch the FSEvents source at all -- not the
    // real one (this crate runs alongside dozens of other concurrent
    // test/CLI processes on a shared machine, where `fseventsd` itself
    // can become the bottleneck under combined load; a `source.replay`
    // call that is merely slow under contention still burns wall time
    // this path has promised never to pay), and not even a canned one in
    // tests (there is nothing to answer). Skipping the call entirely,
    // rather than calling it and discarding the answer, is what actually
    // keeps this path load-free instead of just "load but ignore".
    // Classification rules changed since the store was walked: rows that
    // no longer count (or newly count) as artifacts only get fixed by a
    // walk that visits them, so take the one full walk now.
    //
    // Only a *stored* state can carry an older rules version. A root's
    // first observation has none, and used to read as "rules changed":
    // it took this branch, which keeps the stored event id -- absent --
    // so the second observation had no anchor either, walked fully a
    // second time, and the steady state began on the third (re-review 3:
    // pass 2 still listed 40 directories and stat'ed 5,561 files). The
    // first observation now takes the ordinary path below, whose replay
    // refuses with `no_stored_event_id` and whose checkpoint records the
    // current event id, so the second observation can replay.
    //
    // But a stored state that *does* carry a rules version and no anchor
    // is not "no stored state" (re-review 4, C2): the pre-fix code's
    // rules-changed branch wrote exactly that file, and so does a
    // `--full`-only store. With the real FSEvents source the replay then
    // refuses anyway; with the TUI's live plan (`LivePlanSource`) it did
    // not, and the reclassification was skipped for good because the
    // checkpoint stamps the current version. So a recorded version (any
    // non-zero one) that differs forces the walk, anchor or not. `0` is
    // "never recorded": a unit-root cursor file, or a first observation.
    let has_stored_state = prev_state.event_id.is_some() || prev_state.last_observed_at.is_some();
    let rules_changed = prev_state.rules_version != crate::ecosystem::RULES_VERSION
        && (prev_state.rules_version != 0 || has_stored_state);
    // Read once, ahead of either branch below: both a forced/rules-change
    // full walk and the ordinary incremental-or-full path need it to tell
    // "worktree confirmed gone" from "worktree access lost" (#42).
    let prev_topology_for_check = read_topology(&dir);
    if force_full || rules_changed {
        let reason = if force_full {
            "full_forced"
        } else {
            "full_rules_changed"
        };
        let mut result = full_walk(
            stage,
            &root,
            observed_at,
            large_file_min_bytes,
            reason,
            excluded,
        )?;
        result.unconfirmed_worktree_ids =
            compute_unconfirmed_worktrees(prev_topology_for_check.as_deref(), &result.discovered);
        let checkpoint = observe.then(|| ObservationCheckpoint {
            dir,
            consume: None,
            _lock: lock,
            topology: to_stored_worktrees(&result.discovered),
            unowned: result.attribution.unowned.clone(),
            // The stored FSEvents id/device is deliberately left as-is: a
            // forced full walk has nothing new to report there (no
            // replay ran), and an older stored id just means the next
            // real incremental attempt replays a larger, still-correct
            // window rather than a wrong one. The rules version is
            // stamped so the next call goes incremental again.
            state: rules_changed.then(|| FsEventsState {
                rules_version: crate::ecosystem::RULES_VERSION,
                ..prev_state.clone()
            }),
        });
        return Ok((result, checkpoint));
    }

    // FSEvents' own persisted log can lag a write by longer than the
    // growth store's whole-second timestamp granularity, so a replay
    // requested this soon after the baseline cannot yet distinguish
    // "nothing changed" from "the change has not been logged yet" --
    // most visibly when two observations happen back-to-back (tests;
    // a scripted double-run), where a live FSEvents source can
    // legitimately report zero changes for a write that already
    // happened. Below this floor, skip straight to a full walk rather
    // than trust an answer FSEvents itself cannot yet vouch for.
    // Overridable via `SWAMP_FSEVENTS_MIN_INTERVAL_SECS` so tests
    // that use a canned source (which has no real log-lag to protect
    // against) can set it to `0` and skip real sleeps entirely.
    let too_soon = prev_state
        .last_observed_at
        .is_some_and(|t| observed_at.saturating_sub(t) < min_interval_secs());
    // The instant the window opens from. Without one there is no window
    // at all: a stored row cannot be shown to predate a replay whose
    // start is unknown.
    let window_since = prev_state.last_observed_at;
    let t_replay = std::time::Instant::now();
    let plan = source.replay(&FsEventsRequest {
        root: root.clone(),
        since: prev_state,
        swamp_dir: Some(swamp_dir.to_path_buf()),
        excluded: excluded.to_vec(),
    });
    if std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty()) {
        eprintln!(
            "[trace] fsevents replay: {:?} (incremental={}, reason={}, too_soon={}, changed_dirs={})",
            t_replay.elapsed(),
            plan.incremental,
            plan.reason_str(),
            too_soon,
            plan.changed_dirs.len()
        );
    }
    let prev_topology = prev_topology_for_check.clone();
    // Prune any FSEvents-reported change that falls inside an excluded
    // subtree (#42) before it ever reaches the incremental re-walk
    // machinery: `apply_incremental`/`attribute_one_worktree`/
    // `discover_shallow` have no exclusion list of their own precisely
    // because nothing excluded is ever supposed to reach them.
    let relevant_changed_dirs: Vec<PathBuf> = if excluded.is_empty() {
        plan.changed_dirs.clone()
    } else {
        plan.changed_dirs
            .iter()
            .filter(|p| !excluded.iter().any(|e| *p == e || p.starts_with(e)))
            .cloned()
            .collect()
    };

    let mut result = if too_soon && !plan.live {
        full_walk(
            stage,
            &root,
            observed_at,
            large_file_min_bytes,
            crate::fs_events::RefreshRefusal::TooSoon.as_str(),
            excluded,
        )?
    } else if !plan.incremental {
        full_walk(
            stage,
            &root,
            observed_at,
            large_file_min_bytes,
            plan.reason_str(),
            excluded,
        )?
    } else {
        match prev_topology {
            None => full_walk(
                stage,
                &root,
                observed_at,
                large_file_min_bytes,
                "no_stored_event_id",
                excluded,
            )?,
            Some(ref topo) => {
                // Relist direct unowned directories; remeasure an implicated
                // folded subtree. Sharing retains explicit stale estimates;
                // old boundaries and ownership transitions still reconcile.
                let has_unowned_changes = relevant_changed_dirs
                    .iter()
                    .any(|changed| !topo.iter().any(|wt| changed.starts_with(&wt.path)));
                let known_dirs = read_dir_rows(&dirs_current_path(&dir))?.len().max(20);
                let too_many_changes = relevant_changed_dirs.len() as f64
                    > TOO_MANY_CHANGES_FRACTION * known_dirs as f64;
                let refreshed_unowned = if has_unowned_changes && !too_many_changes {
                    // A root-local hardlink charge may belong to an unchanged
                    // checkout. Label local refresh estimates instead of
                    // silently treating allocation as reconciled uniqueness.
                    let shared_root = read_rows(&current_path(&dir))?
                        .iter()
                        .any(|r| r.present() && r.hardlinked());
                    if let Ok(previous) = try_read_unowned_family(&dir, "unowned") {
                        crate::walk::refresh_unowned(
                            &root,
                            &previous,
                            &relevant_changed_dirs,
                            &topo.iter().map(|w| w.path.clone()).collect::<Vec<_>>(),
                            excluded,
                            observed_at,
                            shared_root,
                        )
                    } else {
                        None
                    }
                } else {
                    None
                };
                if has_unowned_changes && !too_many_changes && refreshed_unowned.is_none() {
                    full_walk(
                        stage,
                        &root,
                        observed_at,
                        large_file_min_bytes,
                        if topo.is_empty() {
                            "checkoutless_changes"
                        } else {
                            "unowned_changes"
                        },
                        excluded,
                    )?
                } else {
                    // Floored at a minimum so a tiny tree (a handful of
                    // Source directories) doesn't trip the "too many
                    // changes" guard on the very first touched file --
                    // the guard exists to protect large trees, where a
                    // fraction is the meaningful signal.
                    if too_many_changes {
                        full_walk(
                            stage,
                            &root,
                            observed_at,
                            large_file_min_bytes,
                            "too_many_changes",
                            excluded,
                        )?
                    } else {
                        apply_incremental(
                            stage,
                            topo,
                            &relevant_changed_dirs,
                            observed_at,
                            large_file_min_bytes,
                            &dir,
                            refreshed_unowned,
                        )?
                    }
                }
            }
        }
    };
    result.unconfirmed_worktree_ids =
        compute_unconfirmed_worktrees(prev_topology_for_check.as_deref(), &result.discovered);
    // A window only where the incremental path was actually taken: a
    // full walk, a refusal, `too_soon` and `too_many_changes` all mean
    // this pass cannot say what did *not* change, which is exactly the
    // claim a reuse rests on.
    result.event_window = match (result.mode, window_since) {
        ("incremental", Some(since)) => Some((plan.changed_dirs.clone(), since)),
        _ => None,
    };

    let consume = plan.consume.clone();
    let checkpoint = observe.then(|| ObservationCheckpoint {
        consume,
        _lock: lock,
        // Re-anchor for the next call regardless of which path was taken.
        state: Some(FsEventsState {
            event_id: Some(plan.current_event_id),
            device: plan.device,
            last_observed_at: Some(observed_at),
            rules_version: crate::ecosystem::RULES_VERSION,
            // The unit-root half of this file belongs to
            // `replay_unit_roots`; `write_fsevents_state` carries
            // whatever is on disk through rather than taking it from
            // here.
            unit_root: None,
        }),
        topology: to_stored_worktrees(&result.discovered),
        unowned: result.attribution.unowned.clone(),
        dir,
    });

    Ok((result, checkpoint))
}

fn to_stored_worktrees(discovered: &[DiscoveredWorktree]) -> Vec<StoredWorktree> {
    discovered
        .iter()
        .map(|dw| StoredWorktree {
            worktree_id: crate::entities::id_for(&dw.path.display().to_string()),
            project_id: dw.project_id.clone(),
            project_name: dw.project_name.clone(),
            path: dw.path.clone(),
            kind: dw.kind.clone(),
            remote_url: dw.remote_url.clone(),
        })
        .collect()
}

/// Merges a worktree's `Ignored` / `Untracked` rows back into its single
/// `Source` row.
///
/// The split is a presentation of one fact — "everything here that is
/// not a classified artifact" — and the incremental walk's arithmetic is
/// written against that one number: it adjusts the remainder by a delta,
/// re-lists directories into it, and carries it forward. Collapsing on
/// the way in and splitting on the way out keeps that arithmetic in one
/// shape, so the store's rows can be split without every incremental
/// code path learning about three of them.
fn collapse_remainder(attribution: &mut crate::attribution::AttributionResult) {
    for rows in attribution.artifacts_by_worktree.values_mut() {
        if !rows
            .iter()
            .any(|r| matches!(r.kind, ArtifactKind::Ignored | ArtifactKind::Untracked))
        {
            continue;
        }
        let mut merged: Option<ArtifactRow> = None;
        rows.retain(|r| {
            if !r.kind.is_worktree_remainder() {
                return true;
            }
            match merged.as_mut() {
                None => {
                    let mut base = r.clone();
                    base.kind = ArtifactKind::Source;
                    base.track = None;
                    merged = Some(base);
                }
                Some(m) => {
                    m.bytes += r.bytes;
                    m.local_bytes += r.local_bytes;
                    m.observed_at = m.observed_at.max(r.observed_at);
                    m.hardlinked |= r.hardlinked;
                    if let (Some(a), Some(b)) = (m.growth_bytes, r.growth_bytes) {
                        m.growth_bytes = Some(a + b);
                    }
                    m.mtime_max = m.mtime_max.max(r.mtime_max);
                }
            }
            false
        });
        if let Some(m) = merged {
            rows.push(m);
        }
    }
}

/// Splits each worktree's remainder row into what git tracks, what a
/// gitignore rule matches, and what is in no version control at all.
///
/// A single row labelled `source` was a lie about every checkout that
/// holds ignored output or private scratch data: those bytes are not
/// authored work and no remote has a copy. Each kind now states its own
/// recovery contract, and the three totals still sum to the one the
/// walk measured — the split apportions that number rather than
/// re-measuring, so no accounting is invented here.
///
/// Leaves the row undivided when the checkout is not a repository, when
/// the walk kept no directory rows for it, or when the split would be
/// entirely one bucket anyway.
fn split_remainder(
    discovered: &[DiscoveredWorktree],
    attribution: &mut crate::attribution::AttributionResult,
) {
    let roots: std::collections::HashMap<String, PathBuf> = discovered
        .iter()
        .map(|d| {
            (
                crate::entities::id_for(&d.path.display().to_string()),
                d.path.clone(),
            )
        })
        .collect();
    for (wt_id, rows) in attribution.artifacts_by_worktree.iter_mut() {
        let Some(root) = roots.get(wt_id) else {
            continue;
        };
        let Some(idx) = rows.iter().position(|r| r.kind == ArtifactKind::Source) else {
            continue;
        };
        let artifact_rels: std::collections::HashSet<String> = rows
            .iter()
            .filter(|r| !r.kind.is_worktree_remainder())
            .map(|r| rel_path_string(root, &r.path))
            .collect();
        let wt_dirs: Vec<crate::report::DirRollup> = attribution
            .dirs
            .iter()
            .filter(|d| &d.worktree_id == wt_id)
            .cloned()
            .collect();
        if wt_dirs.is_empty() {
            continue;
        }
        let wt_files: Vec<crate::report::FileRow> = attribution
            .files
            .iter()
            .filter(|f| &f.worktree_id == wt_id)
            .cloned()
            .collect();
        let Some(split) =
            crate::ignore::split_dirs_by_track(root, &wt_dirs, &wt_files, &artifact_rels)
        else {
            continue;
        };
        let measured = split.total();
        if measured == 0 || split.tracked == measured {
            continue;
        }
        let base = rows.remove(idx);
        // Apportion what the walk measured, rather than substituting the
        // directory rows' own sum: the walk's number is the one every
        // total in the report was built from, and hardlink dedup can
        // make the two differ.
        let parts = [
            (
                ArtifactKind::Source,
                crate::ignore::TrackState::Tracked,
                split.tracked,
            ),
            (
                ArtifactKind::Ignored,
                crate::ignore::TrackState::Ignored,
                split.ignored,
            ),
            (
                ArtifactKind::Untracked,
                crate::ignore::TrackState::Untracked,
                split.untracked,
            ),
        ];
        let largest = parts
            .iter()
            .enumerate()
            .max_by_key(|(_, (_, _, b))| *b)
            .map(|(i, _)| i)
            .unwrap_or(0);
        let share = |total: u64, bucket: u64| -> u64 {
            (total as u128 * bucket as u128 / measured as u128) as u64
        };
        // Every bucket but the largest takes its share; the largest then
        // takes whatever is left, so the parts add back up to exactly
        // what the walk measured no matter how the division rounds.
        let mut bytes: [u64; 3] = [0; 3];
        let mut local: [u64; 3] = [0; 3];
        for (i, (_, _, bucket)) in parts.iter().enumerate() {
            if i == largest {
                continue;
            }
            bytes[i] = share(base.bytes, *bucket);
            local[i] = share(base.local_bytes, *bucket);
        }
        bytes[largest] = base.bytes.saturating_sub(bytes.iter().sum::<u64>());
        local[largest] = base.local_bytes.saturating_sub(local.iter().sum::<u64>());
        for (i, (kind, track, bucket)) in parts.iter().enumerate() {
            if bytes[i] == 0 && *bucket == 0 {
                continue;
            }
            let mut row = base.clone();
            row.kind = kind.clone();
            row.track = Some(*track);
            row.bytes = bytes[i];
            row.local_bytes = local[i];
            // The growth history belongs to the undivided remainder; a
            // share of it would be a number nothing observed.
            row.growth_bytes = None;
            rows.push(row);
        }
    }
}

fn full_walk(
    stage: &crate::bus::Stage,
    root: &Path,
    observed_at: u64,
    large_file_min_bytes: u64,
    reason: &'static str,
    excluded: &[PathBuf],
) -> Result<TrackedWalk> {
    let (discovered, mut attribution) = crate::walk::discover_and_attribute(
        stage,
        root,
        observed_at,
        large_file_min_bytes,
        excluded,
    )?;
    split_remainder(&discovered, &mut attribution);
    Ok(TrackedWalk {
        discovered,
        attribution,
        mode: "full",
        changed_paths: None,
        event_window: None,
        reason,
        changed_dirs: 0,
        rewalked: None,
        in_place: (0, 0, 0, 0),
        unconfirmed_worktree_ids: Vec::new(),
    })
}

/// Worktree ids from `prev` whose path is absent from `discovered` this
/// pass, split into "confirmed gone" (tombstoning is correct) versus
/// "could not confirm" (the path still exists but could not be read, so
/// the growth store must preserve its rows as-is). Only the latter are
/// returned. A single non-recursive `symlink_metadata`/`read_dir` pair
/// per candidate; bounded by the number of worktrees that dropped out of
/// this observation, never by tree size.
fn compute_unconfirmed_worktrees(
    prev: Option<&[StoredWorktree]>,
    discovered: &[DiscoveredWorktree],
) -> Vec<String> {
    let Some(prev) = prev else {
        return Vec::new();
    };
    let discovered_paths: HashSet<&Path> = discovered.iter().map(|d| d.path.as_path()).collect();
    prev.iter()
        .filter(|pw| !discovered_paths.contains(pw.path.as_path()))
        .filter(|pw| match crate::fs_gate::symlink_metadata(&pw.path) {
            // Gone entirely: real deletion, tombstoning is correct.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            // Existed, could not be statted for some other reason (also
            // commonly permission-denied on a parent directory): treat as
            // unconfirmed, the conservative choice.
            Err(_) => true,
            // The path itself still exists. If it can be listed, a real
            // walk would have discovered it, so its absence from
            // `discovered` means it is no longer a git worktree (e.g.
            // `.git` was removed) -- a real change, not a coverage gap.
            // If it cannot be listed, access was lost, not the worktree.
            Ok(_) => crate::fs_gate::probe_listable(&pw.path).is_err(),
        })
        .map(|pw| pw.worktree_id.clone())
        .collect()
}

fn rel_path_string(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let s = rel.display().to_string();
    if s == "." { String::new() } else { s }
}

/// `rel` is `root` or lies under it (`root` empty = the worktree root).
fn under(rel: &str, root: &str) -> bool {
    root.is_empty() || rel == root || rel.starts_with(&format!("{root}/"))
}

/// Re-sizes a folded artifact from its stored interior rows and the
/// directories FSEvents named, without walking the rest of it. Each
/// changed directory is re-listed (own bytes, counts, mtime); a vanished
/// directory drops its subtree's rows; a new subdirectory is walked and
/// gets rows. Then the unit's rows are re-aggregated and the root row's
/// total is its path allocation, not a deduplicated count. Returns `None` when the
/// store has no row for the root (older store: caller re-sizes whole).
fn resize_interior(
    wt_root: &Path,
    worktree_id: &str,
    rel_root: &str,
    changed: &[PathBuf],
    dirs: &mut Vec<DirRollup>,
) -> Option<(u64, u64, bool)> {
    use crate::fs_gate::MetadataExt;
    let mut mtime_max: u64 = 0;
    let mut saw_hardlink = false;
    let mut changed_rels: Vec<String> = changed
        .iter()
        .map(|c| rel_path_string(wt_root, c))
        .filter(|r| under(r, rel_root))
        .collect();
    changed_rels.sort();
    changed_rels.dedup();
    for rel_c in &changed_rels {
        let abs = wt_root.join(rel_c);
        let Ok(meta) = crate::fs_gate::symlink_metadata(&abs) else {
            // Gone: its whole subtree with it.
            dirs.retain(|d| !(d.worktree_id == worktree_id && under(&d.rel_path, rel_c)));
            continue;
        };
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }
        let measured = crate::walk::measure_directory(&abs).ok()?;
        let own = measured.allocated;
        let files = measured.files;
        let subdirs = measured.children.len() as u32;
        let symlinks = measured.symlinks;
        let dir_mtime = meta.mtime().max(measured.mtime);
        saw_hardlink |= measured.hardlinked;
        let on_disk_subdirs: HashSet<String> = measured
            .children
            .into_iter()
            .map(|name| format!("{rel_c}/{name}"))
            .collect();
        mtime_max = mtime_max.max(dir_mtime.max(0) as u64);
        // Children the store knows that are no longer on disk.
        let stored_children: HashSet<String> = dirs
            .iter()
            .filter(|d| d.worktree_id == worktree_id && d.parent_rel_path.as_deref() == Some(rel_c))
            .map(|d| d.rel_path.clone())
            .collect();
        for gone in stored_children
            .iter()
            .filter(|c| !on_disk_subdirs.contains(c.as_str()))
        {
            dirs.retain(|d| !(d.worktree_id == worktree_id && under(&d.rel_path, gone)));
        }
        // Subdirectories on disk the store has never seen: walk them.
        for new_rel in on_disk_subdirs
            .iter()
            .filter(|c| !stored_children.contains(c.as_str()))
        {
            let (measured, rows) = crate::walk::resize_artifact_with_dirs(
                &wt_root.join(new_rel),
                ArtifactKind::Cache,
                0,
                Some((worktree_id, wt_root)),
            );
            saw_hardlink |= measured.hardlinked;
            for r in &rows {
                mtime_max = mtime_max.max((r.mod_time_min as i64 * 60).max(0) as u64);
            }
            dirs.retain(|d| !(d.worktree_id == worktree_id && under(&d.rel_path, new_rel)));
            dirs.extend(rows);
        }
        // This directory's own row.
        let parent_rel_path = if rel_c.is_empty() {
            None
        } else {
            Some(
                rel_c
                    .rsplit_once('/')
                    .map(|(p, _)| p.to_string())
                    .unwrap_or_default(),
            )
        };
        let row = DirRollup {
            worktree_id: worktree_id.to_string(),
            track: None,
            rel_path: rel_c.clone(),
            parent_rel_path,
            allocated_total: own,
            own_allocated: own,
            file_count: files,
            entry_count: files + subdirs + symlinks,
            symlink_count: symlinks,
            mod_time_min: (dir_mtime / 60) as i32,
            complete: true,
            growth_bytes: None,
        };
        if let Some(existing) = dirs
            .iter_mut()
            .find(|d| d.worktree_id == worktree_id && &d.rel_path == rel_c)
        {
            *existing = row;
        } else {
            dirs.push(row);
        }
    }
    // Re-aggregate this unit's rows; the root row's total is the unit.
    let mut interior: Vec<DirRollup> = dirs
        .iter()
        .filter(|d| d.worktree_id == worktree_id && under(&d.rel_path, rel_root))
        .cloned()
        .collect();
    if interior.is_empty() {
        return None;
    }
    crate::report::aggregate_dir_totals(&mut interior, &std::collections::HashSet::new());
    let Some(root_total) = interior
        .iter()
        .find(|d| d.rel_path == rel_root)
        .map(|d| d.allocated_total)
    else {
        if std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty()) {
            eprintln!(
                "[trace]   interior rows exist ({}) but no root row {rel_root:?}",
                interior.len()
            );
        }
        return None;
    };
    dirs.retain(|d| !(d.worktree_id == worktree_id && under(&d.rel_path, rel_root)));
    dirs.extend(interior);
    Some((root_total, mtime_max, saw_hardlink))
}

/// Re-lists changed Source directories of one worktree from their stored
/// rows and returns the worktree's new Source byte total (the root row's
/// aggregate, artifact roots excluded) plus the artifact roots that
/// vanished with a deleted directory. `None` means the store cannot
/// answer without a walk: a changed directory it has no row for, or a
/// subdirectory it has never seen (which could be a new artifact or a
/// nested checkout).
fn relist_source_dirs(
    wt_root: &Path,
    worktree_id: &str,
    changed: &[PathBuf],
    artifact_rels: &HashSet<String>,
    dirs: &mut Vec<DirRollup>,
    files: &mut Vec<crate::report::FileRow>,
    large_file_min_bytes: u64,
) -> Option<(u64, HashSet<String>)> {
    use crate::fs_gate::MetadataExt;
    let mut removed_artifacts: HashSet<String> = HashSet::new();
    let mut rels: Vec<String> = changed
        .iter()
        .map(|c| rel_path_string(wt_root, c))
        .collect();
    rels.sort();
    rels.dedup();
    for rel_c in &rels {
        let abs = if rel_c.is_empty() {
            wt_root.to_path_buf()
        } else {
            wt_root.join(rel_c)
        };
        let known = dirs
            .iter()
            .any(|d| d.worktree_id == worktree_id && &d.rel_path == rel_c);
        let Ok(meta) = crate::fs_gate::symlink_metadata(&abs) else {
            if rel_c.is_empty() {
                return None; // the worktree itself is gone; handled by the caller.
            }
            // Deleted: its subtree's rows go, and any artifact rooted in it.
            dirs.retain(|d| !(d.worktree_id == worktree_id && under(&d.rel_path, rel_c)));
            files.retain(|f| !(f.worktree_id == worktree_id && under(&f.rel_path, rel_c)));
            for a in artifact_rels {
                if under(a, rel_c) {
                    removed_artifacts.insert(a.clone());
                }
            }
            continue;
        };
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }
        if !known {
            return None;
        }
        let Ok(entries) = crate::fs_gate::read_dir(&abs) else {
            continue;
        };
        let mut own: u64 = 0;
        let (mut nfiles, mut ndirs, mut nsymlinks) = (0u32, 0u32, 0u32);
        let mut dir_mtime: i64 = meta.mtime();
        let mut on_disk_subdirs: Vec<String> = Vec::new();
        let mut new_files: Vec<crate::report::FileRow> = Vec::new();
        for e in entries.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            if ft.is_symlink() {
                nsymlinks += 1;
                continue;
            }
            let child_rel = if rel_c.is_empty() {
                name.clone()
            } else {
                format!("{rel_c}/{name}")
            };
            if ft.is_dir() {
                // `.git` is the Git artifact root: a stored child like any other.
                ndirs += 1;
                on_disk_subdirs.push(child_rel);
            } else if ft.is_file() {
                let Ok(fm) = crate::fs_gate::symlink_metadata(e.path()) else {
                    continue;
                };
                if fm.file_type().is_symlink() || !fm.is_file() {
                    continue;
                }
                nfiles += 1;
                let bytes = crate::attribution::allocated_bytes(&fm);
                own += bytes;
                dir_mtime = dir_mtime.max(fm.mtime());
                if bytes >= large_file_min_bytes {
                    new_files.push(crate::report::FileRow {
                        worktree_id: worktree_id.to_string(),
                        rel_path: child_rel,
                        allocated: bytes,
                        mod_time_min: (fm.mtime() / 60) as i32,
                        growth_bytes: None,
                    });
                }
            }
        }
        // Children the store knows here: Source dir rows and artifact roots.
        let stored_children: HashSet<String> = dirs
            .iter()
            .filter(|d| d.worktree_id == worktree_id && d.parent_rel_path.as_deref() == Some(rel_c))
            .map(|d| d.rel_path.clone())
            .chain(
                artifact_rels
                    .iter()
                    .filter(|a| {
                        a.rsplit_once('/').map(|(p, _)| p) == Some(rel_c.as_str())
                            || (rel_c.is_empty() && !a.contains('/'))
                    })
                    .cloned(),
            )
            .collect();
        if on_disk_subdirs.iter().any(|c| !stored_children.contains(c)) {
            return None; // something new under here: the walker decides what it is.
        }
        for gone in stored_children
            .iter()
            .filter(|c| !on_disk_subdirs.contains(c))
        {
            dirs.retain(|d| !(d.worktree_id == worktree_id && under(&d.rel_path, gone)));
            files.retain(|f| !(f.worktree_id == worktree_id && under(&f.rel_path, gone)));
            if artifact_rels.contains(gone) {
                removed_artifacts.insert(gone.clone());
            }
        }
        // This directory's own row and its large-file rows.
        let parent_rel_path = if rel_c.is_empty() {
            None
        } else {
            Some(
                rel_c
                    .rsplit_once('/')
                    .map(|(p, _)| p.to_string())
                    .unwrap_or_default(),
            )
        };
        if let Some(existing) = dirs
            .iter_mut()
            .find(|d| d.worktree_id == worktree_id && &d.rel_path == rel_c)
        {
            existing.own_allocated = own;
            existing.allocated_total = own;
            existing.file_count = nfiles;
            existing.entry_count = nfiles + ndirs + nsymlinks;
            existing.symlink_count = nsymlinks;
            existing.mod_time_min = (dir_mtime / 60) as i32;
            existing.parent_rel_path = parent_rel_path;
        }
        files.retain(|f| {
            !(f.worktree_id == worktree_id
                && f.rel_path.rsplit_once('/').map(|(p, _)| p).unwrap_or("") == rel_c.as_str())
        });
        files.extend(new_files);
    }
    // Re-aggregate this worktree's directory rows; the root row's total,
    // with artifact roots excluded from the roll-up, is the Source bytes.
    let mut mine: Vec<DirRollup> = dirs
        .iter()
        .filter(|d| d.worktree_id == worktree_id)
        .cloned()
        .collect();
    let roots: HashSet<(String, String)> = artifact_rels
        .iter()
        .map(|r| (worktree_id.to_string(), r.clone()))
        .collect();
    crate::report::aggregate_dir_totals(&mut mine, &roots);
    let source_total = mine
        .iter()
        .find(|d| d.rel_path.is_empty())
        .map(|d| d.allocated_total)?;
    dirs.retain(|d| d.worktree_id != worktree_id);
    dirs.extend(mine);
    Some((source_total, removed_artifacts))
}

/// The incremental path: re-walks only the worktrees/artifact roots
/// FSEvents implicated, carrying every other row forward from the store
/// unchanged (see [`reconstruct_attribution`]).
fn apply_incremental(
    stage: &crate::bus::Stage,
    prev: &[StoredWorktree],
    changed_dirs: &[PathBuf],
    observed_at: u64,
    large_file_min_bytes: u64,
    dir: &Path,
    refreshed_unowned: Option<Vec<UnownedRow>>,
) -> Result<TrackedWalk> {
    let trace = std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty());
    let t0 = std::time::Instant::now();
    let mut attribution = reconstruct_attribution(dir)?;
    if let Some(rows) = refreshed_unowned {
        let new_total: u64 = rows.iter().map(|r| r.bytes).sum();
        attribution.walked_total = attribution.walked_total - attribution.unowned_total + new_total;
        attribution.unowned_total = new_total;
        attribution.unowned = rows;
    }
    // The incremental arithmetic below works on one remainder row per
    // worktree; the store holds it already split. See `collapse_remainder`.
    collapse_remainder(&mut attribution);
    if trace {
        eprintln!("[trace] incremental: reconstruct store: {:?}", t0.elapsed());
    }
    let worktree_root: HashMap<String, PathBuf> = prev
        .iter()
        .map(|w| (w.worktree_id.clone(), w.path.clone()))
        .collect();
    // Rows reconstructed above carry a bare relative path; re-join it
    // against the worktree's root now that we know it.
    for (worktree_id, rows) in attribution.artifacts_by_worktree.iter_mut() {
        let Some(root) = worktree_root.get(worktree_id) else {
            continue;
        };
        for row in rows.iter_mut() {
            row.path = if row.path.as_os_str().is_empty() {
                root.clone()
            } else {
                root.join(&row.path)
            };
        }
    }

    let mut discovered: Vec<DiscoveredWorktree> = prev
        .iter()
        .map(|w| DiscoveredWorktree {
            project_id: w.project_id.clone(),
            project_name: w.project_name.clone(),
            path: w.path.clone(),
            kind: w.kind.clone(),
            remote_url: w.remote_url.clone(),
        })
        .collect();

    let mut worktrees_to_rewalk: HashSet<String> = HashSet::new();
    let mut source_dirs_to_relist: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut artifact_roots_to_resize: HashMap<PathBuf, (String, ArtifactKind, Vec<PathBuf>)> =
        HashMap::new();
    let mut discovery_scan_roots: Vec<PathBuf> = Vec::new();

    for changed in changed_dirs {
        let nearest = prev
            .iter()
            .filter(|w| changed.starts_with(&w.path))
            .max_by_key(|w| w.path.as_os_str().len());
        let Some(wt) = nearest else {
            // The caller refreshed unowned boundaries and checked new
            // subtrees for checkouts already (or selected a full walk).
            // Repeating discovery here would traverse unchanged siblings.
            continue;
        };
        let artifact_hit = attribution
            .artifacts_by_worktree
            .get(&wt.worktree_id)
            .and_then(|rows| {
                rows.iter()
                    .filter(|r| {
                        !r.kind.is_worktree_remainder()
                            && (changed == &r.path || changed.starts_with(&r.path))
                    })
                    .max_by_key(|r| r.path.as_os_str().len())
            });
        if let Some(row) = artifact_hit {
            artifact_roots_to_resize
                .entry(row.path.clone())
                .or_insert_with(|| (wt.worktree_id.clone(), row.kind.clone(), Vec::new()))
                .2
                .push(changed.clone());
        } else {
            source_dirs_to_relist
                .entry(wt.worktree_id.clone())
                .or_default()
                .push(changed.clone());
            // A changed directory that gained (or lost) a `.git` inside
            // an already-known worktree's tree is a nested checkout; the
            // worktree-level rewalk below re-sizes but does not itself
            // run project discovery, so scan explicitly too.
            if changed != &wt.path && crate::fs_gate::exists(changed.join(".git")) {
                discovery_scan_roots.push(changed.clone());
                worktrees_to_rewalk.insert(wt.worktree_id.clone());
            }
        }
    }
    let mut total_delta: i64 = 0;
    let (mut n_interior, mut n_whole) = (0usize, 0usize);
    // A changed Source directory is re-listed in place from its stored
    // row: own bytes, vanished children dropped, the worktree's Source
    // total re-aggregated. Only a directory the store has never seen (a
    // new subtree, which may be a new artifact or a nested checkout)
    // sends the whole worktree back to the walker.
    let t_relist = std::time::Instant::now();
    let mut relisted = 0usize;
    for (wt_id, changed) in &source_dirs_to_relist {
        if worktrees_to_rewalk.contains(wt_id) {
            continue;
        }
        let Some(root) = worktree_root.get(wt_id).cloned() else {
            continue;
        };
        let source_hardlinked = attribution
            .artifacts_by_worktree
            .get(wt_id)
            .and_then(|rows| rows.iter().find(|r| r.kind == ArtifactKind::Source))
            .map(|r| r.hardlinked)
            .unwrap_or(true);
        if source_hardlinked {
            if trace {
                eprintln!("[trace]   source relist skipped ({wt_id}): unit has hardlinks");
            }
            worktrees_to_rewalk.insert(wt_id.clone());
            continue;
        }
        let artifact_rels: HashSet<String> = attribution
            .artifacts_by_worktree
            .get(wt_id)
            .map(|rows| {
                rows.iter()
                    .filter(|r| !r.kind.is_worktree_remainder())
                    .map(|r| rel_path_string(&root, &r.path))
                    .collect()
            })
            .unwrap_or_default();
        match relist_source_dirs(
            &root,
            wt_id,
            changed,
            &artifact_rels,
            &mut attribution.dirs,
            &mut attribution.files,
            large_file_min_bytes,
        ) {
            Some((new_source_local, removed_artifact_rels)) => {
                relisted += 1;
                if let Some(rows) = attribution.artifacts_by_worktree.get_mut(wt_id) {
                    // Artifact roots that vanished with a deleted directory.
                    rows.retain(|r| {
                        let rel = rel_path_string(&root, &r.path);
                        if !r.kind.is_worktree_remainder() && removed_artifact_rels.contains(&rel) {
                            total_delta -= r.bytes as i64;
                            false
                        } else {
                            true
                        }
                    });
                    if let Some(src) = rows.iter_mut().find(|r| r.kind == ArtifactKind::Source) {
                        let old_local = if src.local_bytes == 0 {
                            src.bytes
                        } else {
                            src.local_bytes
                        };
                        let delta = new_source_local as i64 - old_local as i64;
                        src.bytes = (src.bytes as i64 + delta).max(0) as u64;
                        src.local_bytes = new_source_local;
                        src.observed_at = observed_at;
                        total_delta += delta;
                    }
                }
            }
            None => {
                if trace {
                    eprintln!(
                        "[trace]   source relist fell back ({wt_id}): a changed directory is new to the store"
                    );
                }
                worktrees_to_rewalk.insert(wt_id.clone());
            }
        }
    }
    if trace {
        eprintln!(
            "[trace] incremental: relist {} worktrees' source dirs in place ({} fell back to a re-walk): {:?}",
            relisted,
            source_dirs_to_relist.len() - relisted,
            t_relist.elapsed()
        );
    }

    // New checkouts/worktrees discovered under any scan root.
    let t_scan = std::time::Instant::now();
    for scan_root in &discovery_scan_roots {
        {
            let found = crate::walk::discover_shallow(scan_root);
            for dw in found {
                if discovered.iter().any(|w| w.path == dw.path) {
                    continue;
                }
                let worktree_id = crate::entities::id_for(&dw.path.display().to_string());
                worktrees_to_rewalk.insert(worktree_id.clone());
                discovered.push(dw);
            }
        }
    }
    if trace {
        eprintln!(
            "[trace] incremental: shallow discovery at {} changed dirs: {:?}",
            discovery_scan_roots.len(),
            t_scan.elapsed()
        );
    }
    // Rebuild the root lookup now that new worktrees may have been added.
    let worktree_root: HashMap<String, PathBuf> = discovered
        .iter()
        .map(|dw| {
            (
                crate::entities::id_for(&dw.path.display().to_string()),
                dw.path.clone(),
            )
        })
        .collect();

    // Drop worktrees whose root has disappeared entirely: their rows are
    // simply not carried into this result, which tombstones them the next
    // time `growth::observe_and_annotate*` runs (a present row this
    // observation no longer emits is marked absent automatically).
    discovered.retain(|dw| crate::fs_gate::exists(&dw.path));
    let discovered_ids: HashSet<String> = discovered
        .iter()
        .map(|dw| crate::entities::id_for(&dw.path.display().to_string()))
        .collect();
    attribution
        .artifacts_by_worktree
        .retain(|id, _| discovered_ids.contains(id));
    attribution
        .dirs
        .retain(|d| discovered_ids.contains(&d.worktree_id));
    attribution
        .files
        .retain(|f| discovered_ids.contains(&f.worktree_id));

    // Resize individual artifact roots.
    let t_resize = std::time::Instant::now();
    for (root_path, (worktree_id, kind, changed_here)) in &artifact_roots_to_resize {
        if worktrees_to_rewalk.contains(worktree_id) {
            continue; // superseded by the full worktree rewalk below.
        }
        if !crate::fs_gate::exists(root_path) {
            // The artifact directory itself was removed: drop the row
            // entirely rather than leaving a phantom zero-byte entry a
            // full walk would never have produced. A later change that
            // recreates this path finds no artifact_hit for it next
            // time (the row is gone), so it correctly falls through to
            // a worktree rewalk instead of a resize.
            if let Some(rows) = attribution.artifacts_by_worktree.get_mut(worktree_id)
                && let Some(pos) = rows.iter().position(|r| &r.path == root_path)
            {
                total_delta -= rows.remove(pos).bytes as i64;
            }
            let wt_root = worktree_root.get(worktree_id).cloned().unwrap_or_default();
            let rel_root = rel_path_string(&wt_root, root_path);
            attribution
                .dirs
                .retain(|d| !(d.worktree_id == *worktree_id && under(&d.rel_path, &rel_root)));
            continue;
        }
        let Some(wt_root) = worktree_root.get(worktree_id).cloned() else {
            continue;
        };
        let rel_root = rel_path_string(&wt_root, root_path);
        // Cheap path: the store holds this unit's interior directory
        // rows, so only the directories FSEvents named are re-listed and
        // the unit's total is re-aggregated from the rows.
        let new_local: u64;
        let new_mtime: u64;
        let measured_hardlinked: bool;
        // Update path allocations without retaining an inode inventory.
        // Hardlinked units keep their last unique-byte measurement as stale.
        let hardlinked = attribution
            .artifacts_by_worktree
            .get(worktree_id)
            .and_then(|rows| rows.iter().find(|r| &r.path == root_path))
            .map(|r| r.hardlinked)
            .unwrap_or(true);
        let has_interior = attribution
            .dirs
            .iter()
            .any(|d| d.worktree_id == *worktree_id && d.rel_path == rel_root);
        if has_interior
            && let Some((local, mtime, saw_hardlink)) = resize_interior(
                &wt_root,
                worktree_id,
                &rel_root,
                changed_here,
                &mut attribution.dirs,
            )
        {
            if hardlinked || saw_hardlink {
                if let Some(existing) = attribution
                    .artifacts_by_worktree
                    .get_mut(worktree_id)
                    .and_then(|rows| rows.iter_mut().find(|r| &r.path == root_path))
                {
                    // Allocation rollups are current. Unique-byte charges stay
                    // at their last measurement until a full reconciliation.
                    existing.dedup_stale = true;
                    existing.hardlinked = true;
                    existing.mtime_max = existing.mtime_max.max(mtime);
                    existing.observed_at = observed_at;
                }
                n_interior += 1;
                continue;
            }
            new_local = local;
            new_mtime = mtime;
            measured_hardlinked = false;
            n_interior += 1;
        } else {
            n_whole += 1;
            let (row, dirs) = crate::walk::resize_artifact_with_dirs(
                root_path,
                kind.clone(),
                observed_at,
                Some((worktree_id, &wt_root)),
            );
            attribution
                .dirs
                .retain(|d| !(d.worktree_id == *worktree_id && under(&d.rel_path, &rel_root)));
            attribution.dirs.extend(dirs);
            new_local = row.local_bytes.max(row.bytes);
            new_mtime = row.mtime_max;
            measured_hardlinked = row.hardlinked;
        }
        if let Some(rows) = attribution.artifacts_by_worktree.get_mut(worktree_id) {
            if let Some(existing) = rows.iter_mut().find(|r| &r.path == root_path) {
                // Hardlink-safe: the full walk charged shared inodes to
                // whichever row saw them first, so compare per-row local
                // figures and apply that delta to the globally-deduped
                // `bytes` instead of replacing it with a re-count (#29).
                let old_local = if existing.local_bytes == 0 {
                    existing.bytes
                } else {
                    existing.local_bytes
                };
                let delta = new_local as i64 - old_local as i64;
                existing.bytes = (existing.bytes as i64 + delta).max(0) as u64;
                existing.local_bytes = new_local;
                existing.dedup_stale = false;
                existing.hardlinked = measured_hardlinked;
                existing.mtime_max = existing.mtime_max.max(new_mtime);
                existing.observed_at = observed_at;
                existing.source = crate::report::Source::new("filesystem.fsevents");
                total_delta += delta;
            } else {
                let (row, _) = crate::walk::resize_artifact_with_dirs(
                    root_path,
                    kind.clone(),
                    observed_at,
                    Some((worktree_id, &wt_root)),
                );
                total_delta += row.bytes as i64;
                rows.push(row);
            }
        }
    }

    // Rewalk whole worktrees whose Source tree (or newly discovered
    // subtree) was implicated. `all_worktree_refs` is the *complete*
    // known worktree list (every worktree, not just the one being
    // rewalked): a linked worktree frequently lives inside its main
    // checkout's own directory tree (e.g. `.worktrees/<name>`), so
    // walking with only one worktree in the known list would let
    // `nearest_worktree` fold a nested worktree's own bytes into this
    // one -- on top of that nested worktree's unrelated, still-correct
    // carried-forward rows, double counting them. Passing the full list
    // keeps nested-worktree boundaries exactly as a full walk would;
    // only the entries keyed by *this* `worktree_id` are taken out of
    // the result below, since every other worktree here (including any
    // nested one this walk happened to pass through) keeps its
    // carried-forward rows untouched.
    let worktree_ids: Vec<String> = discovered
        .iter()
        .map(|dw| crate::entities::id_for(&dw.path.display().to_string()))
        .collect();
    let all_worktree_refs: Vec<(&Path, &str)> = discovered
        .iter()
        .zip(worktree_ids.iter())
        .map(|(dw, id)| (dw.path.as_path(), id.as_str()))
        .collect();

    if trace {
        eprintln!(
            "[trace] incremental: resize {} artifact roots: {:?}",
            artifact_roots_to_resize.len(),
            t_resize.elapsed()
        );
    }
    let t_rewalk = std::time::Instant::now();
    for worktree_id in &worktrees_to_rewalk {
        let Some(root) = worktree_root.get(worktree_id) else {
            continue;
        };
        // Every stored artifact root in this worktree that no changed
        // directory touches is carried forward as-is; the walk re-sizes
        // only the implicated ones (a touch inside `target/` re-sizes
        // `target/`, not the six other artifacts next to it).
        let carry: HashMap<PathBuf, ArtifactRow> = attribution
            .artifacts_by_worktree
            .get(worktree_id)
            .map(|rows| {
                rows.iter()
                    .filter(|r| !r.kind.is_worktree_remainder())
                    .filter(|r| {
                        !changed_dirs
                            .iter()
                            .any(|c| c == &r.path || c.starts_with(&r.path))
                    })
                    .map(|r| (r.path.clone(), r.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let carried_rels: Vec<String> = carry.keys().map(|p| rel_path_string(root, p)).collect();
        let fresh = crate::walk::attribute_one_worktree(
            stage,
            root,
            &all_worktree_refs,
            observed_at,
            large_file_min_bytes,
            carry,
        );
        // Merge per row by (kind, path): a row present before and after
        // keeps its globally-deduped `bytes` adjusted by the change in its
        // own per-row local figure; a brand-new row starts from its local
        // figure; a vanished row is subtracted in full (#29 hardlinks).
        let old_rows = attribution
            .artifacts_by_worktree
            .remove(worktree_id)
            .unwrap_or_default();
        let mut merged: Vec<ArtifactRow> = Vec::new();
        let fresh_rows = fresh
            .artifacts_by_worktree
            .get(worktree_id)
            .cloned()
            .unwrap_or_default();
        let mut matched = vec![false; old_rows.len()];
        for mut nr in fresh_rows {
            let pos = old_rows
                .iter()
                .position(|o| o.kind == nr.kind && o.path == nr.path);
            match pos {
                Some(i) => {
                    matched[i] = true;
                    let o = &old_rows[i];
                    let old_local = if o.local_bytes == 0 {
                        o.bytes
                    } else {
                        o.local_bytes
                    };
                    let new_local = if nr.local_bytes == 0 {
                        nr.bytes
                    } else {
                        nr.local_bytes
                    };
                    let delta = new_local as i64 - old_local as i64;
                    nr.bytes = (o.bytes as i64 + delta).max(0) as u64;
                    nr.regrowth_count = o.regrowth_count;
                    total_delta += delta;
                }
                None => {
                    let local = if nr.local_bytes == 0 {
                        nr.bytes
                    } else {
                        nr.local_bytes
                    };
                    nr.bytes = local;
                    total_delta += local as i64;
                }
            }
            merged.push(nr);
        }
        for (i, o) in old_rows.iter().enumerate() {
            if !matched[i] {
                total_delta -= o.bytes as i64;
            }
        }
        if merged.is_empty() {
            attribution.artifacts_by_worktree.remove(worktree_id);
        } else {
            attribution
                .artifacts_by_worktree
                .insert(worktree_id.clone(), merged);
        }
        // Only this worktree's own dir/file rows come out of `fresh`;
        // any nested worktree's rows the walk happened to also produce
        // are discarded here (that worktree's carried-forward rows are
        // already correct and were not queued for rewalk).
        // Rows under a carried-forward artifact are still current: the
        // walk did not enter those trees, so it produced no rows for them.
        attribution.dirs.retain(|d| {
            &d.worktree_id != worktree_id || carried_rels.iter().any(|r| under(&d.rel_path, r))
        });
        attribution.dirs.extend(
            fresh
                .dirs
                .into_iter()
                .filter(|d| &d.worktree_id == worktree_id),
        );
        attribution.files.retain(|f| &f.worktree_id != worktree_id);
        attribution.files.extend(
            fresh
                .files
                .into_iter()
                .filter(|f| &f.worktree_id == worktree_id),
        );
    }

    attribution.attributed_total =
        (attribution.attributed_total as i64 + total_delta).max(0) as u64;
    attribution.walked_total = (attribution.walked_total as i64 + total_delta).max(0) as u64;

    if trace {
        eprintln!(
            "[trace] incremental: re-walk {} worktrees: {:?}",
            worktrees_to_rewalk.len(),
            t_rewalk.elapsed()
        );
    }
    split_remainder(&discovered, &mut attribution);
    Ok(TrackedWalk {
        discovered,
        attribution,
        mode: "incremental",
        // Filled in by the caller, which is the only place that still
        // holds the replay's unfiltered answer.
        event_window: None,
        changed_paths: Some(changed_dirs.to_vec()),
        reason: "incremental",
        changed_dirs: changed_dirs.len(),
        rewalked: Some(
            worktrees_to_rewalk
                .iter()
                .cloned()
                .chain(
                    artifact_roots_to_resize
                        .values()
                        .map(|(id, _, _)| id.clone()),
                )
                .collect(),
        ),
        in_place: (n_interior, n_whole, relisted, worktrees_to_rewalk.len()),
        // Set by the caller (`stage_tracked_with_source`), which has both
        // the previous topology and this result's `discovered` in hand.
        unconfirmed_worktree_ids: Vec::new(),
    })
}

// ---------------------------------------------------------------------
// #43: external/shared storage units -- a new key family in this same
// current + reverse-delta store, not a new store. Identity is
// `(detector_id, category, device, canonical_path)`: independent of any
// project/worktree, unlike the artifact rows above. Kept scope-wide
// (directly under `swamp_dir`, not per-volume) because an external
// unit's device need not match any scan root's device.
// ---------------------------------------------------------------------

pub(crate) fn external_row_key(
    detector_id: &str,
    category: &str,
    device: u64,
    path: &str,
) -> String {
    format!("{detector_id}\u{1}{category}\u{1}{device}\u{1}{path}")
}

fn external_dir(swamp_dir: &Path) -> PathBuf {
    swamp_dir.join("external")
}
fn external_current_path(dir: &Path) -> PathBuf {
    dir.join("current.parquet")
}
fn external_deltas_dir(dir: &Path) -> PathBuf {
    dir.join("deltas")
}

// ---------------------------------------------------------------------
// external/folded.parquet -- the measurement the next pass may reuse
// ---------------------------------------------------------------------

/// `external/folded/<id>.parquet` (R19): one file per unit, keyed by
/// `entities::id_for(unit_path)`. The former single `external/folded.parquet`
/// held every unit's rows, so each of ~70 units' reuse check read the
/// whole table and each walked unit rewrote it -- about a second of an
/// unchanged pass in reads alone. A store written before R19 keeps its
/// old file unread; each unit re-measures once and lands in its own.
fn folded_unit_path(swamp_dir: &Path, unit_path: &str) -> PathBuf {
    external_dir(swamp_dir)
        .join("folded")
        .join(format!("{}.parquet", crate::entities::id_for(unit_path)))
}

/// Every stored folded row for `unit_path`, root row first. Empty when
/// nothing is stored, the store is unreadable, or the table is corrupt:
/// a cache that cannot be read is a cache miss, never an error.
pub fn folded_rows_for(swamp_dir: &Path, unit_path: &str) -> Vec<FoldedRow> {
    let mut rows: Vec<FoldedRow> = read_folded_rows(&folded_unit_path(swamp_dir, unit_path))
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.unit_path == unit_path)
        .collect();
    rows.sort_by_key(|a| a.rel_dir.len());
    rows
}

/// Re-stamps the reused units' rows to `observed_at`, so the next pass's
/// window can still vouch for them.
pub fn touch_folded_rows(swamp_dir: &Path, unit_paths: &[String], observed_at: u64) -> Result<()> {
    for unit_path in unit_paths {
        let path = folded_unit_path(swamp_dir, unit_path);
        let mut rows: Vec<FoldedRow> = read_folded_rows(&path).unwrap_or_default();
        let mut touched = false;
        for row in rows.iter_mut() {
            if row.unit_path == *unit_path && row.observed_at != observed_at {
                row.observed_at = observed_at;
                touched = true;
            }
        }
        if touched {
            write_folded_rows(&path, &rows)?;
        }
    }
    Ok(())
}

/// Replaces `unit_path`'s folded rows wholesale.
pub fn store_folded_rows(swamp_dir: &Path, unit_path: &str, rows: &[FoldedRow]) -> Result<()> {
    let dir = external_dir(swamp_dir).join("folded");
    store::StoreDir::at(&dir)?.create()?;
    write_folded_rows(&folded_unit_path(swamp_dir, unit_path), rows)
}

// ---------------------------------------------------------------------
// external/volume_stamps.parquet (R19) -- see `columns::StoredVolumeStampRow`
// ---------------------------------------------------------------------

fn volume_stamps_path(swamp_dir: &Path) -> PathBuf {
    external_dir(swamp_dir).join("volume_stamps.parquet")
}

/// The stored sealed-mount stamps for `unit_path`; empty on a miss.
pub fn volume_stamps_for(swamp_dir: &Path, unit_path: &str) -> Vec<columns::StoredVolumeStampRow> {
    columns::read_volume_stamp_rows(&volume_stamps_path(swamp_dir))
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.unit_path == unit_path)
        .collect()
}

/// Replaces `unit_path`'s sealed-mount stamps wholesale.
pub fn store_volume_stamps(
    swamp_dir: &Path,
    unit_path: &str,
    rows: &[columns::StoredVolumeStampRow],
) -> Result<()> {
    let dir = external_dir(swamp_dir);
    store::StoreDir::at(&dir)?.create()?;
    let path = volume_stamps_path(swamp_dir);
    let mut all: Vec<columns::StoredVolumeStampRow> = columns::read_volume_stamp_rows(&path)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.unit_path != unit_path)
        .collect();
    all.extend(rows.iter().cloned());
    columns::write_volume_stamp_rows(&path, &all)
}

/// Which family of rows in the shared external current table an
/// observation speaks for. External/detector-level units and agent-tool
/// units share one table and one key scheme (`agent:`-prefixed
/// categories distinguish them), which is deliberate -- one store, one
/// key family, two granularities -- but it means neither observation may
/// assume a key it did not see has disappeared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyFamily {
    /// Detector-level external storage units (`crate::external`).
    External,
    /// Units *inside* a tool home (`crate::agents`).
    Agent,
    /// Identified units *inside* a machine-wide build store
    /// (`crate::build_stores`): a Maven artifact version, a Go module, a
    /// DerivedData project folder. Written by the external observation
    /// that measured the store, and swept only inside stores whose
    /// interior that observation identified this pass.
    BuildStore,
}

/// The category prefix of [`KeyFamily::BuildStore`] rows.
pub const BUILD_STORE_CATEGORY_PREFIX: &str = "build-store:";

impl KeyFamily {
    fn matches(self, category: &str) -> bool {
        let is_agent = category.starts_with("agent:");
        let is_build_store = category.starts_with(BUILD_STORE_CATEGORY_PREFIX);
        match self {
            Self::Agent => is_agent,
            Self::BuildStore => is_build_store,
            Self::External => !is_agent && !is_build_store,
        }
    }
}

/// What one observation pass is entitled to tombstone
/// (`.oh/guardrails/history-sweeps-are-owned.md`).
///
/// The 2026-09-21 review's `unchanged_combined_observation_must_not_invent_regrowth`
/// counterexample: external and agent discovery both swept the shared
/// current table for keys they had not seen, so each tombstoned the
/// other's rows and the next pass recorded the resurrection as regrowth
/// -- pure fiction, on an unchanged filesystem.
///
/// A row may only be marked absent when **both** hold:
///
/// * it belongs to this observation's [`KeyFamily`], and
/// * its path lies inside a root this observation actually covered
///   completely this pass.
///
/// A root that was excluded, whose detector was disabled, that was
/// missing, unreadable, or simply not part of this pass contributes no
/// covered root, so nothing under it can be tombstoned. Coverage changes
/// are not storage changes.
#[derive(Debug, Clone)]
pub struct ObservationOwnership {
    pub family: KeyFamily,
    pub covered_roots: Vec<PathBuf>,
    /// Regions that lie *inside* a covered root but outside what this
    /// pass actually observed: a nested location the user excluded, one
    /// whose detector is disabled, one outside the explicit command
    /// roots.
    ///
    /// A path-prefix window alone cannot express this, and that is
    /// precisely how a one-line `exclude` change became a storage
    /// change: the excluded child's stored row still lay under the
    /// measured parent's root, so the owned sweep tombstoned it, and
    /// removing the line again scored a regrowth. Zero bytes moved on
    /// disk (the 2026-09-22 re-review's CE4, violating both clauses of
    /// `.oh/guardrails/coverage-changes-are-not-storage-changes.md`).
    pub excluded_subtrees: Vec<PathBuf>,
}

impl ObservationOwnership {
    pub fn new(family: KeyFamily, covered_roots: Vec<PathBuf>) -> Self {
        Self {
            family,
            covered_roots,
            excluded_subtrees: Vec::new(),
        }
    }

    /// The same window with the regions this pass did *not* observe
    /// subtracted.
    pub fn excluding(mut self, excluded: Vec<PathBuf>) -> Self {
        self.excluded_subtrees = excluded;
        self
    }

    /// Whether `path` lies inside a region this observation covered
    /// *and* observed. An excluded subtree is inside the window and
    /// outside the pass, so it is not owned: unobserved is not deleted.
    pub fn covers(&self, path: &str) -> bool {
        let p = Path::new(path);
        if self
            .excluded_subtrees
            .iter()
            .any(|e| p == e.as_path() || p.starts_with(e))
        {
            return false;
        }
        self.covered_roots
            .iter()
            .any(|r| p == r.as_path() || p.starts_with(r))
    }

    /// Whether this observation owns the stored row `key` (family +
    /// coverage). The tombstone loop is guarded by this and nothing else.
    fn owns(&self, key: &str) -> bool {
        let mut parts = key.split('\u{1}');
        let (_detector, Some(category), _device, Some(path)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return false;
        };
        self.family.matches(category) && self.covers(path)
    }
}

/// One external unit's observed facts for this pass, before growth
/// annotation. Mirrors [`Observed`] for artifact rows.
pub struct ObservedExternal {
    pub(crate) key: String,
    pub(crate) detector_id: String,
    pub(crate) category: String,
    pub(crate) device: u64,
    pub(crate) path: String,
    pub(crate) bytes: u64,
    pub(crate) hardlinked: bool,
}

fn external_history_index(dir: &Path, retention_days: u64, now: u64) -> Result<HistoryIndex> {
    let retention_secs = retention_days.saturating_mul(86400);
    let horizon = now.saturating_sub(retention_secs);
    let mut index: HashMap<String, Vec<(u64, u64, bool)>> = HashMap::new();
    for row in read_external_rows(&external_current_path(dir))? {
        index
            .entry(external_row_key(
                row.detector_id(),
                row.category(),
                row.device(),
                row.path(),
            ))
            .or_default()
            .push((row.observed_at(), row.bytes(), true));
    }
    for delta_path in list_files_in(&external_deltas_dir(dir)) {
        for row in read_external_rows(&delta_path)? {
            if row.observed_at() < horizon {
                continue;
            }
            index
                .entry(external_row_key(
                    row.detector_id(),
                    row.category(),
                    row.device(),
                    row.path(),
                ))
                .or_default()
                .push((row.observed_at(), row.bytes(), true));
        }
    }
    for values in index.values_mut() {
        values.sort_by_key(|(t, _, _)| *t);
    }
    Ok(index)
}

/// Persists this pass's external-unit observations (current + reverse
/// delta, same layout as the artifact store) and returns
/// `(key -> (growth_bytes, regrowth_count))` for the caller to annotate
/// its own `ExternalUnit` rows with. `protected_keys` (mirroring `#42`'s
/// `protected_worktree_ids`): a key in this set is never tombstoned by
/// this pass even if absent from `observed` -- used when a unit's path
/// could not be confirmed gone-vs-inaccessible this pass.
pub fn observe_and_annotate_external(
    swamp_dir: &Path,
    observed: &[ObservedExternal],
    protected_keys: &HashSet<String>,
    ownership: &ObservationOwnership,
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
) -> Result<HashMap<String, (Option<i64>, u32)>> {
    let dir = external_dir(swamp_dir);
    crate::fs_gate::store::StoreDir::at(&dir)?.create()?;
    let mut table = ExternalHistory::load(&dir)?;

    let mut seen_keys: HashSet<String> = HashSet::new();
    for obs in observed {
        seen_keys.insert(obs.key.clone());
        table.observe(obs, observed_at);
    }

    // The owned sweep. `ownership.claim` is the whole guard: a key from
    // the other family, or one outside the regions this pass actually
    // covered, yields no claim and is left exactly as it is -- never
    // tombstoned, so never resurrected as invented regrowth on the next
    // pass. `protected_keys` (unconfirmed this pass) are not candidates.
    for key in table.unseen_present(&seen_keys) {
        if protected_keys.contains(&key) {
            continue;
        }
        if let Some(owned) = ownership.claim(&key) {
            table.tombstone(owned, observed_at);
        }
    }

    let target_time = observed_at.saturating_sub(since_secs);
    let history_index = external_history_index(&dir, retention_days, observed_at)?;
    let mut annotations: HashMap<String, (Option<i64>, u32)> = HashMap::new();
    for obs in observed {
        let history = history_index.get(&obs.key).cloned().unwrap_or_default();
        let growth = growth_since(&history, obs.bytes, target_time);
        let regrowth = table.row(&obs.key).map(|r| r.regrowth_count()).unwrap_or(0);
        annotations.insert(obs.key.clone(), (growth, regrowth));
    }

    table.commit()?;

    let files = list_files_in(&external_deltas_dir(&dir));
    if should_compact(&files) {
        let retention_secs = retention_days.saturating_mul(86400);
        let horizon = observed_at.saturating_sub(retention_secs);
        compact_external_deltas(&dir, &files, horizon)?;
    }

    Ok(annotations)
}

/// The current stored `(bytes, regrowth_count)` for one external-unit
/// key, straight off `current.parquet`, with no history-window
/// computation -- what a caller needs to show a unit's last known value
/// when this pass could not re-measure it (access lost, not deleted).
pub fn peek_external_current(swamp_dir: &Path, key: &str) -> Result<Option<(u64, u32)>> {
    let dir = external_dir(swamp_dir);
    let current_file = external_current_path(&dir);
    if !crate::fs_gate::exists(&current_file) {
        return Ok(None);
    }
    for row in read_external_rows(&current_file)? {
        if external_row_key(row.detector_id(), row.category(), row.device(), row.path()) == key {
            return Ok(Some((row.bytes(), row.regrowth_count())));
        }
    }
    Ok(None)
}

/// Read-only counterpart to [`observe_and_annotate_external`]: annotates
/// from existing history without writing a new observation.
pub fn annotate_readonly_external(
    swamp_dir: &Path,
    keys: &[String],
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
) -> Result<HashMap<String, (Option<i64>, u32)>> {
    let dir = external_dir(swamp_dir);
    let current_file = external_current_path(&dir);
    if !crate::fs_gate::exists(&current_file) {
        return Ok(HashMap::new());
    }
    let current: HashMap<String, StoredExternalRow> = read_external_rows(&current_file)?
        .into_iter()
        .map(|r| {
            (
                external_row_key(r.detector_id(), r.category(), r.device(), r.path()),
                r,
            )
        })
        .collect();
    let target_time = observed_at.saturating_sub(since_secs);
    let history_index = external_history_index(&dir, retention_days, observed_at)?;
    let mut out = HashMap::new();
    for key in keys {
        let history = history_index.get(key).cloned().unwrap_or_default();
        let bytes_now = current.get(key).map(|r| r.bytes()).unwrap_or(0);
        let growth = growth_since(&history, bytes_now, target_time);
        let regrowth = current.get(key).map(|r| r.regrowth_count()).unwrap_or(0);
        out.insert(key.clone(), (growth, regrowth));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    #[allow(unused_imports)]
    use std::fs::{self, File};
    #[test]
    fn artifact_and_file_compaction_preserve_sources_on_publish_failure() -> anyhow::Result<()> {
        use super::*;
        for artifact in [true, false] {
            let tmp = tempfile::tempdir()?;
            let dir = tmp.path();
            let delta_dir = if artifact {
                deltas_dir(dir)
            } else {
                files_deltas_dir(dir)
            };
            for i in 0..8 {
                let path = next_seq_path(&delta_dir, "delta-");
                if artifact {
                    StoredRow::write_for_test(
                        &path,
                        &[StoredRow::for_test(
                            "p",
                            "w",
                            "BuildOutput",
                            "target",
                            i,
                            i % 2 == 0,
                            i,
                            i as u32,
                        )],
                    )?;
                } else {
                    write_file_rows(
                        &path,
                        &[StoredFileRow {
                            worktree_id: "w".into(),
                            rel_path: "a".into(),
                            allocated: i,
                            mod_time_min: i as i32,
                            observed_at: i,
                        }],
                        3,
                    )?;
                }
            }
            let files = list_files_in(&delta_dir);
            let contents = files
                .iter()
                .map(fs::read)
                .collect::<std::io::Result<Vec<_>>>()?;
            let blocker = next_seq_path(&delta_dir, "delta-");
            fs::create_dir(&blocker)?;
            let result = if artifact {
                compact_if_needed(dir, 30, 100)
            } else {
                compact_file_deltas_if_needed(dir, 30, 100)
            };
            assert!(result.is_err());
            for (p, b) in files.iter().zip(&contents) {
                assert_eq!(&fs::read(p)?, b);
            }
            fs::remove_dir(blocker)?; // empty directory belonging to this fixture
            if artifact {
                let expected = files
                    .iter()
                    .map(|p| read_rows(p))
                    .collect::<Result<Vec<_>>>()?
                    .concat();
                compact_if_needed(dir, 30, 100)?;
                let after = list_files_in(&delta_dir);
                assert_eq!(after.len(), 1);
                assert_eq!(read_rows(&after[0])?, expected);
            } else {
                let expected = files
                    .iter()
                    .map(|p| read_file_rows(p))
                    .collect::<Result<Vec<_>>>()?
                    .concat();
                compact_file_deltas_if_needed(dir, 30, 100)?;
                let after = list_files_in(&delta_dir);
                assert_eq!(after.len(), 1);
                assert_eq!(read_file_rows(&after[0])?, expected);
            }
        }
        Ok(())
    }
    #[test]
    fn small_delta_compaction_is_lossless_and_publication_failure_keeps_sources()
    -> anyhow::Result<()> {
        use super::*;
        for fail in [false, true] {
            let tmp = tempfile::tempdir()?;
            let dir = tmp.path();
            let mut expected = Vec::new();
            for i in 0..8 {
                let row = StoredDirRow {
                    worktree_id: "w".into(),
                    rel_path: "target/debug".into(),
                    parent_rel_path: Some("target".into()),
                    allocated_total: i * 4096,
                    own_allocated: i * 512,
                    file_count: i as u32,
                    entry_count: i as u32 + 1,
                    symlink_count: 0,
                    mod_time_min: i as i32,
                    complete: i % 2 == 0,
                    observed_at: 100 + i,
                };
                write_dir_rows(
                    &next_seq_path(&dirs_deltas_dir(dir), "delta-"),
                    std::slice::from_ref(&row),
                    3,
                )?;
                expected.push(row);
            }
            let before = list_files_in(&dirs_deltas_dir(dir));
            let bytes: u64 = before.iter().map(|p| fs::metadata(p).unwrap().len()).sum();
            let contents = before
                .iter()
                .map(fs::read)
                .collect::<std::io::Result<Vec<_>>>()?;
            if fail {
                fs::create_dir(next_seq_path(&dirs_deltas_dir(dir), "delta-"))?;
                assert!(compact_dir_deltas_if_needed(dir, 30, 200).is_err());
                for (path, content) in before.iter().zip(contents) {
                    assert_eq!(fs::read(path)?, content);
                }
            } else {
                compact_dir_deltas_if_needed(dir, 30, 200)?;
                let after = list_files_in(&dirs_deltas_dir(dir));
                assert_eq!(after.len(), 1);
                let restored = read_dir_rows(&after[0])?;
                assert_eq!(
                    expected, restored,
                    "all metadata, coverage and timestamps must survive"
                );
                assert!(fs::metadata(&after[0])?.len() < bytes / 2);
            }
        }
        Ok(())
    }

    #[test]
    #[ignore = "read-only real-store delta packing comparison; set SWAMP_ENCODING_INPUT"]
    fn compare_real_delta_packing() -> anyhow::Result<()> {
        use super::*;
        let source =
            PathBuf::from(std::env::var_os("SWAMP_ENCODING_INPUT").context("input required")?);
        let tmp = tempfile::tempdir()?;
        for volume in fs::read_dir(source)? {
            let volume = volume?;
            if !volume.file_type()?.is_dir() {
                continue;
            }
            let dest = tmp.path().join(volume.file_name());
            fs::create_dir(&dest)?;
            for name in ["deltas", "dirs_deltas", "files_deltas"] {
                let inputs = list_files_in(&volume.path().join(name));
                if inputs.is_empty() {
                    continue;
                }
                fs::create_dir_all(dest.join(name))?;
                let before: u64 = inputs.iter().map(|p| fs::metadata(p).unwrap().len()).sum();
                for p in &inputs {
                    fs::copy(p, dest.join(name).join(p.file_name().unwrap()))?;
                }
                match name {
                    "deltas" => {
                        let mut expected = Vec::new();
                        for p in &inputs {
                            expected.extend(read_rows(p)?);
                        }
                        expected.sort_by_key(|r| format!("{:?}", r));
                        compact_if_needed(&dest, u64::MAX, 0)?;
                        let mut actual = Vec::new();
                        for p in list_files_in(&dest.join(name)) {
                            actual.extend(read_rows(&p)?);
                        }
                        actual.sort_by_key(|r| format!("{:?}", r));
                        assert_eq!(expected, actual);
                    }
                    "dirs_deltas" => {
                        let mut expected = Vec::new();
                        for p in &inputs {
                            expected.extend(read_dir_rows(p)?);
                        }
                        expected.sort_by_key(|r| format!("{:?}", r));
                        compact_dir_deltas_if_needed(&dest, u64::MAX, 0)?;
                        let mut actual = Vec::new();
                        for p in list_files_in(&dest.join(name)) {
                            actual.extend(read_dir_rows(&p)?);
                        }
                        actual.sort_by_key(|r| format!("{:?}", r));
                        assert_eq!(expected, actual);
                    }
                    _ => {
                        let mut expected = Vec::new();
                        for p in &inputs {
                            expected.extend(read_file_rows(p)?);
                        }
                        expected.sort_by_key(|r| format!("{:?}", r));
                        compact_file_deltas_if_needed(&dest, u64::MAX, 0)?;
                        let mut actual = Vec::new();
                        for p in list_files_in(&dest.join(name)) {
                            actual.extend(read_file_rows(&p)?);
                        }
                        actual.sort_by_key(|r| format!("{:?}", r));
                        assert_eq!(expected, actual);
                    }
                }
                let after: u64 = list_files_in(&dest.join(name))
                    .iter()
                    .map(|p| fs::metadata(p).unwrap().len())
                    .sum();
                println!(
                    "{}/{name}: before={before} after={after}",
                    volume.file_name().to_string_lossy()
                );
            }
        }
        Ok(())
    }
    #[test]
    #[ignore = "read-only real-store comparison; set SWAMP_ENCODING_INPUT"]
    fn compare_real_store_encoding() -> anyhow::Result<()> {
        use super::*;
        fn collect(path: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    collect(&entry.path(), out)?;
                } else if entry.path().extension().is_some_and(|x| x == "parquet") {
                    out.push(entry.path());
                }
            }
            Ok(())
        }
        let source = PathBuf::from(
            std::env::var_os("SWAMP_ENCODING_INPUT").context("SWAMP_ENCODING_INPUT required")?,
        );
        let tmp = tempfile::tempdir()?;
        let mut files = Vec::new();
        collect(&source, &mut files)?;
        let (mut old_total, mut new_total) = (0, 0);
        for (i, path) in files.iter().enumerate() {
            let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(path)?)?
                .with_batch_size(1_000_000)
                .build()?;
            let batches = reader.collect::<std::result::Result<Vec<_>, _>>()?;
            if batches.is_empty() {
                continue;
            }
            let schema = batches[0].schema();
            let out = tmp.path().join(format!("{i}.parquet"));
            let level = if path
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("delta")
            {
                3
            } else {
                9
            };
            crate::fs_gate::columns::write_parquet_atomic(
                &out,
                schema,
                batches.iter().cloned().map(Ok),
                level,
            )?;
            let restored = ParquetRecordBatchReaderBuilder::try_new(File::open(&out)?)?
                .with_batch_size(1_000_000)
                .build()?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            assert_eq!(batches, restored, "{}", path.display());
            let old = fs::metadata(path)?.len();
            let new = fs::metadata(&out)?.len();
            old_total += old;
            new_total += new;
            println!(
                "{} old={old} new={new}",
                path.strip_prefix(&source)?.display()
            );
        }
        println!("total old={old_total} new={new_total}");
        Ok(())
    }

    use super::*;

    #[test]
    fn parses_common_durations() {
        assert_eq!(parse_duration_secs("24h"), Some(24 * 3600));
        assert_eq!(parse_duration_secs("30d"), Some(30 * 86400));
        assert_eq!(parse_duration_secs("10m"), Some(600));
        assert_eq!(parse_duration_secs("45s"), Some(45));
        assert_eq!(parse_duration_secs("90"), Some(90));
        assert_eq!(parse_duration_secs(""), None);
        assert_eq!(parse_duration_secs("bogus"), None);
    }

    #[test]
    fn config_defaults_when_file_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = load_config(tmp.path());
        assert_eq!(cfg.retention_days, DEFAULT_RETENTION_DAYS);
        assert_eq!(cfg.since, DEFAULT_SINCE);
    }

    #[test]
    fn config_reads_toml_scalars() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("config.toml"),
            "retention_days = 14\nsince = \"6h\"\n",
        )
        .unwrap();
        let cfg = load_config(tmp.path());
        assert_eq!(cfg.retention_days, 14);
        assert_eq!(cfg.since, "6h");
    }

    #[test]
    fn checked_config_reads_the_scan_table() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("config.toml"),
            "[scan]\ndefaults = false\ninclude = [\"~/code\"]\nexclude = [\"~/code/scratch\"]\ndisabled_detectors = [\"homebrew\"]\n",
        )
        .unwrap();
        let cfg = load_config_checked(tmp.path()).expect("valid config");
        assert!(!cfg.scan.defaults);
        assert_eq!(cfg.scan.include, vec!["~/code".to_string()]);
        assert_eq!(cfg.scan.exclude, vec!["~/code/scratch".to_string()]);
        assert_eq!(cfg.scan.disabled_detectors, vec!["homebrew".to_string()]);
    }

    /// #41's core requirement: invalid explicit scope config must fail
    /// visibly, never silently broaden to the all-defaults scope. This
    /// is the shortcut the acceptance criteria calls out by name --
    /// falling back to `GrowthConfig::default()` on a parse error would
    /// make a typo in `[scan]` silently re-enable everything.
    #[test]
    fn invalid_scan_table_fails_visibly_instead_of_broadening_scope() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("config.toml"),
            "[scan]\ndefaults = \"yes\"\n", // wrong type: must be a bool
        )
        .unwrap();
        let err =
            load_config_checked(tmp.path()).expect_err("wrong-typed defaults must be rejected");
        assert!(
            err.to_string().contains("config.toml") || format!("{err:#}").contains("config.toml"),
            "error should name the offending file: {err:#}"
        );
        // The infallible convenience wrapper used deep in the report
        // pipeline still falls back to scalar defaults (unaffected
        // pipeline behavior); only scope-resolving call sites are
        // required to treat this as fatal (see `crates/cli/src/main.rs`'s
        // `resolve_scope`).
        assert_eq!(
            load_config(tmp.path()).scan,
            crate::scope::ScanConfig::default()
        );
    }

    #[test]
    fn malformed_toml_syntax_fails_visibly() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("config.toml"), "this is not [ valid toml\n").unwrap();
        assert!(load_config_checked(tmp.path()).is_err());
    }

    use crate::entities::Confidence;
    use crate::report::{ArtifactKind, ArtifactRow, ProjectRow, Source, WorktreeKind, WorktreeRow};
    use std::path::PathBuf;

    fn one_artifact_project(worktree_root: &Path, bytes: u64) -> ProjectRow {
        ProjectRow {
            project_id: "proj-1".to_string(),
            name: "proj".to_string(),
            remote: None,
            ecosystems: Vec::new(),
            worktrees: vec![WorktreeRow {
                worktree_id: "wt-1".to_string(),
                path: worktree_root.to_path_buf(),
                kind: WorktreeKind::Main,
                artifacts: vec![ArtifactRow {
                    kind: ArtifactKind::DependencyTree,
                    path: worktree_root.join("node_modules"),
                    bytes,
                    mtime_max: 0,
                    ecosystem: None,
                    hardlinked: false,
                    dedup_stale: false,
                    local_bytes: 0,
                    allocated_bytes: None,
                    allocated_growth_bytes: None,
                    track: None,
                    growth_bytes: None,
                    regrowth_count: 0,
                    observed_at: 0,
                    confidence: Confidence::High,
                    source: Source::new("test"),
                    note: None,
                    created_at: None,
                    containers: Vec::new(),
                    shared_with: Vec::new(),
                    dangling: false,
                    evidence: Vec::new(),
                }],
                signals: vec![],
                branch: None,
                github: None,
                merge_complete: None,
                idle_secs: None,
            }],
        }
    }

    fn artifact_row(projects: &[ProjectRow]) -> &ArtifactRow {
        &projects[0].worktrees[0].artifacts[0]
    }

    #[test]
    fn stale_unique_measurements_are_gaps_until_reconciled() {
        let tmp = tempfile::tempdir().unwrap();
        let root = PathBuf::from("/repo");
        let mut projects = vec![one_artifact_project(&root, 1000)];
        observe_and_annotate(
            &crate::bus::Stage::for_tests(),
            tmp.path(),
            1,
            &mut projects,
            1000,
            30,
            1000,
            &HashSet::new(),
        )
        .unwrap();
        projects[0].worktrees[0].artifacts[0].dedup_stale = true;
        observe_and_annotate(
            &crate::bus::Stage::for_tests(),
            tmp.path(),
            1,
            &mut projects,
            2000,
            30,
            1000,
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(artifact_row(&projects).growth_bytes, None);
        let dir = volume_dir(tmp.path(), 1);
        assert!(read_rows(&current_path(&dir)).unwrap()[0].dedup_stale());
        let (_, totals) = history_series(&dir, 1000, 2, 2000);
        assert_eq!(totals, vec![Some(1000), None]);
        projects[0].worktrees[0].artifacts[0].dedup_stale = false;
        projects[0].worktrees[0].artifacts[0].bytes = 2000;
        observe_and_annotate(
            &crate::bus::Stage::for_tests(),
            tmp.path(),
            1,
            &mut projects,
            3000,
            30,
            1000,
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(
            artifact_row(&projects).growth_bytes,
            None,
            "baseline at 2000 was stale"
        );
        let (_, totals) = history_series(&dir, 2000, 3, 3000);
        assert_eq!(totals, vec![Some(1000), None, Some(2000)]);
    }

    #[test]
    fn first_observation_has_no_growth() {
        let tmp = tempfile::tempdir().unwrap();
        let root = PathBuf::from("/repo");
        let mut projects = vec![one_artifact_project(&root, 1_000_000)];
        observe_and_annotate(
            &crate::bus::Stage::for_tests(),
            tmp.path(),
            1,
            &mut projects,
            1_000,
            30,
            3600,
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(artifact_row(&projects).growth_bytes, None);
        assert_eq!(artifact_row(&projects).regrowth_count, 0);
    }

    #[test]
    fn second_observation_reports_growth_since_first() {
        let tmp = tempfile::tempdir().unwrap();
        let root = PathBuf::from("/repo");

        let mut first = vec![one_artifact_project(&root, 1_000_000)];
        observe_and_annotate(
            &crate::bus::Stage::for_tests(),
            tmp.path(),
            1,
            &mut first,
            1_000,
            30,
            3600,
            &HashSet::new(),
        )
        .unwrap();

        let mut second = vec![one_artifact_project(&root, 4_000_000)];
        observe_and_annotate(
            &crate::bus::Stage::for_tests(),
            tmp.path(),
            1,
            &mut second,
            2_000,
            30,
            3600,
            &HashSet::new(),
        )
        .unwrap();

        assert_eq!(artifact_row(&second).growth_bytes, Some(3_000_000));
    }

    #[test]
    fn unchanged_observation_appends_no_delta_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = PathBuf::from("/repo");

        let mut first = vec![one_artifact_project(&root, 1_000_000)];
        observe_and_annotate(
            &crate::bus::Stage::for_tests(),
            tmp.path(),
            1,
            &mut first,
            1_000,
            30,
            3600,
            &HashSet::new(),
        )
        .unwrap();
        let dir = volume_dir(tmp.path(), 1);
        let after_first = list_delta_files(&dir).len();
        assert_eq!(
            after_first, 0,
            "a brand-new row has no prior state to diff against, so the very \
             first observation must not fabricate a delta either"
        );

        let mut second = vec![one_artifact_project(&root, 1_000_000)];
        observe_and_annotate(
            &crate::bus::Stage::for_tests(),
            tmp.path(),
            1,
            &mut second,
            2_000,
            30,
            3600,
            &HashSet::new(),
        )
        .unwrap();
        let after_second = list_delta_files(&dir).len();
        assert_eq!(
            after_second, after_first,
            "no-change observation must not append a delta file"
        );
        assert_eq!(artifact_row(&second).growth_bytes, Some(0));
    }

    #[test]
    fn absence_then_reappearance_counts_one_regrowth() {
        let tmp = tempfile::tempdir().unwrap();
        let root = PathBuf::from("/repo");

        let mut present = vec![one_artifact_project(&root, 1_000_000)];
        observe_and_annotate(
            &crate::bus::Stage::for_tests(),
            tmp.path(),
            1,
            &mut present,
            1_000,
            30,
            3600,
            &HashSet::new(),
        )
        .unwrap();

        // target/ deleted: no artifacts observed this pass at all.
        let mut absent: Vec<ProjectRow> = vec![ProjectRow {
            project_id: "proj-1".to_string(),
            name: "proj".to_string(),
            remote: None,
            ecosystems: Vec::new(),
            worktrees: vec![WorktreeRow {
                worktree_id: "wt-1".to_string(),
                path: root.clone(),
                kind: WorktreeKind::Main,
                artifacts: vec![],
                signals: vec![],
                branch: None,
                github: None,
                merge_complete: None,
                idle_secs: None,
            }],
        }];
        observe_and_annotate(
            &crate::bus::Stage::for_tests(),
            tmp.path(),
            1,
            &mut absent,
            2_000,
            30,
            3600,
            &HashSet::new(),
        )
        .unwrap();

        // target/ recreated.
        let mut recreated = vec![one_artifact_project(&root, 500_000)];
        observe_and_annotate(
            &crate::bus::Stage::for_tests(),
            tmp.path(),
            1,
            &mut recreated,
            3_000,
            30,
            3600,
            &HashSet::new(),
        )
        .unwrap();

        assert_eq!(artifact_row(&recreated).regrowth_count, 1);
    }

    /// Regression for the bug reported against the real `~/src` live run:
    /// grow a row, then shrink it back to its original size. Growth at
    /// the 3rd observation, measured against a baseline far enough back
    /// to predate the very first observation, must be 0 -- the row is
    /// back to the value it started at. Before the fix, the delta
    /// written for a *brand-new* row (a synthetic "previously absent,
    /// bytes=0" entry timestamped at the row's first observation) could
    /// tie with -- and be preferred over -- the real historical entry
    /// once the row changed twice, so this returned `bytes_now` (as if
    /// there were no prior observation at all) instead of `0`.
    #[test]
    fn growth_after_grow_then_shrink_back_to_original_is_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let root = PathBuf::from("/repo");
        let original_bytes = 1_000_000;

        let mut obs1 = vec![one_artifact_project(&root, original_bytes)];
        observe_and_annotate(
            &crate::bus::Stage::for_tests(),
            tmp.path(),
            1,
            &mut obs1,
            1_000,
            30,
            5_000,
            &HashSet::new(),
        )
        .unwrap();

        let mut obs2 = vec![one_artifact_project(&root, original_bytes + 200_000_000)];
        observe_and_annotate(
            &crate::bus::Stage::for_tests(),
            tmp.path(),
            1,
            &mut obs2,
            2_000,
            30,
            5_000,
            &HashSet::new(),
        )
        .unwrap();

        // Shrunk back to exactly the original size.
        let mut obs3 = vec![one_artifact_project(&root, original_bytes)];
        observe_and_annotate(
            &crate::bus::Stage::for_tests(),
            tmp.path(),
            1,
            &mut obs3,
            3_000,
            30,
            5_000,
            &HashSet::new(),
        )
        .unwrap();

        assert_eq!(
            artifact_row(&obs3).growth_bytes,
            Some(0),
            "back to the original size: growth against a far-back baseline must be 0, \
             not bytes_now as if no prior observation existed"
        );
    }

    /// After two consecutive changes, a growth query whose baseline time
    /// lands on the *first* change must use that change's real recorded
    /// value, never fall through to treating the row as previously
    /// absent (bytes=0).
    #[test]
    fn growth_after_two_changes_uses_correct_historical_value_not_absence() {
        let tmp = tempfile::tempdir().unwrap();
        let root = PathBuf::from("/repo");

        let mut obs1 = vec![one_artifact_project(&root, 1_000_000)];
        observe_and_annotate(
            &crate::bus::Stage::for_tests(),
            tmp.path(),
            1,
            &mut obs1,
            1_000,
            30,
            2_000,
            &HashSet::new(),
        )
        .unwrap();

        let mut obs2 = vec![one_artifact_project(&root, 2_000_000)];
        observe_and_annotate(
            &crate::bus::Stage::for_tests(),
            tmp.path(),
            1,
            &mut obs2,
            2_000,
            30,
            2_000,
            &HashSet::new(),
        )
        .unwrap();

        // since_secs=2_000 at observed_at=3_000 targets time 1_000 --
        // exactly obs1's timestamp -- so the baseline must be obs1's
        // 1_000_000 bytes, not 0.
        let mut obs3 = vec![one_artifact_project(&root, 5_000_000)];
        observe_and_annotate(
            &crate::bus::Stage::for_tests(),
            tmp.path(),
            1,
            &mut obs3,
            3_000,
            30,
            2_000,
            &HashSet::new(),
        )
        .unwrap();

        assert_eq!(
            artifact_row(&obs3).growth_bytes,
            Some(4_000_000),
            "baseline must be obs1's real recorded value (1_000_000), not absence (0)"
        );
    }

    #[test]
    fn delta_count_crossing_threshold_compacts() {
        let tmp = tempfile::tempdir().unwrap();
        let root = PathBuf::from("/repo");
        let dir = volume_dir(tmp.path(), 1);

        for i in 0..(COMPACTION_THRESHOLD as u64 + 5) {
            let mut obs = vec![one_artifact_project(&root, 1_000 + i)];
            observe_and_annotate(
                &crate::bus::Stage::for_tests(),
                tmp.path(),
                1,
                &mut obs,
                1_000 + i,
                30,
                3600,
                &HashSet::new(),
            )
            .unwrap();
        }

        let files = list_delta_files(&dir);
        assert!(
            files.len() <= COMPACTION_THRESHOLD,
            "expected compaction to keep the delta file count bounded, got {}",
            files.len()
        );
        assert!(
            !files.is_empty(),
            "compacted history must not be discarded entirely"
        );
    }

    #[test]
    fn row_keys_never_carry_an_absolute_path() {
        // The stored rel_path for an artifact under the worktree root must
        // be relative, never the artifact's absolute filesystem path.
        let tmp = tempfile::tempdir().unwrap();
        let root = PathBuf::from("/some/absolute/worktree/root");
        let mut projects = vec![one_artifact_project(&root, 42)];
        observe_and_annotate(
            &crate::bus::Stage::for_tests(),
            tmp.path(),
            1,
            &mut projects,
            1_000,
            30,
            3600,
            &HashSet::new(),
        )
        .unwrap();

        let dir = volume_dir(tmp.path(), 1);
        let current = read_rows(&current_path(&dir)).unwrap();
        let row = current
            .iter()
            .find(|r| r.rel_path().contains("node_modules"))
            .unwrap();
        assert_eq!(row.rel_path(), "node_modules");
        assert!(!row.rel_path().starts_with('/'));
    }
    /// R15 tables 2/3: a `ProjectRow`/`WorktreeRow` with every optional
    /// field populated (a GitHub PR with all of its scalars, a merged
    /// status carrying `merged_at`/`pr_number`, a merge-complete verdict
    /// with ordered terms, ordered signals, several ecosystem tags, a
    /// remote) round-trips through `projects.parquet`/`worktrees.parquet`/
    /// `worktree_facts.parquet` field for field. The tempting shortcut
    /// this fails is a lossy flattening -- dropping a PR field, or
    /// letting the child table lose list order.
    #[test]
    fn project_and_worktree_tables_round_trip_every_field_and_keep_list_order() {
        use crate::github::{
            GithubFacts, MergeComplete, MergedStatus, PrState, PrStatus, PullRequestInfo,
            ReviewDecision, TriState,
        };
        use crate::report::{Signal, WorktreeKind};
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path();
        let wt = |id: &str, kind: WorktreeKind, github: Option<GithubFacts>| WorktreeRow {
            worktree_id: id.to_string(),
            path: PathBuf::from(format!("/src/{id}")),
            kind,
            artifacts: Vec::new(),
            signals: vec![
                Signal {
                    name: "zeta".into(),
                    value: "1".into(),
                },
                Signal {
                    name: "alpha".into(),
                    value: "2".into(),
                },
                Signal {
                    name: "alpha".into(),
                    value: "3".into(),
                },
            ],
            branch: Some("feature/x".into()),
            merge_complete: github.as_ref().map(|_| MergeComplete {
                verdict: TriState::No,
                terms: vec!["merged=yes".into(), "dirty=yes".into(), "unpushed=0".into()],
            }),
            github,
            idle_secs: Some(12_345),
        };
        let full = GithubFacts {
            default_branch: Some("main".into()),
            branch_exists_on_remote: Some(true),
            merged: MergedStatus::Yes {
                merged_at: Some("2026-09-01T00:00:00Z".into()),
                pr_number: Some(42),
            },
            pull_request: PrStatus::Some(PullRequestInfo {
                number: 42,
                state: PrState::Merged,
                draft: true,
                url: "https://github.com/o/r/pull/42".into(),
                title: "a title".into(),
                review_decision: ReviewDecision::ChangesRequested,
                updated_at: "2026-09-02T00:00:00Z".into(),
            }),
            unavailable_reason: Some("stale cache".into()),
        };
        let sparse = GithubFacts::unknown(None);
        let projects = vec![
            ProjectRow {
                project_id: "p1".into(),
                name: "one".into(),
                worktrees: vec![
                    wt("w1", WorktreeKind::Main, Some(full)),
                    wt("w2", WorktreeKind::Linked, Some(sparse)),
                ],
                ecosystems: vec!["rs".into(), "js".into()],
                remote: Some("github.com/o/r".into()),
            },
            ProjectRow {
                project_id: "p2".into(),
                name: "two".into(),
                worktrees: vec![WorktreeRow {
                    signals: Vec::new(),
                    branch: None,
                    idle_secs: None,
                    ..wt("w3", WorktreeKind::Clone, None)
                }],
                ecosystems: Vec::new(),
                remote: None,
            },
        ];
        write_project_worktree_tables(store, "k", &projects, 7).unwrap();
        // A second scope's rows must not disturb the first's.
        write_project_worktree_tables(store, "other", &projects[..1], 8).unwrap();

        let tables = read_project_worktree_tables(store, "k").expect("rows for k");
        assert_eq!(tables.projects.len(), 2);
        assert_eq!(tables.projects[0].ecosystems, "rs|js");
        assert_eq!(tables.projects[0].worktree_count, 2);
        let rebuilt: Vec<ProjectRow> = tables
            .projects
            .iter()
            .map(|sp| {
                let mut p = project_row_from_stored(sp);
                p.worktrees = tables
                    .worktrees
                    .iter()
                    .filter(|w| w.project_id == sp.project_id)
                    .map(|w| worktree_row_from_stored(w, &tables.worktree_facts))
                    .collect();
                p
            })
            .collect();
        assert_eq!(
            serde_json::to_value(&rebuilt).unwrap(),
            serde_json::to_value(&projects).unwrap()
        );
        assert!(read_project_worktree_tables(store, "missing").is_none());
    }

    /// R18a-3b: `artifact_shape.parquet`/`artifact_shape_lists.parquet`
    /// round-trip every `ArtifactRow` shape field
    /// (`kind`/`path`/`track`/`confidence`/`source`/`note`/`created_at`/
    /// `containers`/`shared_with`/`dangling`/`allocated_bytes`/
    /// `allocated_growth_bytes`/`growth_bytes`), keep list order, keep
    /// artifact order within a worktree, and a second scope's rows do
    /// not disturb the first's.
    #[test]
    fn artifact_shape_table_round_trips_every_field_and_keeps_order() {
        use crate::ignore::TrackState;
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path();

        let full = ArtifactRow {
            kind: ArtifactKind::BuildOutput,
            path: PathBuf::from("/src/w1/target"),
            bytes: 0,
            mtime_max: 0,
            ecosystem: None,
            hardlinked: true,
            dedup_stale: false,
            allocated_bytes: None,
            allocated_growth_bytes: None,
            local_bytes: 0,
            track: Some(TrackState::Ignored),
            growth_bytes: None,
            regrowth_count: 0,
            observed_at: 0,
            confidence: Confidence::Medium,
            source: crate::report::Source::new("cargo.layout"),
            note: Some("nested-id=abc123".into()),
            created_at: Some("2026-09-01T00:00:00Z".into()),
            containers: vec!["c1".into(), "c2".into()],
            shared_with: vec!["img-a".into(), "img-b".into()],
            dangling: true,
            evidence: Vec::new(),
        };
        let sparse = ArtifactRow {
            kind: ArtifactKind::Source,
            path: PathBuf::from("/src/w1"),
            bytes: 0,
            mtime_max: 0,
            ecosystem: None,
            hardlinked: true,
            dedup_stale: false,
            allocated_bytes: None,
            allocated_growth_bytes: None,
            local_bytes: 0,
            track: None,
            growth_bytes: None,
            regrowth_count: 0,
            observed_at: 0,
            confidence: Confidence::High,
            source: crate::report::Source::new("git"),
            note: None,
            created_at: None,
            containers: Vec::new(),
            shared_with: Vec::new(),
            dangling: false,
            evidence: Vec::new(),
        };
        let wt = crate::report::WorktreeRow {
            worktree_id: "w1".into(),
            path: PathBuf::from("/src/w1"),
            kind: crate::report::WorktreeKind::Main,
            artifacts: vec![full.clone(), sparse.clone()],
            signals: Vec::new(),
            branch: None,
            github: None,
            merge_complete: None,
            idle_secs: None,
        };
        let projects = vec![ProjectRow {
            project_id: "p1".into(),
            name: "one".into(),
            worktrees: vec![wt],
            ecosystems: Vec::new(),
            remote: None,
        }];

        write_artifact_shape_table(store, "k", &projects, 9).unwrap();
        // A second scope's rows must not disturb the first's.
        write_artifact_shape_table(store, "other", &projects, 10).unwrap();

        let by_worktree = artifact_shape_rows_by_worktree(store, "k");
        let rebuilt = by_worktree.get("w1").expect("rows for w1").clone();
        assert_eq!(rebuilt.len(), 2, "artifact order must be kept");
        // Mirrors `report::rebuild_projects_from_tables`'s own join,
        // including its empty-relative-path special case (the
        // worktree-root row) -- see that call site's comment.
        let join = |rel: &Path| -> PathBuf {
            if rel.as_os_str().is_empty() {
                PathBuf::from("/src/w1")
            } else {
                PathBuf::from("/src/w1").join(rel)
            }
        };
        let mut rebuilt_full = rebuilt[0].clone();
        rebuilt_full.path = join(&rebuilt_full.path);
        let mut rebuilt_sparse = rebuilt[1].clone();
        rebuilt_sparse.path = join(&rebuilt_sparse.path);
        assert_eq!(
            serde_json::to_value(&rebuilt_full).unwrap(),
            serde_json::to_value(&full).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&rebuilt_sparse).unwrap(),
            serde_json::to_value(&sparse).unwrap()
        );
        assert!(artifact_shape_rows_by_worktree(store, "missing").is_empty());
    }

    /// A direct on-disk tamper of `artifact_shape.parquet`/
    /// `artifact_shape_lists.parquet` (edit the table, read back through
    /// `artifact_shape_rows_by_worktree`) is what the rebuild reflects,
    /// not the value the artifacts were first written with.
    #[test]
    fn artifact_shape_tables_rebuild_reflects_a_direct_tamper_not_the_original_value() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path();
        let a = ArtifactRow {
            kind: ArtifactKind::Cache,
            path: PathBuf::from("/src/w1/.cache"),
            bytes: 0,
            mtime_max: 0,
            ecosystem: None,
            hardlinked: true,
            dedup_stale: false,
            allocated_bytes: None,
            allocated_growth_bytes: None,
            local_bytes: 0,
            track: None,
            growth_bytes: None,
            regrowth_count: 0,
            observed_at: 0,
            confidence: Confidence::Low,
            source: crate::report::Source::new("filesystem"),
            note: None,
            created_at: None,
            containers: vec!["original".into()],
            shared_with: Vec::new(),
            dangling: false,
            evidence: Vec::new(),
        };
        let wt = crate::report::WorktreeRow {
            worktree_id: "w1".into(),
            path: PathBuf::from("/src/w1"),
            kind: crate::report::WorktreeKind::Main,
            artifacts: vec![a],
            signals: Vec::new(),
            branch: None,
            github: None,
            merge_complete: None,
            idle_secs: None,
        };
        let projects = vec![ProjectRow {
            project_id: "p1".into(),
            name: "one".into(),
            worktrees: vec![wt],
            ecosystems: Vec::new(),
            remote: None,
        }];
        write_artifact_shape_table(store, "k", &projects, 1).unwrap();

        // Tamper the table directly, as if a hand-edit had happened.
        let shape_file = artifact_shape_path(store);
        let mut rows = columns::read_artifact_shape_rows(&shape_file).unwrap();
        rows[0].dangling = true;
        rows[0].note = Some("tampered-note".into());
        columns::write_artifact_shape_rows(&shape_file, &rows).unwrap();
        let lists_file = artifact_shape_lists_path(store);
        let mut lists = columns::read_artifact_shape_list_rows(&lists_file).unwrap();
        lists[0].value = "tampered".into();
        columns::write_artifact_shape_list_rows(&lists_file, &lists).unwrap();

        let rebuilt = artifact_shape_rows_by_worktree(store, "k");
        let rebuilt = rebuilt.get("w1").expect("rows for w1");
        assert!(rebuilt[0].dangling);
        assert_eq!(rebuilt[0].note.as_deref(), Some("tampered-note"));
        assert_eq!(rebuilt[0].containers, vec!["tampered".to_string()]);
    }

    /// R18a-4: `git_signals.parquet`/`git_signals_values.parquet` round-trip
    /// every `RawSignals` field, `branch`, and the rendered `Signal` rows
    /// in order, root-keyed (not scope-keyed): a second root's rows do
    /// not disturb the first's, and a missing root reads back `None`,
    /// not an empty map (the meta/presence distinction
    /// `read_cargo_replay_cache` also relies on).
    #[test]
    fn git_signals_table_round_trips_every_field_and_keeps_order() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path();

        let mut by_worktree = HashMap::new();
        by_worktree.insert(
            "w1".to_string(),
            WorktreeSignals {
                path: PathBuf::from("/src/w1"),
                branch: Some("main".into()),
                raw: RawSignals {
                    last_commit_age_secs: Some(3_600),
                    dirty: Some(true),
                    unpushed: Some(2),
                    locked: Some(false),
                    idle_for_secs: Some(7_200),
                },
                rows: vec![
                    Signal {
                        name: "last_commit".into(),
                        value: "1h ago".into(),
                    },
                    Signal {
                        name: "dirty".into(),
                        value: "dirty".into(),
                    },
                ],
            },
        );
        by_worktree.insert(
            "w2".to_string(),
            WorktreeSignals {
                path: PathBuf::from("/src/w2"),
                branch: None,
                raw: RawSignals {
                    last_commit_age_secs: None,
                    dirty: None,
                    unpushed: None,
                    locked: None,
                    idle_for_secs: None,
                },
                rows: Vec::new(),
            },
        );
        write_git_signals_table(store, "root-a", 100, &by_worktree).unwrap();
        // A second root's rows must not disturb the first's.
        write_git_signals_table(store, "root-b", 200, &by_worktree).unwrap();

        let (observed_at, rebuilt) = read_git_signals_table(store, "root-a").expect("root-a rows");
        assert_eq!(observed_at, 100);
        assert_eq!(rebuilt.len(), 2);
        let w1 = &rebuilt["w1"];
        assert_eq!(w1.branch.as_deref(), Some("main"));
        assert_eq!(w1.raw.last_commit_age_secs, Some(3_600));
        assert_eq!(w1.raw.dirty, Some(true));
        assert_eq!(w1.raw.unpushed, Some(2));
        assert_eq!(w1.raw.locked, Some(false));
        assert_eq!(w1.raw.idle_for_secs, Some(7_200));
        assert_eq!(
            w1.rows.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["last_commit", "dirty"],
            "signal row order must be kept"
        );
        let w2 = &rebuilt["w2"];
        assert!(w2.branch.is_none());
        assert!(w2.rows.is_empty());

        assert!(read_git_signals_table(store, "missing").is_none());
    }

    /// A direct on-disk tamper of `git_signals.parquet`/
    /// `git_signals_values.parquet` is what `read_git_signals_table`
    /// reflects, not the value the signals were first written with.
    #[test]
    fn git_signals_table_rebuild_reflects_a_direct_tamper_not_the_original_value() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path();
        let mut by_worktree = HashMap::new();
        by_worktree.insert(
            "w1".to_string(),
            WorktreeSignals {
                path: PathBuf::from("/src/w1"),
                branch: Some("main".into()),
                raw: RawSignals {
                    last_commit_age_secs: Some(10),
                    dirty: Some(false),
                    unpushed: Some(0),
                    locked: Some(false),
                    idle_for_secs: Some(10),
                },
                rows: vec![Signal {
                    name: "dirty".into(),
                    value: "clean".into(),
                }],
            },
        );
        write_git_signals_table(store, "root-a", 1, &by_worktree).unwrap();

        let mut rows = columns::read_git_signal_rows(&git_signals_path(store)).unwrap();
        rows[0].branch = Some("tampered-branch".into());
        rows[0].dirty = Some(true);
        columns::write_git_signal_rows(&git_signals_path(store), &rows).unwrap();
        let mut values =
            columns::read_git_signal_value_rows(&git_signal_values_path(store)).unwrap();
        values[0].value = "tampered".into();
        columns::write_git_signal_value_rows(&git_signal_values_path(store), &values).unwrap();

        let (_, rebuilt) = read_git_signals_table(store, "root-a").unwrap();
        let w1 = &rebuilt["w1"];
        assert_eq!(w1.branch.as_deref(), Some("tampered-branch"));
        assert_eq!(w1.raw.dirty, Some(true));
        assert_eq!(w1.rows[0].value, "tampered");
    }

    /// R18a-4: a second observe of an unchanged worktree ages the stored
    /// signals forward (`crate::signals::age_signals`) rather than
    /// recomputing them -- the same claim the deleted
    /// `last_report-<key>.json.zst` cache used to support, now proven
    /// against `git_signals.parquet` directly: `last_commit_age_secs`/
    /// `idle_for_secs` grow by exactly the elapsed gap between the two
    /// writes, and `dirty`/`unpushed`/`locked` (never aged) carry
    /// through unchanged, which is only possible if the second write
    /// read the first's stored values back rather than recomputing
    /// fresh (all-`None`) `RawSignals` for an unwalked worktree.
    #[test]
    fn an_unchanged_worktree_ages_stored_signals_instead_of_recomputing_them() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path();
        let mut first = HashMap::new();
        first.insert(
            "w1".to_string(),
            WorktreeSignals {
                path: PathBuf::from("/src/w1"),
                branch: Some("main".into()),
                raw: RawSignals {
                    last_commit_age_secs: Some(100),
                    dirty: Some(true),
                    unpushed: Some(3),
                    locked: Some(false),
                    idle_for_secs: Some(100),
                },
                rows: vec![
                    Signal {
                        name: "last_commit".into(),
                        // `age_signals` re-renders `last_commit`/`idle_for`
                        // from `raw` regardless of this stored text, so a
                        // placeholder is enough here.
                        value: "100s ago".into(),
                    },
                    Signal {
                        name: "idle_for".into(),
                        value: "idle 100s".into(),
                    },
                ],
            },
        );
        write_git_signals_table(store, "root-a", 1_000, &first).unwrap();

        // Simulate the incremental replay `consumers::signals` performs
        // for a worktree FSEvents reported nothing under: read the
        // stored previous pass back and age it by the elapsed gap,
        // never recompute a fresh `RawSignals`.
        let (prev_observed_at, prev_by_worktree) =
            read_git_signals_table(store, "root-a").expect("previous pass");
        let elapsed = 1_500u64.saturating_sub(prev_observed_at);
        assert_eq!(elapsed, 500);
        let prev_w1 = &prev_by_worktree["w1"];
        let (aged_rows, aged_raw) =
            crate::signals::age_signals(&prev_w1.rows, &prev_w1.raw, elapsed);

        assert_eq!(
            aged_raw.last_commit_age_secs,
            Some(600),
            "last_commit_age_secs must grow by exactly the elapsed gap"
        );
        assert_eq!(
            aged_raw.idle_for_secs,
            Some(600),
            "idle_for_secs must grow by exactly the elapsed gap"
        );
        assert_eq!(
            aged_raw.dirty,
            Some(true),
            "dirty is never aged -- it must carry through from the stored value unchanged"
        );
        assert_eq!(aged_raw.unpushed, Some(3));
        assert_eq!(aged_raw.locked, Some(false));

        let mut second = HashMap::new();
        second.insert(
            "w1".to_string(),
            WorktreeSignals {
                path: PathBuf::from("/src/w1"),
                branch: prev_w1.branch.clone(),
                raw: aged_raw,
                rows: aged_rows,
            },
        );
        write_git_signals_table(store, "root-a", 1_500, &second).unwrap();
        let (observed_at, rebuilt) = read_git_signals_table(store, "root-a").unwrap();
        assert_eq!(observed_at, 1_500);
        assert_eq!(rebuilt["w1"].raw.last_commit_age_secs, Some(600));
    }

    /// R18a-4: `cargo_replay_cache.parquet` (+ its list/evidence/meta
    /// tables) round-trips every `NestedArtifact` field via the exact
    /// `nested_artifacts.parquet` conversions, root-keyed instead of
    /// scope-keyed. Presence is decided by the meta row, not by the
    /// units list being non-empty: a root with genuinely zero nested
    /// artifacts this pass is still a real previous observation.
    #[test]
    fn cargo_replay_cache_round_trips_every_field_and_distinguishes_never_observed_from_empty() {
        use crate::artifact::{
            AccountingBasis, ArtifactCoverage, ArtifactEvidence, ArtifactRole, ArtifactVariant,
            Membership, NestedActionCapability, NestedArtifact, TimeSource,
        };
        use crate::entities::Confidence;

        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path();

        let unit = NestedArtifact {
            id: "n1".into(),
            path: PathBuf::from("/src/one/target/debug"),
            relative_path: "target/debug".into(),
            parent_id: None,
            container_id: Some("n0".into()),
            role: ArtifactRole::Profile,
            membership: Membership::Exclusive,
            is_dir: true,
            device: 5,
            inode: 99,
            logical_bytes: 4000,
            bytes: 3000,
            physical_bytes: 3000,
            physical_total: 3000,
            mtime_max: 555,
            variant: ArtifactVariant {
                profile: Some("debug".into()),
                ..ArtifactVariant::default()
            },
            producer_evidence: vec![ArtifactEvidence {
                source: "cargo-metadata".into(),
                detail: "target-dir declared by workspace manifest".into(),
                confidence: Confidence::High,
            }],
            consumer_evidence: Vec::new(),
            coverage: ArtifactCoverage {
                supported: true,
                complete: true,
                limits: Vec::new(),
            },
            action_group: None,
            present: true,
            growth_bytes: Some(100),
            regrowth_count: 0,
            decision_evidence: Vec::new(),
            adapter: Some("cargo".into()),
            basis: AccountingBasis::Allocated,
            time_source: TimeSource::FoldedDirectoryModification,
            action: NestedActionCapability::InspectionOnly,
            consequence: Some("rebuild with `cargo build`".into()),
            reported_by: None,
            writer_lock: None,
            guidance: crate::cargo_cleanup::Guidance::default(),
        };

        write_cargo_replay_cache(store, "root-a", std::slice::from_ref(&unit), 42).unwrap();
        // A root genuinely observed with zero nested artifacts is a real
        // previous observation, distinct from a root never observed.
        write_cargo_replay_cache(store, "root-b", &[], 43).unwrap();

        let (observed_at, units) = read_cargo_replay_cache(store, "root-a").expect("root-a rows");
        assert_eq!(observed_at, 42);
        assert_eq!(units.len(), 1);
        assert_eq!(
            serde_json::to_value(&units[0]).unwrap(),
            serde_json::to_value(&unit).unwrap()
        );

        let (empty_observed_at, empty_units) =
            read_cargo_replay_cache(store, "root-b").expect("root-b was observed, just empty");
        assert_eq!(empty_observed_at, 43);
        assert!(empty_units.is_empty());

        assert!(
            read_cargo_replay_cache(store, "root-never-observed").is_none(),
            "a root with no meta row must read back None, not Some(empty)"
        );
    }

    /// A direct on-disk tamper of `cargo_replay_cache.parquet`'s row (and
    /// its list/evidence children) is what `read_cargo_replay_cache`
    /// reflects, not the value the unit was first written with -- the
    /// same claim `nested_artifact_tables_rebuild_reflects_a_direct_tamper_
    /// not_the_original_value` proves for the scope-keyed table, proven
    /// here for the root-keyed replay cache instead.
    #[test]
    fn cargo_replay_cache_rebuild_reflects_a_direct_tamper_not_the_original_value() {
        use crate::artifact::{
            AccountingBasis, ArtifactCoverage, ArtifactRole, ArtifactVariant, Membership,
            NestedActionCapability, NestedArtifact, TimeSource,
        };

        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path();
        let unit = NestedArtifact {
            id: "n1".into(),
            path: PathBuf::from("/src/one/target/debug"),
            relative_path: "target/debug".into(),
            parent_id: None,
            container_id: None,
            role: ArtifactRole::Profile,
            membership: Membership::Exclusive,
            is_dir: true,
            device: 0,
            inode: 0,
            logical_bytes: 0,
            bytes: 10,
            physical_bytes: 10,
            physical_total: 10,
            mtime_max: 1,
            variant: ArtifactVariant::default(),
            producer_evidence: Vec::new(),
            consumer_evidence: Vec::new(),
            coverage: ArtifactCoverage {
                supported: true,
                complete: true,
                limits: vec!["original".into()],
            },
            action_group: None,
            present: true,
            growth_bytes: None,
            regrowth_count: 0,
            decision_evidence: Vec::new(),
            adapter: None,
            basis: AccountingBasis::Allocated,
            time_source: TimeSource::FoldedDirectoryModification,
            action: NestedActionCapability::InspectionOnly,
            consequence: None,
            reported_by: None,
            writer_lock: None,
            guidance: crate::cargo_cleanup::Guidance::default(),
        };
        write_cargo_replay_cache(store, "root-a", std::slice::from_ref(&unit), 1).unwrap();

        let file = cargo_replay_cache_path(store);
        let mut rows = columns::read_nested_artifact_rows(&file).unwrap();
        rows[0].bytes = 999;
        columns::write_nested_artifact_rows(&file, &rows).unwrap();
        let lists_file = cargo_replay_cache_lists_path(store);
        let mut lists = columns::read_nested_artifact_list_rows(&lists_file).unwrap();
        lists[0].value = "tampered".into();
        columns::write_nested_artifact_list_rows(&lists_file, &lists).unwrap();

        let (_, units) = read_cargo_replay_cache(store, "root-a").unwrap();
        assert_eq!(units[0].bytes, 999);
        assert_eq!(units[0].coverage.limits, vec!["tampered".to_string()]);
    }

    /// R15 table 4: the current-artifact table carries `ecosystem`, keeps
    /// it across an unchanged re-observation, and refreshes it when it
    /// changes; a store without the column reads `None`, not an error.
    #[test]
    fn artifact_history_round_trips_the_ecosystem_column() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let obs = |eco: Option<&str>, bytes: u64| Observed {
            key: row_key("p", "w", "BuildOutput", "target"),
            project_id: "p".into(),
            worktree_id: "w".into(),
            kind: "BuildOutput".into(),
            rel_path: "target".into(),
            bytes,
            local_bytes: bytes,
            mtime_max: 5,
            hardlinked: false,
            dedup_stale: false,
            ecosystem: eco.map(|s| s.to_string()),
        };
        let mut h = ArtifactHistory::load(dir).unwrap();
        h.observe(&obs(Some("rs"), 10), 1);
        h.commit().unwrap();
        let rows = read_rows(&current_path(dir)).unwrap();
        assert_eq!(rows[0].ecosystem(), Some("rs"));

        // Unchanged bytes, changed ecosystem: still rewritten.
        let mut h = ArtifactHistory::load(dir).unwrap();
        h.observe(&obs(Some("js"), 10), 2);
        h.commit().unwrap();
        let rows = read_rows(&current_path(dir)).unwrap();
        assert_eq!(rows[0].ecosystem(), Some("js"));
        assert_eq!(rows[0].bytes(), 10);

        // The facts map `report_scope_from_store` reads is keyed like the
        // history and carries the column.
        let facts = artifact_table_facts_for_roots(dir, &[]);
        assert!(facts.is_empty(), "no roots, no facts");

        // A null cell reads as `None` (a table written without the
        // column at all goes through the same `Option` path).
        let old = dir.join("old.parquet");
        StoredRow::write_for_test(
            &old,
            &[StoredRow::for_test("p", "w", "k", "r", 1, true, 1, 0)],
        )
        .unwrap();
        assert_eq!(read_rows(&old).unwrap()[0].ecosystem(), None);
    }

    /// R16/R18a item 1: every `ProjectLinkState` variant round-trips
    /// through `linkage_state`/`linkage_basis`/`project_id`, an
    /// `ExternalUnit`'s consumers keep their `Vec` order, and
    /// `provenance`/`hardlinked` (external) and `tool_home`/
    /// `relative_path`/`members`/`action`/`hardlinked` (agent) round-trip
    /// through their own typed columns -- `old: None` here proves it,
    /// since the pre-R18a fallback path can only produce a value when
    /// `old` is `Some`.
    #[test]
    fn unit_tables_round_trip_every_field_and_every_linkage_state() {
        use crate::agents::{
            AgentActionCapability, AgentCategory, AgentMember, AgentMemberKind, AgentUnit,
            LinkSource, ProjectLinkState,
        };
        use crate::external::{ExternalConsumer, ExternalUnit};
        use crate::locations::{Provenance, StorageCategory};

        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path();

        let external = ExternalUnit {
            detector_id: "cargo-home".into(),
            detector_name: "Cargo home".into(),
            category: StorageCategory::Cache,
            provenance: Provenance::EnvVar("CARGO_HOME".into()),
            path: PathBuf::from("/home/u/.cargo"),
            bytes: 1_048_576,
            mtime_max: 111,
            hardlinked: false,
            growth_bytes: Some(2048),
            regrowth_count: 3,
            observed_at: 222,
            consumers: vec![
                ExternalConsumer {
                    label: "zeta-project".into(),
                    note: Some("declared in Cargo.lock".into()),
                },
                ExternalConsumer {
                    label: "alpha-project".into(),
                    note: None,
                },
            ],
            note: Some("coverage incomplete this pass: could not be read".into()),
            evidence: Vec::new(),
        };

        let link_states = [
            ProjectLinkState::Linked {
                project_id: "proj-1".into(),
                project_name: "one".into(),
                project_path: PathBuf::from("/src/one"),
                source: LinkSource::Declared,
                fallback_reason: None,
                worktree_kind: "main".into(),
            },
            ProjectLinkState::Linked {
                project_id: "proj-2".into(),
                project_name: "two".into(),
                project_path: PathBuf::from("/src/two"),
                source: LinkSource::Inferred,
                fallback_reason: Some("declared cwd missing: /old\u{1}two".into()),
                worktree_kind: "linked".into(),
            },
            ProjectLinkState::Unresolved {
                reason: "no cwd field found".into(),
            },
            ProjectLinkState::Missing {
                path: PathBuf::from("/gone"),
            },
            ProjectLinkState::NotAProject {
                path: PathBuf::from("/not-a-repo"),
            },
            ProjectLinkState::Moved {
                from: PathBuf::from("/old"),
                to: PathBuf::from("/new"),
            },
            ProjectLinkState::Remote {
                host: "other-host".into(),
                path: PathBuf::from("/remote/path"),
            },
            ProjectLinkState::Shared {
                project_ids: vec!["p1".into(), "p2".into()],
            },
            ProjectLinkState::NotApplicable,
        ];

        let agents: Vec<AgentUnit> = link_states
            .iter()
            .enumerate()
            .map(|(i, link)| AgentUnit {
                tool_id: "claude-code".into(),
                tool_name: "Claude Code".into(),
                tool_home: PathBuf::from("/home/u/.claude"),
                category: AgentCategory::Sessions,
                id: format!("agent-{i}"),
                relative_path: format!("projects/x/{i}.jsonl"),
                path: PathBuf::from(format!("/home/u/.claude/projects/x/{i}.jsonl")),
                members: vec![
                    AgentMember {
                        path: PathBuf::from(format!("/home/u/.claude/projects/x/{i}.jsonl")),
                        bytes: 4096 + i as u64,
                        kind: AgentMemberKind::Transcript,
                    },
                    AgentMember {
                        path: PathBuf::from(format!("/home/u/.claude/projects/x/{i}-todos.json")),
                        bytes: 12,
                        kind: AgentMemberKind::Todos,
                    },
                ],
                bytes: 4096 + i as u64,
                hardlinked: i % 2 == 0,
                complete: i % 2 == 0,
                growth_bytes: Some(i as i64 - 4),
                regrowth_count: i as u32,
                observed_at: 333,
                mtime_max: 444 + i as u64,
                protected: i % 3 == 0,
                protect_reason: (i % 3 == 0).then(|| "session history".to_string()),
                project_link: link.clone(),
                action: AgentActionCapability::SessionRemoval,
                note: Some(format!("note-{i}")),
                evidence: Vec::new(),
            })
            .collect();

        write_unit_tables(store, "k", std::slice::from_ref(&external), &agents).unwrap();
        // A second scope's rows must not disturb the first's.
        write_unit_tables(
            store,
            "other",
            std::slice::from_ref(&external),
            &agents[..1],
        )
        .unwrap();

        let tables = read_unit_tables(store, "k").expect("rows for k");
        assert_eq!(tables.external.len(), 1);
        assert_eq!(tables.agent.len(), agents.len());

        let mut consumer_rows: Vec<&columns::StoredUnitConsumerRow> = tables
            .consumers
            .iter()
            .filter(|c| c.unit_id == tables.external[0].id)
            .collect();
        consumer_rows.sort_by_key(|c| c.seq);
        let rebuilt_consumers: Vec<ExternalConsumer> = consumer_rows
            .into_iter()
            .map(|c| ExternalConsumer {
                label: c.consumer_label.clone(),
                note: c.basis.clone(),
            })
            .collect();
        let rebuilt_external = external_unit_from_stored(&tables.external[0], rebuilt_consumers);
        assert_eq!(
            serde_json::to_value(&rebuilt_external).unwrap(),
            serde_json::to_value(&external).unwrap(),
            "an ExternalUnit must round-trip field for field, consumers in order"
        );

        for (stored, original) in tables.agent.iter().zip(agents.iter()) {
            let member_rows: Vec<&columns::StoredAgentMemberRow> = tables
                .agent_members
                .iter()
                .filter(|m| m.unit_id == stored.id)
                .collect();
            let rebuilt = agent_unit_from_stored(stored, &member_rows);
            assert_eq!(
                serde_json::to_value(&rebuilt).unwrap(),
                serde_json::to_value(original).unwrap(),
                "AgentUnit with project_link {:?} must round-trip",
                original.project_link
            );
        }

        assert!(read_unit_tables(store, "missing").is_none());
    }

    /// R18a item 3: `unowned.parquet`'s `containers`/`shared_with`/
    /// `evidence` now round-trip through `unowned_lists.parquet`/
    /// `unowned_evidence.parquet` instead of a JSON cell -- list order
    /// and every `Evidence` field must survive, and a row with empty
    /// lists/no evidence must not leave stray rows for other rows to
    /// pick up.
    #[test]
    fn unowned_round_trips_lists_and_evidence_through_their_own_tables() {
        use crate::evidence::{Evidence, EvidenceSource, FactKind, FactSubtype, FactValue};

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        let docker_row = UnownedRow {
            measurement: None,
            path_or_object: "sha256:abc123".into(),
            bytes: 4096,
            reason: UnownedReason::DockerNoJoin,
            shared_bytes: Some(8192),
            note: Some("base image source: example/upstream".into()),
            docker_kind: Some("image".into()),
            created_at: Some("2026-01-01T00:00:00Z".into()),
            containers: vec!["web (running)".into(), "worker (exited)".into()],
            shared_with: vec!["example/other:latest".into()],
            dangling: false,
            evidence: vec![Evidence::known(
                FactKind::Activity,
                FactSubtype::Modified,
                FactValue::Timestamp(1_700_000_000),
                EvidenceSource::FilesystemMetadata {
                    detail: "docker inspect".into(),
                },
                1_700_000_100,
            )],
        };
        let plain_row = UnownedRow {
            measurement: None,
            path_or_object: "/src/scratch/leftover.bin".into(),
            bytes: 512,
            reason: UnownedReason::OutsideAnyCheckout,
            shared_bytes: None,
            note: None,
            docker_kind: None,
            created_at: None,
            containers: Vec::new(),
            shared_with: Vec::new(),
            dangling: false,
            evidence: Vec::new(),
        };

        write_unowned(dir, &[docker_row.clone(), plain_row.clone()]).unwrap();

        let rebuilt = read_unowned(dir);
        assert_eq!(rebuilt.len(), 2);
        let rebuilt_docker = rebuilt
            .iter()
            .find(|r| r.path_or_object == docker_row.path_or_object)
            .unwrap();
        assert_eq!(
            serde_json::to_value(rebuilt_docker).unwrap(),
            serde_json::to_value(&docker_row).unwrap(),
            "containers/shared_with order and every evidence field must round-trip"
        );
        let rebuilt_plain = rebuilt
            .iter()
            .find(|r| r.path_or_object == plain_row.path_or_object)
            .unwrap();
        assert!(
            rebuilt_plain.containers.is_empty()
                && rebuilt_plain.shared_with.is_empty()
                && rebuilt_plain.evidence.is_empty(),
            "a row with nothing to carry must not pick up another row's list/evidence rows"
        );

        // A second full-walk write replaces both tables wholesale, same
        // as `unowned.parquet` itself -- no leftover rows from the first
        // write's now-gone docker row.
        write_unowned(dir, &[plain_row.clone()]).unwrap();
        let rebuilt2 = read_unowned(dir);
        assert_eq!(rebuilt2.len(), 1);
        assert_eq!(rebuilt2[0].path_or_object, plain_row.path_or_object);
        assert!(rebuilt2[0].containers.is_empty() && rebuilt2[0].evidence.is_empty());
    }

    /// R18a-2: every `NestedArtifact` field -- including the ones this
    /// slice adds typed columns for (`relative_path`, `parent_id`,
    /// `membership`, `is_dir`, `device`, `inode`, `logical_bytes`,
    /// `physical_bytes`, `physical_total`, `time_source`, `coverage`
    /// (with non-empty `limits`), `action_group`, `present`,
    /// `growth_bytes`, `regrowth_count`, `action` (the `Unsupported`
    /// variant, to prove the reason string round-trips), `reported_by`,
    /// `writer_lock`, the variant's `package`/`version`/`toolchain`/
    /// `features`/`generation`/`unknowns`, and non-empty
    /// `producer_evidence`/`consumer_evidence`) -- round-trips with NO
    /// JSON merge fallback of any kind, and `nested_artifacts.parquet`
    /// splits a `Report.nested_artifacts` row back out from a
    /// `ReportSnapshot.store_interiors` row by `origin`, never mixing
    /// the two lists. This fails the shortcut "still secretly reading a
    /// snapshot fallback" outright: `nested_artifact_from_stored` no
    /// longer takes a fallback value to fall back to at all.
    #[test]
    fn nested_artifact_table_round_trips_every_field_and_splits_by_origin() {
        use crate::artifact::{
            AccountingBasis, ArtifactCoverage, ArtifactEvidence, ArtifactRole, ArtifactVariant,
            Membership, NestedActionCapability, NestedArtifact, TimeSource,
        };
        use crate::entities::Confidence;

        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path();

        let report_one = NestedArtifact {
            id: "n1".into(),
            path: PathBuf::from("/src/one/target/debug"),
            relative_path: "target/debug".into(),
            parent_id: Some("n0".into()),
            container_id: Some("n0".into()),
            role: ArtifactRole::Profile,
            membership: Membership::Exclusive,
            is_dir: true,
            device: 5,
            inode: 99,
            logical_bytes: 4000,
            bytes: 3000,
            physical_bytes: 3000,
            physical_total: 3000,
            mtime_max: 555,
            variant: ArtifactVariant {
                profile: Some("debug".into()),
                configuration: Some("dev".into()),
                target: Some("x86_64-unknown-linux-gnu".into()),
                architecture: Some("x86_64".into()),
                package: Some("a".into()),
                version: Some("0.1.0".into()),
                toolchain: Some("stable".into()),
                features: Some("default".into()),
                generation: None,
                unknowns: vec!["extra".into()],
            },
            producer_evidence: vec![ArtifactEvidence {
                source: "cargo-metadata".into(),
                detail: "target-dir declared by workspace manifest".into(),
                confidence: Confidence::High,
            }],
            consumer_evidence: vec![
                ArtifactEvidence {
                    source: "lockfile".into(),
                    detail: "package a v0.1.0 depends on this target".into(),
                    confidence: Confidence::Medium,
                },
                ArtifactEvidence {
                    source: "open-file".into(),
                    detail: "rustc holds a file open under this path".into(),
                    confidence: Confidence::Low,
                },
            ],
            coverage: ArtifactCoverage {
                supported: true,
                complete: false,
                limits: vec![
                    "profile detection needs cargo-metadata".into(),
                    "workspace member count unknown".into(),
                ],
            },
            action_group: Some("group-1".into()),
            present: true,
            growth_bytes: Some(100),
            regrowth_count: 1,
            decision_evidence: Vec::new(),
            adapter: Some("cargo".into()),
            basis: AccountingBasis::Allocated,
            time_source: TimeSource::FoldedDirectoryModification,
            action: NestedActionCapability::Unsupported {
                reason: "shared store entry".into(),
            },
            consequence: Some("rebuild with `cargo build`".into()),
            reported_by: None,
            writer_lock: Some(PathBuf::from("/src/one/target/.cargo-lock")),
            guidance: crate::cargo_cleanup::Guidance {
                recommendation: "Cleanup candidate · last modified 2h ago".into(),
                modified_age_secs: Some(7_200),
                consequence:
                    "Remove to trade cached compilation work for space; next build may be slower"
                        .into(),
                scope: "group".into(),
                check_status: "unchecked".into(),
                reason_code: "checks_not_run".into(),
                message:
                    "Cleanup candidate; review rebuilding cost and run exact-selection checks."
                        .into(),
                next_action: "review_cleanup".into(),
            },
        };
        let interior_one = NestedArtifact {
            id: "n2".into(),
            container_id: None,
            role: ArtifactRole::SharedStoreEntry,
            path: PathBuf::from("/store/pnpm/pkg@1.0.0"),
            basis: AccountingBasis::UniqueAllocated,
            adapter: None,
            consequence: None,
            variant: ArtifactVariant::default(),
            ..report_one.clone()
        };

        write_nested_artifact_table(
            store,
            "k",
            std::slice::from_ref(&report_one),
            std::slice::from_ref(&interior_one),
        )
        .unwrap();

        let (report_rows, interior_rows) =
            read_nested_artifact_table(store, "k").expect("rows for k");
        assert_eq!(report_rows.len(), 1);
        assert_eq!(interior_rows.len(), 1);

        // Every field comes back from typed storage alone --
        // `nested_artifact_from_stored` takes no snapshot/JSON fallback
        // value at all, so this is the only way these assertions could
        // pass.
        let (report_stored, report_lists, report_evidence) = &report_rows[0];
        let rebuilt_report =
            nested_artifact_from_stored(report_stored, report_lists, report_evidence);
        assert_eq!(
            serde_json::to_value(&rebuilt_report).unwrap(),
            serde_json::to_value(&report_one).unwrap()
        );
        let (interior_stored, interior_lists, interior_evidence) = &interior_rows[0];
        let rebuilt_interior =
            nested_artifact_from_stored(interior_stored, interior_lists, interior_evidence);
        assert_eq!(
            serde_json::to_value(&rebuilt_interior).unwrap(),
            serde_json::to_value(&interior_one).unwrap()
        );

        assert!(read_nested_artifact_table(store, "missing").is_none());
    }

    /// R18a-2 tamper test: editing `nested_artifacts.parquet`'s row and
    /// `nested_artifact_lists.parquet`/`nested_artifact_evidence.
    /// parquet`'s child rows directly on disk (never touching
    /// `report_one`/`interior_one` in memory) and reading back through
    /// `read_nested_artifact_table`/`nested_artifact_from_stored` must
    /// reflect exactly the edit -- proof the rebuild is driven by these
    /// tables' own bytes, not any leftover in-memory/JSON copy. Also
    /// asserts the tampered scalar and the tampered list values do not
    /// leak onto the *other* artifact's row (`interior_one` must stay
    /// exactly as written), so a bug that keyed a table read only by
    /// `list_kind`/`kind` without also filtering by `artifact_id` would
    /// fail this.
    #[test]
    fn nested_artifact_tables_rebuild_reflects_a_direct_tamper_not_the_original_value() {
        use crate::artifact::{
            AccountingBasis, ArtifactCoverage, ArtifactEvidence, ArtifactRole, ArtifactVariant,
            Membership, NestedActionCapability, NestedArtifact, TimeSource,
        };
        use crate::entities::Confidence;

        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path();

        let report_one = NestedArtifact {
            id: "n1".into(),
            path: PathBuf::from("/src/one/target/debug"),
            relative_path: "target/debug".into(),
            parent_id: None,
            container_id: None,
            role: ArtifactRole::Profile,
            membership: Membership::Exclusive,
            is_dir: true,
            device: 5,
            inode: 99,
            logical_bytes: 4000,
            bytes: 3000,
            physical_bytes: 3000,
            physical_total: 3000,
            mtime_max: 555,
            variant: ArtifactVariant {
                unknowns: vec!["extra".into()],
                ..Default::default()
            },
            producer_evidence: vec![ArtifactEvidence {
                source: "cargo-metadata".into(),
                detail: "original detail".into(),
                confidence: Confidence::High,
            }],
            consumer_evidence: Vec::new(),
            coverage: ArtifactCoverage {
                supported: true,
                complete: true,
                limits: vec!["original limit".into()],
            },
            action_group: None,
            present: true,
            growth_bytes: None,
            regrowth_count: 0,
            decision_evidence: Vec::new(),
            adapter: None,
            basis: AccountingBasis::Allocated,
            time_source: TimeSource::FileModification,
            action: NestedActionCapability::InspectionOnly,
            consequence: None,
            reported_by: None,
            writer_lock: None,
            guidance: crate::cargo_cleanup::Guidance::default(),
        };
        let interior_one = NestedArtifact {
            id: "n2".into(),
            path: PathBuf::from("/store/pnpm/pkg@1.0.0"),
            ..report_one.clone()
        };

        write_nested_artifact_table(
            store,
            "k",
            std::slice::from_ref(&report_one),
            std::slice::from_ref(&interior_one),
        )
        .unwrap();

        // Directly rewrite the three files on disk, bypassing every
        // production writer, exactly like a corrupted/hand-edited store
        // -- only `report_one`'s row/list/evidence rows are touched.
        let rows_file = nested_artifacts_path(store);
        let mut rows = columns::read_nested_artifact_rows(&rows_file).unwrap();
        for r in &mut rows {
            if r.id == "n1" {
                r.bytes = 999_999;
            }
        }
        columns::write_nested_artifact_rows(&rows_file, &rows).unwrap();

        let lists_file = nested_artifact_lists_path(store);
        let mut lists = columns::read_nested_artifact_list_rows(&lists_file).unwrap();
        for l in &mut lists {
            if l.artifact_id == "n1" {
                l.value = format!("TAMPERED-{}", l.value);
            }
        }
        columns::write_nested_artifact_list_rows(&lists_file, &lists).unwrap();

        let evidence_file = nested_artifact_evidence_path(store);
        let mut evidence = columns::read_nested_artifact_evidence_rows(&evidence_file).unwrap();
        for e in &mut evidence {
            if e.artifact_id == "n1" {
                e.detail = format!("TAMPERED-{}", e.detail);
            }
        }
        columns::write_nested_artifact_evidence_rows(&evidence_file, &evidence).unwrap();

        let (report_rows, interior_rows) =
            read_nested_artifact_table(store, "k").expect("rows for k");
        let (report_stored, report_lists, report_evidence) = &report_rows[0];
        let rebuilt_report =
            nested_artifact_from_stored(report_stored, report_lists, report_evidence);
        assert_eq!(
            rebuilt_report.bytes, 999_999,
            "the rebuild must reflect the tampered row's own bytes, not the original in-memory value"
        );
        assert!(
            rebuilt_report.coverage.limits[0].starts_with("TAMPERED-"),
            "the rebuild must reflect the tampered coverage-limit list row"
        );
        assert!(
            rebuilt_report.variant.unknowns[0].starts_with("TAMPERED-"),
            "a variant-unknown list row keyed by list_kind must not be conflated with a \
             coverage-limit row for the same artifact"
        );
        assert!(
            rebuilt_report.producer_evidence[0]
                .detail
                .starts_with("TAMPERED-"),
            "the rebuild must reflect the tampered evidence row"
        );

        let (interior_stored, interior_lists, interior_evidence) = &interior_rows[0];
        let rebuilt_interior =
            nested_artifact_from_stored(interior_stored, interior_lists, interior_evidence);
        assert_eq!(
            rebuilt_interior.bytes, interior_one.bytes,
            "n2's row must be untouched by n1's tamper"
        );
        assert_eq!(
            rebuilt_interior.coverage.limits, interior_one.coverage.limits,
            "n2's coverage-limit list must not pick up n1's tampered value"
        );
        assert_eq!(
            rebuilt_interior.producer_evidence, interior_one.producer_evidence,
            "n2's evidence must not pick up n1's tampered value"
        );
    }

    /// R16 item 3: every `FactStatus` (`Known` with every `FactValue`
    /// variant, `Unknown`, `Unavailable`, `Conflicting` with two
    /// candidates -- the production shape, never a single-candidate
    /// list), every `EvidenceSource` variant, `event_at`/`freshness`/
    /// `note`, and list order (`seq`) round-trip through
    /// `evidence.parquet`. Also pins `evidence_table_exists`: `false`
    /// before any write, `true` after, independent of whether that
    /// scope's own rows are empty.
    #[test]
    fn evidence_table_round_trips_every_status_and_source_variant() {
        use crate::evidence::{
            Evidence, EvidenceSource, FactKind, FactSubtype, FactValue, Freshness,
        };

        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path();
        assert!(!evidence_table_exists(store));

        let mut e1 = Evidence::known(
            FactKind::Activity,
            FactSubtype::Modified,
            FactValue::Timestamp(1_700_000_000),
            EvidenceSource::FilesystemMetadata {
                detail: "folded walk stat".into(),
            },
            1_700_000_100,
        );
        e1.event_at = Some(1_700_000_050);
        e1.freshness = Freshness {
            expires_after_secs: Some(3600),
            coverage_note: Some("scanned roots only".into()),
        };
        e1.note = Some("a human note".into());

        let e_bool = Evidence::known(
            FactKind::CurrentUse,
            FactSubtype::Mounted,
            FactValue::Bool(true),
            EvidenceSource::ProcessQuery {
                tool: "lsof".into(),
            },
            1,
        );
        let e_bytes = Evidence::known(
            FactKind::Reclaimability,
            FactSubtype::LogicalBytes,
            FactValue::Bytes(123_456),
            EvidenceSource::ManagerLock {
                tool: "gradle".into(),
                path: "/w/x/.lock".into(),
            },
            2,
        );
        let e_signed = Evidence::known(
            FactKind::Reclaimability,
            FactSubtype::ObservedFreed,
            FactValue::SignedBytes(-4096),
            EvidenceSource::ConfigDeclaration {
                path: "/w/.tool-versions".into(),
            },
            3,
        );
        let e_count = Evidence::known(
            FactKind::Recovery,
            FactSubtype::BackupDependent,
            FactValue::Count(7),
            EvidenceSource::Lockfile {
                ecosystem: "node".into(),
                path: "/w/package-lock.json".into(),
            },
            4,
        );
        let e_list = Evidence::known(
            FactKind::Consumer,
            FactSubtype::InferredConsumer,
            FactValue::List(vec!["p1".into(), "p2".into()]),
            EvidenceSource::BuildMetadata {
                path: "/w/Info.plist".into(),
            },
            5,
        );
        let e_unknown = Evidence::unknown(
            FactKind::CurrentUse,
            FactSubtype::Process,
            EvidenceSource::DockerApi {
                detail: "daemon unreachable".into(),
            },
            6,
            crate::reason!("no process query available on this platform"),
        );
        let e_unavailable = Evidence::unavailable(
            FactKind::CurrentUse,
            FactSubtype::OpenFile,
            EvidenceSource::Statvfs,
            7,
            crate::reason!("permission denied"),
        );
        let e_conflicting_text = Evidence::conflicting(
            FactKind::Consumer,
            FactSubtype::DeclaredConsumer,
            vec![
                FactValue::Text("3.12.4".into()),
                FactValue::Text("3.12.1".into()),
            ],
            EvidenceSource::Inferred {
                basis: "two installations match".into(),
            },
            8,
            crate::reason!("more than one installation matches"),
        );
        let e_conflicting_bytes = Evidence::conflicting(
            FactKind::Reclaimability,
            FactSubtype::EstimatedReclaimable,
            vec![FactValue::Bytes(1000), FactValue::Bytes(5000)],
            EvidenceSource::Inferred {
                basis: "min/max bound".into(),
            },
            9,
            crate::reason!("only a bound is known"),
        );

        let evidence = vec![
            e1,
            e_bool,
            e_bytes,
            e_signed,
            e_count,
            e_list,
            e_unknown,
            e_unavailable,
            e_conflicting_text,
            e_conflicting_bytes,
        ];

        write_evidence_table(
            store,
            "k",
            &[("artifact:row-1".to_string(), evidence.as_slice())],
        )
        .unwrap();
        assert!(evidence_table_exists(store));

        let by_key = read_evidence_table(store, "k");
        let rebuilt = by_key.get("artifact:row-1").expect("evidence for the key");
        assert_eq!(
            serde_json::to_value(rebuilt).unwrap(),
            serde_json::to_value(&evidence).unwrap(),
            "every FactStatus/FactValue/EvidenceSource variant, freshness, event_at and \
             note must round-trip, in order"
        );

        // A different scope key's evidence does not leak in.
        let empty = read_evidence_table(store, "other-scope");
        assert!(empty.is_empty());
    }
}
