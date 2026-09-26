//! Live change watching for a platform whose kernel keeps no history
//! (#81): Linux inotify, used directly rather than through `notify`
//! (docs/platform.md, reuse assessment: `notify` has no named way to
//! surface `IN_Q_OVERFLOW`, the one event a coverage claim depends on).
//!
//! # What a live watch can and cannot claim
//!
//! A watch reports what happens *while it is open*. So everything here
//! is organised around one claim and the ways it stops being true:
//!
//! > Since `opened_at`, every change under the root is in the dirty set.
//!
//! `opened_at` is when **registration finished** -- not when it began.
//! inotify is not recursive: a watch is added per directory, and a
//! change inside a directory whose watch is not yet installed is never
//! reported. So bootstrap registers each directory *before* listing it
//! (a subdirectory created during the listing is then reported by its
//! parent's watch), records everything reported during bootstrap as
//! dirty, and only then opens the epoch. A change during bootstrap is
//! either in the dirty set or before `opened_at`; there is no third
//! place for it to hide.
//!
//! The claim ends -- [`Coverage::Lost`], with a named [`Loss`] -- on:
//!
//! * `IN_Q_OVERFLOW`: the kernel dropped events;
//! * watch exhaustion (`ENOSPC` from `inotify_add_watch`, i.e.
//!   `fs.inotify.max_user_watches`);
//! * a directory that cannot be watched or listed (permissions);
//! * `IN_UNMOUNT`, and a watch the kernel removed that swamp did not
//!   ask it to (`IN_IGNORED` without a delete);
//! * a dirty set past its bound (see [`DIRTY_BOUND`]).
//!
//! A lost claim is never an empty change set: every consumer turns it
//! into a full walk of the root with the loss as the named reason
//! ([`Loss::refusal`]), and a new epoch opens only after that.
//!
//! # Where it is used
//!
//! * The TUI's live refresh on Linux ([`crate::fs_events::watch`]),
//!   feeding the same `FsEventsPlan::from_live` incremental pipeline
//!   FSEvents feeds on macOS.
//! * The opt-in user-owned collector (`swamp collect`, [`crate::continuity`]),
//!   which persists the dirty set so a later one-shot CLI run can reuse
//!   it.
//!
//! The state machine ([`LiveTree`]) is pure over a [`Kernel`] so its
//! rules run as unit tests on either platform; the inotify kernel and
//! the real-filesystem tests are Linux-only.

use crate::fs_events::RefreshRefusal;
use crate::fs_gate::MetadataExt;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// inotify's event bits, with the values of the Linux ABI (stable since
/// 2.6.13; `include/uapi/linux/inotify.h`). Declared here rather than
/// taken from `libc` so the state machine compiles, and is tested, on
/// both targets; `inotify::tests` asserts they equal `libc`'s on Linux.
pub mod mask {
    pub const IN_MODIFY: u32 = 0x0000_0002;
    pub const IN_ATTRIB: u32 = 0x0000_0004;
    pub const IN_CLOSE_WRITE: u32 = 0x0000_0008;
    pub const IN_MOVED_FROM: u32 = 0x0000_0040;
    pub const IN_MOVED_TO: u32 = 0x0000_0080;
    pub const IN_CREATE: u32 = 0x0000_0100;
    pub const IN_DELETE: u32 = 0x0000_0200;
    pub const IN_DELETE_SELF: u32 = 0x0000_0400;
    pub const IN_MOVE_SELF: u32 = 0x0000_0800;
    pub const IN_UNMOUNT: u32 = 0x0000_2000;
    pub const IN_Q_OVERFLOW: u32 = 0x0000_4000;
    pub const IN_IGNORED: u32 = 0x0000_8000;
    pub const IN_ONLYDIR: u32 = 0x0100_0000;
    pub const IN_DONT_FOLLOW: u32 = 0x0200_0000;
    pub const IN_EXCL_UNLINK: u32 = 0x0400_0000;
    pub const IN_ISDIR: u32 = 0x4000_0000;

