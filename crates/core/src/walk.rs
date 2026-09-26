//! Parallel discovery and attribution over a bounded thread pool.
//!
//! `git::discover` and `attribution::attribute` are both plain recursive
//! walks: one directory at a time, one thread, waiting on each `lstat`
//! and `read_dir` before starting the next. On a large tree that serial
//! syscall latency, not CPU, is almost the entire wall time (measured:
//! ~45% CPU on a 15 s walk). This module re-implements each walk's exact
//! predicates and stop conditions as jobs on a bounded worker pool, so
//! many directories are in flight at once, while leaving `git::discover`
//! and `attribution::attribute` themselves untouched for their existing
//! unit tests and as the reference semantics this module must match.
//!
//! The two walks use *different* stop conditions on purpose (discovery
//! only stops at [`STOP_DIRS`] plus `.git`; attribution stops at every
//! [`classify`]-matched name, a much longer list), so a project can be
//! discovered arbitrarily deep inside a directory — `vendor`, `.venv`,
//! `Pods` — that attribution nonetheless sizes as a single opaque unit
//! without descending into it. Fusing the two walks into one pass would
//! collapse that distinction; the two are kept as independent parallel
//! passes here to preserve it exactly.

use crate::attribution::{
    AttributionResult, Classified, allocated_bytes, classified_at, is_shared_cache_name,
};
use crate::entities::{Confidence, id_for};
use crate::fs_gate as fs;
use crate::fs_gate::MetadataExt;
use crate::git::{DiscoveredWorktree, classify_git_file, classify_main_checkout};
use crate::report::{
    ArtifactKind, ArtifactRow, DirRollup, FileRow, Source, UnownedReason, UnownedRow,
};
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// Same boundary `git::discover` stops recursion at: `.git` itself, plus
/// artifact roots that would otherwise be misread as nested project
/// roots. Duplicated here (rather than making `git::STOP_DIRS` `pub`)
/// because it is part of the exact-match contract this module documents,
/// not an implementation detail to import.
const STOP_DIRS: &[&str] = &["node_modules", "target", "dist", "build"];

/// A generic bounded job queue. `outstanding` counts every job pushed but
/// not yet finished (including one still being processed by a worker), so
/// the pool is done exactly when the queue is empty and `outstanding` is
/// zero.
///
/// The counter lives under the same mutex as the queue on purpose: an
/// earlier version kept it in an atomic and decremented/notified without
/// the lock, which allowed a lost wakeup — a waiter that had just read
/// `outstanding == 1` and was entering `wait` missed the `notify_all`
/// fired between those two steps and slept forever (#36). Every
/// transition and every notify now happens with the lock held, and the
/// wait is bounded so a waiter re-checks even if a notify is ever missed.
struct Pool<J> {
    state: Mutex<PoolState<J>>,
    cv: Condvar,
}

struct PoolState<J> {
    queue: VecDeque<J>,
    outstanding: usize,
}

impl<J: Send> Pool<J> {
    fn new() -> Self {
        Self {
            state: Mutex::new(PoolState {
                queue: VecDeque::new(),
                outstanding: 0,
            }),
            cv: Condvar::new(),
        }
    }

    fn push(&self, job: J) {
        let mut st = self.state.lock().unwrap();
        st.outstanding += 1;
        st.queue.push_back(job);
        self.cv.notify_one();
    }

    /// Takes a queued job without waiting; `None` when the queue is empty.
    /// For callers that pump the pool inline instead of draining it.
    fn try_pop(&self) -> Option<J> {
        let mut st = self.state.lock().unwrap();
        let job = st.queue.pop_front();
        if job.is_some() {
            st.outstanding = st.outstanding.saturating_sub(1);
        }
        job
    }

    fn finish_one(&self) {
        let mut st = self.state.lock().unwrap();
        debug_assert!(st.outstanding > 0, "finish_one without a matching push");
        st.outstanding -= 1;
        if st.outstanding == 0 {
            debug_assert!(st.queue.is_empty(), "outstanding == 0 with queued jobs");
            self.cv.notify_all();
        }
    }

    fn next(&self) -> Option<J> {
        let mut st = self.state.lock().unwrap();
        loop {
            if let Some(job) = st.queue.pop_front() {
                return Some(job);
            }
            if st.outstanding == 0 {
                return None;
            }
            // Bounded wait: correctness no longer depends on never missing
            // a notify, only on re-checking the shared state.
            st = self
                .cv
                .wait_timeout(st, std::time::Duration::from_millis(50))
                .unwrap()
                .0;
        }
    }

    /// Runs `process` on a bounded pool of worker threads until the queue
    /// drains and every in-flight job has finished. `process` is
    /// responsible for calling `pool.push` for follow-on work and must
    /// not call `finish_one` itself.
    fn drain(self: &Arc<Self>, workers: usize, process: impl Fn(J) + Sync) {
        // The work these threads do belongs to whoever started the pool:
        // without this, a `work_counters::measured` scope would miss
        // every listing and stat the pool performs, which is exactly how
        // the instrument came to report "2 dirs listed" for a
        // 20,000-file traversal.
        let counters = crate::work_counters::current();
        std::thread::scope(|scope| {
            for _ in 0..workers {
                let pool = Arc::clone(self);
                let process = &process;
                let counters = counters.clone();
                scope.spawn(move || {
                    crate::work_counters::install(counters);
                    while let Some(job) = pool.next() {
                        process(job);
                        pool.finish_one();
                    }
                });
            }
        });
    }
}

/// Worker count: directory traversal here is latency-bound on metadata
/// syscalls, not CPU, so running more workers than cores keeps more
/// lookups in flight. Doubling cores was measured against 1x/4x/8x/16x on
/// a 38 GB, ~330-top-level-directory tree and gave the best wall time.
fn worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        * 2
}

// ---------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct DirectoryMeasurement {
    pub allocated: u64,
    pub files: u32,
    pub symlinks: u32,
    pub children: Vec<String>,
    pub mtime: i64,
    pub hardlinked: bool,
}

/// Measure one directory, not its descendants. Reuse the walk pool to overlap
/// metadata reads in wide compiler-output directories. At most 256 entries per
/// worker are materialized; no per-file measurements survive the call.
pub(crate) fn measure_directory(path: &Path) -> std::io::Result<DirectoryMeasurement> {
    let entries = Mutex::new(crate::fs_gate::read_dir(path)?);
    crate::work_counters::record_dir_listed();
    let result = Mutex::new(DirectoryMeasurement::default());
    let error = Mutex::new(None);
    let pool = Arc::new(Pool::new());
    let workers = worker_count();
    for _ in 0..workers {
        pool.push(());
    }
    pool.drain(workers, |_| {
        let mut local = DirectoryMeasurement::default();
        loop {
            let batch: Vec<_> = entries.lock().unwrap().by_ref().take(256).collect();
            if batch.is_empty() {
                break;
            }
            for entry in batch {
                let entry = entry.and_then(|e| e.file_type().map(|ft| (e, ft)));
                let (entry, ft) = match entry {
                    Ok(pair) => pair,
                    Err(e) => {
                        *error.lock().unwrap() = Some(e);
                        continue;
                    }
                };
                if ft.is_symlink() {
                    local.symlinks += 1;
                    continue;
                }
                if ft.is_dir() {
                    local
                        .children
                        .push(entry.file_name().to_string_lossy().into_owned());
                } else if ft.is_file() {
                    crate::work_counters::record_files_statted(1);
                    let m = match crate::fs_gate::symlink_metadata(entry.path()) {
                        Ok(m) => m,
                        Err(e) => {
                            *error.lock().unwrap() = Some(e);
                            continue;
                        }
                    };
                    if m.is_file() {
                        local.files += 1;
                        local.allocated += allocated_bytes(&m);
                        local.mtime = local.mtime.max(m.mtime());
                        local.hardlinked |= m.nlink() > 1;
                    }
                }
            }
        }
        let mut out = result.lock().unwrap();
        out.allocated += local.allocated;
        out.files += local.files;
        out.symlinks += local.symlinks;
        out.children.extend(local.children);
        out.mtime = out.mtime.max(local.mtime);
        out.hardlinked |= local.hardlinked;
    });
    if let Some(e) = error.into_inner().unwrap() {
        return Err(e);
    }
    Ok(result.into_inner().unwrap())
}

#[test]
fn shallow_parallel_measurement_counts_allocations_without_following_links_or_children() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("measured");
    std::fs::create_dir(&root).unwrap();
    let mut expected = 0;
    for i in 0..513 {
        let path = root.join(format!("{i}.o"));
        std::fs::write(&path, vec![1u8; 4096]).unwrap();
        expected += fs::symlink_metadata(path).unwrap().blocks() * 512;
    }
    std::fs::hard_link(root.join("0.o"), root.join("alias.o")).unwrap();
    expected += fs::symlink_metadata(root.join("0.o")).unwrap().blocks() * 512;
    std::fs::create_dir(root.join("child")).unwrap();
    std::fs::write(root.join("child/not-counted"), vec![1u8; 8192]).unwrap();
    std::os::unix::fs::symlink(root.join("child"), root.join("symlink")).unwrap();
    let measured = measure_directory(&root).unwrap();
    assert_eq!(measured.allocated, expected);
    assert_eq!(measured.files, 514);
    assert_eq!(measured.children, vec!["child"]);
    assert_eq!(measured.symlinks, 1);
    assert!(measured.hardlinked);
}

