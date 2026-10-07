//! The one place a *unit* (an external storage location, a tool home) is
//! turned into bytes.
//!
//! The 2026-09-21 review's P1 on non-incremental traversal: `external.rs`
//! recursively re-sized every external root on every call, and the agent
//! layer added a second recursive traversal of its own. Both are now
//! funnelled through here, so there is exactly one implementation to
//! make incremental and exactly one place the work counters live
//! (`.oh/guardrails/no-second-traversal-on-report-path.md` allow-lists
//! this module and forbids the callers).
//!
//! What this module does today, honestly:
//!
//! * it owns the readability probe and the folded measurement, so
//!   `external.rs` and `agents/**` contain no directory traversal at
//!   all; and
//! * it records the work each measurement actually did
//!   (`crate::work_counters`), which is what makes "unchanged work is
//!   cheap" a testable claim rather than an assertion.
//!
//! Since 2026-09-22 it also **reuses** the folded rows a previous pass
//! persisted, which is the other half of the incrementality repair. See
//! [`reuse_folded_measurement`] for what the reuse is allowed to
//! conclude and, just as importantly, what it cannot see.

use crate::report::ArtifactKind;
use std::path::{Path, PathBuf};

/// Whether a unit can be measured this pass at all, distinguishing the
/// three outcomes that must never be collapsed: gone, present but
/// unreadable, and measurable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitAccess {
    /// Nothing at this path. A real disappearance, which the owning
    /// observation's sweep may tombstone.
    Absent,
    /// The path exists but this pass could not read it (permission on
    /// the path or a parent). Coverage is incomplete -- never a
    /// disappearance, never a zero.
    Unreadable(String),
    Measurable,
}

/// One `symlink_metadata` plus, for a directory, one listing attempt.
/// The listing is the only directory read in the external/agent
/// measurement path, and it exists to tell "unreadable" from "empty".
pub fn access(path: &Path) -> UnitAccess {
    match crate::fs_gate::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => UnitAccess::Absent,
        Err(e) => UnitAccess::Unreadable(e.to_string()),
        Ok(meta) if meta.is_dir() => {
            crate::work_counters::record_dir_listed();
            match crate::fs_gate::read_dir(path) {
                Ok(_) => UnitAccess::Measurable,
                // `stat` succeeds (reaching the path itself needs only
                // the *parent's* execute bit) while the directory's own
                // contents cannot be listed, e.g. `chmod 000`.
                Err(e) => UnitAccess::Unreadable(e.to_string()),
            }
        }
        Ok(_) => UnitAccess::Measurable,
    }
}

/// A unit's folded byte total, excluding any nested locations that are
/// separately measured as their own units (so the same bytes are never
/// counted twice).
pub struct FoldedUnit {
    /// The stored measurement was replayed rather than re-taken. The
    /// caller re-stamps such a unit's rows through
    /// `crate::growth::touch_folded_rows`, so the next pass's window can
    /// still vouch for them.
    pub reused: bool,
    pub bytes: u64,
    pub hardlinked: bool,
    /// Newest recorded modification among the measured children, from
    /// the same folded walk. Carried so an external unit can have an
    /// Activity fact without a second pass: `docs/usage.md` claimed
    /// modification age for external locations while `ExternalUnit` had
    /// no `mtime_max` field at all (the 2026-09-22 re-review).
    pub mtime_max: u64,
    /// Every directory this measurement descended into was readable.
    /// `false` when some subdirectory *inside* the unit -- not the unit's
    /// own root, which [`access`] already probes -- could not be listed
    /// (a `chmod 000` a few levels down). `bytes` is still the honest sum
    /// of what *was* read, but it is a lower bound, not the unit's true
    /// size: the caller must never let it overwrite a stored measurement
    /// or feed a growth/regrowth delta (`.oh/guardrails/coverage-changes-
    /// are-not-storage-changes.md`) -- a folder going unreadable for one
    /// pass is coverage shrinking, not the unit shrinking.
    pub complete: bool,
    /// The oldest time a sealed volume inside was actually walked, when
    /// this measurement replayed one from its stamp (#181): the bytes are
    /// that walk's, so the report says when it was.
    pub sealed_walked_at: Option<u64>,
}

// ---------------------------------------------------------------------
// Sealed mounts (R19). A read-only filesystem mounted directly under a
// unit root -- `/Library/Developer/CoreSimulator/Volumes/<runtime>`, a
// sealed APFS volume per installed simulator runtime, 17 GB and 600k
// files each -- is outside every FSEvents window (events are per
// volume), so the event-keyed reuse above never vouched for it and every
// pass re-walked it: 482k listings and 1.77M stats on an unchanged
// machine (2026-09-24 measurement). A read-only filesystem cannot change
// under us; its `statfs` stamp (device, block totals) is the proof. Each
// such mount is measured as its own part, its stamp and result stored,
// and reused without a walk while the stamp holds. The unit's own walk
// excludes the mounts, so the folded rows and their event-keyed reuse
// describe the writable part only.
// ---------------------------------------------------------------------

struct SealedMount {
    path: PathBuf,
    stamp: crate::fs_gate::fs_space::VolumeStamp,
}

