//! Linux continuity between observations (#82): what a live watch can
//! let a *later* process reuse, and the exact conditions under which it
//! cannot.
//!
//! Linux keeps no change history, so a one-shot `swamp report` has
//! nothing to replay and walks fully -- unless a user-owned **collector**
//! (`swamp collect`, opt-in, never started by swamp on its own) has been
//! watching the root the whole time since the last observation. The
//! collector keeps a bounded **checkpoint** under the store,
//! `continuity/<root-id>.json`:
//!
//! * the root's identity (canonical path, `st_dev`, root inode) and the
//!   boot it was written in;
//! * the watch **epoch** -- an id and `opened_at`, the second the
//!   watch's registration finished ([`crate::live_watch`]);
//! * whether coverage has held since `opened_at`, and if not the named
//!   loss;
//! * the dirty directories since `opened_at`, each with a sequence
//!   number, bounded by [`crate::live_watch::DIRTY_BOUND`];
//! * the exclusions it watches under.
//!
//! An observation ([`CollectorSource`]) reuses that list -- an
//! incremental walk of exactly those directories -- only when **all** of
//! these hold, and otherwise walks fully naming the first that fails:
//!
//! | condition | refusal |
//! |---|---|
//! | a collector for this root is alive (holds its lock) | `no_persisted_change_history` (none ever) / `collector_stopped` |
//! | same boot, same root identity | `collector_stopped` / `root_mismatch` |
//! | its exclusions do not hide anything this observation walks | `scope_changed` |
//! | it confirms it has drained its event queue ([`sync`]) | `collector_unresponsive` |
//! | coverage held since the epoch opened | the loss (`watch_queue_overflow`, ...) |
//! | the stored observation is inside the epoch | `live_watch_gap`, or the loss that ended the previous epoch |
//!
//! **Consumption follows the history write.** The plan carries a
//! [`Consumption`]: "entries up to sequence N were re-walked". It is
//! applied by `growth::ObservationCheckpoint::commit` *after* every
//! history table and the replay cursor are written, under the dirty-set
//! lock the collector also takes. A crash before the commit leaves the
//! entries (the next run re-walks them); a crash between the history
//! write and the consumption leaves them too (re-walked again: redundant,
//! never wrong). An entry dirtied again after N keeps its newer sequence
//! and survives.
//!
//! macOS never uses any of this: FSEvents replays history, and
//! [`crate::fs_events::platform_source`] there is the replay.

use crate::fs_events::{FsEventsPlan, FsEventsRequest, FsEventsSource, RefreshRefusal};
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long an observation waits for the collector to confirm it has
/// read every event queued before the request. Overridable for tests
/// via `SWAMP_COLLECTOR_SYNC_TIMEOUT_MS`.
pub const SYNC_TIMEOUT: Duration = Duration::from_secs(2);

fn sync_timeout() -> Duration {
    std::env::var("SWAMP_COLLECTOR_SYNC_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(SYNC_TIMEOUT)
}

/// A loss, as persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LossRecord {
    /// A `RefreshRefusal::as_str` code.
    pub reason: String,
    pub detail: String,
    pub at: u64,
}

/// One dirty directory, relative to the root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirtyEntry {
    pub p: String,
    pub s: u64,
}

/// The collector's checkpoint for one root. Small by construction: the
/// dirty list is bounded, everything else is a handful of scalars.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub root: PathBuf,
    pub device: u64,
    pub root_ino: u64,
    pub boot_id: Option<String>,
    pub epoch_id: String,
    pub opened_at: u64,
    pub pid: u32,
    /// `None` while coverage has held since `opened_at`.
    pub lost: Option<LossRecord>,
    /// The loss that ended the previous epoch, if one did.
    pub previous_loss: Option<LossRecord>,
    pub seq: u64,
    pub dirty: Vec<DirtyEntry>,
    pub excluded: Vec<PathBuf>,
    pub sync_token: Option<String>,
    pub flushed_at: u64,
    pub stopped_at: Option<u64>,
    pub watches: u64,
    pub max_user_watches: Option<u64>,
    pub kernel_bytes_estimate: u64,
}