    /// What every directory watch asks for. `IN_MODIFY`, not only
    /// `IN_CLOSE_WRITE`: a file appended to while it stays open (an
    /// agent's session transcript) changes without ever closing, and a
    /// growth tool that cannot see the file that is growing is not doing
    /// its job.
    pub const WATCH: u32 = IN_MODIFY
        | IN_ATTRIB
        | IN_CLOSE_WRITE
        | IN_MOVED_FROM
        | IN_MOVED_TO
        | IN_CREATE
        | IN_DELETE
        | IN_DELETE_SELF
        | IN_MOVE_SELF
        | IN_ONLYDIR
        | IN_DONT_FOLLOW
        | IN_EXCL_UNLINK;
}

/// The most dirty directories a watch holds before it gives the claim
/// up. Past this a piecemeal re-walk costs more than a full one (the
/// growth store's own `TOO_MANY_CHANGES_FRACTION` makes the same call
/// per observation), and a persisted checkpoint stays a small control
/// file rather than becoming an inventory.
pub const DIRTY_BOUND: usize = 512;

/// One event as the kernel delivered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEvent {
    pub wd: i32,
    pub mask: u32,
    pub cookie: u32,
    pub name: Option<OsString>,
}

/// Why a live watch stopped being able to vouch for its root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loss {
    QueueOverflow,
    WatchLimit,
    PermissionGap,
    Unmounted,
    WatchRemoved,
    DirtyBound,
    RootMoved,
}

impl Loss {
    /// The refusal an observation gives when this loss ends the claim.
    pub fn refusal(self) -> RefreshRefusal {
        match self {
            Loss::QueueOverflow => RefreshRefusal::WatchQueueOverflow,
            Loss::WatchLimit => RefreshRefusal::WatchLimitReached,
            Loss::PermissionGap => RefreshRefusal::WatchPermissionGap,
            Loss::Unmounted => RefreshRefusal::WatchedFilesystemUnmounted,
            Loss::WatchRemoved => RefreshRefusal::WatchRemoved,
            Loss::DirtyBound => RefreshRefusal::TooManyChanges,
            Loss::RootMoved => RefreshRefusal::RootMismatch,
        }
    }

    pub fn as_str(self) -> &'static str {
        self.refusal().as_str()
    }

    /// Whether opening a new epoch can restore the claim. A lost event
    /// can be made up for by one full walk; a directory that cannot be
    /// watched cannot, until the limit or the permission changes.
    pub fn recoverable(self) -> bool {
        !matches!(self, Loss::WatchLimit | Loss::PermissionGap)
    }
}

/// Whether the claim holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Coverage {
    Complete,
    Lost { loss: Loss, detail: String, at: u64 },
}

/// The kernel side of a watch, so the rules above can be driven by a
/// test without one.
pub trait Kernel {
    /// Adds (or refreshes) a watch on `dir`, returning its descriptor.
    /// Adding one for a directory already watched returns the existing
    /// descriptor, as inotify does.
    fn add(&mut self, dir: &Path) -> std::io::Result<i32>;
    fn remove(&mut self, wd: i32);
}

/// Resource limits the kernel imposes, read where available.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Limits {
    pub max_user_watches: Option<u64>,
    pub max_queued_events: Option<u64>,
}

/// What a watch says about itself: coverage, cost, limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stats {
    pub watches: usize,
    pub dirty: usize,
    pub limits: Limits,
    /// The kernel's own accounting: one `struct inotify_watch` plus its
    /// mark per watch, about 1 KiB on 64-bit (inotify(7) quotes 540
    /// bytes on 32-bit and 1080 on 64-bit for the watch itself).
    pub kernel_bytes_estimate: u64,
    pub coverage: Coverage,
    pub opened_at: Option<u64>,
}

/// One root under live watch: the dirty set, the epoch, the coverage.
pub struct LiveTree<K: Kernel> {
    root: PathBuf,
    device: u64,
    exclude: Vec<PathBuf>,
    kernel: K,
    wds: HashMap<i32, PathBuf>,
    /// Descriptors swamp asked the kernel to drop, whose `IN_IGNORED`
    /// is expected rather than a loss.
    dropping: HashSet<i32>,
    /// Descriptors whose directory reported its own deletion.
    deleted: HashSet<i32>,
    /// A directory moved out of place in this batch, keyed by cookie,
    /// with the descriptors under its old path.
    moving: HashMap<u32, Vec<i32>>,
    dirty: BTreeMap<PathBuf, u64>,
    seq: u64,
    coverage: Coverage,
    opened_at: Option<u64>,
    limits: Limits,
}