/// Same as [`discover_parallel`], pruning any subtree at or under a path
/// in `excluded` (#42 -- `scope::EffectiveScope::pruned_subtrees`): a
/// pruned directory is never entered, so nothing under it is ever
/// discovered as a worktree. Excluded, not partially observed -- the
/// coverage region for it is `Excluded`, never `Missing`/`Partial`.
fn discover_parallel_excluding(
    root: &Path,
    excluded: &[PathBuf],
) -> Result<Vec<DiscoveredWorktree>> {
    if !crate::fs_gate::exists(root) {
        return Ok(Vec::new());
    }
    let device = crate::fs_gate::symlink_metadata(root)
        .with_context(|| format!("stat {}", root.display()))?
        .dev();

    let discovered: Mutex<Vec<DiscoveredWorktree>> = Mutex::new(Vec::new());
    let pool: Arc<Pool<PathBuf>> = Arc::new(Pool::new());
    pool.push(root.to_path_buf());

    pool.drain(worker_count(), |path| {
        discover_one(&path, device, &pool, &discovered, excluded);
    });

    Ok(discovered.into_inner().unwrap())
}

fn discover_one(
    dir: &Path,
    device: u64,
    pool: &Pool<PathBuf>,
    discovered: &Mutex<Vec<DiscoveredWorktree>>,
    excluded: &[PathBuf],
) {
    if excluded.iter().any(|e| dir == e || dir.starts_with(e)) {
        return;
    }
    crate::work_counters::record_files_statted(1);
    let Ok(meta) = crate::fs_gate::symlink_metadata(dir) else {
        return;
    };
    if meta.dev() != device || meta.file_type().is_symlink() || !meta.is_dir() {
        return;
    }

    let git_path = dir.join(".git");
    crate::work_counters::record_files_statted(1);
    if let Ok(git_meta) = crate::fs_gate::symlink_metadata(&git_path) {
        let dw = if git_meta.is_dir() {
            classify_main_checkout(dir, &git_path)
        } else if git_meta.is_file() {
            classify_git_file(dir, &git_path)
        } else {
            None
        };
        if let Some(dw) = dw {
            discovered.lock().unwrap().push(dw);
        }
    }

    let Ok(entries) = crate::fs_gate::read_dir(dir) else {
        return;
    };
    crate::work_counters::record_dir_listed();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() || !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == ".git" || STOP_DIRS.contains(&name.as_ref()) {
            continue;
        }
        pool.push(entry.path());
    }
}

// ---------------------------------------------------------------------
// Attribution
// ---------------------------------------------------------------------

/// A worktree known to the attribution pass: its path (for nearest-match)
/// and the id it should attach rows to. Mirrors
/// `attribution::KnownWorktree`.
struct KnownWorktree {
    path: PathBuf,
    worktree_id: String,
}

/// Finds the id of the worktree whose path is the longest prefix of
/// `path` (the nearest containing checkout/worktree), if any. Mirrors
/// `attribution::nearest_worktree`.
fn nearest_worktree<'a>(worktrees: &'a [KnownWorktree], path: &Path) -> Option<&'a str> {
    worktrees
        .iter()
        .filter(|w| path.starts_with(&w.path))
        .max_by_key(|w| w.path.as_os_str().len())
        .map(|w| w.worktree_id.as_str())
}

/// The root path of the worktree with this id, for turning an absolute
/// path into a rel_path (`DirRollup`/`FileRow` keys are always relative,
/// same contract as `growth.rs`'s stored rows).
fn worktree_root_path<'a>(worktrees: &'a [KnownWorktree], id: &str) -> Option<&'a Path> {
    worktrees
        .iter()
        .find(|w| w.worktree_id == id)
        .map(|w| w.path.as_path())
}

fn rel_path_string(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let s = rel.display().to_string();
    if s == "." { String::new() } else { s }
}

fn parent_rel_path_of(rel_path: &str) -> Option<String> {
    if rel_path.is_empty() {
        return None;
    }
    match Path::new(rel_path).parent() {
        Some(p) => Some(p.display().to_string()),
        None => Some(String::new()),
    }
}

/// Number of shards for the hardlink-dedup set. One `lstat`'s worth of
/// work (a mutex lock + hash-set insert) happens per regular file across
/// every worker, so a single global mutex there would serialize the
/// whole walk; sharding by inode spreads that contention out.
const INODE_SHARDS: usize = 64;

struct ShardedInodeSet {
    shards: Vec<Mutex<HashSet<(u64, u64)>>>,
}

impl ShardedInodeSet {
    fn new() -> Self {
        Self {
            shards: (0..INODE_SHARDS)
                .map(|_| Mutex::new(HashSet::new()))
                .collect(),
        }
    }

    /// Returns true the first time `key` is seen (i.e. it should be
    /// counted), matching `HashSet::insert`'s return value.
    fn insert_first(&self, key: (u64, u64)) -> bool {
        let shard = &self.shards[key.1 as usize % INODE_SHARDS];
        shard.lock().unwrap().insert(key)
    }
}

/// Wait-group + accumulator for one classified artifact directory's
/// subtree, sized by its own bounded pool of `Size` jobs. `remaining`
/// starts at 1 (for the root job); every spawned child increments it
/// before being queued, and every finished job (root or child)
/// decrements it. The job that takes it to zero is the last one done and
/// emits the row — exactly one row per classified directory, regardless
/// of how many jobs sized its subtree.
struct SizeGroup {
    root_path: PathBuf,
    kind: ArtifactKind,
    worktree: Option<String>,
    /// The owning worktree's root, when known: interior directories of
    /// the folded unit then get their own rollup rows in the store (never
    /// in the report), so a later change deep inside re-sizes one
    /// directory instead of the whole tree.
    worktree_root: Option<PathBuf>,
    total: AtomicU64,
    /// Per-row hardlink dedup (see `ArtifactRow::local_bytes`): only
    /// inodes with nlink > 1 are recorded, so the set stays tiny.
    local_total: AtomicU64,
    local_seen: Mutex<HashSet<(u64, u64)>>,
    remaining: AtomicUsize,
    /// Newest mtime (secs) of any file inside the unit: the artifact's age.
    mtime_max: AtomicU64,
}

enum AttrJob {
    /// An unclassified directory: recurse, classifying each child by
    /// name and either sizing it as a unit or walking further.
    Walk(PathBuf),
    /// A directory inside an already-classified artifact subtree. The
    /// [`Classified`] is the proof it is one.
    Size {
        path: PathBuf,
        group: Arc<SizeGroup>,
        classified: Classified,
    },
}

/// Per-row local byte total plus the nlink>1 inodes already counted into it.
type LocalAcc = (u64, HashSet<(u64, u64)>);

/// Live progress of the walk in flight, for any frontend to read while a
/// `report`/`observe` runs on another thread: bytes and directories seen
/// so far, and whether a walk is running. Relaxed atomics; a few counters
/// per directory cost nothing next to the stat calls.
pub mod progress {
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    pub static BYTES: AtomicU64 = AtomicU64::new(0);
    pub static DIRS: AtomicU64 = AtomicU64::new(0);
    pub static ACTIVE: AtomicBool = AtomicBool::new(false);
    /// Walks in flight. An observation re-sizes artifacts with their own
    /// nested walks; only the outermost one owns the counters, or a
    /// nested walk resets them mid-observation and the reading jumps
    /// backwards (and, run repeatedly, past the total).
    static DEPTH: AtomicUsize = AtomicUsize::new(0);

    pub fn start() {
        if DEPTH.fetch_add(1, Ordering::SeqCst) == 0 {
            BYTES.store(0, Ordering::Relaxed);
            DIRS.store(0, Ordering::Relaxed);
            ACTIVE.store(true, Ordering::Relaxed);
        }
    }
    pub fn finish() {
        if DEPTH.fetch_sub(1, Ordering::SeqCst) == 1 {
            ACTIVE.store(false, Ordering::Relaxed);
        }
    }
    /// `(bytes, dirs, active)` right now.
    pub fn snapshot() -> (u64, u64, bool) {
        (
            BYTES.load(Ordering::Relaxed),
            DIRS.load(Ordering::Relaxed),
            ACTIVE.load(Ordering::Relaxed),
        )
    }
}