/// The read-only filesystems mounted directly under `path` (one listing
/// of the unit root, one `statfs` per child directory).
fn sealed_mounts_under(path: &Path) -> Vec<SealedMount> {
    let Some(root) = crate::fs_gate::fs_space::volume_stamp(path) else {
        return Vec::new();
    };
    let Ok(entries) = crate::fs_gate::read_dir(path) else {
        return Vec::new();
    };
    let mut out: Vec<SealedMount> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| crate::fs_gate::is_dir(p))
        .filter_map(|p| {
            let stamp = crate::fs_gate::fs_space::volume_stamp(&p)?;
            (stamp.read_only && stamp.device != root.device)
                .then_some(SealedMount { path: p, stamp })
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Whether `path` holds nothing but sealed read-only mounts whose stored
/// stamps still hold, and is itself the directory its stored root row
/// describes (#181). Such a location cannot have changed without a new
/// stamp: its only entries are mount points of read-only filesystems
/// (nothing can be written inside them), and an entry added, removed or
/// renamed moves the directory's own mtime/ctime. It is then vouched for
/// without an event window, so a pass whose window was lost, or a store
/// last written by another swamp, does not re-walk 1.77 million files of
/// simulator runtimes. Any other entry, a missing stamp or a changed
/// one: not vouched, and the usual rules apply.
pub(crate) fn sealed_only_unchanged(store: Option<&Path>, path: &Path) -> bool {
    let Some(dir) = store else { return false };
    let mounts = sealed_mounts_under(path);
    if mounts.is_empty() {
        return false;
    }
    let unit_path = path.display().to_string();
    let stored = crate::growth::volume_stamps_for(dir, &unit_path);
    if !mounts.iter().all(|m| reuse_sealed(&stored, m).is_some()) {
        return false;
    }
    let rows = crate::growth::folded_rows_for(dir, &unit_path);
    let Some(root) = rows.iter().find(|r| r.rel_dir.is_empty()) else {
        return false;
    };
    let Ok(meta) = crate::fs_gate::symlink_metadata(path) else {
        return false;
    };
    if stamp_ns(&meta) != (root.mtime_ns, root.ctime_ns) {
        return false;
    }
    let Ok(entries) = crate::fs_gate::read_dir(path) else {
        return false;
    };
    entries
        .map(|e| e.map(|e| e.path()))
        .all(|p| p.is_ok_and(|p| mounts.iter().any(|m| m.path == p)))
}

/// One sealed mount's contribution, replayed from its stored stamp or
/// walked.
struct SealedPart {
    bytes: u64,
    hardlinked: bool,
    mtime_max: u64,
    complete: bool,
    reused: bool,
    /// When this volume was last actually walked: kept across replays.
    walked_at: u64,
    dirs: Vec<crate::report::DirRollup>,
}

/// The stored result for `mount` if its stamp still holds.
fn reuse_sealed(
    stored: &[crate::growth::columns::StoredVolumeStampRow],
    mount: &SealedMount,
) -> Option<SealedPart> {
    let row = stored
        .iter()
        .find(|r| r.mount_path == mount.path.display().to_string())?;
    let same = row.device == mount.stamp.device
        && row.total_blocks == mount.stamp.total_blocks
        && row.root_ino == mount.stamp.root_ino
        && row.root_mtime == mount.stamp.root_mtime;
    same.then(|| SealedPart {
        bytes: row.bytes,
        hardlinked: row.hardlinked,
        mtime_max: row.mtime_max,
        complete: row.complete,
        reused: true,
        walked_at: row.observed_at,
        dirs: Vec::new(),
    })
}

/// Walks one sealed mount as part of `unit` (directory rows relative to
/// the unit, like the unit's own walk produces them).
fn walk_sealed(
    unit: &Path,
    mount: &SealedMount,
    exclusions: &[PathBuf],
    observed_at: u64,
    stamp_dirs: bool,
) -> SealedPart {
    // The unit's exclusions that fall inside this mount -- never the
    // mount itself, which the unit's own walk excludes and this one is.
    let nested: Vec<PathBuf> = exclusions
        .iter()
        .filter(|e| *e != &mount.path && e.starts_with(&mount.path))
        .cloned()
        .collect();
    let (row, dirs, _stamps, complete) = crate::walk::resize_artifact_stamped(
        &mount.path,
        ArtifactKind::Unknown,
        observed_at,
        Some((STORE_WORKTREE_ID, unit)),
        &nested,
        stamp_dirs,
    );
    SealedPart {
        bytes: row.bytes,
        hardlinked: row.hardlinked,
        mtime_max: row.mtime_max,
        complete,
        reused: false,
        walked_at: observed_at,
        dirs,
    }
}

/// Every sealed mount under `path`: reused where the stamp holds, walked
/// otherwise (or always, with `walk_all`, when the caller needs every
/// directory row). Records the stamps of what was walked and complete.
fn sealed_parts(
    store: Option<&Path>,
    path: &Path,
    mounts: &[SealedMount],
    exclusions: &[PathBuf],
    observed_at: u64,
    walk_all: bool,
) -> Vec<SealedPart> {
    if mounts.is_empty() {
        return Vec::new();
    }
    let unit_path = path.display().to_string();
    let stored = store
        .map(|dir| crate::growth::volume_stamps_for(dir, &unit_path))
        .unwrap_or_default();
    let mut parts = Vec::with_capacity(mounts.len());
    let mut rows: Vec<crate::growth::columns::StoredVolumeStampRow> = Vec::new();
    for mount in mounts {
        let reusable = reuse_sealed(&stored, mount);
        if std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty()) {
            eprintln!(
                "[xtrace] sealed {} stamp={:?} stored={} walk_all={walk_all} reused={}",
                mount.path.display(),
                mount.stamp,
                stored
                    .iter()
                    .any(|r| r.mount_path == mount.path.display().to_string()),
                !walk_all && reusable.is_some()
            );
        }
        let part = match (walk_all, reusable) {
            (false, Some(part)) => {
                crate::work_counters::record_cache_hit();
                part
            }
            _ => {
                crate::work_counters::record_cache_miss();
                walk_sealed(path, mount, exclusions, observed_at, store.is_some())
            }
        };
        // Stored complete or not: a sealed volume's unreadable corners
        // are the same unreadable corners next pass (an Xcode runtime
        // has root-only directories), and the stamp says nothing moved.
        // The `complete` flag travels with the row so the unit reports
        // the lower bound as such.
        rows.push(crate::growth::columns::StoredVolumeStampRow {
            unit_path: unit_path.clone(),
            mount_path: mount.path.display().to_string(),
            device: mount.stamp.device,
            total_blocks: mount.stamp.total_blocks,
            root_ino: mount.stamp.root_ino,
            root_mtime: mount.stamp.root_mtime,
            bytes: part.bytes,
            hardlinked: part.hardlinked,
            mtime_max: part.mtime_max,
            complete: part.complete,
            // The time the volume was walked, not this replay's.
            observed_at: part.walked_at,
        });
        parts.push(part);
    }
    if let Some(dir) = store {
        // A stamp write that fails is a reuse that will miss next time.
        let _ = crate::growth::store_volume_stamps(dir, &unit_path, &rows);
    }
    parts
}

fn add_sealed(folded: &mut FoldedUnit, parts: &[SealedPart]) {
    for p in parts {
        folded.bytes += p.bytes;
        folded.hardlinked |= p.hardlinked;
        folded.mtime_max = folded.mtime_max.max(p.mtime_max);
        folded.complete &= p.complete;
        folded.reused &= p.reused;
        if p.reused {
            folded.sealed_walked_at = Some(
                folded
                    .sealed_walked_at
                    .map_or(p.walked_at, |w| w.min(p.walked_at)),
            );
        }
    }
}

/// The unit's exclusions plus its sealed mounts: what the unit's own
/// walk (and its stored measurement) must leave out.
fn with_sealed(exclusions: &[PathBuf], mounts: &[SealedMount]) -> Vec<PathBuf> {
    let mut all = exclusions.to_vec();
    all.extend(mounts.iter().map(|m| m.path.clone()));
    all.sort();
    all.dedup();
    all
}

/// Measures `path`, reusing the previous pass's folded rows when this
/// pass can show they still describe the tree.
///
/// `store` is the swamp store directory; `None` means "no history to
/// reuse and nowhere to record this measurement", which is what a
/// store-less caller (a one-shot `--root` report, most unit tests) gets.
pub fn measure(
    store: Option<&Path>,
    path: &Path,
    exclusions: &[PathBuf],
    observed_at: u64,
    coverage: &crate::fs_events::EventCoverage,
) -> FoldedUnit {
    let mounts = sealed_mounts_under(path);
    let exclusions = with_sealed(exclusions, &mounts);
    let exclusions = exclusions.as_slice();
    if let Some(mut folded) = reuse_folded_measurement(store, path, exclusions, coverage) {
        crate::work_counters::record_cache_hit();
        add_sealed(
            &mut folded,
            &sealed_parts(store, path, &mounts, exclusions, observed_at, false),
        );
        return folded;
    }
    if mounts.is_empty()
        && let Some((folded, _)) =
            partial_measure(store, path, exclusions, observed_at, coverage, None)
    {
        return folded;
    }
    let (row, _dirs, stamps, complete) = crate::walk::resize_artifact_stamped(
        path,
        ArtifactKind::Unknown,
        observed_at,
        None,
        exclusions,
        store.is_some(),
    );
    crate::work_counters::record_cache_miss();
    // `complete` is `resize_artifact_stamped`'s own record of whether
    // every directory its `Size` jobs tried to list actually listed:
    // `_dirs` (per-worktree `DirRollup`s) is empty here, because this
    // call has no worktree, so it cannot be used to detect an
    // unreadable subdirectory the way the Source-tree walk does.
    let folded = FoldedUnit {
        sealed_walked_at: None,
        reused: false,
        bytes: row.bytes,
        hardlinked: row.hardlinked,
        mtime_max: row.mtime_max,
        complete,
    };
    // An incomplete measurement is stored only with its root row marked
    // as a lower bound (`INCOMPLETE_MARK`), so it can never be replayed
    // whole; its readable subfolders' totals let the next changed pass
    // re-walk only the rest (#181: `~/Library/Caches` holds folders this
    // process may not list, so it was walked whole every pass). The
    // caller decides how to report this pass
    // (`.oh/guardrails/coverage-changes-are-not-storage-changes.md`).
    if let Some(dir) = store {
        record_folded_measurement(dir, path, exclusions, observed_at, &folded, &stamps);
    }
    let mut folded = folded;
    add_sealed(
        &mut folded,
        &sealed_parts(store, path, &mounts, exclusions, observed_at, false),
    );
    folded
}

/// The exclusion set a stored measurement was taken under, as one
/// string. A measurement taken while a nested location was excluded
/// describes different bytes than one taken without it, so a changed
/// exclusion set is a cache miss rather than a silent wrong answer
/// (`.oh/guardrails/coverage-changes-are-not-storage-changes.md` is the
/// other half of this: the miss re-measures, it never invents a delta).
fn exclusions_digest(exclusions: &[PathBuf]) -> String {
    let mut parts: Vec<String> = exclusions.iter().map(|p| p.display().to_string()).collect();
    parts.sort();
    parts.join("\u{1}")
}

/// The previous pass's folded measurement of `path`, if this pass's
/// [`crate::fs_events::EventCoverage`] can show that nothing under
/// `path` has changed since that measurement was taken.
///
/// **Cost.** Nothing: no listing, no `stat`, no read. The decision is
/// made from the stored root row and the replay window.
///
/// **What this detects.** Everything FSEvents reports under the unit --
/// creations, deletions, renames, *and writes into existing files*. The
/// last of those is why this is keyed on events and not on directory
/// stamps any more. Until 2026-09-22 the key was each recorded
/// directory's own `mtime`/`ctime`, which moves when an entry is
/// created, deleted, renamed or replaced and **not** when a file inside
/// it is appended to or rewritten in place. That is the normal way an
/// agent session transcript grows, so a growth tool keyed on stamps
/// reported the growing file at its old size. The integration decision
/// of 2026-09-22 removed stamp-only reuse as a sufficient condition;
/// this is its external-family half. A changed exclusion set is still a
/// miss, for the reason [`exclusions_digest`] gives.
///
/// **When there is no reuse.** Whenever this pass has no trusted window
/// over `path` -- a full walk, any [`crate::fs_events::RefreshRefusal`],
/// a first observation, a store-less caller -- and whenever the stored
/// rows predate the window (a pass that skipped this unit family leaves
/// exactly that gap). The unit is then re-measured, which is the slow
/// answer and always the correct one.
pub fn reuse_folded_measurement(
    store: Option<&Path>,
    path: &Path,
    exclusions: &[PathBuf],
    coverage: &crate::fs_events::EventCoverage,
) -> Option<FoldedUnit> {
    let dir = store?;
    let unit_path = path.display().to_string();
    let rows = crate::growth::folded_rows_for(dir, &unit_path);
    let root = rows.iter().find(|r| r.rel_dir.is_empty())?;
    if root.exclusions != exclusions_digest(exclusions) {
        return None;
    }
    // The gate. Everything below the unit root is covered by one
    // question, because an event anywhere under the unit -- including a
    // write into an existing file, which no directory stamp can show --
    // fails it.
    if !coverage.unchanged_since(path, root.observed_at) {
        return None;
    }
    Some(FoldedUnit {
        sealed_walked_at: None,
        reused: true,
        bytes: root.bytes,
        hardlinked: root.hardlinked,
        mtime_max: root.mtime_max,
        // Only a `complete` fold is ever stored (see `measure`), so a
        // replayed root row was complete when it was taken.
        complete: true,
    })
}

fn stamp_ns(meta: &crate::fs_gate::Metadata) -> (i64, i64) {
    use crate::fs_gate::MetadataExt;
    (
        meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
        meta.ctime() * 1_000_000_000 + meta.ctime_nsec(),
    )
}

fn record_folded_measurement(
    store: &Path,
    path: &Path,
    exclusions: &[PathBuf],
    observed_at: u64,
    folded: &FoldedUnit,
    stamps: &[crate::walk::DirStamp],
) {
    let started = std::time::Instant::now();
    if let Some(rows) = folded_rows(path, exclusions, observed_at, folded, stamps, &[]) {
        let folded_elapsed = started.elapsed();
        // A cache write that fails is a cache that will miss next time,
        // which is the correct outcome and not worth failing a report over.
        let _ = crate::growth::store_folded_rows(store, &path.display().to_string(), &rows);
        if std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty()) {
            eprintln!(
                "[xtrace] record_folded {} rows={} fold={folded_elapsed:?} write={:?}",
                path.display(),
                rows.len(),
                started.elapsed().saturating_sub(folded_elapsed)
            );
        }
    }
}

/// Marks a depth-1 row whose `bytes`/`mtime_max` hold that immediate
/// subfolder's folded total (#181). Appended to the row's exclusions
/// digest, which only the root row is ever compared on, so a reader that
/// predates per-child totals ignores them and a reader that knows them
/// never mistakes a plain stamp row for a total.
const CHILD_TOTAL_MARK: &str = "\u{2}child-total";

/// Appended to the root row's digest of a measurement that was a lower
/// bound: stored only so its complete subfolders can be replayed.
const INCOMPLETE_MARK: &str = "\u{2}incomplete";

/// The name, under a subfolder's rel_dir, of the row holding that
/// subfolder's bytes with every hard link counted (what its directory
/// rollups sum to, which the drilldown shows beside an adjustment row).
/// A NUL cannot be in a file name, so this never names a real directory.
const RAW_BYTES_ROW: &str = "\0raw";

