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

/// One sealed mount's contribution, replayed from its stored stamp or
/// walked.
struct SealedPart {
    bytes: u64,
    hardlinked: bool,
    mtime_max: u64,
    complete: bool,
    reused: bool,
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
            observed_at,
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
        reused: false,
        bytes: row.bytes,
        hardlinked: row.hardlinked,
        mtime_max: row.mtime_max,
        complete,
    };
    // An incomplete measurement is never stored: writing it would either
    // overwrite a good prior measurement with a smaller partial one (a
    // fabricated shrink) or anchor a future reuse on rows that undercount
    // the tree. The key keeps whatever it last had; the caller decides
    // how to report this pass (`.oh/guardrails/coverage-changes-are-not-
    // storage-changes.md`).
    if let Some(dir) = store
        && complete
    {
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
    let unit_path = path.display().to_string();
    let digest = exclusions_digest(exclusions);
    let mut rows: Vec<crate::growth::FoldedRow> = Vec::with_capacity(stamps.len());
    let mut saw_root = false;
    for stamp in stamps {
        let rel = match stamp.path.strip_prefix(path) {
            Ok(r) => r.display().to_string(),
            // A stamp from outside the unit cannot be validated against
            // the unit root later, so it is not stored -- and, since a
            // directory the measurement listed would then go unwatched,
            // the whole measurement is not stored either.
            Err(_) => return,
        };
        saw_root |= rel.is_empty();
        rows.push(crate::growth::FoldedRow {
            unit_path: unit_path.clone(),
            rel_dir: rel,
            mtime_ns: stamp.mtime_ns,
            ctime_ns: stamp.ctime_ns,
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
        return;
    }
    for row in rows.iter_mut().filter(|r| r.rel_dir.is_empty()) {
        row.bytes = folded.bytes;
        row.hardlinked = folded.hardlinked;
        row.mtime_max = folded.mtime_max;
    }
    // A cache write that fails is a cache that will miss next time,
    // which is the correct outcome and not worth failing a report over.
    let _ = crate::growth::store_folded_rows(store, &unit_path, &rows);
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
                reused: false,
                bytes: row.bytes,
                hardlinked: row.hardlinked,
                mtime_max: row.mtime_max,
                complete,
            };
            // Same rule as `measure`: an incomplete fold is never stored
            // (it would overwrite a good prior measurement with a
            // partial one), but the store's own directory rows are
            // still handed back so the adapter can identify whatever
            // *was* read -- best-effort, same as any other unreadable
            // subdirectory.
            if let Some(dir) = store
                && complete
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
        });
        let mut here = 0u64;
        for entry in rd.flatten() {
            seen += 1;
            if seen > max_entries {
                truncated = true;
                break;
            }
            let Ok(m) = entry.metadata() else { continue };
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
        std::fs::write(sealed.join("a/b/big"), vec![b'x'; 65_536]).unwrap();
        std::fs::write(sealed.join("top"), vec![b'y'; 1024]).unwrap();
        std::fs::write(unit.join("loose"), vec![b'z'; 512]).unwrap();
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
        std::fs::write(sealed.join("a/b/more"), vec![b'w'; 2048]).unwrap();
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
        std::fs::write(unit.join("a/b/f"), vec![b'x'; 4096]).unwrap();
        std::fs::write(unit.join("a/g"), vec![b'y'; 4096]).unwrap();

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

        std::fs::write(unit.join("a/b/new"), vec![b'z'; 4096]).unwrap();
        let (third, cost) = crate::work_counters::measured(|| {
            measure(Some(store.path()), &unit, &[], 4_000, &noisy)
        });
        assert!(third.bytes > second.bytes, "a new file must be measured");
        assert!(
            cost.dirs_listed > 0,
            "a changed directory must be re-listed"
        );
    }

    /// A different exclusion set describes different bytes, so it must
    /// not be answered from a measurement taken under the old one.
    #[test]
    fn a_changed_exclusion_set_is_a_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let unit = tmp.path().join("cache");
        std::fs::create_dir_all(unit.join("nested")).unwrap();
        std::fs::write(unit.join("nested/f"), vec![b'x'; 8192]).unwrap();
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
}