fn now() -> u64 {
    crate::entities::now()
}

impl<K: Kernel> LiveTree<K> {
    /// Registers every directory under `root` (on its filesystem, never
    /// through a symlink, never under an `exclude` path) and opens the
    /// epoch when registration is complete. Events delivered during
    /// registration are applied with [`LiveTree::apply`] as they arrive
    /// by the caller; nothing reported is discarded.
    pub fn new(
        root: &Path,
        exclude: Vec<PathBuf>,
        kernel: K,
        limits: Limits,
    ) -> std::io::Result<Self> {
        let root = crate::fs_gate::canonicalize(root)?;
        let device = crate::fs_gate::metadata_following(&root)?.dev();
        Ok(Self {
            root,
            device,
            exclude,
            kernel,
            wds: HashMap::new(),
            dropping: HashSet::new(),
            deleted: HashSet::new(),
            moving: HashMap::new(),
            dirty: BTreeMap::new(),
            seq: 0,
            coverage: Coverage::Complete,
            opened_at: None,
            limits,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn device(&self) -> u64 {
        self.device
    }

    pub fn kernel_mut(&mut self) -> &mut K {
        &mut self.kernel
    }

    /// Registers the whole tree. Call once; then [`LiveTree::open`].
    pub fn register_all(&mut self) {
        let root = self.root.clone();
        self.register_subtree(&root, false);
    }

    /// Opens the epoch: from now on every change is in the dirty set.
    /// Whole seconds, rounded **up**, so a stored observation that began
    /// in the same second the registration finished is not covered --
    /// the safe direction.
    pub fn open(&mut self) {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = t.as_secs() + u64::from(t.subsec_nanos() > 0);
        self.opened_at = Some(secs);
    }

    pub fn opened_at(&self) -> Option<u64> {
        self.opened_at
    }

    pub fn coverage(&self) -> &Coverage {
        &self.coverage
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    pub fn stats(&self) -> Stats {
        Stats {
            watches: self.wds.len(),
            dirty: self.dirty.len(),
            limits: self.limits,
            kernel_bytes_estimate: self.wds.len() as u64 * 1080,
            coverage: self.coverage.clone(),
            opened_at: self.opened_at,
        }
    }

    /// Dirty directories with the sequence number of their latest
    /// change, oldest first.
    pub fn dirty(&self) -> impl Iterator<Item = (&Path, u64)> {
        self.dirty.iter().map(|(p, s)| (p.as_path(), *s))
    }

    /// Drains the dirty set (the TUI consumes each batch as it goes).
    pub fn take_dirty(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.dirty).into_keys().collect()
    }

    /// Forgets dirty entries up to and including `through` (a consumer
    /// re-walked them).
    pub fn consume_through(&mut self, through: u64) {
        self.dirty.retain(|_, s| *s > through);
    }

    /// Opens a fresh epoch after a *recoverable* loss: the watches are
    /// still installed, but events were lost, so nothing before now is
    /// vouched for. A consumer's next observation of this root predates
    /// the new `opened_at` and so walks fully -- that is the recovery.
    pub fn reopen_after_loss(&mut self) -> bool {
        match &self.coverage {
            Coverage::Lost { loss, .. } if loss.recoverable() => {
                self.dirty.clear();
                self.coverage = Coverage::Complete;
                self.open();
                true
            }
            _ => false,
        }
    }

    fn excluded(&self, p: &Path) -> bool {
        self.exclude.iter().any(|e| p == e || p.starts_with(e))
    }

    fn lose(&mut self, loss: Loss, detail: String) {
        if matches!(self.coverage, Coverage::Complete) {
            self.coverage = Coverage::Lost {
                loss,
                detail,
                at: now(),
            };
        }
    }

    fn mark(&mut self, p: &Path) {
        if !p.starts_with(&self.root) || self.excluded(p) {
            return;
        }
        self.seq += 1;
        self.dirty.insert(p.to_path_buf(), self.seq);
        if self.dirty.len() > DIRTY_BOUND {
            self.dirty.clear();
            self.lose(
                Loss::DirtyBound,
                format!("more than {DIRTY_BOUND} directories changed since the epoch opened"),
            );
        }
    }

    /// A directory that vanished (its own `IN_DELETE_SELF`): the change
    /// is visible in its parent's listing, so the parent is what is dirty.
    fn mark_parent(&mut self, dir: &Path) {
        if let Some(parent) = dir.parent()
            && parent.starts_with(&self.root)
        {
            self.mark(parent);
        }
    }

    /// Watches `dir` and everything under it. `dirty` marks every
    /// directory registered (a newly created or moved-in subtree is all
    /// news to the store).
    fn register_subtree(&mut self, dir: &Path, dirty: bool) {
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            if self.excluded(&d) {
                continue;
            }
            match crate::fs_gate::symlink_metadata(&d) {
                Ok(m) if m.is_dir() && !m.file_type().is_symlink() && m.dev() == self.device => {}
                // Another filesystem is outside the walk too; a symlink
                // is never followed; a vanished entry was reported by
                // its parent.
                Ok(_) => continue,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    self.lose(
                        Loss::PermissionGap,
                        format!("cannot stat {}: {e}", d.display()),
                    );
                    continue;
                }
            }
            // Watch first, then list: an entry created between the two
            // is reported by this watch.
            match self.kernel.add(&d) {
                Ok(wd) => {
                    self.dropping.remove(&wd);
                    self.deleted.remove(&wd);
                    self.wds.insert(wd, d.clone());
                }
                Err(e) if crate::fs_gate::sys::is_enospc(&e) => {
                    self.lose(
                        Loss::WatchLimit,
                        format!(
                            "no inotify watch left for {} ({} watches in use; the limit is \
                             fs.inotify.max_user_watches{})",
                            d.display(),
                            self.wds.len(),
                            self.limits
                                .max_user_watches
                                .map(|n| format!(" = {n}"))
                                .unwrap_or_default()
                        ),
                    );
                    return;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    self.lose(
                        Loss::PermissionGap,
                        format!("cannot watch {}: {e}", d.display()),
                    );
                    continue;
                }
            }
            if dirty {
                self.mark(&d);
            }
            let entries = match crate::fs_gate::read_dir(&d) {
                Ok(rd) => rd,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    self.lose(
                        Loss::PermissionGap,
                        format!("cannot list {}: {e}", d.display()),
                    );
                    continue;
                }
            };
            for entry in entries {
                match entry {
                    Ok(entry) => {
                        if entry.file_type().is_ok_and(|t| t.is_dir()) {
                            stack.push(entry.path());
                        }
                    }
                    Err(e) => {
                        self.lose(
                            Loss::PermissionGap,
                            format!("listing {} failed part-way: {e}", d.display()),
                        );
                    }
                }
            }
        }
    }