/// The rows one folded measurement stores: one stamp row per directory it
/// listed (plus `carried`, the stored rows of subfolders this pass
/// replayed), the folded total on the root row and, when they provably
/// add up to it, each immediate subfolder's own total on its row.
///
/// Per-child totals (each hard-linked inode counted once within its
/// subfolder) are recorded only when no inode is linked from two
/// subfolders or from the root's own files (it would be counted twice
/// when one side is replayed and the other re-walked), which holds
/// exactly when root files plus subfolder totals equal the folded total;
/// the newest mtime must match too. Anything else stores the plain rows, and the next changed
/// pass walks the whole unit, which is always the correct answer.
fn folded_rows(
    path: &Path,
    exclusions: &[PathBuf],
    observed_at: u64,
    folded: &FoldedUnit,
    stamps: &[crate::walk::DirStamp],
    carried: &[crate::growth::FoldedRow],
) -> Option<Vec<crate::growth::FoldedRow>> {
    let unit_path = path.display().to_string();
    let digest = exclusions_digest(exclusions);
    let marked = format!("{digest}{CHILD_TOTAL_MARK}");
    let mut rows: Vec<crate::growth::FoldedRow> = Vec::with_capacity(stamps.len() + carried.len());
    let mut saw_root = false;
    let mut root_own: u64 = 0;
    let mut root_linked = false;
    let mut files_max: u64 = 0;
    // Per immediate subfolder: bytes (each hard-linked inode once within
    // the subfolder), newest file mtime, whether it holds a linked file.
    let mut children: std::collections::HashMap<String, (u64, u64, bool)> =
        std::collections::HashMap::new();
    let mut raw: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    let mut seen: std::collections::HashSet<(String, u64, u64)> = std::collections::HashSet::new();
    let mut incomplete_buckets: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    let mut root_incomplete = false;
    let mut unreplayable: std::collections::HashSet<String> = std::collections::HashSet::new();
    for stamp in stamps {
        // A stamp from outside the unit cannot be validated against the
        // unit root later, so the whole measurement is not stored.
        let Ok(rel) = stamp.path.strip_prefix(path) else {
            if std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty()) {
                eprintln!(
                    "[xtrace] not stored: stamp {} outside {}",
                    stamp.path.display(),
                    path.display()
                );
            }
            return None;
        };
        let rel = rel.display().to_string();
        if stamp.pending && !rel.is_empty() {
            // A file whose allocation was still pending: its subfolder's
            // total may rise at writeback with no event (#197), so it is
            // walked again rather than replayed.
            unreplayable.insert(rel.split('/').next().unwrap_or("").to_string());
        }
        if stamp.incomplete {
            // Something here could not be read: its subfolder is never
            // replayed. The root is re-walked on every pass anyway (an
            // unlistable root has no listed stamp, so nothing is stored).
            if rel.is_empty() {
                root_incomplete = true;
            } else {
                incomplete_buckets.insert(rel.split('/').next().unwrap_or("").to_string());
            }
            if stamp.mtime_ns == 0 && stamp.ctime_ns == 0 && stamp.own_bytes == 0 {
                continue;
            }
        }
        saw_root |= rel.is_empty();
        files_max = files_max.max(stamp.files_mtime_max);
        let bucket = rel.split('/').next().unwrap_or("").to_string();
        let mut bytes = stamp.own_bytes;
        for (dev, ino, b, _) in &stamp.linked {
            if !seen.insert((bucket.clone(), *dev, *ino)) {
                bytes = bytes.saturating_sub(*b);
            }
        }
        if rel.is_empty() {
            root_own += bytes;
            root_linked |= stamp.shared_inode;
        } else {
            *raw.entry(bucket.clone()).or_insert(0) += stamp.own_bytes;
            let entry = children.entry(bucket).or_insert((0, 0, false));
            entry.0 += bytes;
            entry.1 = entry.1.max(stamp.files_mtime_max);
            entry.2 |= stamp.shared_inode;
        }
        rows.push(crate::growth::FoldedRow {
            unit_path: unit_path.clone(),
            rel_dir: rel,
            mtime_ns: stamp.mtime_ns,
            ctime_ns: stamp.ctime_ns,
            access_atime: stamp.access_atime,
            access_observed_at: stamp.access_observed_at,
            bytes: 0,
            hardlinked: false,
            mtime_max: 0,
            observed_at,
            exclusions: digest.clone(),
        });
    }
    // No root stamp means the root itself was never listed (an excluded
    // or unreadable root): there is nothing to anchor a later reuse on.
    if !saw_root {
        if std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty()) {
            eprintln!("[xtrace] not stored: no root stamp for {}", path.display());
        }
        return None;
    }
    let first_carried = rows.len();
    let mut carried_total: u64 = 0;
    for row in carried {
        let mut row = row.clone();
        row.observed_at = observed_at;
        // A subfolder's total row, not its all-links byte row below it.
        if row.exclusions == marked && !row.rel_dir.contains('/') {
            carried_total += row.bytes;
            files_max = files_max.max(row.mtime_max);
        }
        rows.push(row);
    }
    // The buckets add up to the deduplicated total exactly only when no
    // inode is linked from two of them: then a subfolder can be replayed
    // on its own without counting a shared file twice.
    // Every lower bound is accounted for by a subfolder that will be
    // walked again; otherwise nothing is marked.
    let attributed = folded.complete || root_incomplete || !incomplete_buckets.is_empty();
    let consistent = attributed
        && !root_linked
        && root_own + children.values().map(|c| c.0).sum::<u64>() + carried_total == folded.bytes
        && files_max == folded.mtime_max;
    if !consistent && std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty()) {
        eprintln!(
            "[xtrace] subfolder totals not recorded for {}: complete={} attributed={attributed} root_linked={root_linked} parts={} total={} newest={files_max}/{}",
            path.display(),
            folded.complete,
            root_own + children.values().map(|c| c.0).sum::<u64>() + carried_total,
            folded.bytes,
            folded.mtime_max
        );
    }
    for (i, row) in rows.iter_mut().enumerate() {
        if row.rel_dir.is_empty() {
            row.bytes = folded.bytes;
            row.hardlinked = folded.hardlinked;
            row.mtime_max = folded.mtime_max;
            // A lower bound is never replayed whole: its root row carries
            // a digest the whole-unit reuse cannot match.
            if !folded.complete {
                row.exclusions = format!("{digest}{INCOMPLETE_MARK}");
            }
            continue;
        }
        if row.rel_dir.contains('/') {
            continue;
        }
        if !consistent {
            row.bytes = 0;
            row.mtime_max = 0;
            row.exclusions = digest.clone();
        } else if i < first_carried
            && !incomplete_buckets.contains(&row.rel_dir)
            && !unreplayable.contains(&row.rel_dir)
            && let Some((bytes, mtime_max, linked)) = children.get(&row.rel_dir)
        {
            row.bytes = *bytes;
            row.mtime_max = *mtime_max;
            row.hardlinked = *linked;
            row.exclusions = marked.clone();
        }
    }
    if consistent {
        for (name, (_, _, linked)) in &children {
            if *linked && !incomplete_buckets.contains(name) {
                rows.push(crate::growth::FoldedRow {
                    unit_path: unit_path.clone(),
                    rel_dir: format!("{name}/{RAW_BYTES_ROW}"),
                    mtime_ns: 0,
                    ctime_ns: 0,
                    access_atime: None,
                    access_observed_at: None,
                    bytes: raw.get(name).copied().unwrap_or(0),
                    hardlinked: true,
                    mtime_max: 0,
                    observed_at,
                    exclusions: marked.clone(),
                });
            }
        }
    }
    Some(rows)
}

/// A changed unit, measured by re-walking only what changed (#181).
///
/// When this pass's event window shows changes under the unit, the whole
/// unit used to be walked again: `~/Library/Caches` (163k files) on
/// every pass, because some cache under it is always being written. With
/// per-child totals stored (see [`folded_rows`]), every immediate
/// subfolder the window vouches for -- no event at or under it since its
/// total was taken, and its own directory stamp unchanged, so it was not
/// replaced by a rename -- is replayed, and one walk measures the root's
/// own files plus every other subfolder (changed, new, or not vouched
/// for). The sum is what a full walk would have found, because nothing
/// is linked across subfolders (the stored totals prove it for the
/// replayed ones; a linked file in the walked part falls back to the
/// full walk).
///
/// `None` whenever that cannot be shown: no trusted window, no stored
/// per-child totals, a changed exclusion set, nothing to replay. The
/// caller then walks the whole unit, as before.
fn partial_measure(
    store: Option<&Path>,
    path: &Path,
    exclusions: &[PathBuf],
    observed_at: u64,
    coverage: &crate::fs_events::EventCoverage,
    worktree: Option<(&str, &Path)>,
) -> Option<(FoldedUnit, Vec<crate::report::DirRollup>)> {
    let dir = store?;
    let unit_path = path.display().to_string();
    let rows = crate::growth::folded_rows_for(dir, &unit_path);
    let root = rows.iter().find(|r| r.rel_dir.is_empty())?;
    let digest = exclusions_digest(exclusions);
    if root.exclusions != digest && root.exclusions != format!("{digest}{INCOMPLETE_MARK}") {
        return None;
    }
    let marked = format!("{digest}{CHILD_TOTAL_MARK}");
    let debug_trace = std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty());
    let mut reused: Vec<&crate::growth::FoldedRow> = Vec::new();
    for r in rows
        .iter()
        .filter(|r| !r.rel_dir.is_empty() && !r.rel_dir.contains('/') && r.exclusions == marked)
    {
        // A subfolder holding a linked file is walked: a link made or
        // removed elsewhere changes its count without an event under it.
        let reason = if r.hardlinked {
            Some("linked")
        } else {
            let child = path.join(&r.rel_dir);
            let unchanged = coverage.unchanged_since(&child, root.observed_at);
            if !unchanged {
                if debug_trace {
                    eprintln!(
                        "[xtrace] child {} freshness stored_at={} reason={}",
                        child.display(),
                        root.observed_at,
                        coverage.unchanged_since_trace_reason(&child, root.observed_at)
                    );
                }
                Some("coverage-changed-or-unproven")
            } else if !same_directory(&child, r) {
                Some("directory-stamp-changed-or-missing")
            } else {
                reused.push(r);
                None
            }
        };
        if debug_trace && let Some(reason) = reason {
            eprintln!(
                "[xtrace] child {} skip={reason}",
                path.join(&r.rel_dir).display()
            );
        }
    }
    if debug_trace {
        let marked_n = rows
            .iter()
            .filter(|r| !r.rel_dir.is_empty() && !r.rel_dir.contains('/') && r.exclusions == marked)
            .count();
        eprintln!(
            "[xtrace] partial {}: {} of {marked_n} recorded subfolders replayable",
            path.display(),
            reused.len()
        );
    }
    if reused.is_empty() {
        return None;
    }
    // A linked child can force the whole-unit fallback after a partial walk.
    // If almost all recorded directory rows are in the walked remainder,
    // measure once in full instead: replay would save little work but risks
    // doing that large remainder twice. This only chooses more measurement;
    // it never relaxes coverage or hardlink accounting.
    let linked_remainder = rows.iter().any(|r| {
        !r.rel_dir.is_empty() && !r.rel_dir.contains('/') && r.exclusions == marked && r.hardlinked
    });
    if linked_remainder {
        let replayed_names: std::collections::HashSet<&str> =
            reused.iter().map(|r| r.rel_dir.as_str()).collect();
        let replayed_rows = rows
            .iter()
            .filter(|r| replayed_names.contains(r.rel_dir.split('/').next().unwrap_or("")))
            .count();
        if replayed_rows.saturating_mul(16) < rows.len() {
            if debug_trace {
                eprintln!(
                    "[xtrace] partial {}: full walk selected; linked remainder and only {replayed_rows}/{} rows replayable",
                    path.display(),
                    rows.len()
                );
            }
            return None;
        }
    }
    let mut pruned = exclusions.to_vec();
    pruned.extend(reused.iter().map(|r| path.join(&r.rel_dir)));
    let walk_started = std::time::Instant::now();
    let walk_before = debug_trace.then(crate::work_counters::snapshot);
    let (row, mut dirs, stamps, complete) = crate::walk::resize_artifact_stamped(
        path,
        ArtifactKind::Unknown,
        observed_at,
        worktree,
        &pruned,
        true,
    );
    if debug_trace {
        let after = crate::work_counters::snapshot();
        let before = walk_before.unwrap();
        let mut walked_children: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for stamp in &stamps {
            if let Ok(rel) = stamp.path.strip_prefix(path)
                && let Some(child) = rel.components().next()
            {
                *walked_children
                    .entry(child.as_os_str().to_string_lossy().into_owned())
                    .or_default() += 1;
            }
        }
        eprintln!(
            "[xtrace] partial walk {} reused_children={} walked_dirs={} elapsed={:?} dirs_delta={} files_delta={} walked_child_stamps={:?}",
            path.display(),
            reused.len(),
            stamps.len(),
            walk_started.elapsed(),
            after.dirs_listed - before.dirs_listed,
            after.files_statted - before.files_statted,
            walked_children
        );
    }
    crate::work_counters::record_cache_miss();
    // A linked file in the walked part may share its inode with a
    // replayed subfolder unless every one of its links was walked here:
    // otherwise walk the whole unit.
    if row.hardlinked {
        let mut seen: std::collections::HashMap<(u64, u64), u64> = std::collections::HashMap::new();
        let mut nlinks: std::collections::HashMap<(u64, u64), u64> =
            std::collections::HashMap::new();
        for (dev, ino, _, n) in stamps.iter().flat_map(|s| s.linked.iter()) {
            *seen.entry((*dev, *ino)).or_insert(0) += 1;
            nlinks.insert((*dev, *ino), *n);
        }
        if seen
            .iter()
            .any(|(k, c)| nlinks.get(k).is_some_and(|n| c < n))
        {
            return None;
        }
    }
    for _ in &reused {
        crate::work_counters::record_subtree_reused();
    }
    let folded = FoldedUnit {
        sealed_walked_at: None,
        reused: false,
        bytes: row.bytes + reused.iter().map(|r| r.bytes).sum::<u64>(),
        hardlinked: row.hardlinked,
        mtime_max: reused
            .iter()
            .map(|r| r.mtime_max)
            .fold(row.mtime_max, u64::max),
        complete,
    };
    let in_reused = |rel: &str| {
        reused.iter().any(|c| {
            rel == c.rel_dir
                || rel
                    .strip_prefix(c.rel_dir.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        })
    };
    let carried: Vec<crate::growth::FoldedRow> = rows
        .iter()
        .filter(|r| in_reused(&r.rel_dir))
        .cloned()
        .collect();
    if let Some(new_rows) = folded_rows(path, exclusions, observed_at, &folded, &stamps, &carried) {
        let _ = crate::growth::store_folded_rows(dir, &unit_path, &new_rows);
    }
    if let Some((worktree_id, _)) = worktree {
        // The drilldown reads one row per immediate subfolder: a
        // replayed one is its stored total, with the newest modification
        // a full walk would have rolled up (its directories' own mtimes
        // and its files').
        if let Some(root_dir) = dirs.iter_mut().find(|d| d.rel_path.is_empty()) {
            root_dir.entry_count += reused.len() as u32;
        }
        for child in &reused {
            let dirs_newest = carried
                .iter()
                .filter(|r| {
                    r.rel_dir == child.rel_dir
                        || r.rel_dir
                            .strip_prefix(child.rel_dir.as_str())
                            .is_some_and(|rest| rest.starts_with('/'))
                })
                .map(|r| r.mtime_ns.div_euclid(1_000_000_000))
                .max()
                .unwrap_or(0);
            let newest = dirs_newest.max(child.mtime_max as i64);
            let raw_name = format!("{}/{RAW_BYTES_ROW}", child.rel_dir);
            let own = carried
                .iter()
                .find(|r| r.rel_dir == raw_name)
                .map_or(child.bytes, |r| r.bytes);
            let cached = carried.iter().find(|r| r.rel_dir == child.rel_dir);
            dirs.push(crate::report::DirRollup {
                worktree_id: worktree_id.to_string(),
                track: None,
                rel_path: child.rel_dir.clone(),
                parent_rel_path: Some(String::new()),
                allocated_total: own,
                own_allocated: own,
                file_count: 0,
                entry_count: 0,
                symlink_count: 0,
                mod_time_min: (newest / 60) as i32,
                access_atime: cached.and_then(|r| r.access_atime),
                access_observed_at: cached.and_then(|r| r.access_observed_at),
                complete: true,
                growth_bytes: None,
            });
        }
    }
    Some((folded, dirs))
}