/// Where one root's continuity files live.
#[derive(Debug, Clone)]
pub struct Paths {
    pub dir: PathBuf,
    pub checkpoint: PathBuf,
    /// Held exclusively by the collector for as long as it runs. The
    /// kernel releases it when the process dies, however it dies.
    pub alive: PathBuf,
    /// Taken by every read-modify-write of the checkpoint's dirty list.
    pub dirty_lock: PathBuf,
    /// An observation's "drain your queue and tell me" request.
    pub sync: PathBuf,
}

pub fn paths(store: &Path, root: &Path) -> Paths {
    let id = crate::growth::root_scoped_volume_id(root);
    let dir = store.join("continuity");
    Paths {
        checkpoint: dir.join(format!("{id}.parquet")),
        alive: dir.join(format!("{id}.lock")),
        dirty_lock: dir.join(format!("{id}.dirty.lock")),
        sync: dir.join(format!("{id}.sync")),
        dir,
    }
}

fn entries_path(checkpoint: &Path) -> PathBuf {
    let stem = checkpoint
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    checkpoint.with_file_name(format!("{stem}_entries.parquet"))
}

/// The collector's checkpoint for one root (`<id>.parquet`, one row,
/// with its dirty/excluded paths in `<id>_entries.parquet`) -- R18b,
/// replacing `<id>.json`.
pub fn read_checkpoint(p: &Paths) -> Option<Checkpoint> {
    use crate::growth::columns as c;
    let row = c::read_checkpoint_rows(&p.checkpoint)
        .ok()?
        .into_iter()
        .next()?;
    let mut entries = c::read_checkpoint_entry_rows(&entries_path(&p.checkpoint)).ok()?;
    entries.sort_by_key(|e| e.seq);
    let loss = |reason: Option<String>, detail: Option<String>, at: Option<u64>| {
        reason.map(|reason| LossRecord {
            reason,
            detail: detail.unwrap_or_default(),
            at: at.unwrap_or(0),
        })
    };
    Some(Checkpoint {
        root: PathBuf::from(row.root),
        device: row.device,
        root_ino: row.root_ino,
        boot_id: row.boot_id,
        epoch_id: row.epoch_id,
        opened_at: row.opened_at,
        pid: row.pid,
        lost: loss(row.lost_reason, row.lost_detail, row.lost_at),
        previous_loss: loss(
            row.previous_loss_reason,
            row.previous_loss_detail,
            row.previous_loss_at,
        ),
        seq: row.seq,
        dirty: entries
            .iter()
            .filter(|e| e.kind == "dirty")
            .map(|e| DirtyEntry {
                p: e.path.clone(),
                s: e.size.unwrap_or(0),
            })
            .collect(),
        excluded: entries
            .iter()
            .filter(|e| e.kind == "excluded")
            .map(|e| PathBuf::from(&e.path))
            .collect(),
        sync_token: row.sync_token,
        flushed_at: row.flushed_at,
        stopped_at: row.stopped_at,
        watches: row.watches,
        max_user_watches: row.max_user_watches,
        kernel_bytes_estimate: row.kernel_bytes_estimate,
    })
}

pub fn write_checkpoint(p: &Paths, c: &Checkpoint) -> Result<()> {
    use crate::growth::columns as cols;
    crate::fs_gate::continuity::ensure_dir(&p.dir)?;
    let mut entries: Vec<cols::StoredCheckpointEntryRow> = Vec::new();
    for (seq, d) in c.dirty.iter().enumerate() {
        entries.push(cols::StoredCheckpointEntryRow {
            kind: "dirty".to_string(),
            seq: seq as u32,
            path: d.p.clone(),
            size: Some(d.s),
        });
    }
    for (seq, e) in c.excluded.iter().enumerate() {
        entries.push(cols::StoredCheckpointEntryRow {
            kind: "excluded".to_string(),
            seq: (c.dirty.len() + seq) as u32,
            path: e.display().to_string(),
            size: None,
        });
    }
    cols::write_checkpoint_entry_rows(&entries_path(&p.checkpoint), &entries)
        .with_context(|| format!("publish {}", entries_path(&p.checkpoint).display()))?;
    cols::write_checkpoint_rows(
        &p.checkpoint,
        &[cols::StoredCheckpointRow {
            root: c.root.display().to_string(),
            device: c.device,
            root_ino: c.root_ino,
            boot_id: c.boot_id.clone(),
            epoch_id: c.epoch_id.clone(),
            opened_at: c.opened_at,
            pid: c.pid,
            lost_reason: c.lost.as_ref().map(|l| l.reason.clone()),
            lost_detail: c.lost.as_ref().map(|l| l.detail.clone()),
            lost_at: c.lost.as_ref().map(|l| l.at),
            previous_loss_reason: c.previous_loss.as_ref().map(|l| l.reason.clone()),
            previous_loss_detail: c.previous_loss.as_ref().map(|l| l.detail.clone()),
            previous_loss_at: c.previous_loss.as_ref().map(|l| l.at),
            seq: c.seq,
            sync_token: c.sync_token.clone(),
            flushed_at: c.flushed_at,
            stopped_at: c.stopped_at,
            watches: c.watches,
            max_user_watches: c.max_user_watches,
            kernel_bytes_estimate: c.kernel_bytes_estimate,
        }],
    )
    .with_context(|| format!("publish {}", p.checkpoint.display()))?;
    Ok(())
}