    /// Applies one batch of kernel events.
    pub fn apply(&mut self, events: &[RawEvent]) {
        for ev in events {
            self.apply_one(ev);
        }
        // A directory moved out of the tree in this batch (no matching
        // `IN_MOVED_TO`): its watches now describe somewhere else.
        for (_, wds) in std::mem::take(&mut self.moving) {
            for wd in wds {
                if self.wds.remove(&wd).is_some() {
                    self.dropping.insert(wd);
                    self.kernel.remove(wd);
                }
            }
        }
    }

    fn apply_one(&mut self, ev: &RawEvent) {
        use mask::*;
        if ev.mask & IN_Q_OVERFLOW != 0 {
            self.lose(
                Loss::QueueOverflow,
                format!(
                    "the kernel's event queue overflowed{}; events were dropped",
                    self.limits
                        .max_queued_events
                        .map(|n| format!(" (fs.inotify.max_queued_events = {n})"))
                        .unwrap_or_default()
                ),
            );
            return;
        }
        if ev.mask & IN_UNMOUNT != 0 {
            let at = self.wds.get(&ev.wd).cloned().unwrap_or_default();
            self.lose(
                Loss::Unmounted,
                format!("the filesystem holding {} was unmounted", at.display()),
            );
            return;
        }
        if ev.mask & IN_IGNORED != 0 {
            let expected = self.dropping.remove(&ev.wd) | self.deleted.remove(&ev.wd);
            let path = self.wds.remove(&ev.wd);
            if !expected && let Some(p) = path {
                self.lose(
                    Loss::WatchRemoved,
                    format!("the kernel removed the watch on {}", p.display()),
                );
            }
            return;
        }
        let Some(dir) = self.wds.get(&ev.wd).cloned() else {
            // A descriptor already dropped (moved out, deleted): its
            // parent reported what matters.
            return;
        };
        if ev.mask & IN_DELETE_SELF != 0 {
            self.deleted.insert(ev.wd);
            self.mark_parent(&dir);
            return;
        }
        if ev.mask & IN_MOVE_SELF != 0 {
            if dir == self.root {
                self.lose(
                    Loss::RootMoved,
                    format!("the watched root {} was moved or replaced", dir.display()),
                );
            }
            // A subdirectory's move is handled from its parent's
            // MOVED_FROM/MOVED_TO pair.
            return;
        }
        let child = ev.name.as_ref().map(|n| dir.join(n));
        if let Some(c) = &child
            && self.excluded(c)
        {
            return;
        }
        // inotify names the directory an entry changed in exactly, so
        // that directory is what is dirty -- plus the entry itself when
        // it is a directory. FSEvents' directory-level stream is coarser
        // and its replay adds each reported path's parent; here the
        // parent is only dirty when its own listing changed, which is an
        // event on the parent's watch.
        self.mark(&dir);
        let is_dir = ev.mask & IN_ISDIR != 0;
        let Some(child) = child else {
            return;
        };
        if is_dir {
            self.mark(&child);
        }
        if is_dir && ev.mask & IN_MOVED_FROM != 0 {
            let under: Vec<i32> = self
                .wds
                .iter()
                .filter(|(_, p)| p.starts_with(&child))
                .map(|(wd, _)| *wd)
                .collect();
            self.moving.entry(ev.cookie).or_default().extend(under);
        }
        if is_dir && ev.mask & (IN_CREATE | IN_MOVED_TO) != 0 {
            // A moved-in directory keeps its old descriptors (inotify
            // watches inodes); re-registering refreshes their paths and
            // adds whatever is new.
            if ev.mask & IN_MOVED_TO != 0 {
                self.moving.remove(&ev.cookie);
            }
            self.register_subtree(&child, true);
        }
    }
}