/// The per-subfolder totals `unit`'s last measurement stored (name to
/// bytes and newest file mtime), or `None` when it stored none (#181).
/// The manager pass keys a stored answer on these
/// (`manager_facts::collect`).
pub(crate) fn child_totals(
    store: &Path,
    unit: &Path,
) -> Option<std::collections::HashMap<String, (u64, u64)>> {
    let rows = crate::growth::folded_rows_for(store, &unit.display().to_string());
    let root = rows.iter().find(|r| r.rel_dir.is_empty())?;
    let digest = root
        .exclusions
        .strip_suffix(INCOMPLETE_MARK)
        .unwrap_or(&root.exclusions);
    let marked = format!("{digest}{CHILD_TOTAL_MARK}");
    let totals: std::collections::HashMap<String, (u64, u64)> = rows
        .iter()
        .filter(|r| r.exclusions == marked && !r.rel_dir.contains('/'))
        .map(|r| (r.rel_dir.clone(), (r.bytes, r.mtime_max)))
        .collect();
    (!totals.is_empty()).then_some(totals)
}

/// The subfolder is still the directory its stored row describes: a
/// directory renamed into its place under the same name brings its own
/// change stamp, and an event window over the parent would not show it.
fn same_directory(child: &Path, row: &crate::growth::FoldedRow) -> bool {
    match crate::fs_gate::symlink_metadata(child) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
            stamp_ns(&meta) == (row.mtime_ns, row.ctime_ns)
        }
        _ => false,
    }
}

/// Access and measurement in one call, so the ordinary report path never
/// pays for the readability probe on a unit whose stored measurement is
/// still good: [`reuse_folded_measurement`] answers from stats alone,
/// and only a miss reaches [`access`]'s listing.
pub enum UnitObservation {
    Absent,
    Unreadable(String),
    Unit(FoldedUnit),
}

pub fn observe_unit(
    store: Option<&Path>,
    path: &Path,
    exclusions: &[PathBuf],
    observed_at: u64,
    coverage: &crate::fs_events::EventCoverage,
) -> UnitObservation {
    // `measure` handles both the event-keyed reuse and the sealed
    // mounts; the readability probe is skipped only when nothing under
    // the unit needs listing at all.
    let mounts = sealed_mounts_under(path);
    let all = with_sealed(exclusions, &mounts);
    if mounts.is_empty()
        && let Some(folded) = reuse_folded_measurement(store, path, &all, coverage)
    {
        crate::work_counters::record_cache_hit();
        return UnitObservation::Unit(folded);
    }
    match access(path) {
        UnitAccess::Absent => UnitObservation::Absent,
        UnitAccess::Unreadable(why) => UnitObservation::Unreadable(why),
        UnitAccess::Measurable => {
            UnitObservation::Unit(measure(store, path, exclusions, observed_at, coverage))
        }
    }
}

/// [`observe_unit`] for a unit a build adapter will identify the
/// interior of: a machine-wide store.
///
/// With `allow_reuse`, exactly [`observe_unit`] -- a stored measurement
/// the event window vouches for is replayed, and the caller replays the
/// store's identified units under the same window, so an unchanged store
/// costs no listing and no read. Without it (the stored units cannot be
/// replayed), or when the stored measurement is not good, the store is
/// measured by the one folded walk, which this time also returns its
/// per-directory rows: the structure the adapter identifies from. That
/// is the same walk a miss would have done anyway, handing over rows it
/// already produced -- never a second traversal.
///
/// The directory rows are relative to `path`, with each directory's own
/// bytes in `own_allocated`; `report::aggregate_dir_totals` rolls them
/// up.
pub fn observe_unit_with_dirs(
    store: Option<&Path>,
    path: &Path,
    exclusions: &[PathBuf],
    observed_at: u64,
    coverage: &crate::fs_events::EventCoverage,
    allow_reuse: bool,
    allow_partial: bool,
) -> (UnitObservation, Option<Vec<crate::report::DirRollup>>) {
    let mounts = sealed_mounts_under(path);
    let exclusions = with_sealed(exclusions, &mounts);
    let exclusions = exclusions.as_slice();
    if allow_reuse
        && let Some(mut folded) = reuse_folded_measurement(store, path, exclusions, coverage)
    {
        // The store's identified units are replayed as a whole, so every
        // sealed mount must replay too; one changed mount walks them all
        // (the adapter needs their directory rows again).
        let unit_path = path.display().to_string();
        let stored = store
            .map(|dir| crate::growth::volume_stamps_for(dir, &unit_path))
            .unwrap_or_default();
        let all_sealed_reusable = mounts.iter().all(|m| reuse_sealed(&stored, m).is_some());
        if std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty()) {
            eprintln!(
                "[xtrace] unit {} root_rows=reused sealed={} all_sealed_reusable={all_sealed_reusable}",
                path.display(),
                mounts.len()
            );
        }
        if all_sealed_reusable {
            crate::work_counters::record_cache_hit();
            add_sealed(
                &mut folded,
                &sealed_parts(store, path, &mounts, exclusions, observed_at, false),
            );
            return (UnitObservation::Unit(folded), None);
        }
    }
    match access(path) {
        UnitAccess::Absent => (UnitObservation::Absent, None),
        UnitAccess::Unreadable(why) => (UnitObservation::Unreadable(why), None),
        UnitAccess::Measurable => {
            if allow_partial
                && mounts.is_empty()
                && let Some((folded, dirs)) = partial_measure(
                    store,
                    path,
                    exclusions,
                    observed_at,
                    coverage,
                    Some((STORE_WORKTREE_ID, path)),
                )
            {
                return (UnitObservation::Unit(folded), Some(dirs));
            }
            let (row, mut dirs, stamps, complete) = crate::walk::resize_artifact_stamped(
                path,
                ArtifactKind::Unknown,
                observed_at,
                Some((STORE_WORKTREE_ID, path)),
                exclusions,
                store.is_some(),
            );
            crate::work_counters::record_cache_miss();
            let mut folded = FoldedUnit {
                sealed_walked_at: None,
                reused: false,
                bytes: row.bytes,
                hardlinked: row.hardlinked,
                mtime_max: row.mtime_max,
                complete,
            };
            // A build store's incomplete fold is never stored (its
            // adapter replays the store whole); a drilled unit's is, as in
            // `measure`, marked as a lower bound. The store's own
            // directory rows are still handed back so the adapter can
            // identify whatever *was* read -- best-effort, same as any
            // other unreadable subdirectory.
            if let Some(dir) = store
                && (complete || allow_partial)
            {
                record_folded_measurement(dir, path, exclusions, observed_at, &folded, &stamps);
            }
            // The sealed mounts come after the unit's own rows are
            // recorded: the stored root row is the writable part only,
            // and the parts are added on every read (a row that already
            // held them would be doubled on the next reuse).
            let parts = sealed_parts(store, path, &mounts, exclusions, observed_at, true);
            for part in &parts {
                dirs.extend(part.dirs.iter().cloned());
            }
            add_sealed(&mut folded, &parts);
            (UnitObservation::Unit(folded), Some(dirs))
        }
    }
}

/// The pseudo-worktree id a store's own directory rows are recorded
/// under: they are relative to the store, never to a checkout.
pub const STORE_WORKTREE_ID: &str = "build-store";

/// Bounded, stat-only folded byte total for `path` (file or directory),
/// returning `(bytes, mtime_max, truncated)`.
///
/// This is deliberately *not* [`measure`]'s parallel-pool machinery:
/// that is tuned for a handful of potentially huge artifact roots, not
/// hundreds of small per-session directories, and spinning up its thread
/// pool that many times would itself be the "unacceptable scanning cost"
/// the agent epic guards against. Reads directory names and `stat` calls
/// only -- never file contents. Bounded by `max_entries`; a directory
/// that hits the bound is reported truncated rather than silently
/// under-measured. The same `truncated` signal also covers an
/// unreadable subdirectory a few levels inside `path` (e.g. `chmod
/// 000`, `path` itself is not this): the walk still sums whatever it
/// *can* read, but that sum is a lower bound, not the tree's true size,
/// so it must never be mistaken for a complete measurement -- neither
/// displayed as one nor fed into a growth/regrowth delta
/// (`.oh/guardrails/coverage-changes-are-not-storage-changes.md`).
///
/// It lives here rather than in `agents/mod.rs` because this module is
/// the one place allowed to traverse on the ordinary report path
/// (`.oh/guardrails/no-second-traversal-on-report-path.md`); adapters
/// reach it through `agents::IdentifyCtx::folded_bytes`, never directly.
pub fn folded_bytes_bounded(path: &Path, max_entries: usize) -> (u64, u64, bool) {
    let (bytes, mtime_max, truncated, _stamps) = folded_bytes_bounded_stamped(path, max_entries);
    (bytes, mtime_max, truncated)
}