struct AttrShared {
    sharing: Option<Mutex<crate::sharing::Collector>>,
    seen_inodes: ShardedInodeSet,
    artifacts_by_worktree: Mutex<HashMap<String, Vec<ArtifactRow>>>,
    source_bytes: Mutex<HashMap<String, u64>>,
    /// Per-worktree Source-row local bytes and the nlink>1 inodes already
    /// counted into it (see `ArtifactRow::local_bytes`).
    source_local: Mutex<HashMap<String, LocalAcc>>,
    unowned: Mutex<Vec<UnownedRow>>,
    unowned_hardlinks: Mutex<HashSet<PathBuf>>,
    walked_total: AtomicU64,
    attributed_total: AtomicU64,
    unowned_total: AtomicU64,
    observed_at: u64,
    /// R4c: one entry per Source-tree directory, keyed by
    /// `(worktree_id, rel_path)`. Never populated for a directory inside
    /// a folded artifact (those are `Size` jobs, not `Walk` jobs) or for
    /// a directory outside every known worktree.
    dirs: Mutex<HashMap<(String, String), DirRollup>>,
    /// R4c: large-file rows (>= `large_file_min_bytes`) found directly
    /// while walking the Source tree.
    files: Mutex<Vec<FileRow>>,
    large_file_min_bytes: u64,
    /// Artifact roots whose stored row is still current (nothing under
    /// them changed since it was recorded): when the walk reaches one, it
    /// takes the row instead of sizing the tree again. Empty on a full
    /// walk.
    carry: HashMap<PathBuf, ArtifactRow>,
    /// Subtrees pruned from measurement (#42 --
    /// `scope::EffectiveScope::pruned_subtrees`). A directory at or under
    /// one of these is never entered: not measured, not reported as
    /// unowned, not walked at all. Excluded, never partially observed.
    excluded: Vec<PathBuf>,
    /// One entry per directory a `Size` job actually listed, with the
    /// stamp that decides whether the same directory can be believed
    /// unchanged on a later pass without listing it again
    /// (`folded_measurement::reuse_folded_measurement`). Populated only
    /// when `stamp_dirs` is set -- the full walk has no use for it and
    /// would pay for a vector the size of the tree's directory count.
    dir_stamps: Mutex<Vec<DirStamp>>,
    stamp_dirs: bool,
    /// Set when any `Size` job hit a directory it could not list, root or
    /// not. `dirs` only carries per-directory completeness for a real
    /// Source-tree worktree (`worktree_root: Some`); a folded re-size
    /// called with no worktree (every external-unit and build-container
    /// measurement) gets no `DirRollup`s at all, so a caller that needs
    /// "did this fold see everything" -- `resize_artifact_stamped`'s
    /// fourth return value -- reads this flag instead
    /// (`.oh/guardrails/coverage-changes-are-not-storage-changes.md`: an
    /// unreadable subdirectory is incomplete coverage, never a smaller
    /// complete tree).
    incomplete: AtomicBool,
}

/// Explicit reconciliation only. The ordinary folded sizing pool visits the
/// selected paths with one ephemeral device/inode ledger. Neither identities
/// nor charge ownership escape this call; history keeps its existing basis.
#[cfg(test)]
pub(crate) fn reconcile_unique_bytes(paths: &[PathBuf], excluded: &[PathBuf]) -> Option<u64> {
    reconcile_shared_bytes(paths, excluded, paths).map(|(bytes, _)| bytes)
}

pub(crate) fn reconcile_shared_bytes(
    paths: &[PathBuf],
    excluded: &[PathBuf],
    containers: &[PathBuf],
) -> Option<(u64, crate::sharing::SharingSummary)> {
    let shared = Arc::new(AttrShared {
        sharing: Some(Mutex::new(crate::sharing::Collector::new(
            containers.to_vec(),
        ))),
        seen_inodes: ShardedInodeSet::new(),
        artifacts_by_worktree: Mutex::new(HashMap::new()),
        source_bytes: Mutex::new(HashMap::new()),
        source_local: Mutex::new(HashMap::new()),
        unowned: Mutex::new(Vec::new()),
        unowned_hardlinks: Mutex::new(HashSet::new()),
        walked_total: AtomicU64::new(0),
        attributed_total: AtomicU64::new(0),
        unowned_total: AtomicU64::new(0),
        observed_at: 0,
        dirs: Mutex::new(HashMap::new()),
        files: Mutex::new(Vec::new()),
        large_file_min_bytes: u64::MAX,
        carry: HashMap::new(),
        excluded: excluded.to_vec(),
        dir_stamps: Mutex::new(Vec::new()),
        stamp_dirs: false,
        incomplete: AtomicBool::new(false),
    });
    let mut paths = paths.to_vec();
    paths.sort();
    paths.dedup();
    let mut roots: Vec<PathBuf> = Vec::new();
    let pool = Arc::new(Pool::new());
    for path in paths {
        if roots.iter().chain(excluded).any(|r| path.starts_with(r)) {
            continue;
        }
        let meta = crate::fs_gate::symlink_metadata(&path).ok()?;
        if meta.file_type().is_symlink() {
            continue;
        }
        roots.push(path.clone());
        if meta.is_file() {
            record_file(&path, &meta, &[], &shared);
        } else if meta.is_dir() {
            pool.push(AttrJob::Size {
                group: Arc::new(SizeGroup {
                    root_path: path.clone(),
                    kind: ArtifactKind::Cache,
                    worktree: None,
                    worktree_root: None,
                    total: AtomicU64::new(0),
                    local_total: AtomicU64::new(0),
                    local_seen: Mutex::new(HashSet::new()),
                    remaining: AtomicUsize::new(1),
                    mtime_max: AtomicU64::new(0),
                }),
                path,
                classified: Classified::stored(ArtifactKind::Cache),
            });
        }
    }
    progress::start();
    pool.drain(worker_count(), |job| {
        if let AttrJob::Size {
            path,
            group,
            classified,
        } = job
        {
            process_size(path, &group, &classified, &shared, &pool);
        }
    });
    progress::finish();
    #[cfg(test)]
    {
        let (entries, slots) = shared.seen_inodes.shards.iter().fold((0, 0), |(n, c), s| {
            let shard = s.lock().unwrap();
            (n + shard.len(), c + shard.capacity())
        });
        eprintln!(
            "reconciliation ledger: {entries} distinct inodes, {slots} key slots, {} key-capacity bytes (excluding hash/control/allocator overhead)",
            slots * std::mem::size_of::<(u64, u64)>()
        );
    }
    (!shared.incomplete.load(Ordering::Relaxed)).then(|| {
        (
            shared.walked_total.load(Ordering::Relaxed),
            shared.sharing.as_ref().unwrap().lock().unwrap().summary(),
        )
    })
}

#[cfg(test)]
mod scope_reconciliation_tests {
    use super::*;

    #[test]
    fn sharing_groups_are_not_pair_edges_or_ancestor_ownership() {
        use std::fs;
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        let cache = tmp.path().join("cache");
        let nested = a.join("node_modules");
        for dir in [&nested, &b, &cache] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(cache.join("file"), vec![1u8; 8192]).unwrap();
        fs::hard_link(cache.join("file"), nested.join("file")).unwrap();
        fs::hard_link(cache.join("file"), b.join("file")).unwrap();
        let roots = vec![a.clone(), b.clone(), cache.clone()];
        let containers = vec![a.clone(), nested.clone(), b.clone(), cache.clone()];
        let (bytes, summary) = reconcile_shared_bytes(&roots, &[], &containers).unwrap();
        assert_eq!(summary.groups.len(), 1);
        assert_eq!(summary.groups[0].bytes, bytes);
        assert_eq!(summary.groups[0].containers.len(), 3);
        assert!(!summary.groups[0].containers.contains(&a));
        assert!(!summary.groups[0].unresolved_links);
        let mut reverse = roots.clone();
        reverse.reverse();
        assert_eq!(
            reconcile_shared_bytes(&reverse, &[], &containers).unwrap(),
            (bytes, summary)
        );
        let (_, excluded) = reconcile_shared_bytes(&roots, &[cache.clone()], &containers).unwrap();
        assert_eq!(excluded.groups.len(), 1);
        assert!(excluded.groups[0].unresolved_links);
        assert!(!excluded.groups[0].containers.contains(&cache));
        fs::remove_file(b.join("file")).unwrap();
        fs::write(b.join("file"), vec![2u8; 8192]).unwrap();
        let (_, changed) = reconcile_shared_bytes(&roots, &[], &containers).unwrap();
        assert_eq!(changed.groups.len(), 1);
        assert_eq!(changed.groups[0].containers.len(), 2);
        assert!(!changed.groups[0].containers.contains(&b));
        assert!(!changed.groups[0].unresolved_links);
    }

    #[test]
    fn reconciliation_keys_include_the_device() {
        let ledger = ShardedInodeSet::new();
        assert!(ledger.insert_first((1, 7)));
        assert!(ledger.insert_first((2, 7)));
        assert!(!ledger.insert_first((1, 7)));
    }

    #[test]
    fn reconciliation_prunes_exclusions_and_does_not_follow_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("keep"), vec![1; 8192]).unwrap();
        std::fs::write(root.join("excluded"), vec![1; 16384]).unwrap();
        std::fs::write(tmp.path().join("outside"), vec![1; 32768]).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("outside"), root.join("link")).unwrap();
        let expected = allocated_bytes(&std::fs::metadata(root.join("keep")).unwrap());
        assert_eq!(
            reconcile_unique_bytes(&[root.clone(), root.join("keep")], &[root.join("excluded")]),
            Some(expected)
        );
        assert_eq!(
            reconcile_unique_bytes(&[tmp.path().join("missing")], &[]),
            None
        );
    }

    #[test]
    fn twenty_thousand_shared_entries_are_counted_once_without_retained_state() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let mut expected = 0;
        for i in 0..1000 {
            let source = a.join(i.to_string());
            std::fs::write(&source, [1; 32]).unwrap();
            expected += allocated_bytes(&std::fs::metadata(&source).unwrap());
            for j in 0..19 {
                std::fs::hard_link(&source, b.join(format!("{i}-{j}"))).unwrap();
            }
        }
        let (value, work) = crate::work_counters::measured(|| reconcile_unique_bytes(&[b, a], &[]));
        assert_eq!(value, Some(expected));
        assert_eq!(work.dirs_listed, 2);
        assert_eq!(work.files_statted, 20002);
        // A subsequent independent call cannot inherit the prior pass's set.
        assert_eq!(
            reconcile_unique_bytes(&[tmp.path().join("a")], &[]),
            Some(expected)
        );
    }
}

/// One directory's identity and change stamp, recorded while it was
/// listed. `mtime_ns`/`ctime_ns` are the directory's own, so an entry
/// added, removed or renamed inside it moves the stamp; a file rewritten
/// *in place* does not (see the limitation recorded on
/// `folded_measurement::reuse_folded_measurement`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirStamp {
    pub path: PathBuf,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
}