// ---------------------------------------------------------------------
// The Linux kernel
// ---------------------------------------------------------------------

#[cfg(target_os = "linux")]
pub use crate::fs_gate::inotify;

/// A live watch over one root on Linux, on its own thread, delivering
/// [`crate::fs_events::WatchBatch`]es the TUI consumes exactly as it
/// consumes FSEvents'. The first batch after registration carries
/// `epoch_opened_at`; any loss arrives as `coverage_lost`, after which
/// the watch opens a new epoch (recoverable losses) or stops claiming
/// anything (watch limit, permissions).
#[cfg(target_os = "linux")]
pub fn spawn_watch(
    root: &Path,
    exclude: Vec<PathBuf>,
    tx: std::sync::mpsc::Sender<crate::fs_events::WatchBatch>,
) -> Option<crate::fs_events::Watcher> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let root = root.to_path_buf();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<bool>();
    let thread = std::thread::Builder::new()
        .name("inotify-watch".into())
        .spawn(move || {
            let Ok(kernel) = inotify::Inotify::new() else {
                let _ = ready_tx.send(false);
                return;
            };
            let Ok(mut tree) = LiveTree::new(&root, exclude, kernel, inotify::limits()) else {
                let _ = ready_tx.send(false);
                return;
            };
            let _ = ready_tx.send(true);
            tree.register_all();
            // Events that arrived during registration are applied before
            // the epoch opens: all of them are dirty, none is dropped.
            match tree.kernel_mut().read(0) {
                Ok(evs) => tree.apply(&evs),
                Err(_) => tree.lose_io(),
            }
            tree.open();
            let mut announce_epoch = true;
            while !stop_thread.load(Ordering::Relaxed) {
                let evs = match tree.kernel_mut().read(250) {
                    Ok(evs) => evs,
                    Err(_) => {
                        tree.lose_io();
                        Vec::new()
                    }
                };
                tree.apply(&evs);
                let lost = match tree.coverage() {
                    Coverage::Lost { loss, detail, .. } => Some((loss.refusal(), detail.clone())),
                    Coverage::Complete => None,
                };
                let changed = tree.take_dirty();
                if announce_epoch || lost.is_some() || !changed.is_empty() {
                    let batch = crate::fs_events::WatchBatch {
                        root: tree.root().to_path_buf(),
                        changed_dirs: changed,
                        last_event_id: tree.seq(),
                        coverage_lost: lost.clone(),
                        epoch_opened_at: announce_epoch.then(|| tree.opened_at()).flatten(),
                    };
                    if tx.send(batch).is_err() {
                        return;
                    }
                    announce_epoch = false;
                }
                if lost.is_some() {
                    if tree.reopen_after_loss() {
                        announce_epoch = true;
                    } else {
                        // Unrecoverable: say so once and stop claiming.
                        return;
                    }
                }
            }
        })
        .ok()?;
    match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(true) => Some(crate::fs_events::Watcher::from_parts(stop, thread)),
        _ => {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = thread.join();
            None
        }
    }
}