/// [`folded_bytes_bounded`] plus one [`crate::walk::DirStamp`] per
/// directory it listed, taken from the `stat` that listing already did.
///
/// This is the agent family's half of the bargain
/// [`crate::walk::resize_artifact_stamped`] strikes for the external
/// family: the pass that pays for a fold hands back exactly what a later
/// pass needs in order to decide, from `stat`s alone, whether that fold
/// still describes the tree. A *truncated* fold returns no stamps at
/// all -- a measurement that stopped at the bound does not describe the
/// whole subtree, so it must never anchor a reuse.
pub fn folded_bytes_bounded_stamped(
    path: &Path,
    max_entries: usize,
) -> (u64, u64, bool, Vec<crate::walk::DirStamp>) {
    let Ok(meta) = crate::fs_gate::symlink_metadata(path) else {
        return (0, 0, false, Vec::new());
    };
    if meta.is_file() {
        return (meta.len(), mtime_secs(&meta), false, Vec::new());
    }
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return (0, 0, false, Vec::new());
    }
    let mut total = 0u64;
    let mut mtime_max = mtime_secs(&meta);
    let mut stack = vec![(path.to_path_buf(), meta)];
    let mut seen = 0usize;
    let mut truncated = false;
    let mut stamps: Vec<crate::walk::DirStamp> = Vec::new();
    while let Some((dir, dir_meta)) = stack.pop() {
        crate::work_counters::record_dir_listed();
        let Ok(rd) = crate::fs_gate::read_dir(&dir) else {
            // An unreadable subdirectory (e.g. `chmod 000` a few levels
            // in; the root itself is handled by the caller before this
            // function is ever reached) makes `total` a partial sum, not
            // the tree's true size. Treated exactly like hitting
            // `max_entries`: `truncated` is this function's one
            // incompleteness signal, and a silent `continue` here used
            // to leave it untouched, so the caller received a smaller
            // number with nothing to say it was not the whole answer
            // (`.oh/guardrails/coverage-changes-are-not-storage-changes.md`).
            truncated = true;
            continue;
        };
        let (mtime_ns, ctime_ns) = stamp_ns(&dir_meta);
        stamps.push(crate::walk::DirStamp {
            path: dir.clone(),
            mtime_ns,
            ctime_ns,
            access_atime: {
                use crate::fs_gate::MetadataExt;
                (dir_meta.atime() > 0).then_some(dir_meta.atime() as u64)
            },
            access_observed_at: Some(crate::entities::now()),
            own_bytes: 0,
            files_mtime_max: 0,
            shared_inode: false,
            linked: Vec::new(),
            incomplete: false,
            pending: false,
        });
        let mut here = 0u64;
        for entry in rd {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    truncated = true;
                    continue;
                }
            };
            seen += 1;
            if seen > max_entries {
                truncated = true;
                break;
            }
            let m = match entry.metadata() {
                Ok(meta) => meta,
                Err(_) => {
                    truncated = true;
                    continue;
                }
            };
            here += 1;
            mtime_max = mtime_max.max(mtime_secs(&m));
            if m.is_dir() && !m.file_type().is_symlink() {
                stack.push((entry.path(), m));
            } else if m.is_file() {
                total += m.len();
            }
        }
        crate::work_counters::record_files_statted(here);
        if truncated {
            break;
        }
    }
    if truncated {
        stamps.clear();
    }
    (total, mtime_max, truncated, stamps)
}