/// This boot's id, where the kernel has one (Linux). Two checkpoints
/// from different boots are different epochs whatever they claim.
pub fn boot_id() -> Option<String> {
    match crate::fs_gate::continuity::read_text("/proc/sys/kernel/random/boot_id") {
        Ok(s) => Some(s.trim().to_string()),
        Err(_) => None,
    }
}

// ---------------------------------------------------------------------
// Advisory locks
// ---------------------------------------------------------------------

/// An `flock` held for as long as this value lives. The type (and the
/// `std::fs::File` it wraps) lives in `fs_gate::continuity`, inside the
/// capability gate; re-exported here under its original name since this
/// is where every caller already points.
pub use crate::fs_gate::continuity::FileLock;

/// Tries to take `path`'s lock without blocking. `Ok(None)` when another
/// process holds it.
pub fn try_lock(path: &Path, exclusive: bool) -> std::io::Result<Option<FileLock>> {
    crate::fs_gate::continuity::try_lock(path, exclusive)
}

/// Whether a collector holds this root's alive lock right now. Never
/// creates the lock file: asking must leave no state behind.
pub fn collector_alive(p: &Paths) -> bool {
    crate::fs_gate::continuity::collector_alive(&p.alive)
}

/// Takes `path`'s lock, waiting up to `timeout`.
pub fn lock_wait(path: &Path, exclusive: bool, timeout: Duration) -> Result<FileLock> {
    let start = Instant::now();
    loop {
        if let Some(l) = try_lock(path, exclusive)? {
            return Ok(l);
        }
        if start.elapsed() > timeout {
            anyhow::bail!(
                "{} is still locked by another swamp process after {}s",
                path.display(),
                timeout.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ---------------------------------------------------------------------
// The consumer side
// ---------------------------------------------------------------------

/// "The entries of epoch `epoch_id` up to `through_seq` were re-walked
/// by an observation whose history is now written."
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Consumption {
    pub checkpoint: PathBuf,
    pub dirty_lock: PathBuf,
    pub epoch_id: String,
    pub through_seq: u64,
}

/// Applies a consumption under the dirty-set lock. A checkpoint from a
/// different epoch is left alone: its entries are not the ones this
/// observation re-walked.
pub fn consume(c: &Consumption) -> Result<()> {
    let _lock = lock_wait(&c.dirty_lock, true, Duration::from_secs(10))?;
    let dir = c
        .checkpoint
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let p = Paths {
        dir,
        checkpoint: c.checkpoint.clone(),
        alive: PathBuf::new(),
        dirty_lock: c.dirty_lock.clone(),
        sync: PathBuf::new(),
    };
    let Some(mut ck) = read_checkpoint(&p) else {
        return Ok(());
    };
    if ck.epoch_id != c.epoch_id {
        return Ok(());
    }
    ck.dirty.retain(|d| d.s > c.through_seq);
    write_checkpoint(&p, &ck)
}

/// Asks the collector to drain its queue and waits for it to say so.
/// The request is a file in the directory the collector watches on the
/// *same* inotify instance as the root, so it is queued after every
/// event that happened before it; when the collector answers, all of
/// those are in the checkpoint.
pub fn sync(p: &Paths, timeout: Duration) -> Result<Option<Checkpoint>> {
    let token = uuid::Uuid::new_v4().to_string();
    let tmp = p
        .sync
        .with_extension(format!("sync.{}.tmp", std::process::id()));
    let dir = p.sync.parent().map(Path::to_path_buf).unwrap_or_default();
    crate::fs_gate::continuity::write_atomic(&dir, &tmp, &p.sync, token.as_bytes())?;
    let start = Instant::now();
    loop {
        if let Some(ck) = read_checkpoint(p)
            && ck.sync_token.as_deref() == Some(token.as_str())
        {
            return Ok(Some(ck));
        }
        if start.elapsed() > timeout {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The plan an observation gets from a collector checkpoint, or the
/// named refusal. `boot` is this boot's id; `live` answers "is the
/// collector alive" (the real one tries its lock).
pub fn plan_from_checkpoint(
    req: &FsEventsRequest,
    p: &Paths,
    boot: Option<&str>,
    alive: bool,
    timeout: Duration,
) -> FsEventsPlan {
    use crate::fs_gate::MetadataExt;
    let device = crate::fs_gate::metadata_following(&req.root)
        .ok()
        .map(|m| m.dev());
    let refuse = |r: RefreshRefusal| FsEventsPlan::refused(r, device);
    if !crate::fs_gate::exists(&p.checkpoint) && !crate::fs_gate::exists(&p.alive) {
        return FsEventsPlan::refused(crate::fs_events::platform_refusal(), device);
    }
    if !alive {
        return refuse(RefreshRefusal::CollectorStopped);
    }
    let ck = match sync(p, timeout) {
        Ok(Some(ck)) => ck,
        Ok(None) | Err(_) => return refuse(RefreshRefusal::CollectorUnresponsive),
    };
    if ck.boot_id.as_deref() != boot {
        return refuse(RefreshRefusal::CollectorStopped);
    }
    let here = crate::fs_gate::metadata_following(&req.root).ok();
    if ck.root != req.root
        || here
            .as_ref()
            .is_none_or(|m| m.dev() != ck.device || m.ino() != ck.root_ino)
    {
        return refuse(RefreshRefusal::RootMismatch);
    }
    // The collector may watch *more* than this observation walks (fewer
    // exclusions: its extra entries are filtered below), never less.
    let store = p.dir.parent().map(Path::to_path_buf);
    let hidden = ck.excluded.iter().any(|e| {
        Some(e) != store.as_ref()
            && !req
                .excluded
                .iter()
                .any(|mine| e == mine || e.starts_with(mine))
    });
    if hidden {
        return refuse(RefreshRefusal::ScopeChanged);
    }
    if let Some(lost) = &ck.lost {
        return refuse(code_to_refusal(&lost.reason));
    }
    // From here the collector is live, synced and complete: a full walk
    // this observation makes now starts after every entry in its list,
    // so it consumes them as an incremental one would.
    let consumption = Consumption {
        checkpoint: p.checkpoint.clone(),
        dirty_lock: p.dirty_lock.clone(),
        epoch_id: ck.epoch_id.clone(),
        through_seq: ck.seq,
    };
    let refuse_consuming = |r: RefreshRefusal| {
        let mut plan = FsEventsPlan::refused(r, device);
        plan.consume = Some(consumption.clone());
        plan
    };
    let Some(since) = req.since.last_observed_at else {
        return refuse_consuming(RefreshRefusal::NoStoredEventId);
    };
    if since < ck.opened_at {
        // Name the loss that ended the previous epoch when it happened
        // after the stored observation; otherwise it is the plain gap.
        let r = match &ck.previous_loss {
            Some(l) if l.at >= since => code_to_refusal(&l.reason),
            _ => RefreshRefusal::LiveWatchGap,
        };
        return refuse_consuming(r);
    }
    let changed: Vec<PathBuf> = ck
        .dirty
        .iter()
        .map(|d| {
            if d.p.is_empty() {
                ck.root.clone()
            } else {
                ck.root.join(&d.p)
            }
        })
        .collect();
    let mut plan = FsEventsPlan::from_live(changed, ck.seq, device);
    plan.consume = Some(consumption);
    plan
}

fn code_to_refusal(code: &str) -> RefreshRefusal {
    RefreshRefusal::ALL
        .iter()
        .copied()
        .find(|r| r.as_str() == code)
        // An unrecognised code is still a loss: never read as coverage.
        .unwrap_or(RefreshRefusal::WatchRemoved)
}

/// The Linux platform source: a live collector's checkpoint, or the
/// platform's refusal when there is none.
pub struct CollectorSource;

impl FsEventsSource for CollectorSource {
    fn replay(&self, req: &FsEventsRequest) -> FsEventsPlan {
        let Some(store) = req.swamp_dir.as_deref() else {
            return FsEventsPlan::refused(crate::fs_events::platform_refusal(), None);
        };
        let p = paths(store, &req.root);
        plan_from_checkpoint(
            req,
            &p,
            boot_id().as_deref(),
            collector_alive(&p),
            sync_timeout(),
        )
    }
}

// ---------------------------------------------------------------------
// The collector (Linux)
// ---------------------------------------------------------------------

/// Makes SIGINT and SIGTERM stop the collector cleanly (a final flush
/// marked `stopped_at`), and returns the flag they set. The signal
/// handler itself (`extern "C"`, `libc::signal`) lives in
/// `fs_gate::sys`, inside the capability gate.
#[cfg(target_os = "linux")]
pub fn stop_on_signals() -> &'static std::sync::atomic::AtomicBool {
    crate::fs_gate::continuity::stop_on_signals()
}

/// One root the collector watches, and what it must not watch under it.
#[derive(Debug, Clone)]
pub struct CollectRoot {
    pub root: PathBuf,
    pub excluded: Vec<PathBuf>,
}

/// Runs the collector for `roots` until `stop` is set: one thread per
/// root, each holding that root's alive lock for its whole life.
/// Refuses a root another collector already holds.
#[cfg(target_os = "linux")]
pub fn run_collector(
    store: &Path,
    roots: &[CollectRoot],
    stop: &'static std::sync::atomic::AtomicBool,
    log: impl Fn(&str) + Send + Sync + 'static,
) -> Result<()> {
    let log = std::sync::Arc::new(log);
    let mut handles = Vec::new();
    for r in roots {
        let p = paths(store, &r.root);
        let Some(alive) = try_lock(&p.alive, true)? else {
            anyhow::bail!(
                "another collector is already running for {} (it holds {})",
                r.root.display(),
                p.alive.display()
            );
        };
        let (store, root, log) = (store.to_path_buf(), r.clone(), log.clone());
        handles.push(std::thread::spawn(move || {
            let _alive = alive;
            if let Err(e) = collect_one(&store, &root, stop, &*log) {
                log(&format!(
                    "collector for {} stopped: {e:#}",
                    root.root.display()
                ));
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn collect_one(
    store: &Path,
    r: &CollectRoot,
    stop: &std::sync::atomic::AtomicBool,
    log: &dyn Fn(&str),
) -> Result<()> {
    use crate::fs_gate::MetadataExt;
    use crate::live_watch::{Coverage, LiveTree, inotify};
    use std::sync::atomic::Ordering;

    let p = paths(store, &r.root);
    crate::fs_gate::continuity::ensure_dir(&p.dir)?;
    let root = crate::fs_gate::canonicalize(&r.root)?;
    let meta = crate::fs_gate::metadata_following(&root)?;
    let mut excluded = r.excluded.clone();
    excluded.push(store.to_path_buf());
    let kernel = inotify::Inotify::new().context("inotify_init1")?;
    let mut tree = LiveTree::new(&root, excluded.clone(), kernel, inotify::limits())?;
    // The control watch, on the same instance as the tree, so a sync
    // request is queued behind every earlier event.
    let control = tree.kernel_mut().add_control(&p.dir)?;
    tree.register_all();
    let first = tree.kernel_mut().read(0).unwrap_or_default();
    let (ctl, rest): (Vec<_>, Vec<_>) = first.into_iter().partition(|e| e.wd == control);
    tree.apply(&rest);
    tree.open();

    let boot = boot_id();
    let new_epoch = || uuid::Uuid::new_v4().to_string();
    let mut epoch = new_epoch();
    let mut previous_loss: Option<LossRecord> = None;
    let mut sync_token: Option<String> = None;
    let mut pending_sync = !ctl.is_empty();
    let mut last_flush = Instant::now() - Duration::from_secs(60);
    let mut last_event = Instant::now();
    let mut dirty_since_flush = true;
    // An unrecoverable loss is written once, not on every pass.
    let mut loss_flushed = false;
    log(&format!(
        "watching {} ({} watches, epoch {epoch} opened at {})",
        root.display(),
        tree.stats().watches,
        tree.opened_at().unwrap_or(0)
    ));

    loop {
        let stopping = stop.load(Ordering::Relaxed);
        let evs = match tree.kernel_mut().read(if stopping { 0 } else { 200 }) {
            Ok(evs) => evs,
            Err(_) => {
                tree.lose_io();
                Vec::new()
            }
        };
        let (ctl, rest): (Vec<_>, Vec<_>) = evs.into_iter().partition(|e| e.wd == control);
        if !rest.is_empty() {
            last_event = Instant::now();
            dirty_since_flush = true;
        }
        tree.apply(&rest);
        if ctl.iter().any(|e| {
            e.name
                .as_ref()
                .is_some_and(|n| p.sync.file_name() == Some(n.as_os_str()))
        }) {
            pending_sync = true;
        }
        if pending_sync {
            sync_token = crate::fs_gate::continuity::read_text(&p.sync)
                .ok()
                .map(|s| s.trim().to_string());
        }
        let lost = match tree.coverage() {
            Coverage::Lost { loss, detail, at } => Some(LossRecord {
                reason: loss.as_str().to_string(),
                detail: detail.clone(),
                at: *at,
            }),
            Coverage::Complete => None,
        };
        let due = pending_sync
            || (lost.is_some() && !loss_flushed)
            || stopping
            || (dirty_since_flush
                && (last_event.elapsed() >= Duration::from_millis(400)
                    || last_flush.elapsed() >= Duration::from_secs(5)));
        if due {
            let stats = tree.stats();
            let fresh: Vec<(PathBuf, u64)> =
                tree.dirty().map(|(p, s)| (p.to_path_buf(), s)).collect();
            let _held = lock_wait(&p.dirty_lock, true, Duration::from_secs(10))?;
            let mut merged: std::collections::BTreeMap<String, u64> = match read_checkpoint(&p) {
                Some(ck) if ck.epoch_id == epoch => {
                    ck.dirty.into_iter().map(|d| (d.p, d.s)).collect()
                }
                _ => Default::default(),
            };
            for (path, s) in fresh {
                let rel = path
                    .strip_prefix(&root)
                    .map(|r| r.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let e = merged.entry(rel).or_insert(0);
                *e = (*e).max(s);
            }
            tree.consume_through(tree.seq());
            let over = merged.len() > crate::live_watch::DIRTY_BOUND;
            let ck = Checkpoint {
                root: root.clone(),
                device: meta.dev(),
                root_ino: meta.ino(),
                boot_id: boot.clone(),
                epoch_id: epoch.clone(),
                opened_at: tree.opened_at().unwrap_or(0),
                pid: std::process::id(),
                lost: lost.clone().or_else(|| {
                    over.then(|| LossRecord {
                        reason: RefreshRefusal::TooManyChanges.as_str().into(),
                        detail: format!(
                            "more than {} directories changed since the epoch opened",
                            crate::live_watch::DIRTY_BOUND
                        ),
                        at: crate::entities::now(),
                    })
                }),
                previous_loss: previous_loss.clone(),
                seq: tree.seq(),
                dirty: if over {
                    Vec::new()
                } else {
                    merged
                        .into_iter()
                        .map(|(p, s)| DirtyEntry { p, s })
                        .collect()
                },
                excluded: excluded.clone(),
                sync_token: sync_token.clone(),
                flushed_at: crate::entities::now(),
                stopped_at: stopping.then(crate::entities::now),
                watches: stats.watches as u64,
                max_user_watches: stats.limits.max_user_watches,
                kernel_bytes_estimate: stats.kernel_bytes_estimate,
            };
            write_checkpoint(&p, &ck)?;
            drop(_held);
            last_flush = Instant::now();
            dirty_since_flush = false;
            pending_sync = false;
            if let Some(l) = ck.lost {
                log(&format!(
                    "{}: coverage lost ({}: {})",
                    root.display(),
                    l.reason,
                    l.detail
                ));
                // A recoverable loss opens a new epoch: nothing before
                // now is vouched for, so the next observation walks
                // fully and the one after can reuse the list again.
                if tree.reopen_after_loss() || over {
                    if over {
                        tree.take_dirty();
                        tree.open();
                    }
                    previous_loss = Some(l);
                    epoch = new_epoch();
                    dirty_since_flush = true;
                    log(&format!("{}: new epoch {epoch}", root.display()));
                } else {
                    // Unrecoverable (watch limit, permissions): said once,
                    // and every sync request is still answered with it, so
                    // an observation hears it rather than timing out.
                    loss_flushed = true;
                }
            }
        }
        if stopping {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_events::FsEventsState;

    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf, Paths) {
        let tmp = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(tmp.path()).unwrap();
        let root = base.join("src");
        std::fs::create_dir_all(root.join("p/target")).unwrap();
        let store = base.join("store");
        let p = paths(&store, &root);
        (tmp, root, store, p)
    }

    fn checkpoint(root: &Path, opened_at: u64) -> Checkpoint {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(root).unwrap();
        Checkpoint {
            root: root.to_path_buf(),
            device: m.dev(),
            root_ino: m.ino(),
            boot_id: Some("boot-1".into()),
            epoch_id: "e1".into(),
            opened_at,
            pid: 1,
            lost: None,
            previous_loss: None,
            seq: 7,
            dirty: vec![DirtyEntry {
                p: "p/target".into(),
                s: 7,
            }],
            excluded: Vec::new(),
            sync_token: None,
            flushed_at: opened_at,
            stopped_at: None,
            watches: 3,
            max_user_watches: Some(8192),
            kernel_bytes_estimate: 3240,
        }
    }

    fn request(root: &Path, store: &Path, since: Option<u64>) -> FsEventsRequest {
        FsEventsRequest {
            root: root.to_path_buf(),
            since: FsEventsState {
                last_observed_at: since,
                ..Default::default()
            },
            swamp_dir: Some(store.to_path_buf()),
            excluded: Vec::new(),
        }
    }

    /// A fake collector: answers every new sync request by echoing its
    /// token into the checkpoint, as the real one does after draining.
    fn answer_syncs(p: &Paths, ck: Checkpoint) -> std::thread::JoinHandle<()> {
        let p = p.clone();
        let _ = std::fs::remove_file(&p.sync);
        write_checkpoint(&p, &ck).unwrap();
        std::thread::spawn(move || {
            let start = Instant::now();
            let mut answered = String::new();
            while start.elapsed() < Duration::from_millis(1500) {
                if let Ok(t) = std::fs::read_to_string(&p.sync)
                    && t.trim() != answered
                {
                    answered = t.trim().to_string();
                    let mut c = ck.clone();
                    c.sync_token = Some(answered.clone());
                    write_checkpoint(&p, &c).unwrap();
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    }

    #[test]
    fn a_live_collector_inside_its_epoch_yields_its_dirty_list_and_a_consumption() {
        let (_t, root, store, p) = setup();
        let h = answer_syncs(&p, checkpoint(&root, 1_000));
        let plan = plan_from_checkpoint(
            &request(&root, &store, Some(1_500)),
            &p,
            Some("boot-1"),
            true,
            Duration::from_secs(2),
        );
        h.join().unwrap();
        assert!(plan.incremental, "{:?}", plan.refusal);
        assert_eq!(plan.changed_dirs, vec![root.join("p/target")]);
        let c = plan
            .consume
            .expect("an incremental plan names what it consumes");
        assert_eq!((c.epoch_id.as_str(), c.through_seq), ("e1", 7));
    }

    /// The table in the module docs, one row at a time.
    #[test]
    fn every_way_continuity_fails_is_a_named_full_walk() {
        let (_t, root, store, p) = setup();
        let run = |ck: Option<Checkpoint>,
                   since: Option<u64>,
                   boot: &str,
                   alive: bool,
                   req_excl: Vec<PathBuf>| {
            if let Some(ck) = ck.clone() {
                let h = answer_syncs(&p, ck);
                let mut req = request(&root, &store, since);
                req.excluded = req_excl;
                let plan =
                    plan_from_checkpoint(&req, &p, Some(boot), alive, Duration::from_millis(500));
                let _ = h.join();
                plan
            } else {
                plan_from_checkpoint(
                    &request(&root, &store, since),
                    &p,
                    Some(boot),
                    alive,
                    Duration::from_millis(200),
                )
            }
        };
        // Never a collector: the platform's own refusal.
        let plan = run(None, Some(10), "boot-1", false, vec![]);
        assert_eq!(plan.refusal, Some(crate::fs_events::platform_refusal()));

        let base = checkpoint(&root, 1_000);
        // Dead collector.
        write_checkpoint(&p, &base).unwrap();
        assert_eq!(
            run(None, Some(1_500), "boot-1", false, vec![]).refusal,
            Some(RefreshRefusal::CollectorStopped)
        );
        // Alive but never answers.
        std::fs::remove_file(&p.sync).ok();
        assert_eq!(
            plan_from_checkpoint(
                &request(&root, &store, Some(1_500)),
                &p,
                Some("boot-1"),
                true,
                Duration::from_millis(100)
            )
            .refusal,
            Some(RefreshRefusal::CollectorUnresponsive)
        );
        // Another boot.
        assert_eq!(
            run(Some(base.clone()), Some(1_500), "boot-2", true, vec![]).refusal,
            Some(RefreshRefusal::CollectorStopped)
        );
        // Root replaced (different inode).
        let mut moved = base.clone();
        moved.root_ino += 1;
        assert_eq!(
            run(Some(moved), Some(1_500), "boot-1", true, vec![]).refusal,
            Some(RefreshRefusal::RootMismatch)
        );
        // The collector excludes something this observation walks.
        let mut narrow = base.clone();
        narrow.excluded = vec![root.join("p")];
        assert_eq!(
            run(Some(narrow.clone()), Some(1_500), "boot-1", true, vec![]).refusal,
            Some(RefreshRefusal::ScopeChanged)
        );
        // ... which is fine when this observation excludes it too.
        assert!(
            run(
                Some(narrow),
                Some(1_500),
                "boot-1",
                true,
                vec![root.join("p")]
            )
            .incremental
        );
        // Coverage lost.
        let mut lost = base.clone();
        lost.lost = Some(LossRecord {
            reason: "watch_queue_overflow".into(),
            detail: "x".into(),
            at: 1_200,
        });
        assert_eq!(
            run(Some(lost), Some(1_500), "boot-1", true, vec![]).refusal,
            Some(RefreshRefusal::WatchQueueOverflow)
        );
        // The stored observation predates the epoch.
        assert_eq!(
            run(Some(base.clone()), Some(999), "boot-1", true, vec![]).refusal,
            Some(RefreshRefusal::LiveWatchGap)
        );
        // ... and the previous epoch ended in a loss after it: name it.
        let mut after = base.clone();
        after.previous_loss = Some(LossRecord {
            reason: "watch_queue_overflow".into(),
            detail: "x".into(),
            at: 995,
        });
        assert_eq!(
            run(Some(after), Some(990), "boot-1", true, vec![]).refusal,
            Some(RefreshRefusal::WatchQueueOverflow)
        );
        // No stored observation at all.
        assert_eq!(
            run(Some(base), None, "boot-1", true, vec![]).refusal,
            Some(RefreshRefusal::NoStoredEventId)
        );
    }

    /// Consumption removes what was re-walked, keeps what was dirtied
    /// again later, and leaves another epoch's list alone.
    #[test]
    fn consumption_is_by_sequence_and_by_epoch() {
        let (_t, root, _store, p) = setup();
        let mut ck = checkpoint(&root, 1_000);
        ck.dirty = vec![
            DirtyEntry {
                p: "a".into(),
                s: 3,
            },
            DirtyEntry {
                p: "b".into(),
                s: 9,
            },
        ];
        write_checkpoint(&p, &ck).unwrap();
        let c = Consumption {
            checkpoint: p.checkpoint.clone(),
            dirty_lock: p.dirty_lock.clone(),
            epoch_id: "e1".into(),
            through_seq: 5,
        };
        consume(&c).unwrap();
        let left = read_checkpoint(&p).unwrap().dirty;
        assert_eq!(
            left,
            vec![DirtyEntry {
                p: "b".into(),
                s: 9
            }]
        );
        consume(&Consumption {
            epoch_id: "other".into(),
            through_seq: 99,
            ..c
        })
        .unwrap();
        assert_eq!(
            read_checkpoint(&p).unwrap().dirty.len(),
            1,
            "another epoch's list is not touched"
        );
    }

    #[test]
    fn the_dirty_lock_excludes_a_second_holder() {
        let (_t, _root, _store, p) = setup();
        let a = try_lock(&p.dirty_lock, true).unwrap();
        assert!(a.is_some());
        // flock is per open file description: a second open conflicts
        // even within one process.
        assert!(try_lock(&p.dirty_lock, true).unwrap().is_none());
        drop(a);
        assert!(try_lock(&p.dirty_lock, true).unwrap().is_some());
    }
}