impl<K: Kernel> LiveTree<K> {
    /// The event stream itself failed to read: nothing after this point
    /// is known.
    pub fn lose_io(&mut self) {
        self.lose(
            Loss::QueueOverflow,
            "reading the event queue failed; events may have been dropped".into(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::mask::*;
    use super::*;

    /// A kernel that hands out descriptors and remembers them.
    #[derive(Default)]
    struct Fake {
        next: i32,
        by_path: HashMap<PathBuf, i32>,
        removed: Vec<i32>,
        fail_after: Option<usize>,
    }

    impl Kernel for Fake {
        fn add(&mut self, dir: &Path) -> std::io::Result<i32> {
            if let Some(&wd) = self.by_path.get(dir) {
                return Ok(wd);
            }
            if self.fail_after.is_some_and(|n| self.by_path.len() >= n) {
                return Err(std::io::Error::from_raw_os_error(libc::ENOSPC));
            }
            self.next += 1;
            self.by_path.insert(dir.to_path_buf(), self.next);
            Ok(self.next)
        }
        fn remove(&mut self, wd: i32) {
            self.removed.push(wd);
            self.by_path.retain(|_, w| *w != wd);
        }
    }

    fn tree_at(root: &Path) -> LiveTree<Fake> {
        let mut t = LiveTree::new(root, Vec::new(), Fake::default(), Limits::default()).unwrap();
        t.register_all();
        t.open();
        t
    }

    fn wd_of(t: &LiveTree<Fake>, p: &Path) -> i32 {
        *t.kernel.by_path.get(p).expect("watched")
    }

    fn ev(wd: i32, mask: u32, name: &str) -> RawEvent {
        RawEvent {
            wd,
            mask,
            cookie: 0,
            name: (!name.is_empty()).then(|| OsString::from(name)),
        }
    }

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::create_dir_all(root.join("c")).unwrap();
        (tmp, root)
    }

    #[test]
    fn every_directory_is_watched_and_the_epoch_opens_after_registration() {
        let (_t, root) = fixture();
        let before = crate::entities::now();
        let t = tree_at(&root);
        assert_eq!(t.stats().watches, 4, "root, a, a/b, c");
        assert!(t.opened_at().unwrap() >= before);
        assert_eq!(t.coverage(), &Coverage::Complete);
        assert_eq!(t.dirty().count(), 0);
    }

    /// Exactly the directory the entry changed in: marking its parent
    /// too would turn a write inside a build directory into a re-walk of
    /// the worktree around it.
    #[test]
    fn a_file_change_marks_exactly_its_directory() {
        let (_t, root) = fixture();
        let mut t = tree_at(&root);
        let b = root.join("a/b");
        t.apply(&[ev(wd_of(&t, &b), IN_MODIFY, "x.o")]);
        let dirty: Vec<PathBuf> = t.dirty().map(|(p, _)| p.to_path_buf()).collect();
        assert_eq!(dirty, vec![b.clone()]);
        // A new directory: the parent's listing changed, and the child is
        // new -- both.
        t.apply(&[ev(wd_of(&t, &b), IN_CREATE | IN_ISDIR, "new")]);
        std::fs::create_dir_all(b.join("new")).ok();
        assert!(t.dirty().any(|(p, _)| p == b.join("new")));
    }

    /// A new directory tree is registered and every directory in it is
    /// dirty -- including the ones created before its watch existed.
    #[test]
    fn a_nested_new_directory_is_registered_and_wholly_dirty() {
        let (_t, root) = fixture();
        let mut t = tree_at(&root);
        std::fs::create_dir_all(root.join("c/new/deeper/deepest")).unwrap();
        t.apply(&[ev(wd_of(&t, &root.join("c")), IN_CREATE | IN_ISDIR, "new")]);
        for d in ["c/new", "c/new/deeper", "c/new/deeper/deepest"] {
            assert!(t.kernel.by_path.contains_key(&root.join(d)), "{d} watched");
            assert!(t.dirty().any(|(p, _)| p == root.join(d)), "{d} dirty");
        }
    }

    #[test]
    fn overflow_loses_coverage_and_is_never_an_empty_change_set() {
        let (_t, root) = fixture();
        let mut t = tree_at(&root);
        t.apply(&[ev(-1, IN_Q_OVERFLOW, "")]);
        match t.coverage() {
            Coverage::Lost { loss, .. } => {
                assert_eq!(*loss, Loss::QueueOverflow);
                assert_eq!(loss.refusal(), RefreshRefusal::WatchQueueOverflow);
            }
            other => panic!("{other:?}"),
        }
        // Recovery opens a new epoch and only then claims again.
        let old = t.opened_at();
        assert!(t.reopen_after_loss());
        assert_eq!(t.coverage(), &Coverage::Complete);
        assert!(t.opened_at() >= old);
    }

    #[test]
    fn running_out_of_watches_is_named_and_not_recoverable_by_reopening() {
        let (_t, root) = fixture();
        let kernel = Fake {
            fail_after: Some(2),
            ..Fake::default()
        };
        let mut t = LiveTree::new(
            &root,
            Vec::new(),
            kernel,
            Limits {
                max_user_watches: Some(2),
                max_queued_events: None,
            },
        )
        .unwrap();
        t.register_all();
        match t.coverage() {
            Coverage::Lost { loss, detail, .. } => {
                assert_eq!(*loss, Loss::WatchLimit);
                assert!(detail.contains("max_user_watches = 2"), "{detail}");
            }
            other => panic!("{other:?}"),
        }
        assert!(
            !t.reopen_after_loss(),
            "a watch limit is not fixed by reopening"
        );
    }

    #[test]
    fn a_watch_the_kernel_removes_unasked_is_a_loss_but_a_deleted_directory_is_not() {
        let (_t, root) = fixture();
        let mut t = tree_at(&root);
        let c = wd_of(&t, &root.join("c"));
        t.apply(&[
            ev(c, IN_DELETE_SELF, ""),
            ev(c, IN_IGNORED, ""),
            ev(wd_of(&t, &root), IN_DELETE | IN_ISDIR, "c"),
        ]);
        assert_eq!(
            t.coverage(),
            &Coverage::Complete,
            "a delete is a change, not a loss"
        );
        assert!(t.dirty().any(|(p, _)| p == root));

        let b = wd_of(&t, &root.join("a/b"));
        t.apply(&[ev(b, IN_IGNORED, "")]);
        assert!(matches!(
            t.coverage(),
            Coverage::Lost {
                loss: Loss::WatchRemoved,
                ..
            }
        ));
    }

    #[test]
    fn unmount_is_a_loss() {
        let (_t, root) = fixture();
        let mut t = tree_at(&root);
        t.apply(&[ev(wd_of(&t, &root.join("a")), IN_UNMOUNT, "")]);
        assert!(matches!(
            t.coverage(),
            Coverage::Lost {
                loss: Loss::Unmounted,
                ..
            }
        ));
    }

    /// A rename pair inside the tree keeps the moved directory watched
    /// under its new path; a directory moved out of the tree is dropped.
    #[test]
    fn rename_pairs_follow_the_directory_and_moves_out_drop_it() {
        let (_t, root) = fixture();
        let mut t = tree_at(&root);
        let a = wd_of(&t, &root.join("a"));
        let c = wd_of(&t, &root.join("c"));
        let b_wd = wd_of(&t, &root.join("a/b"));
        std::fs::rename(root.join("a/b"), root.join("c/b2")).unwrap();
        t.apply(&[
            RawEvent {
                wd: a,
                mask: IN_MOVED_FROM | IN_ISDIR,
                cookie: 7,
                name: Some("b".into()),
            },
            RawEvent {
                wd: c,
                mask: IN_MOVED_TO | IN_ISDIR,
                cookie: 7,
                name: Some("b2".into()),
            },
        ]);
        // The fake kernel keys by path, so a real re-registration under
        // the new path gets a descriptor; the old one was not dropped.
        assert!(!t.kernel.removed.contains(&b_wd));
        assert!(t.dirty().any(|(p, _)| p == root.join("a")));
        assert!(t.dirty().any(|(p, _)| p == root.join("c/b2")));

        let outside = tempfile::tempdir().unwrap();
        let moved_wd = wd_of(&t, &root.join("c/b2"));
        std::fs::rename(root.join("c/b2"), outside.path().join("gone")).unwrap();
        t.apply(&[RawEvent {
            wd: c,
            mask: IN_MOVED_FROM | IN_ISDIR,
            cookie: 9,
            name: Some("b2".into()),
        }]);
        assert!(
            t.kernel.removed.contains(&moved_wd),
            "watches moved out of the tree are dropped"
        );
        assert_eq!(t.coverage(), &Coverage::Complete);
    }

    #[test]
    fn excluded_paths_are_neither_watched_nor_dirty() {
        let (_t, root) = fixture();
        let store = root.join("a");
        let mut t = LiveTree::new(
            &root,
            vec![store.clone()],
            Fake::default(),
            Limits::default(),
        )
        .unwrap();
        t.register_all();
        t.open();
        assert!(!t.kernel.by_path.contains_key(&store));
        t.apply(&[ev(wd_of(&t, &root), IN_CREATE | IN_ISDIR, "a")]);
        assert!(
            !t.dirty().any(|(p, _)| p.starts_with(&store)),
            "the store's own writes are ignored"
        );
    }

    #[test]
    fn a_dirty_set_past_its_bound_gives_up_the_claim() {
        let (_t, root) = fixture();
        let mut t = tree_at(&root);
        let wd = wd_of(&t, &root);
        let evs: Vec<RawEvent> = (0..DIRTY_BOUND + 2)
            .map(|i| ev(wd, IN_CREATE | IN_ISDIR, &format!("d{i}")))
            .collect();
        t.apply(&evs);
        assert!(matches!(
            t.coverage(),
            Coverage::Lost {
                loss: Loss::DirtyBound,
                ..
            }
        ));
    }

    #[test]
    fn consumption_forgets_only_what_was_re_walked() {
        let (_t, root) = fixture();
        let mut t = tree_at(&root);
        t.apply(&[ev(wd_of(&t, &root.join("c")), IN_MODIFY, "f")]);
        let through = t.seq();
        t.apply(&[ev(wd_of(&t, &root.join("a/b")), IN_MODIFY, "g")]);
        t.consume_through(through);
        let left: Vec<PathBuf> = t.dirty().map(|(p, _)| p.to_path_buf()).collect();
        assert!(left.contains(&root.join("a/b")), "{left:?}");
        assert!(!left.contains(&root.join("c")), "{left:?}");
    }
}