/// `attribute_parallel` that takes `carry`ed artifact rows as read (see
/// `AttrShared::carry`) and a set of subtrees to prune (#42), for the
/// full-walk and incremental paths respectively.
fn attribute_parallel_carrying(
    root: &Path,
    worktrees: &[(&Path, &str)],
    observed_at: u64,
    large_file_min_bytes: u64,
    carry: HashMap<PathBuf, ArtifactRow>,
    excluded: &[PathBuf],
) -> AttributionResult {
    progress::start();
    let result = attribute_parallel_inner(
        root,
        worktrees,
        observed_at,
        large_file_min_bytes,
        carry,
        excluded,
    );
    progress::finish();
    result
}

fn attribute_parallel_inner(
    root: &Path,
    worktrees: &[(&Path, &str)],
    observed_at: u64,
    large_file_min_bytes: u64,
    carry: HashMap<PathBuf, ArtifactRow>,
    excluded: &[PathBuf],
) -> AttributionResult {
    let known: Vec<KnownWorktree> = worktrees
        .iter()
        .map(|(path, id)| KnownWorktree {
            path: path.to_path_buf(),
            worktree_id: id.to_string(),
        })
        .collect();
    let known = Arc::new(known);

    let shared = Arc::new(AttrShared {
        seen_inodes: ShardedInodeSet::new(),
        sharing: None,
        artifacts_by_worktree: Mutex::new(HashMap::new()),
        source_bytes: Mutex::new(HashMap::new()),
        source_local: Mutex::new(HashMap::new()),
        unowned: Mutex::new(Vec::new()),
        unowned_hardlinks: Mutex::new(HashSet::new()),
        walked_total: AtomicU64::new(0),
        attributed_total: AtomicU64::new(0),
        unowned_total: AtomicU64::new(0),
        observed_at,
        dirs: Mutex::new(HashMap::new()),
        files: Mutex::new(Vec::new()),
        large_file_min_bytes,
        carry,
        excluded: excluded.to_vec(),
        dir_stamps: Mutex::new(Vec::new()),
        stamp_dirs: false,
        incomplete: AtomicBool::new(false),
    });

    let pool: Arc<Pool<AttrJob>> = Arc::new(Pool::new());
    pool.push(AttrJob::Walk(root.to_path_buf()));

    pool.drain(worker_count(), |job| match job {
        AttrJob::Walk(path) => process_walk(path, &known, &shared, &pool),
        AttrJob::Size {
            path,
            group,
            classified,
        } => process_size(path, &group, &classified, &shared, &pool),
    });

    let shared = Arc::try_unwrap(shared).unwrap_or_else(|_| unreachable!("workers joined"));
    let mut artifacts_by_worktree = shared.artifacts_by_worktree.into_inner().unwrap();
    let source_bytes = shared.source_bytes.into_inner().unwrap();
    let source_local = shared.source_local.into_inner().unwrap();

    for (worktree_id, bytes) in source_bytes {
        let local_bytes = source_local.get(&worktree_id).map(|e| e.0).unwrap_or(bytes);
        let source_hardlinked = source_local
            .get(&worktree_id)
            .is_some_and(|e| !e.1.is_empty());
        if bytes == 0 {
            continue;
        }
        let path = worktrees
            .iter()
            .find(|(_, id)| *id == worktree_id)
            .map(|(p, _)| p.to_path_buf())
            .unwrap_or_default();
        artifacts_by_worktree
            .entry(worktree_id)
            .or_default()
            .push(ArtifactRow {
                kind: ArtifactKind::Source,
                path,
                bytes,
                mtime_max: 0,
                ecosystem: None,
                hardlinked: source_hardlinked,
                dedup_stale: false,
                allocated_bytes: None,
                allocated_growth_bytes: None,
                local_bytes,
                track: None,
                growth_bytes: None,
                regrowth_count: 0,
                observed_at,
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

    AttributionResult {
        artifacts_by_worktree,
        unowned: shared.unowned.into_inner().unwrap(),
        walked_total: shared.walked_total.load(Ordering::Acquire),
        attributed_total: shared.attributed_total.load(Ordering::Acquire),
        unowned_total: shared.unowned_total.load(Ordering::Acquire),
        dirs: shared.dirs.into_inner().unwrap().into_values().collect(),
        files: shared.files.into_inner().unwrap(),
    }
}

/// Processes one directory in the non-classified part of the tree. Files
/// are recorded inline (one extra `lstat` for size/hardlink identity,
/// same as the serial walk); only subdirectories become new pool jobs, so
/// the number of jobs tracks the directory count rather than the much
/// larger file count. The root of the whole walk is the one path that
/// arrives here without having been through a parent's `read_dir`
/// (`is_dir`/`is_symlink` come from `DirEntry::file_type` for every other
/// call site), so it alone still needs its own `symlink_metadata` check.
fn process_walk(path: PathBuf, known: &[KnownWorktree], shared: &AttrShared, pool: &Pool<AttrJob>) {
    // Pruned by a config exclusion (#42): not entered, not measured, not
    // reported as unowned. This is what makes a subtree exclusion inside
    // an otherwise-included root an `Excluded` coverage region rather
    // than merely a recorded-but-ignored note.
    if shared
        .excluded
        .iter()
        .any(|e| path == *e || path.starts_with(e))
    {
        return;
    }
    crate::work_counters::record_files_statted(1);
    let Ok(meta) = crate::fs_gate::symlink_metadata(&path) else {
        return;
    };
    if meta.file_type().is_symlink() {
        return;
    }
    if meta.is_file() {
        // The walk root itself is a single file (rare: an explicit
        // include naming a file, not a directory) -- at most one row,
        // so no directory to fold into; push it directly.
        if let (_, FileTally::Unowned(bytes)) = record_file(&path, &meta, known, shared) {
            push_unowned_dir(&path, bytes, shared);
        }
        return;
    }
    if !meta.is_dir() {
        return;
    }

    let entries = match crate::fs_gate::read_dir(&path) {
        Ok(e) => {
            crate::work_counters::record_dir_listed();
            e
        }
        Err(_) => {
            shared.unowned.lock().unwrap().push(UnownedRow {
                measurement: None,
                path_or_object: path.display().to_string(),
                bytes: 0,
                reason: UnownedReason::PermissionDenied,
                shared_bytes: None,
                note: None,
                created_at: None,
                containers: Vec::new(),
                shared_with: Vec::new(),
                dangling: false,
                docker_kind: None,
                evidence: Vec::new(),
            });
            return;
        }
    };

    // R4c: this directory's own rollup, accumulated as entries are
    // classified below. Only recorded at the end if `path` is inside a
    // known worktree's Source tree (`nearest_worktree` returns `Some`).
    let mut dir_own_allocated: u64 = 0;
    // Direct-file unowned bytes, folded into exactly one `UnownedRow` for
    // `path` at the end of this call instead of one row per file (#R10
    // item 1).
    let mut dir_unowned_bytes: u64 = 0;
    let mut dir_file_count: u32 = 0;
    let mut dir_dir_count: u32 = 0;
    let mut dir_symlink_count: u32 = 0;
    // Floor at this directory's own mtime (already lstat'd above as
    // `meta`), never epoch: a directory whose direct entries are all
    // subdirectories still has its own st_mtime as real evidence, and
    // that must win over a fabricated zero.
    let mut dir_mtime_max: i64 = meta.mtime();

    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        let child_path = entry.path();
        if ft.is_symlink() {
            dir_symlink_count += 1;
            crate::work_counters::record_files_statted(1);
            if let Ok(smeta) = crate::fs_gate::symlink_metadata(&child_path) {
                dir_mtime_max = dir_mtime_max.max(smeta.mtime());
            }
            continue;
        }
        if ft.is_file() {
            dir_file_count += 1;
            if let Some((mtime, tally)) = record_file_typed(&child_path, known, shared) {
                dir_mtime_max = dir_mtime_max.max(mtime);
                match tally {
                    FileTally::Owned(bytes) => {
                        dir_own_allocated += bytes;
                        if bytes >= shared.large_file_min_bytes {
                            record_large_file(&child_path, bytes, mtime, known, shared);
                        }
                    }
                    FileTally::Unowned(bytes) => {
                        dir_unowned_bytes += bytes;
                    }
                    FileTally::Duplicate => {}
                }
            }
        } else if ft.is_dir() {
            dir_dir_count += 1;
            progress::DIRS.fetch_add(1, Ordering::Relaxed);
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(classified) = classified_at(&path, &name) {
                let kind = classified.kind().clone();
                let worktree = nearest_worktree(known, &child_path).map(str::to_string);
                let worktree_root = worktree
                    .as_deref()
                    .and_then(|id| worktree_root_path(known, id))
                    .map(Path::to_path_buf);
                if let (Some(row), Some(wt)) = (shared.carry.get(&child_path), worktree.as_ref()) {
                    // Nothing under this artifact changed since its row
                    // was stored: take the row, skip the tree.
                    shared.walked_total.fetch_add(row.bytes, Ordering::Relaxed);
                    shared
                        .attributed_total
                        .fetch_add(row.bytes, Ordering::Relaxed);
                    progress::BYTES.fetch_add(row.bytes, Ordering::Relaxed);
                    shared
                        .artifacts_by_worktree
                        .lock()
                        .unwrap()
                        .entry(wt.clone())
                        .or_default()
                        .push(row.clone());
                    continue;
                }
                let group = Arc::new(SizeGroup {
                    root_path: child_path.clone(),
                    kind,
                    worktree,
                    worktree_root,
                    total: AtomicU64::new(0),
                    local_total: AtomicU64::new(0),
                    local_seen: Mutex::new(HashSet::new()),
                    remaining: AtomicUsize::new(1),
                    mtime_max: AtomicU64::new(0),
                });
                pool.push(AttrJob::Size {
                    path: child_path,
                    group,
                    classified,
                });
            } else {
                pool.push(AttrJob::Walk(child_path));
            }
        }
    }

    // Direct-file unowned bytes fold into exactly one row for this
    // directory (never one per file); zero when `path` is inside a known
    // worktree, since every direct child file then shares that worktree.
    push_unowned_dir(&path, dir_unowned_bytes, shared);

    if let Some(worktree_id) = nearest_worktree(known, &path).map(str::to_string)
        && let Some(root) = worktree_root_path(known, &worktree_id)
    {
        let rel_path = rel_path_string(root, &path);
        let parent_rel_path = parent_rel_path_of(&rel_path);
        let mod_time_min = (dir_mtime_max / 60) as i32;
        shared.dirs.lock().unwrap().insert(
            (worktree_id.clone(), rel_path.clone()),
            DirRollup {
                worktree_id,
                track: None,
                rel_path,
                parent_rel_path,
                // Bottom-up aggregation into `allocated_total` happens
                // once in `report::aggregate_dir_totals`, after every
                // directory job has finished; starting equal to
                // `own_allocated` keeps this row valid even if that
                // aggregation step is skipped.
                allocated_total: dir_own_allocated,
                own_allocated: dir_own_allocated,
                file_count: dir_file_count,
                entry_count: dir_file_count + dir_dir_count + dir_symlink_count,
                symlink_count: dir_symlink_count,
                mod_time_min,
                complete: true,
                growth_bytes: None,
            },
        );
    }
}

fn record_large_file(
    path: &Path,
    bytes: u64,
    mtime_secs: i64,
    known: &[KnownWorktree],
    shared: &AttrShared,
) {
    let Some(worktree_id) = nearest_worktree(known, path) else {
        return;
    };
    let Some(root) = worktree_root_path(known, worktree_id) else {
        return;
    };
    shared.files.lock().unwrap().push(FileRow {
        worktree_id: worktree_id.to_string(),
        rel_path: rel_path_string(root, path),
        allocated: bytes,
        mod_time_min: (mtime_secs / 60) as i32,
        growth_bytes: None,
    });
}

/// What a recorded file's allocated bytes counted against, so the caller
/// can fold unowned bytes per *directory* instead of pushing one
/// [`UnownedRow`] per file (#R10 item 1: a store that scales with the
/// unowned file count, not the directory count, is exactly the "giant
/// JSON artifact cache" the handoff forbids).
enum FileTally {
    /// Already counted via another hardlink to the same inode; no bytes
    /// to fold into any rollup.
    Duplicate,
    /// Counted against a known worktree.
    Owned(u64),
    /// Counted against no known worktree -- folds into this file's
    /// containing directory's unowned row, never its own row.
    Unowned(u64),
}

/// Like `record_file`, but for an entry already known (from
/// `DirEntry::file_type`) to be a non-symlink file, so it does the one
/// `lstat` a regular file needs for size/hardlink identity without a
/// redundant type check first. Returns `(mtime_secs, tally)` so the
/// caller can fold this file into its directory's rollup (mtime always;
/// bytes only when this was not a hardlink dup, matching how
/// `walked_total`/`attributed_total` already dedup).
fn record_file_typed(
    path: &Path,
    known: &[KnownWorktree],
    shared: &AttrShared,
) -> Option<(i64, FileTally)> {
    crate::work_counters::record_files_statted(1);
    let meta = crate::fs_gate::symlink_metadata(path).ok()?;
    Some(record_file(path, &meta, known, shared))
}

fn record_file(
    path: &Path,
    meta: &fs::Metadata,
    known: &[KnownWorktree],
    shared: &AttrShared,
) -> (i64, FileTally) {
    if meta.nlink() > 1
        && let Some(collector) = &shared.sharing
    {
        collector.lock().unwrap().record(path, meta);
    }
    let mtime = meta.mtime();
    if meta.nlink() > 1 && nearest_worktree(known, path).is_none() {
        shared
            .unowned_hardlinks
            .lock()
            .unwrap()
            .insert(path.parent().unwrap_or(path).to_path_buf());
    }
    // Per-row local figure first: independent of which row the global
    // dedup below happens to charge.
    if let Some(worktree_id) = nearest_worktree(known, path) {
        let mut local = shared.source_local.lock().unwrap();
        let entry = local.entry(worktree_id.to_string()).or_default();
        if meta.nlink() <= 1 || entry.1.insert((meta.dev(), meta.ino())) {
            entry.0 += allocated_bytes(meta);
        }
    }
    if !shared.seen_inodes.insert_first((meta.dev(), meta.ino())) {
        return (mtime, FileTally::Duplicate);
    }
    let bytes = allocated_bytes(meta);
    shared.walked_total.fetch_add(bytes, Ordering::Relaxed);
    progress::BYTES.fetch_add(bytes, Ordering::Relaxed);
    match nearest_worktree(known, path) {
        Some(worktree_id) => {
            shared.attributed_total.fetch_add(bytes, Ordering::Relaxed);
            *shared
                .source_bytes
                .lock()
                .unwrap()
                .entry(worktree_id.to_string())
                .or_default() += bytes;
            (mtime, FileTally::Owned(bytes))
        }
        None => {
            shared.unowned_total.fetch_add(bytes, Ordering::Relaxed);
            (mtime, FileTally::Unowned(bytes))
        }
    }
}

/// Pushes exactly one folded [`UnownedRow`] for a directory none of whose
/// direct files belong to a known worktree, summing every direct
/// unowned file's bytes into `bytes` -- never one row per file. The
/// directory's own name decides the shared-cache label, matching
/// `finish_size_job`'s already-folded convention for classified unowned
/// artifact directories (e.g. a stray `node_modules` outside any
/// worktree) so both folding paths agree on labeling.
fn push_unowned_dir(dir_path: &Path, bytes: u64, shared: &AttrShared) {
    let hardlinked = shared.unowned_hardlinks.lock().unwrap().contains(dir_path);
    if bytes == 0 && !hardlinked {
        return;
    }
    shared.unowned.lock().unwrap().push(unowned_row(
        dir_path,
        bytes,
        if hardlinked {
            crate::report::UnownedMeasurement::DirectShared
        } else {
            crate::report::UnownedMeasurement::Direct
        },
    ));
}

fn unowned_row(
    dir_path: &Path,
    bytes: u64,
    measurement: crate::report::UnownedMeasurement,
) -> UnownedRow {
    let name = dir_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let reason = if is_shared_cache_name(name) {
        UnownedReason::SharedCache
    } else {
        UnownedReason::NoContainingRepo
    };
    UnownedRow {
        measurement: Some(measurement),
        path_or_object: dir_path.display().to_string(),
        bytes,
        reason,
        shared_bytes: None,
        note: None,
        created_at: None,
        containers: Vec::new(),
        shared_with: Vec::new(),
        dangling: false,
        docker_kind: None,
        evidence: Vec::new(),
    }
}

/// Sizes one directory inside a classified artifact subtree: sums
/// allocated bytes of its regular files (deduped by hardlink) into the
/// group's running total, and hands subdirectories to the pool as more
/// `Size` jobs under the same group. Read errors here are swallowed, same
/// as the serial `size_as_unit`, since a classified directory is sized as
/// a best-effort unit rather than reported as a permission gap.
fn process_size(
    path: PathBuf,
    group: &Arc<SizeGroup>,
    classified: &Classified,
    shared: &AttrShared,
    pool: &Pool<AttrJob>,
) {
    // Pruned (#45-#49's external-location double-measurement fix): a
    // subtree named in `shared.excluded` is measured independently
    // elsewhere (typically as its own external unit), so this job
    // contributes nothing for it -- same "not entered, not measured"
    // contract as `process_walk`'s identical check.
    if shared
        .excluded
        .iter()
        .any(|e| path == *e || path.starts_with(e))
    {
        finish_size_job(group, shared);
        return;
    }
    let Ok(entries) = crate::fs_gate::read_dir(&path) else {
        // The unit is still sized best-effort, but the directory that
        // could not be listed is recorded as an *incomplete* row rather
        // than silently absent: without it every ancestor's rollup said
        // `complete: true` over bytes it never saw, and a `node_modules`
        // with one unreadable package read as a complete, smaller tree
        // (#65: partial/unreadable containers are incomplete coverage,
        // never a disappearance or a quiet shrink).
        shared.incomplete.store(true, Ordering::Relaxed);
        if let (Some(worktree_id), Some(root)) = (&group.worktree, &group.worktree_root) {
            let rel_path = rel_path_string(root, &path);
            let parent_rel_path = parent_rel_path_of(&rel_path);
            shared.dirs.lock().unwrap().insert(
                (worktree_id.clone(), rel_path.clone()),
                DirRollup {
                    worktree_id: worktree_id.clone(),
                    track: None,
                    rel_path,
                    parent_rel_path,
                    allocated_total: 0,
                    own_allocated: 0,
                    file_count: 0,
                    entry_count: 0,
                    symlink_count: 0,
                    mod_time_min: 0,
                    complete: false,
                    growth_bytes: None,
                },
            );
        }
        finish_size_job(group, shared);
        return;
    };
    crate::work_counters::record_dir_listed();
    // This directory's own rollup (store depth inside the folded unit).
    let mut own_allocated: u64 = 0;
    let mut file_count: u32 = 0;
    let mut dir_count: u32 = 0;
    let mut symlink_count: u32 = 0;
    crate::work_counters::record_files_statted(1);
    let own_meta = crate::fs_gate::symlink_metadata(&path);
    if shared.stamp_dirs
        && let Ok(m) = own_meta.as_ref()
    {
        shared.dir_stamps.lock().unwrap().push(DirStamp {
            path: path.clone(),
            mtime_ns: m.mtime() * 1_000_000_000 + m.mtime_nsec(),
            ctime_ns: m.ctime() * 1_000_000_000 + m.ctime_nsec(),
        });
    }
    let mut dir_mtime_max: i64 = own_meta.map(|m| m.mtime()).unwrap_or(0);
    for entry in entries {
        let Ok(entry) = entry else {
            shared.incomplete.store(true, Ordering::Relaxed);
            continue;
        };
        if shared.excluded.iter().any(|e| entry.path().starts_with(e)) {
            continue;
        }
        let Ok(ft) = entry.file_type() else {
            shared.incomplete.store(true, Ordering::Relaxed);
            continue;
        };
        if ft.is_symlink() {
            symlink_count += 1;
            continue;
        }
        if ft.is_dir() {
            dir_count += 1;
            group.remaining.fetch_add(1, Ordering::SeqCst);
            pool.push(AttrJob::Size {
                path: entry.path(),
                group: Arc::clone(group),
                classified: classified.clone(),
            });
        } else if ft.is_file() {
            crate::work_counters::record_files_statted(1);
            let Ok(meta) = crate::fs_gate::symlink_metadata(entry.path()) else {
                shared.incomplete.store(true, Ordering::Relaxed);
                continue;
            };
            if meta.file_type().is_symlink() || !meta.is_file() {
                continue;
            }
            file_count += 1;
            own_allocated += allocated_bytes(&meta);
            dir_mtime_max = dir_mtime_max.max(meta.mtime());
            let key = (meta.dev(), meta.ino());
            if meta.nlink() > 1
                && let Some(collector) = &shared.sharing
            {
                collector.lock().unwrap().record(&entry.path(), &meta);
            }
            group
                .mtime_max
                .fetch_max(meta.mtime().max(0) as u64, Ordering::Relaxed);
            if meta.nlink() <= 1 || group.local_seen.lock().unwrap().insert(key) {
                group
                    .local_total
                    .fetch_add(allocated_bytes(&meta), Ordering::Relaxed);
            }
            if !shared.seen_inodes.insert_first(key) {
                continue;
            }
            let bytes = allocated_bytes(&meta);
            group.total.fetch_add(bytes, Ordering::Relaxed);
            shared.walked_total.fetch_add(bytes, Ordering::Relaxed);
            progress::BYTES.fetch_add(bytes, Ordering::Relaxed);
            if group.worktree.is_some() {
                shared.attributed_total.fetch_add(bytes, Ordering::Relaxed);
            } else {
                shared.unowned_total.fetch_add(bytes, Ordering::Relaxed);
            }
        }
    }
    if let (Some(worktree_id), Some(root)) = (&group.worktree, &group.worktree_root) {
        let rel_path = rel_path_string(root, &path);
        let parent_rel_path = parent_rel_path_of(&rel_path);
        shared.dirs.lock().unwrap().insert(
            (worktree_id.clone(), rel_path.clone()),
            DirRollup {
                worktree_id: worktree_id.clone(),
                track: None,
                rel_path,
                parent_rel_path,
                allocated_total: own_allocated,
                own_allocated,
                file_count,
                entry_count: file_count + dir_count + symlink_count,
                symlink_count,
                mod_time_min: (dir_mtime_max / 60) as i32,
                complete: true,
                growth_bytes: None,
            },
        );
    }
    finish_size_job(group, shared);
}

fn finish_size_job(group: &Arc<SizeGroup>, shared: &AttrShared) {
    if group.remaining.fetch_sub(1, Ordering::AcqRel) != 1 {
        return;
    }
    let bytes = group.total.load(Ordering::Acquire);
    match &group.worktree {
        Some(worktree_id) => {
            shared
                .artifacts_by_worktree
                .lock()
                .unwrap()
                .entry(worktree_id.clone())
                .or_default()
                .push(ArtifactRow {
                    kind: group.kind.clone(),
                    path: group.root_path.clone(),
                    bytes,
                    mtime_max: group.mtime_max.load(Ordering::Acquire),
                    ecosystem: None,
                    hardlinked: !group.local_seen.lock().unwrap().is_empty(),
                    dedup_stale: false,
                    allocated_bytes: None,
                    allocated_growth_bytes: None,
                    local_bytes: group.local_total.load(Ordering::Acquire),
                    track: None,
                    growth_bytes: None,
                    regrowth_count: 0,
                    observed_at: shared.observed_at,
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
        None => {
            let name = group
                .root_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            let reason = if is_shared_cache_name(name) {
                UnownedReason::SharedCache
            } else {
                UnownedReason::NoContainingRepo
            };
            shared.unowned.lock().unwrap().push(UnownedRow {
                measurement: Some(if group.local_seen.lock().unwrap().is_empty() {
                    crate::report::UnownedMeasurement::Subtree
                } else {
                    crate::report::UnownedMeasurement::SubtreeShared
                }),
                path_or_object: group.root_path.display().to_string(),
                bytes,
                reason,
                shared_bytes: None,
                note: None,
                created_at: None,
                containers: Vec::new(),
                shared_with: Vec::new(),
                dangling: false,
                docker_kind: None,
                evidence: Vec::new(),
            });
        }
    }
}

// ---------------------------------------------------------------------
// R4b: incremental re-walk primitives. `growth::observe_tracked` uses
// these two entry points to re-walk only what FSEvents implicated,
// carrying every other row forward untouched. Both give exactly the same
// per-directory/per-file classification and sizing rules as
// `attribute_parallel` above -- they simply start the same machinery at a
// narrower root than the whole scan root.
// ---------------------------------------------------------------------

/// Refresh folded unowned measurements without entering unchanged siblings.
/// `None` asks the caller for reconciliation (unknown boundaries,
/// unreadable paths, or changed project ownership). Sharing is an explicitly
/// stale estimate, not a reason to walk unrelated containers.
pub(crate) fn refresh_unowned(
    root: &Path,
    previous: &[UnownedRow],
    changed: &[PathBuf],
    worktrees: &[PathBuf],
    excluded: &[PathBuf],
    observed_at: u64,
    shared_root: bool,
) -> Option<Vec<UnownedRow>> {
    use crate::report::UnownedMeasurement::{
        Direct, DirectEstimate, Hardlinked, Subtree, SubtreeEstimate,
    };
    if previous
        .iter()
        .any(|r| matches!(r.measurement, None | Some(Hardlinked)))
    {
        return None;
    }
    let mut rows = previous.to_vec();
    let mut sharing = shared_root
        || previous
            .iter()
            .any(|r| !matches!(r.measurement, Some(Direct | Subtree)));
    if sharing {
        for row in &mut rows {
            row.measurement = row.measurement.map(|m| {
                if m.folded() {
                    SubtreeEstimate
                } else {
                    DirectEstimate
                }
            });
        }
    }
    let mut pending: Vec<PathBuf> = changed
        .iter()
        .filter(|p| !worktrees.iter().any(|w| p.starts_with(w)))
        .map(|p| {
            previous
                .iter()
                .find(|r| {
                    r.measurement.is_some_and(|m| m.folded()) && p.starts_with(&r.path_or_object)
                })
                .map(|r| PathBuf::from(&r.path_or_object))
                .unwrap_or_else(|| p.clone())
        })
        .collect();
    pending.sort();
    pending.dedup();
    let mut visited = HashSet::new();
    while let Some(path) = pending.pop() {
        if !visited.insert(path.clone())
            || excluded.iter().any(|e| path.starts_with(e))
            || worktrees.iter().any(|w| path.starts_with(w))
        {
            continue;
        }
        if !path.starts_with(root) {
            return None;
        }
        // Checking only the final component would follow a replaced ancestor.
        let mut blocked = None;
        for ancestor in path.ancestors().take_while(|p| p.starts_with(root)) {
            crate::work_counters::record_files_statted(1);
            match crate::fs_gate::symlink_metadata(ancestor) {
                Ok(m) if m.file_type().is_symlink() => {
                    blocked = Some(ancestor.to_path_buf());
                    break;
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    blocked = Some(ancestor.to_path_buf());
                    break;
                }
                Err(_) => return None,
            }
        }
        if let Some(blocked) = blocked {
            rows.retain(|r| !Path::new(&r.path_or_object).starts_with(&blocked));
            if let Some(parent) = blocked.parent().filter(|p| p.starts_with(root)) {
                pending.push(parent.to_path_buf());
            }
            continue;
        }
        // Classify only after rejecting symlink ancestors: cache tags read
        // marker bytes, not just compare a basename.
        if let Some(boundary) = path
            .ancestors()
            .take_while(|a| *a != root && a.starts_with(root))
            .filter(|a| unowned_classification(a).is_some())
            .last()
            && boundary != path
        {
            pending.push(boundary.to_path_buf());
            continue;
        }
        if path
            .ancestors()
            .take_while(|p| p.starts_with(root))
            .any(|p| crate::fs_gate::exists(p.join(".git")))
        {
            return None;
        }
        crate::work_counters::record_files_statted(1);
        let meta = crate::fs_gate::symlink_metadata(&path).ok()?;
        if !meta.is_dir() {
            // Item events have no independent row: relist their parent once.
            let parent = path.parent().filter(|p| p.starts_with(root))?;
            pending.push(parent.to_path_buf());
            continue;
        }
        let classified_now = path != root && unowned_classification(&path).is_some();
        let was_folded = previous.iter().any(|r| {
            r.path_or_object == path.to_string_lossy() && r.measurement.is_some_and(|m| m.folded())
        });
        if was_folded && !classified_now {
            return None;
        }
        if classified_now {
            if worktrees.iter().any(|w| w.starts_with(&path)) {
                return None;
            }
            let (measured, _, _, complete) = resize_artifact_stamped(
                &path,
                ArtifactKind::Cache,
                observed_at,
                None,
                excluded,
                false,
            );
            if !complete {
                return None;
            }
            sharing |= measured.hardlinked;
            rows.retain(|r| !Path::new(&r.path_or_object).starts_with(&path));
            rows.push(unowned_row(
                &path,
                measured.bytes,
                if sharing || measured.hardlinked {
                    SubtreeEstimate
                } else {
                    Subtree
                },
            ));
            continue;
        }
        let measured = measure_directory(&path).ok()?;
        sharing |= measured.hardlinked;
        let children: HashSet<PathBuf> = measured
            .children
            .iter()
            .map(|name| path.join(name))
            .collect();
        // A removed known checkout needs the normal discovery/history path.
        if worktrees
            .iter()
            .any(|w| w.starts_with(&path) && !crate::fs_gate::exists(w))
        {
            return None;
        }
        rows.retain(|r| {
            let p = Path::new(&r.path_or_object);
            let Ok(rel) = p.strip_prefix(&path) else {
                return true;
            };
            rel.components()
                .next()
                .is_some_and(|c| children.contains(&path.join(c)))
        });
        if measured.allocated > 0 {
            rows.push(UnownedRow {
                measurement: Some(if sharing || measured.hardlinked {
                    DirectEstimate
                } else {
                    Direct
                }),
                path_or_object: path.display().to_string(),
                bytes: measured.allocated,
                reason: if is_shared_cache_name(
                    path.file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default(),
                ) {
                    UnownedReason::SharedCache
                } else {
                    UnownedReason::NoContainingRepo
                },
                shared_bytes: None,
                note: None,
                docker_kind: None,
                created_at: None,
                containers: Vec::new(),
                shared_with: Vec::new(),
                dangling: false,
                evidence: Vec::new(),
            });
        }
        for child in children {
            if excluded.iter().any(|e| child.starts_with(e))
                || worktrees.iter().any(|w| child.starts_with(w))
            {
                continue;
            }
            if previous
                .iter()
                .any(|r| Path::new(&r.path_or_object).starts_with(&child))
            {
                continue;
            }
            // An unknown child might contain a new checkout. Discovery is
            // bounded to this new subtree, never an unchanged sibling.
            let mut pruned = excluded.to_vec();
            pruned.extend_from_slice(worktrees);
            if !discover_parallel_excluding(&child, &pruned)
                .ok()?
                .is_empty()
            {
                return None;
            }
            let classified = unowned_classification(&child);
            let fresh = if let Some(classified) = classified {
                if worktrees.iter().any(|w| w.starts_with(&child)) {
                    return None;
                }
                let (measured, _, _, complete) = resize_artifact_stamped(
                    &child,
                    classified.kind().clone(),
                    observed_at,
                    None,
                    &pruned,
                    false,
                );
                if !complete {
                    return None;
                }
                vec![unowned_row(
                    &child,
                    measured.bytes,
                    if sharing || measured.hardlinked {
                        SubtreeEstimate
                    } else {
                        Subtree
                    },
                )]
            } else {
                attribute_parallel_carrying(
                    &child,
                    &[],
                    observed_at,
                    u64::MAX,
                    HashMap::new(),
                    &pruned,
                )
                .unowned
            };
            if fresh
                .iter()
                .any(|r| matches!(r.measurement, None | Some(Hardlinked)))
            {
                return None;
            }
            rows.retain(|r| !Path::new(&r.path_or_object).starts_with(&child));
            sharing |= fresh
                .iter()
                .any(|r| !matches!(r.measurement, Some(Direct | Subtree)));
            rows.extend(fresh);
        }
    }
    if sharing {
        for row in &mut rows {
            row.measurement = row.measurement.map(|m| {
                if m.folded() {
                    SubtreeEstimate
                } else {
                    DirectEstimate
                }
            });
        }
    }
    Some(rows)
}

fn unowned_classification(path: &Path) -> Option<Classified> {
    classified_at(path.parent()?, path.file_name()?.to_str()?)
}

/// Re-walks exactly one worktree, matching `discover_and_attribute`'s
/// per-worktree slice of `attribute_parallel` but seeded at
/// `worktree_root` instead of the whole scan root. This is the fallback
/// granularity for a changed directory that lands in a worktree's Source
/// tree (not inside an already-classified artifact directory): re-walking
/// the one worktree is far cheaper than re-walking the whole root.
///
/// `all_worktrees` must be the *complete* known worktree list (every
/// worktree, not just this one), exactly as `discover_and_attribute`
/// passes to `attribute_parallel`. A linked worktree frequently lives
/// **inside** its main checkout's directory tree (e.g. `.worktrees/<name>`
/// under the project root), so walking `worktree_root` with only this one
/// worktree in the known list would mean `nearest_worktree` matches every
/// path under it -- including a nested linked worktree's own
/// `target`/`node_modules`/Source bytes -- to *this* worktree_id, double
/// counting them on top of that nested worktree's own carried-forward
/// rows. Passing the full list keeps nested-worktree boundaries exactly
/// as a full walk would; only the entry keyed by `worktree_id` in the
/// result is meant to be merged back in by the caller, since every other
/// worktree's rows are untouched by construction and already carried
/// forward from the store.
///
/// Hardlink dedup is scoped to this call: a file already counted in this
/// worktree during a broader walk could in principle be seen as "new"
/// here. This only matters for the rare cross-worktree hardlink, and
/// losing that dedup precision on an incremental pass (never on a full
/// walk) is an accepted, documented trade for not having to carry the
/// whole tree's inode set forward between observations.
pub fn attribute_one_worktree(
    _stage: &crate::bus::Stage,
    worktree_root: &Path,
    all_worktrees: &[(&Path, &str)],
    observed_at: u64,
    large_file_min_bytes: u64,
    carry: HashMap<PathBuf, ArtifactRow>,
) -> AttributionResult {
    // The incremental caller (`growth::stage_tracked_with_source`) filters
    // `changed_dirs` against `pruned_subtrees` before ever reaching here,
    // so this re-walk never targets an excluded worktree.
    attribute_parallel_carrying(
        worktree_root,
        all_worktrees,
        observed_at,
        large_file_min_bytes,
        carry,
        &[],
    )
}

/// Re-sizes exactly one already-classified artifact directory as a unit,
/// matching `process_size`'s fold semantics (regular files, hardlink
/// deduped within this call, symlinks and unreadable entries skipped).
/// This is the common incremental case named in #29's acceptance test:
/// FSEvents implicates a directory inside an existing artifact root (e.g.
/// `node_modules/some-pkg`), so only that one row needs re-sizing and
/// every other artifact/Source row in the worktree carries forward with
/// its previously observed bytes untouched.
pub fn resize_artifact(root_path: &Path, kind: ArtifactKind, observed_at: u64) -> ArtifactRow {
    resize_artifact_with_dirs(root_path, kind, observed_at, None).0
}

/// `resize_artifact`, excluding `excluded` subtrees from the measurement
/// entirely -- not measured, not folded into the total, exactly like
/// `process_walk`'s config-exclusion contract (#42). Used by
/// `crate::external::discover_and_measure` so a location that contains
/// another, separately-measured detector location (e.g. Cargo home
/// containing the registry/git subtrees it also proposes as their own
/// units) is not double-measured (#45-#49).
pub fn resize_artifact_excluding(
    root_path: &Path,
    kind: ArtifactKind,
    observed_at: u64,
    excluded: &[PathBuf],
) -> ArtifactRow {
    resize_artifact_with_dirs_excluding(root_path, kind, observed_at, None, excluded).0
}

/// `resize_artifact` that also returns the unit's interior directory
/// rollups (relative to `worktree`), for the store.
pub fn resize_artifact_with_dirs(
    root_path: &Path,
    kind: ArtifactKind,
    observed_at: u64,
    worktree: Option<(&str, &Path)>,
) -> (ArtifactRow, Vec<DirRollup>) {
    resize_artifact_with_dirs_excluding(root_path, kind, observed_at, worktree, &[])
}

/// `resize_artifact_with_dirs`, excluding `excluded` subtrees (see
/// [`resize_artifact_excluding`]).
pub fn resize_artifact_with_dirs_excluding(
    root_path: &Path,
    kind: ArtifactKind,
    observed_at: u64,
    worktree: Option<(&str, &Path)>,
    excluded: &[PathBuf],
) -> (ArtifactRow, Vec<DirRollup>) {
    let (row, dirs, _, _) =
        resize_artifact_stamped(root_path, kind, observed_at, worktree, excluded, false);
    (row, dirs)
}

/// [`resize_artifact_with_dirs_excluding`] that can also return one
/// [`DirStamp`] per directory it listed, so the next pass can decide
/// whether this unit still measures the same without listing anything
/// (`folded_measurement::reuse_folded_measurement`). The stamps come
/// from the `symlink_metadata` each `Size` job already takes, so
/// `stamp_dirs` costs a push per directory and no extra syscall.
pub fn resize_artifact_stamped(
    root_path: &Path,
    kind: ArtifactKind,
    observed_at: u64,
    worktree: Option<(&str, &Path)>,
    excluded: &[PathBuf],
    stamp_dirs: bool,
) -> (ArtifactRow, Vec<DirRollup>, Vec<DirStamp>, bool) {
    // Same machinery as the full walk's folded units: the root is one
    // Size job, subdirectories fan out across the pool. A 16 GB `target/`
    // took ~1.8 s serially; on the pool it takes what the full walk
    // spends on it.
    let shared = Arc::new(AttrShared {
        seen_inodes: ShardedInodeSet::new(),
        sharing: None,
        artifacts_by_worktree: Mutex::new(HashMap::new()),
        source_bytes: Mutex::new(HashMap::new()),
        source_local: Mutex::new(HashMap::new()),
        unowned: Mutex::new(Vec::new()),
        unowned_hardlinks: Mutex::new(HashSet::new()),
        walked_total: AtomicU64::new(0),
        attributed_total: AtomicU64::new(0),
        unowned_total: AtomicU64::new(0),
        observed_at,
        dirs: Mutex::new(HashMap::new()),
        files: Mutex::new(Vec::new()),
        large_file_min_bytes: u64::MAX,
        carry: HashMap::new(),
        excluded: excluded.to_vec(),
        dir_stamps: Mutex::new(Vec::new()),
        stamp_dirs,
        incomplete: AtomicBool::new(false),
    });
    let wt_id = worktree
        .map(|(id, _)| id.to_string())
        .unwrap_or_else(|| "resize".to_string());
    let group = Arc::new(SizeGroup {
        root_path: root_path.to_path_buf(),
        kind: kind.clone(),
        worktree: Some(wt_id.clone()),
        worktree_root: worktree.map(|(_, r)| r.to_path_buf()),
        total: AtomicU64::new(0),
        local_total: AtomicU64::new(0),
        local_seen: Mutex::new(HashSet::new()),
        remaining: AtomicUsize::new(1),
        mtime_max: AtomicU64::new(0),
    });
    let pool: Arc<Pool<AttrJob>> = Arc::new(Pool::new());
    // The kind is the store's: this path was classified when it was
    // first walked, and a re-size never reclassifies.
    pool.push(AttrJob::Size {
        path: root_path.to_path_buf(),
        group,
        classified: Classified::stored(kind.clone()),
    });
    pool.drain(worker_count(), |job| match job {
        AttrJob::Walk(path) => process_walk(path, &[], &shared, &pool),
        AttrJob::Size {
            path,
            group,
            classified,
        } => process_size(path, &group, &classified, &shared, &pool),
    });
    let shared = Arc::try_unwrap(shared).unwrap_or_else(|_| unreachable!("workers joined"));
    let complete = !shared.incomplete.load(Ordering::Relaxed);
    let dirs: Vec<DirRollup> = shared.dirs.into_inner().unwrap().into_values().collect();
    let stamps: Vec<DirStamp> = shared.dir_stamps.into_inner().unwrap();
    let mut rows = shared
        .artifacts_by_worktree
        .into_inner()
        .unwrap()
        .remove(&wt_id)
        .unwrap_or_default();
    let mut row = rows.pop().unwrap_or(ArtifactRow {
        kind: kind.clone(),
        path: root_path.to_path_buf(),
        bytes: 0,
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
        confidence: Confidence::High,
        source: Source::new("filesystem.fsevents"),
        note: None,
        created_at: None,
        containers: Vec::new(),
        shared_with: Vec::new(),
        dangling: false,
        evidence: Vec::new(),
    });
    row.kind = kind;
    row.source = Source::new("filesystem.fsevents");
    // A lone re-size sees every inode once, so its `bytes` are its
    // `local_bytes` (the full walk charges shared inodes to whichever row
    // saw them first; the incremental merge applies the local delta).
    row.local_bytes = row.bytes.max(row.local_bytes);
    (row, dirs, stamps, complete)
}

/// Checkouts at `dir` and its immediate children only: what a changed
/// directory can have gained or lost. FSEvents names the directory whose
/// listing changed, so a new clone under `~/src` shows up as `~/src` (and
/// the clone itself); a recursive discovery of `~/src` here cost ~0.5 s
/// per incremental observation and found nothing new.
pub fn discover_shallow(dir: &Path) -> Vec<DiscoveredWorktree> {
    crate::work_counters::record_files_statted(1);
    let Ok(meta) = crate::fs_gate::symlink_metadata(dir) else {
        return Vec::new();
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Vec::new();
    }
    let device = meta.dev();
    let discovered: Mutex<Vec<DiscoveredWorktree>> = Mutex::new(Vec::new());
    let pool: Pool<PathBuf> = Pool::new();
    // The incremental path filters `changed_dirs` against
    // `pruned_subtrees` before this is ever called (see
    // `growth::stage_tracked_with_source`), so no exclusion list is
    // needed here.
    discover_one(dir, device, &pool, &discovered, &[]);
    // discover_one queued the children it would have recursed into; take
    // exactly one level of them, without recursing further.
    while let Some(child) = pool.try_pop() {
        crate::work_counters::record_files_statted(2);
        let Ok(cm) = crate::fs_gate::symlink_metadata(&child) else {
            continue;
        };
        if cm.file_type().is_symlink() || !cm.is_dir() || cm.dev() != device {
            continue;
        }
        let git_path = child.join(".git");
        if let Ok(git_meta) = crate::fs_gate::symlink_metadata(&git_path) {
            let dw = if git_meta.is_dir() {
                classify_main_checkout(&child, &git_path)
            } else if git_meta.is_file() {
                classify_git_file(&child, &git_path)
            } else {
                None
            };
            if let Some(dw) = dw {
                discovered.lock().unwrap().push(dw);
            }
        }
    }
    discovered.into_inner().unwrap()
}

// ---------------------------------------------------------------------
// Combined entry point used by `report::report_with`.
// ---------------------------------------------------------------------

/// Runs the parallel discovery pass, then the parallel attribution pass
/// over the resulting worktree list, matching the two sequential calls
/// `report_with` used to make to `git::discover` and
/// `attribution::attribute`. `excluded` (#42 --
/// `scope::EffectiveScope::pruned_subtrees`) prunes every subtree in it
/// from both passes: nothing under an excluded path is discovered as a
/// worktree, measured, or reported as unowned. Empty for every caller
/// with no scope-level exclusions to enforce.
pub fn discover_and_attribute(
    _stage: &crate::bus::Stage,
    root: &Path,
    observed_at: u64,
    large_file_min_bytes: u64,
    excluded: &[PathBuf],
) -> Result<(Vec<DiscoveredWorktree>, AttributionResult)> {
    let trace = std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty());
    let t0 = std::time::Instant::now();
    let discovered = discover_parallel_excluding(root, excluded)?;
    if trace {
        eprintln!("[trace] walk::discover_parallel: {:?}", t0.elapsed());
    }
    let worktree_ids: Vec<(PathBuf, String)> = discovered
        .iter()
        .map(|dw| (dw.path.clone(), id_for(&dw.path.display().to_string())))
        .collect();
    let worktree_refs: Vec<(&Path, &str)> = worktree_ids
        .iter()
        .map(|(p, id)| (p.as_path(), id.as_str()))
        .collect();
    let t1 = std::time::Instant::now();
    let attribution = attribute_parallel_carrying(
        root,
        &worktree_refs,
        observed_at,
        large_file_min_bytes,
        HashMap::new(),
        excluded,
    );
    if trace {
        eprintln!("[trace] walk::attribute_parallel: {:?}", t1.elapsed());
    }
    Ok((discovered, attribution))
}

#[cfg(test)]
mod pool_stress {
    use super::Pool;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// Regression for #36: run many small pools to completion while every
    /// core is saturated by busy threads. Before the fix this deadlocked
    /// within a few hundred iterations on a loaded machine.
    #[test]
    fn pool_drains_under_cpu_contention() {
        let stop = Arc::new(AtomicBool::new(false));
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let burners: Vec<_> = (0..cores)
            .map(|_| {
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    let mut x: u64 = 1;
                    while !stop.load(Ordering::Relaxed) {
                        x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
                    }
                    x
                })
            })
            .collect();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        for round in 0..400 {
            assert!(
                std::time::Instant::now() < deadline,
                "pool stress exceeded 60 s at round {round}"
            );
            let pool = Arc::new(Pool::<u32>::new());
            let done = Arc::new(AtomicUsize::new(0));
            for j in 0..64u32 {
                pool.push(j);
            }
            let done2 = Arc::clone(&done);
            let pool2 = Arc::clone(&pool);
            pool.drain(cores * 2, move |j| {
                // fan out two children per job for the first two levels,
                // mirroring directory recursion
                if j < 64 {
                    pool2.push(1000 + j);
                    pool2.push(2000 + j);
                }
                done2.fetch_add(1, Ordering::SeqCst);
            });
            assert_eq!(done.load(Ordering::SeqCst), 64 * 3, "round {round}");
        }
        stop.store(true, Ordering::Relaxed);
        for b in burners {
            let _ = b.join();
        }
    }
}