pub fn mtime_secs(meta: &crate::fs_gate::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_path_is_absent_not_zero_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(access(&tmp.path().join("nope")), UnitAccess::Absent);
    }

    #[test]
    fn an_unlistable_directory_is_unreadable_not_empty() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("locked");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("f"), b"x").unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        let got = access(&dir);
        // Restore before asserting so the tempdir can always clean up.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Running as root defeats the permission bit; only assert the
        // distinction where the platform actually enforces it.
        if !matches!(got, UnitAccess::Measurable) {
            assert!(matches!(got, UnitAccess::Unreadable(_)), "{got:?}");
        }
    }

    #[test]
    fn a_measurable_directory_folds_its_bytes_and_counts_the_work() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a"), b"12345").unwrap();
        assert_eq!(access(tmp.path()), UnitAccess::Measurable);
        let (folded, counted) = crate::work_counters::measured(|| {
            measure(
                None,
                tmp.path(),
                &[],
                1_000,
                &crate::fs_events::EventCoverage::untrusted(),
            )
        });
        assert!(folded.bytes >= 5);
        assert!(counted.identification_cache_misses >= 1);
    }

    /// R19: a read-only filesystem mounted under the unit is reused
    /// from its `statfs` stamp alone -- no event window, no listing --
    /// and re-walked the moment the stamp moves. Staged through the
    /// `fs_space::testing` seam: the temp dir's `sealed/` child answers
    /// as a read-only mount on another device.
    #[test]
    fn a_sealed_read_only_mount_is_reused_from_its_stamp_without_a_window() {
        use crate::fs_events::EventCoverage;
        use crate::fs_gate::fs_space::{VolumeStamp, testing};
        let _serial = SEALED.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let unit = crate::fs_gate::canonicalize(tmp.path())
            .unwrap()
            .join("unit");
        let sealed = unit.join("sealed");
        std::fs::create_dir_all(sealed.join("a/b")).unwrap();
        std::fs::write(
            sealed.join("a/b/big"),
            crate::fs_gate::settle::noise(65_536),
        )
        .unwrap();
        std::fs::write(sealed.join("top"), crate::fs_gate::settle::noise(1024)).unwrap();
        std::fs::write(unit.join("loose"), crate::fs_gate::settle::noise(512)).unwrap();
        testing::set(
            &unit,
            VolumeStamp {
                device: 1,
                read_only: false,
                total_blocks: 1000,
                root_ino: 2,
                root_mtime: 1,
            },
        );
        let stamp = VolumeStamp {
            device: 2,
            read_only: true,
            total_blocks: 4_000,
            root_ino: 2,
            root_mtime: 1_000,
        };
        testing::set(&sealed, stamp);

        crate::fs_gate::settle::settle();
        // First pass: everything is walked, the sealed part is stamped.
        let (first, counted) = crate::work_counters::measured(|| {
            measure(
                Some(store.path()),
                &unit,
                &[],
                1_000,
                &EventCoverage::untrusted(),
            )
        });
        assert!(first.bytes >= 65_536 + 1024 + 512, "{}", first.bytes);
        assert!(counted.files_statted >= 3, "{counted:?}");
        let stamps = crate::growth::volume_stamps_for(store.path(), &unit.display().to_string());
        assert_eq!(stamps.len(), 1, "{stamps:?}");
        assert_eq!(stamps[0].mount_path, sealed.display().to_string());

        // Second pass, no event window: the writable part is re-walked
        // (one file), the sealed part is not listed at all.
        let (second, counted) = crate::work_counters::measured(|| {
            measure(
                Some(store.path()),
                &unit,
                &[],
                2_000,
                &EventCoverage::untrusted(),
            )
        });
        assert_eq!(second.bytes, first.bytes);
        assert!(
            counted.files_statted < 3,
            "the sealed mount must not be statted again: {counted:?}"
        );
        assert!(counted.identification_cache_hits >= 1, "{counted:?}");

        // The stamp moves (a runtime was replaced in place -- a new
        // image, a new root directory): walked again.
        testing::set(
            &sealed,
            VolumeStamp {
                root_mtime: 2_000,
                ..stamp
            },
        );
        std::fs::write(sealed.join("a/b/more"), crate::fs_gate::settle::noise(2048)).unwrap();
        crate::fs_gate::settle::settle();
        let (third, counted) = crate::work_counters::measured(|| {
            measure(
                Some(store.path()),
                &unit,
                &[],
                3_000,
                &EventCoverage::untrusted(),
            )
        });
        assert!(
            third.bytes > first.bytes,
            "the re-walk must see the new file: {} vs {}",
            third.bytes,
            first.bytes
        );
        assert!(counted.files_statted >= 4, "{counted:?}");
        testing::clear();
    }

    static SEALED: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The reuse, end to end, and its gate: a second measurement of a
    /// tree an event window vouches for costs nothing at all, the same
    /// tree with no window is re-measured, and a file added under it is
    /// a miss even with a window.
    #[test]
    fn an_unchanged_unit_is_reused_only_under_a_trusted_event_window() {
        use crate::fs_events::EventCoverage;
        let tmp = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let unit = tmp.path().join("cache");
        std::fs::create_dir_all(unit.join("a/b")).unwrap();
        std::fs::write(unit.join("a/b/f"), crate::fs_gate::settle::noise(4096)).unwrap();
        std::fs::write(unit.join("a/g"), crate::fs_gate::settle::noise(4096)).unwrap();

        crate::fs_gate::settle::settle();
        let none = EventCoverage::untrusted();
        let first = measure(Some(store.path()), &unit, &[], 1_000, &none);
        assert!(first.bytes >= 8192, "{}", first.bytes);
        assert!(!first.reused);

        // No window: the stored rows exist and are still ignored.
        let (again, cost) = crate::work_counters::measured(|| {
            measure(Some(store.path()), &unit, &[], 2_000, &none)
        });
        assert!(
            !again.reused && cost.dirs_listed > 0,
            "without event coverage a stored measurement must not be reused"
        );

        // A window over the unit's parent that reports nothing under it.
        let quiet = EventCoverage::trusted(tmp.path().to_path_buf(), Vec::new(), 1_000);
        let (second, cost) = crate::work_counters::measured(|| {
            measure(Some(store.path()), &unit, &[], 3_000, &quiet)
        });
        assert_eq!(second.bytes, first.bytes);
        assert!(second.reused);
        assert_eq!(
            (cost.dirs_listed, cost.files_statted),
            (0, 0),
            "a unit the window vouches for must cost no listing and no stat"
        );
        assert_eq!(
            cost.identification_cache_hits, 1,
            "the answer must come from the stored rows, not a re-walk"
        );

        // The same quiet window cannot vouch for rows written after it
        // opened -- that gap is where a skipped pass hides.
        let stale = EventCoverage::trusted(tmp.path().to_path_buf(), Vec::new(), 9_999);
        assert!(
            reuse_folded_measurement(Some(store.path()), &unit, &[], &stale).is_none(),
            "rows older than the window must not be replayed"
        );

        // A window that names a changed path under the unit is a miss
        // even though the bytes on disk did not move.
        let noisy = EventCoverage::trusted(tmp.path().to_path_buf(), vec![unit.join("a/b")], 1_000);
        assert!(
            reuse_folded_measurement(Some(store.path()), &unit, &[], &noisy).is_none(),
            "an event under the unit must refuse the reuse"
        );

        std::fs::write(unit.join("a/b/new"), crate::fs_gate::settle::noise(4096)).unwrap();
        crate::fs_gate::settle::settle();
        let (third, cost) = crate::work_counters::measured(|| {
            measure(Some(store.path()), &unit, &[], 4_000, &noisy)
        });
        assert!(third.bytes > second.bytes, "a new file must be measured");
        assert!(
            cost.dirs_listed > 0,
            "a changed directory must be re-listed"
        );
    }

    fn fixture(unit: &Path) {
        let noise = crate::fs_gate::settle::noise;
        for (rel, len) in [
            ("a/x", 4096),
            ("a/sub/y", 8192),
            ("b/z", 4096),
            ("c/w", 12288),
            ("root-file", 4096),
        ] {
            let p = unit.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, noise(len)).unwrap();
        }
        crate::fs_gate::settle::settle();
    }

    /// The answer a fresh store gives: a full walk, nothing replayed.
    fn golden(unit: &Path) -> (u64, u64, bool, bool, String) {
        let fresh = tempfile::tempdir().unwrap();
        let none = crate::fs_events::EventCoverage::untrusted();
        let f = measure(Some(fresh.path()), unit, &[], 9_000, &none);
        let (obs, dirs) =
            observe_unit_with_dirs(Some(fresh.path()), unit, &[], 9_000, &none, false, false);
        assert!(matches!(obs, UnitObservation::Unit(_)));
        let mut children = crate::drilldown::children_of(unit, dirs.unwrap(), f.bytes, 9_000, 10);
        for child in &mut children {
            child.access_evidence = None;
        }
        (
            f.bytes,
            f.mtime_max,
            f.hardlinked,
            f.complete,
            format!("{children:?}"),
        )
    }

    fn shape(
        f: &FoldedUnit,
        unit: &Path,
        dirs: Option<Vec<crate::report::DirRollup>>,
    ) -> (u64, u64, bool, bool, String) {
        let mut children = crate::drilldown::children_of(unit, dirs.unwrap(), f.bytes, 9_000, 10);
        for child in &mut children {
            child.access_evidence = None;
        }
        (
            f.bytes,
            f.mtime_max,
            f.hardlinked,
            f.complete,
            format!("{children:?}"),
        )
    }

    /// #181. Tempting wrong patches: (1) keep walking the whole unit
    /// whenever any event lands under it (correct, and 163k files of
    /// `~/Library/Caches` every pass); (2) replay the stored total of
    /// every subfolder the window does not name, which misses a folder
    /// renamed into place (the event is on the parent) and double-counts
    /// a file hardlinked across two subfolders. A changed unit re-walks
    /// only the changed subfolder, and its total, newest mtime and
    /// depth-2 drilldown equal a fresh full walk's, pass after pass.
    #[test]
    fn a_changed_unit_rewalks_only_the_changed_subfolder_and_matches_a_full_walk() {
        use crate::fs_events::EventCoverage;
        let _guard = SEALED.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let unit = tmp.path().join("cache");
        fixture(&unit);
        let none = EventCoverage::untrusted();
        let (_, dirs) =
            observe_unit_with_dirs(Some(store.path()), &unit, &[], 1_000, &none, true, true);
        let initial_dirs = dirs.expect("initial child metadata");
        let initial_c_access = initial_dirs
            .iter()
            .find(|d| d.rel_path == "c")
            .map(|d| (d.access_atime, d.access_observed_at));

        // Pass 2: a file lands in `b`; the window names `b`.
        std::fs::write(unit.join("b/new"), crate::fs_gate::settle::noise(8192)).unwrap();
        crate::fs_gate::settle::settle();
        let window = EventCoverage::trusted(tmp.path().to_path_buf(), vec![unit.join("b")], 500);
        let ((obs, dirs), cost) = crate::work_counters::measured(|| {
            observe_unit_with_dirs(Some(store.path()), &unit, &[], 2_000, &window, true, true)
        });
        let UnitObservation::Unit(f) = obs else {
            panic!("not measured")
        };
        assert_eq!(cost.subtrees_reused, 2, "a and c are replayed");
        // The readability probe, the root and `b`; a full walk lists six.
        assert_eq!(
            cost.dirs_listed, 3,
            "only the root and b are listed: {cost:?}"
        );
        assert_eq!(
            dirs.as_ref()
                .and_then(|rows| rows.iter().find(|d| d.rel_path == "c"))
                .map(|d| (d.access_atime, d.access_observed_at)),
            initial_c_access,
            "partial replay preserves the original atime sample time for unchanged children"
        );
        assert_eq!(shape(&f, &unit, dirs), golden(&unit));

        // Pass 3: `a/sub` changes. `b` (re-walked on pass 2) is now
        // replayed from the rows pass 2 wrote.
        std::fs::write(unit.join("a/sub/more"), crate::fs_gate::settle::noise(4096)).unwrap();
        crate::fs_gate::settle::settle();
        let window =
            EventCoverage::trusted(tmp.path().to_path_buf(), vec![unit.join("a/sub")], 1_500);
        let ((obs, dirs), cost) = crate::work_counters::measured(|| {
            observe_unit_with_dirs(Some(store.path()), &unit, &[], 3_000, &window, true, true)
        });
        let UnitObservation::Unit(f) = obs else {
            panic!("not measured")
        };
        assert_eq!(cost.subtrees_reused, 2, "b and c are replayed");
        assert_eq!(shape(&f, &unit, dirs), golden(&unit));

        // The plain (non-drill) path agrees.
        let (plain, cost) = crate::work_counters::measured(|| {
            measure(
                Some(store.path()),
                &unit,
                &[],
                4_000,
                &EventCoverage::trusted(tmp.path().to_path_buf(), vec![unit.join("c")], 2_500),
            )
        });
        assert_eq!(cost.subtrees_reused, 2);
        assert_eq!(plain.bytes, golden(&unit).0);

        // A directory renamed into `c`'s place: the event is on the unit
        // root only, so the window does not name `c`; the stamp does.
        std::fs::rename(unit.join("c"), tmp.path().join("old-c")).unwrap();
        std::fs::create_dir_all(tmp.path().join("other")).unwrap();
        std::fs::write(
            tmp.path().join("other/big"),
            crate::fs_gate::settle::noise(65536),
        )
        .unwrap();
        crate::fs_gate::settle::settle();
        std::fs::rename(tmp.path().join("other"), unit.join("c")).unwrap();
        let window = EventCoverage::trusted(tmp.path().to_path_buf(), vec![unit.clone()], 3_500);
        let (obs, dirs) =
            observe_unit_with_dirs(Some(store.path()), &unit, &[], 5_000, &window, true, true);
        let UnitObservation::Unit(f) = obs else {
            panic!("not measured")
        };
        assert_eq!(
            shape(&f, &unit, dirs),
            golden(&unit),
            "a renamed-in folder was replayed"
        );

        // A hardlink inside one replayed subfolder: replayed, counted once.
        std::fs::write(
            unit.join("a/sub/linked"),
            crate::fs_gate::settle::noise(16384),
        )
        .unwrap();
        std::fs::hard_link(unit.join("a/sub/linked"), unit.join("a/sub/linked2")).unwrap();
        crate::fs_gate::settle::settle();
        let window = EventCoverage::trusted(tmp.path().to_path_buf(), vec![unit.join("a")], 4_000);
        let _ = observe_unit_with_dirs(Some(store.path()), &unit, &[], 5_500, &window, true, true);
        let window = EventCoverage::trusted(tmp.path().to_path_buf(), vec![unit.join("b")], 5_000);
        let ((obs, dirs), cost) = crate::work_counters::measured(|| {
            observe_unit_with_dirs(Some(store.path()), &unit, &[], 5_800, &window, true, true)
        });
        let UnitObservation::Unit(f) = obs else {
            panic!("not measured")
        };
        // `a` holds a linked file, so it is walked (both links inside
        // it); only `c` is replayed.
        assert_eq!(cost.subtrees_reused, 1, "c is replayed, a is walked");
        assert!(f.hardlinked);
        assert_eq!(
            shape(&f, &unit, dirs),
            golden(&unit),
            "an inner link was counted twice"
        );

        // A hardlink across two subfolders: never summed twice.
        std::fs::hard_link(unit.join("a/x"), unit.join("b/link")).unwrap();
        crate::fs_gate::settle::settle();
        let window = EventCoverage::trusted(tmp.path().to_path_buf(), vec![unit.join("b")], 5_700);
        let (obs, dirs) =
            observe_unit_with_dirs(Some(store.path()), &unit, &[], 6_000, &window, true, true);
        let UnitObservation::Unit(f) = obs else {
            panic!("not measured")
        };
        assert_eq!(
            shape(&f, &unit, dirs),
            golden(&unit),
            "a hardlink was counted twice"
        );
    }

    #[test]
    fn a_large_linked_remainder_is_measured_once_and_matches_a_full_walk() {
        use crate::fs_events::EventCoverage;
        let _guard = SEALED.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let unit = tmp.path().join("cache");
        std::fs::create_dir_all(unit.join("small")).unwrap();
        std::fs::write(unit.join("small/file"), b"small").unwrap();
        for i in 0..100 {
            let dir = unit.join(format!("large/dir-{i}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("file"), crate::fs_gate::settle::noise(4096)).unwrap();
        }
        // The unvisited alias makes the partial walk unable to prove
        // its link accounting, so the old path walked the large tree twice.
        std::fs::hard_link(
            unit.join("large/dir-0/file"),
            tmp.path().join("outside-link"),
        )
        .unwrap();
        crate::fs_gate::settle::settle();
        let _ = observe_unit_with_dirs(
            Some(store.path()),
            &unit,
            &[],
            1_000,
            &EventCoverage::untrusted(),
            true,
            true,
        );
        let window =
            EventCoverage::trusted(tmp.path().to_path_buf(), vec![unit.join("large")], 500);
        let ((obs, dirs), cost) = crate::work_counters::measured(|| {
            observe_unit_with_dirs(Some(store.path()), &unit, &[], 2_000, &window, true, true)
        });
        let UnitObservation::Unit(folded) = obs else {
            panic!("not measured")
        };
        assert_eq!(shape(&folded, &unit, dirs), golden(&unit));
        assert!(
            cost.dirs_listed <= 110,
            "large remainder walked more than once: {cost:?}"
        );
    }

    /// Tempting wrong patches: (1) store nothing for a unit with an
    /// unreadable folder, so it is walked whole every pass (the
    /// maintainer's `~/Library/Caches`); (2) store it as a complete
    /// measurement, so it is replayed whole as if it were exact. The
    /// readable subfolders are replayed, the unreadable one is tried again,
    /// the answer stays a lower bound equal to a fresh walk's, and the
    /// stored rows are never replayed whole.
    #[test]
    fn a_unit_with_an_unreadable_folder_replays_its_readable_ones() {
        use crate::fs_events::EventCoverage;
        use std::os::unix::fs::PermissionsExt;
        let _guard = SEALED.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let unit = tmp.path().join("cache");
        fixture(&unit);
        std::fs::create_dir_all(unit.join("locked/inner")).unwrap();
        std::fs::set_permissions(unit.join("locked"), std::fs::Permissions::from_mode(0o000))
            .unwrap();
        let none = EventCoverage::untrusted();
        let first = measure(Some(store.path()), &unit, &[], 1_000, &none);
        assert!(!first.complete);
        let quiet = EventCoverage::trusted(tmp.path().to_path_buf(), Vec::new(), 500);
        assert!(
            reuse_folded_measurement(Some(store.path()), &unit, &[], &quiet).is_none(),
            "a lower bound was replayed whole"
        );
        std::fs::write(unit.join("b/new"), crate::fs_gate::settle::noise(8192)).unwrap();
        crate::fs_gate::settle::settle();
        let window = EventCoverage::trusted(tmp.path().to_path_buf(), vec![unit.join("b")], 500);
        let (second, cost) = crate::work_counters::measured(|| {
            measure(Some(store.path()), &unit, &[], 2_000, &window)
        });
        assert_eq!(cost.subtrees_reused, 2, "a and c are replayed: {cost:?}");
        let fresh = tempfile::tempdir().unwrap();
        let golden = measure(Some(fresh.path()), &unit, &[], 9_000, &none);
        std::fs::set_permissions(unit.join("locked"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        assert_eq!(
            (second.bytes, second.mtime_max, second.complete),
            (golden.bytes, golden.mtime_max, golden.complete)
        );
        assert!(!second.complete);
    }

    /// #181. Tempting wrong patches: (1) require an event window before
    /// replaying a store of sealed volumes, so a lost window or another
    /// swamp's write re-walks 1.77M simulator files; (2) vouch for any
    /// unit whose sealed mounts hold, missing a file written beside them.
    /// Only a directory of nothing but unchanged sealed mounts, itself
    /// unchanged, is vouched for, and its replay equals the first walk.
    #[test]
    fn a_store_of_only_sealed_volumes_is_vouched_for_by_its_stamps() {
        use crate::fs_events::EventCoverage;
        use crate::fs_gate::fs_space::{VolumeStamp, testing};
        let _serial = SEALED.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let unit = crate::fs_gate::canonicalize(tmp.path())
            .unwrap()
            .join("Volumes");
        let sealed = unit.join("iOS_1");
        std::fs::create_dir_all(sealed.join("a")).unwrap();
        std::fs::write(sealed.join("a/big"), crate::fs_gate::settle::noise(65_536)).unwrap();
        testing::set(
            &unit,
            VolumeStamp {
                device: 1,
                read_only: false,
                total_blocks: 1000,
                root_ino: 2,
                root_mtime: 1,
            },
        );
        let stamp = VolumeStamp {
            device: 2,
            read_only: true,
            total_blocks: 4_000,
            root_ino: 2,
            root_mtime: 1_000,
        };
        testing::set(&sealed, stamp);
        crate::fs_gate::settle::settle();
        let none = EventCoverage::untrusted();
        let (first, _) =
            observe_unit_with_dirs(Some(store.path()), &unit, &[], 1_000, &none, false, false);
        let UnitObservation::Unit(first) = first else {
            panic!("not measured")
        };
        assert!(sealed_only_unchanged(Some(store.path()), &unit));

        let vouched = EventCoverage::trusted(unit.clone(), Vec::new(), 0);
        let ((again, _), cost) = crate::work_counters::measured(|| {
            observe_unit_with_dirs(Some(store.path()), &unit, &[], 2_000, &vouched, true, false)
        });
        let UnitObservation::Unit(again) = again else {
            panic!("not measured")
        };
        assert!(again.reused);
        assert_eq!(cost.files_statted, 0, "{cost:?}");
        assert_eq!(
            (again.bytes, again.mtime_max, again.complete),
            (first.bytes, first.mtime_max, first.complete)
        );

        // A volume replaced (new stamp): not vouched for.
        testing::set(
            &sealed,
            VolumeStamp {
                root_mtime: 2_000,
                ..stamp
            },
        );
        assert!(!sealed_only_unchanged(Some(store.path()), &unit));
        testing::set(&sealed, stamp);
        assert!(sealed_only_unchanged(Some(store.path()), &unit));
        // A file beside the volumes: not vouched for.
        std::fs::write(unit.join("loose"), b"x").unwrap();
        assert!(!sealed_only_unchanged(Some(store.path()), &unit));
        testing::clear();
    }

    /// Audit round 2 setup: a unit holding one staged sealed volume (a
    /// subfolder answered as a read-only mount), walked once at 1_000.
    fn rev2_sealed_unit(tmp: &Path) -> (PathBuf, PathBuf, crate::fs_gate::fs_space::VolumeStamp) {
        use crate::fs_gate::fs_space::{VolumeStamp, testing};
        let unit = crate::fs_gate::canonicalize(tmp).unwrap().join("Volumes");
        let sealed = unit.join("iOS_1");
        std::fs::create_dir_all(sealed.join("a")).unwrap();
        std::fs::write(sealed.join("a/big"), crate::fs_gate::settle::noise(65_536)).unwrap();
        testing::set(
            &unit,
            VolumeStamp {
                device: 1,
                read_only: false,
                total_blocks: 1000,
                root_ino: 2,
                root_mtime: 1,
            },
        );
        let stamp = VolumeStamp {
            device: 2,
            read_only: true,
            total_blocks: 4_000,
            root_ino: 2,
            root_mtime: 1_000,
        };
        testing::set(&sealed, stamp);
        crate::fs_gate::settle::settle();
        (unit, sealed, stamp)
    }

    fn rev2_unit(o: UnitObservation) -> FoldedUnit {
        let UnitObservation::Unit(u) = o else {
            panic!("not measured")
        };
        u
    }

    /// Audit round 2. Tempting wrong patch (the shipped one): re-stamp the
    /// sealed volume's stored row with the replaying pass's time, so the
    /// time the 1.77M files were actually walked is lost and nothing can
    /// say how old the measurement is (`--full` no longer re-walks it).
    #[test]
    fn rev2_a_vouched_replay_keeps_the_time_the_volume_was_walked() {
        use crate::fs_events::EventCoverage;
        let _serial = SEALED.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let (unit, _sealed, _) = rev2_sealed_unit(tmp.path());
        let none = EventCoverage::untrusted();
        let _ = observe_unit_with_dirs(Some(store.path()), &unit, &[], 1_000, &none, false, false);
        let vouched = EventCoverage::trusted(unit.clone(), Vec::new(), 0);
        let (again, _) =
            observe_unit_with_dirs(Some(store.path()), &unit, &[], 9_000, &vouched, true, false);
        assert!(rev2_unit(again).reused);
        let rows = crate::growth::volume_stamps_for(store.path(), &unit.display().to_string());
        crate::fs_gate::fs_space::testing::clear();
        assert_eq!(
            rows.iter().map(|r| r.observed_at).collect::<Vec<_>>(),
            vec![1_000],
            "the stored volume row no longer says when it was walked"
        );
    }

    /// Audit round 2. Tempting wrong patches: vouch on the volumes' stamps
    /// alone (ignoring the folder's own ctime), or keep vouching after a
    /// volume is unmounted (its mount point is then an empty folder on the
    /// parent volume). After each mutation the unit is not vouched for and
    /// an observe with no window equals a store-less fresh walk.
    #[test]
    fn rev2_sealed_vouching_stops_on_chmod_and_unmount_and_matches_a_fresh_walk() {
        use crate::fs_events::EventCoverage;
        use std::os::unix::fs::PermissionsExt;
        let _serial = SEALED.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let (unit, sealed, _) = rev2_sealed_unit(tmp.path());
        let none = EventCoverage::untrusted();
        let _ = observe_unit_with_dirs(Some(store.path()), &unit, &[], 1_000, &none, false, false);
        assert!(sealed_only_unchanged(Some(store.path()), &unit));

        // Permissions flipped on the folder: its ctime moves.
        std::fs::set_permissions(&unit, std::fs::Permissions::from_mode(0o700)).unwrap();
        crate::fs_gate::settle::settle();
        let vouched_after_chmod = sealed_only_unchanged(Some(store.path()), &unit);
        let golden = |at: u64| {
            let (o, _) = observe_unit_with_dirs(None, &unit, &[], at, &none, false, false);
            let u = rev2_unit(o);
            (u.bytes, u.mtime_max, u.complete)
        };
        let (o, _) =
            observe_unit_with_dirs(Some(store.path()), &unit, &[], 2_000, &none, false, false);
        let o = rev2_unit(o);
        assert_eq!((o.bytes, o.mtime_max, o.complete), golden(2_000));

        // Unmounted: the stamp now says the parent volume.
        crate::fs_gate::fs_space::testing::set(
            &sealed,
            crate::fs_gate::fs_space::VolumeStamp {
                device: 1,
                read_only: false,
                total_blocks: 1000,
                root_ino: 77,
                root_mtime: 5,
            },
        );
        let vouched_after_unmount = sealed_only_unchanged(Some(store.path()), &unit);
        let (o, _) =
            observe_unit_with_dirs(Some(store.path()), &unit, &[], 3_000, &none, false, false);
        let o = rev2_unit(o);
        let g = golden(3_000);
        crate::fs_gate::fs_space::testing::clear();
        assert!(
            !vouched_after_chmod,
            "vouched after the folder's mode changed"
        );
        assert!(!vouched_after_unmount, "vouched with a volume unmounted");
        assert_eq!((o.bytes, o.mtime_max, o.complete), g);
    }

    /// Audit round 2. Tempting wrong patch: a vouched replay reports the
    /// stored bytes as complete. A volume with an unreadable folder was a
    /// lower bound when walked and stays labelled one when replayed.
    #[test]
    fn rev2_an_unreadable_sealed_volume_replays_as_a_lower_bound() {
        use crate::fs_events::EventCoverage;
        use std::os::unix::fs::PermissionsExt;
        let _serial = SEALED.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let (unit, sealed, _) = rev2_sealed_unit(tmp.path());
        std::fs::create_dir_all(sealed.join("root_only/x")).unwrap();
        std::fs::set_permissions(
            sealed.join("root_only"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        let none = EventCoverage::untrusted();
        let (first, _) =
            observe_unit_with_dirs(Some(store.path()), &unit, &[], 1_000, &none, false, false);
        let vouched = sealed_only_unchanged(Some(store.path()), &unit);
        let cov = EventCoverage::trusted(unit.clone(), Vec::new(), 0);
        let (again, _) =
            observe_unit_with_dirs(Some(store.path()), &unit, &[], 2_000, &cov, true, false);
        std::fs::set_permissions(
            sealed.join("root_only"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        crate::fs_gate::fs_space::testing::clear();
        let (first, again) = (rev2_unit(first), rev2_unit(again));
        assert!(!first.complete);
        assert!(vouched);
        assert!(again.reused);
        assert!(!again.complete, "a lower bound replayed as complete");
        assert_eq!(again.bytes, first.bytes);
    }

    /// #181 review round 2 (f). Tempting wrong patch: drop the "every
    /// entry is a sealed mount" check because the folder's own stamp
    /// already moves when an entry is added. A plain folder that was there
    /// at the first walk can change inside without moving the parent's
    /// stamp: the unit is then not vouched for.
    #[test]
    fn a_plain_folder_beside_the_volumes_stops_the_vouching() {
        use crate::fs_events::EventCoverage;
        let _serial = SEALED.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let (unit, _sealed, _) = rev2_sealed_unit(tmp.path());
        std::fs::create_dir_all(unit.join("plain")).unwrap();
        std::fs::write(unit.join("plain/f"), b"x").unwrap();
        crate::fs_gate::settle::settle();
        let none = EventCoverage::untrusted();
        let _ = observe_unit_with_dirs(Some(store.path()), &unit, &[], 1_000, &none, false, false);
        let before = std::fs::symlink_metadata(&unit).unwrap();
        std::fs::write(unit.join("plain/g"), crate::fs_gate::settle::noise(65_536)).unwrap();
        let after = std::fs::symlink_metadata(&unit).unwrap();
        assert_eq!(
            stamp_ns(&before),
            stamp_ns(&after),
            "precondition: the parent's stamp did not move"
        );
        let vouched = sealed_only_unchanged(Some(store.path()), &unit);
        crate::fs_gate::fs_space::testing::clear();
        assert!(!vouched, "a changed plain folder was vouched for");
    }

    /// CI run 36882714289 follow-up (#197). Tempting wrong patch: record
    /// a subfolder total taken while one of its files was still waiting
    /// for its blocks (ZFS before a commit). Writeback raises the bytes
    /// with no event under the folder, so a replay would keep the lower
    /// figure as exact. Such a subfolder gets no total; its siblings do.
    #[test]
    fn a_subfolder_with_pending_allocation_is_never_given_a_total() {
        let unit = PathBuf::from("/u");
        let stamp = |rel: &str, own: u64, pending: bool| crate::walk::DirStamp {
            path: if rel.is_empty() {
                unit.clone()
            } else {
                unit.join(rel)
            },
            mtime_ns: 1,
            ctime_ns: 1,
            access_atime: None,
            access_observed_at: None,
            own_bytes: own,
            files_mtime_max: 5,
            shared_inode: false,
            linked: Vec::new(),
            incomplete: false,
            pending,
        };
        let stamps = vec![
            stamp("", 0, false),
            stamp("a", 4096, true),
            stamp("b", 8192, false),
        ];
        let folded = FoldedUnit {
            reused: false,
            bytes: 12_288,
            hardlinked: false,
            mtime_max: 5,
            complete: true,
            sealed_walked_at: None,
        };
        let rows = folded_rows(&unit, &[], 1_000, &folded, &stamps, &[]).unwrap();
        let marked = |name: &str| {
            rows.iter()
                .any(|r| r.rel_dir == name && r.exclusions.ends_with(CHILD_TOTAL_MARK))
        };
        assert!(
            !marked("a"),
            "a pending subfolder was given a replayable total"
        );
        assert!(marked("b"));
    }

    /// A different exclusion set describes different bytes, so it must
    /// not be answered from a measurement taken under the old one.
    #[test]
    fn a_changed_exclusion_set_is_a_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let unit = tmp.path().join("cache");
        std::fs::create_dir_all(unit.join("nested")).unwrap();
        std::fs::write(unit.join("nested/f"), crate::fs_gate::settle::noise(8192)).unwrap();
        let quiet =
            crate::fs_events::EventCoverage::trusted(tmp.path().to_path_buf(), Vec::new(), 1_000);
        let all = measure(Some(store.path()), &unit, &[], 1_000, &quiet);
        let excluded = measure(
            Some(store.path()),
            &unit,
            &[unit.join("nested")],
            2_000,
            &quiet,
        );
        assert!(
            excluded.bytes < all.bytes,
            "excluding the only populated subtree must measure fewer bytes, got {} vs {}",
            excluded.bytes,
            all.bytes
        );
    }

    /// Adversarial audit of #181 subfolder replay (audit/v080-perf). Each
    /// case: pass 1 is a full walk that stores per-subfolder totals; the
    /// disk is mutated; pass 2 gets the event window FSEvents would hand
    /// it (directory-level events: the changed directory and its parent);
    /// its answer must equal a fresh full walk's.
    mod adversarial_181 {
        use super::*;
        use crate::fs_events::EventCoverage;

        type Shape = (u64, u64, bool, bool, String);

        fn incremental_after(
            tmp: &Path,
            unit: &Path,
            mutate: impl FnOnce(),
            events: &[PathBuf],
        ) -> (Shape, Shape, u64) {
            let store = tempfile::tempdir().unwrap();
            let none = EventCoverage::untrusted();
            let _ = observe_unit_with_dirs(Some(store.path()), unit, &[], 1_000, &none, true, true);
            mutate();
            crate::fs_gate::settle::settle();
            let window = EventCoverage::trusted(tmp.to_path_buf(), events.to_vec(), 500);
            let ((obs, dirs), cost) = crate::work_counters::measured(|| {
                observe_unit_with_dirs(Some(store.path()), unit, &[], 2_000, &window, true, true)
            });
            let UnitObservation::Unit(f) = obs else {
                panic!("not measured")
            };
            (shape(&f, unit, dirs), golden(unit), cost.subtrees_reused)
        }

        /// Tempting wrong patch: treat "no event at or under the
        /// subfolder and its own directory stamp unchanged" as proof that
        /// nothing it holds changed. A hard link made from OUTSIDE the
        /// unit to a file in a replayed subfolder fires an event only
        /// where the link was made; the file's link count changed, so a
        /// full walk now says the unit holds hard-linked files (the
        /// report then labels its bytes as possibly shared) while the
        /// replay keeps the stored `hardlinked = false`.
        #[test]
        #[ignore = "known limit (#181): a link made from outside the unit to a file in a \
                    replayed subfolder changes only that file's ctime and fires no event under \
                    the unit; the bytes stay exact, only the hardlinked label lags until the \
                    subfolder changes. Detecting it needs a stat of every replayed file."]
        fn a_hard_link_made_from_outside_into_a_replayed_subfolder() {
            let _guard = SEALED.lock().unwrap_or_else(|e| e.into_inner());
            let tmp = tempfile::tempdir().unwrap();
            let tmp_path = std::fs::canonicalize(tmp.path()).unwrap();
            let unit = tmp_path.join("cache");
            fixture(&unit);
            std::fs::create_dir_all(tmp_path.join("outside")).unwrap();
            let (got, want, reused) = incremental_after(
                &tmp_path,
                &unit,
                || {
                    std::fs::hard_link(unit.join("a/x"), tmp_path.join("outside/link")).unwrap();
                    // Something else in the unit changed, so the unit is
                    // re-measured (partially) this pass.
                    std::fs::write(unit.join("b/new"), crate::fs_gate::settle::noise(4096))
                        .unwrap();
                },
                &[tmp_path.join("outside"), unit.join("b"), unit.clone()],
            );
            assert!(reused > 0, "nothing was replayed");
            assert_eq!(got, want, "replay kept the old hard-link flag");
        }

        /// The reverse: the outside link is removed; the stored flag says
        /// hard-linked, the disk no longer does.
        #[test]
        fn a_hard_link_from_outside_removed_while_its_subfolder_is_replayed() {
            let _guard = SEALED.lock().unwrap_or_else(|e| e.into_inner());
            let tmp = tempfile::tempdir().unwrap();
            let tmp_path = std::fs::canonicalize(tmp.path()).unwrap();
            let unit = tmp_path.join("cache");
            fixture(&unit);
            std::fs::create_dir_all(tmp_path.join("outside")).unwrap();
            std::fs::hard_link(unit.join("a/x"), tmp_path.join("outside/link")).unwrap();
            crate::fs_gate::settle::settle();
            let (got, want, _) = incremental_after(
                &tmp_path,
                &unit,
                || {
                    std::fs::remove_file(tmp_path.join("outside/link")).unwrap();
                    std::fs::write(unit.join("b/new"), crate::fs_gate::settle::noise(4096))
                        .unwrap();
                },
                &[tmp_path.join("outside"), unit.join("b"), unit.clone()],
            );
            assert_eq!(got, want);
        }

        /// Case-only rename on APFS (case-insensitive): the event is on
        /// the unit root; the stored row is named `a`, the disk says `A`.
        #[test]
        fn a_case_only_rename_of_a_subfolder() {
            let _guard = SEALED.lock().unwrap_or_else(|e| e.into_inner());
            let tmp = tempfile::tempdir().unwrap();
            let tmp_path = std::fs::canonicalize(tmp.path()).unwrap();
            let unit = tmp_path.join("cache");
            fixture(&unit);
            let (got, want, _) = incremental_after(
                &tmp_path,
                &unit,
                || {
                    std::fs::rename(unit.join("a"), unit.join("A")).unwrap();
                },
                &[unit.clone(), tmp_path.clone()],
            );
            assert_eq!(got, want, "a case-only rename replayed the old name");
        }

        /// Directory replaced by a file, file replaced by a directory, a
        /// directory deleted and recreated with the same content, a
        /// symlink to a directory put in a subfolder's place, a change 40
        /// levels down: each must equal a full walk.
        #[test]
        fn replacements_and_deep_changes_equal_a_full_walk() {
            let _guard = SEALED.lock().unwrap_or_else(|e| e.into_inner());
            let noise = crate::fs_gate::settle::noise;
            type Case = (&'static str, fn(&Path, &Path), fn(&Path) -> Vec<PathBuf>);
            let cases: Vec<Case> = vec![
                (
                    "dir to file",
                    |u, _| {
                        std::fs::remove_file(u.join("c/w")).unwrap();
                        std::fs::remove_dir(u.join("c")).unwrap();
                        std::fs::write(u.join("c"), crate::fs_gate::settle::noise(4096)).unwrap();
                    },
                    |u| vec![u.to_path_buf()],
                ),
                (
                    "file to dir",
                    |u, _| {
                        std::fs::remove_file(u.join("root-file")).unwrap();
                        std::fs::create_dir_all(u.join("root-file")).unwrap();
                        std::fs::write(u.join("root-file/q"), crate::fs_gate::settle::noise(4096))
                            .unwrap();
                    },
                    |u| vec![u.to_path_buf(), u.join("root-file")],
                ),
                (
                    "deleted and recreated",
                    |u, _| {
                        std::fs::remove_file(u.join("c/w")).unwrap();
                        std::fs::remove_dir(u.join("c")).unwrap();
                        std::fs::create_dir_all(u.join("c")).unwrap();
                        std::fs::write(u.join("c/w"), crate::fs_gate::settle::noise(12288))
                            .unwrap();
                    },
                    // Coalesced: only the parent named.
                    |u| vec![u.to_path_buf()],
                ),
                (
                    "symlink in place",
                    |u, t| {
                        std::fs::rename(u.join("c"), t.join("moved-c")).unwrap();
                        std::os::unix::fs::symlink(t.join("moved-c"), u.join("c")).unwrap();
                    },
                    |u| vec![u.to_path_buf()],
                ),
                (
                    "moved out",
                    |u, t| {
                        std::fs::rename(u.join("c"), t.join("gone-c")).unwrap();
                    },
                    |u| vec![u.to_path_buf()],
                ),
                (
                    "deep change",
                    |u, _| {
                        let mut d = u.join("b");
                        for i in 0..40 {
                            d = d.join(format!("d{i}"));
                        }
                        std::fs::write(d.join("deep"), crate::fs_gate::settle::noise(8192))
                            .unwrap();
                    },
                    |u| {
                        let mut d = u.join("b");
                        for i in 0..40 {
                            d = d.join(format!("d{i}"));
                        }
                        vec![d.clone(), d.parent().unwrap().to_path_buf()]
                    },
                ),
            ];
            for (name, mutate, events) in cases {
                let tmp = tempfile::tempdir().unwrap();
                let t = std::fs::canonicalize(tmp.path()).unwrap();
                let unit = t.join("cache");
                fixture(&unit);
                let mut d = unit.join("b");
                for i in 0..40 {
                    d = d.join(format!("d{i}"));
                }
                std::fs::create_dir_all(&d).unwrap();
                std::fs::write(d.join("seed"), noise(4096)).unwrap();
                crate::fs_gate::settle::settle();
                let ev = events(&unit);
                let (got, want, _) = incremental_after(&t, &unit, || mutate(&unit, &t), &ev);
                assert_eq!(got, want, "{name}");
            }
        }

        /// Stored rows from a format that predates per-subfolder totals
        /// (no marked rows), or a missing root row, must never be
        /// replayed partially.
        #[test]
        fn rows_without_marks_or_without_a_root_are_never_replayed() {
            let _guard = SEALED.lock().unwrap_or_else(|e| e.into_inner());
            let tmp = tempfile::tempdir().unwrap();
            let t = std::fs::canonicalize(tmp.path()).unwrap();
            let unit = t.join("cache");
            fixture(&unit);
            let store = tempfile::tempdir().unwrap();
            let none = EventCoverage::untrusted();
            let _ = measure(Some(store.path()), &unit, &[], 1_000, &none);
            let key = unit.display().to_string();
            let rows = crate::growth::folded_rows_for(store.path(), &key);
            let digest = exclusions_digest(&[]);
            let old: Vec<_> = rows
                .iter()
                .cloned()
                .map(|mut r| {
                    if !r.rel_dir.is_empty() {
                        r.exclusions = digest.clone();
                        r.bytes = 0;
                    }
                    r
                })
                .filter(|r| !r.rel_dir.contains('\0'))
                .collect();
            crate::growth::store_folded_rows(store.path(), &key, &old).unwrap();
            let window = EventCoverage::trusted(t.clone(), vec![unit.join("b")], 500);
            assert!(
                partial_measure(Some(store.path()), &unit, &[], 2_000, &window, None).is_none()
            );
            let rootless: Vec<_> = rows
                .iter()
                .filter(|r| !r.rel_dir.is_empty())
                .cloned()
                .collect();
            crate::growth::store_folded_rows(store.path(), &key, &rootless).unwrap();
            assert!(
                partial_measure(Some(store.path()), &unit, &[], 2_000, &window, None).is_none()
            );
        }
    }
}
