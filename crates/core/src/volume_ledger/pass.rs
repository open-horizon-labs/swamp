//! The volume pass: a coarse, bounded measurement of the data volume
//! outside swamp's scope, run only inside `swamp observe`.
//!
//! # What it does not do
//!
//! * It never runs on `swamp ui` open or in `report`: those read the
//!   ledger ([`super::read_account`]) and touch nothing else.
//! * It does not re-walk what the observation already measured. A catalog
//!   or declared location's total is copied from the observation's own
//!   units ([`super::Accounted`]); the walk *prunes* those paths so their
//!   bytes are counted once.
//! * It never reads file contents, never follows a symlink, never opens a
//!   FIFO, socket or device (`lstat` only), and never crosses onto
//!   another device: a mounted disk image is a view of a file stored
//!   elsewhere, listed as a non-additive note.
//! * It does not skip `/System` because it is "sealed". On macOS the
//!   pass walks the data volume from its own mount point
//!   (`/System/Volumes/Data`), where `/System` holds exactly what the data
//!   volume keeps there: `/System/Library/AssetsV2`, the simulator runtime
//!   images. The sealed system volume itself is never listed, and its
//!   bytes are the System line from `diskutil`. (`st_dev` cannot tell the
//!   two apart: every path under `/` reports the same device.)
//!
//! # Time budget and cursor
//!
//! A run measures for at most `budget` (default 120 s) at background
//! priority with a small bounded pool, then stops, even mid-folder: a
//! folder that did not finish leaves no row. The cursor is
//! [`crate::growth::VolumeMetaRow::cycle_started_at`]: within a cycle a
//! location is pending until its row is at least that new, so the next
//! run continues where this one stopped, each row keeps its own
//! `measured_at`, and a partial ledger has honest ages. A folder that
//! filled a whole run alone is recorded as `expanded` and measured as its
//! children from then on, so the pass always makes progress.
//!
//! # Locking
//!
//! The pass walks without any store lock (the `observe` process already
//! holds swamp's single-flight lock). Only the two small ledger writes
//! take the observation writer lock, so it is held for milliseconds, far
//! shorter than the observation itself.

use super::system::{self, SystemProbe};
use super::{
    Accounted, Category, Exactness, MAX_NAMED_ROWS, METHOD_EXPANDED, METHOD_OVER_BUDGET,
    METHOD_WALK, MountKind, MountView, Row, accounted_rows, mount_row,
};
use crate::growth::VolumeMetaRow;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// A file's key suffix: the row for the files directly in a folder whose
/// subfolders are rows of their own.
pub const FILES_SUFFIX: &str = "/(files directly here)";

/// Method of a row that only says why something was not walked.
pub const METHOD_LISTING: &str = "listing";

/// Workers in the bounded pool.
pub const WORKERS: usize = 3;

// ---- the filesystem the pass reads ----------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    /// A FIFO, socket or device: never statted, never opened.
    Other,
}

#[derive(Debug, Clone)]
pub struct FsEntry {
    pub name: OsString,
    pub kind: Kind,
}

/// The fields of an `lstat` the pass uses.
#[derive(Debug, Clone, Copy)]
pub struct FsStat {
    pub dev: u64,
    pub ino: u64,
    pub nlink: u64,
    /// 512-byte blocks actually allocated (`st_blocks`).
    pub blocks: u64,
    pub mtime: i64,
}

/// What the pass may ask of a filesystem: list one level (names and kinds
/// from the directory read, no stat), `lstat` one path, and how much a
/// mounted volume holds.
pub trait VolumeFs: Sync {
    fn list(&self, dir: &Path) -> io::Result<Vec<FsEntry>>;
    /// [`Self::list`] that gives up with `TimedOut` once `deadline` has
    /// passed, checked while entries are read, so one enormous folder
    /// cannot outlast the pass's time budget.
    fn list_until(&self, dir: &Path, _deadline: Instant) -> io::Result<Vec<FsEntry>> {
        self.list(dir)
    }
    fn lstat(&self, path: &Path) -> io::Result<FsStat>;
    /// What is mounted, as the OS's mount table says, in the paths a
    /// person sees from `/`.
    fn mounts(&self) -> Vec<MountView>;
}

/// The real filesystem, through the gate.
pub struct RealFs;

impl RealFs {
    fn list_bounded(&self, dir: &Path, deadline: Option<Instant>) -> io::Result<Vec<FsEntry>> {
        let entries = crate::fs_gate::read_dir(dir)?;
        crate::work_counters::record_dir_listed();
        let mut out = Vec::new();
        for entry in entries {
            if let Some(d) = deadline
                && out.len().is_multiple_of(1024)
                && Instant::now() >= d
            {
                return Err(io::Error::from(io::ErrorKind::TimedOut));
            }
            let entry = entry?;
            let kind = match entry.file_type() {
                Ok(t) if t.is_symlink() => Kind::Symlink,
                Ok(t) if t.is_dir() => Kind::Dir,
                Ok(t) if t.is_file() => Kind::File,
                _ => Kind::Other,
            };
            out.push(FsEntry {
                name: entry.file_name(),
                kind,
            });
        }
        Ok(out)
    }
}

impl VolumeFs for RealFs {
    fn list(&self, dir: &Path) -> io::Result<Vec<FsEntry>> {
        self.list_bounded(dir, None)
    }

    fn list_until(&self, dir: &Path, deadline: Instant) -> io::Result<Vec<FsEntry>> {
        self.list_bounded(dir, Some(deadline))
    }

    fn lstat(&self, path: &Path) -> io::Result<FsStat> {
        use crate::fs_gate::MetadataExt;
        crate::work_counters::record_files_statted(1);
        let m = crate::fs_gate::symlink_metadata(path)?;
        Ok(FsStat {
            dev: m.dev(),
            ino: m.ino(),
            nlink: m.nlink(),
            blocks: m.blocks(),
            mtime: m.mtime(),
        })
    }

    fn mounts(&self) -> Vec<MountView> {
        let root_total = crate::fs_gate::fs_space::total_bytes(Path::new("/"));
        crate::fs_gate::sys::mount_points()
            .into_iter()
            .filter(|m| {
                // Not a place anyone looks for files: the system's own
                // mounts, devices and automounter maps.
                m.path != Path::new("/")
                    && !m.path.starts_with("/System/Volumes")
                    && !m.path.starts_with("/dev")
                    && !matches!(
                        m.fs_type.as_str(),
                        "devfs"
                            | "autofs"
                            | "procfs"
                            | "proc"
                            | "sysfs"
                            | "tmpfs"
                            | "devtmpfs"
                            | "cgroup"
                            | "cgroup2"
                            | "overlay"
                            | "squashfs"
                    )
            })
            .map(|m| {
                if !m.local {
                    return MountView {
                        path: m.path,
                        kind: MountKind::Remote,
                        used: None,
                    };
                }
                let total = crate::fs_gate::fs_space::total_bytes(&m.path);
                let free = crate::fs_gate::fs_space::available_bytes(&m.path);
                match (total, free) {
                    (Some(t), Some(f)) if Some(t) != root_total => MountView {
                        path: m.path,
                        kind: MountKind::OwnStorage,
                        used: Some(t.saturating_sub(f)),
                    },
                    (Some(_), Some(_)) => MountView {
                        path: m.path,
                        kind: MountKind::SameContainer,
                        used: None,
                    },
                    _ => MountView {
                        path: m.path,
                        kind: MountKind::Unknown,
                        used: None,
                    },
                }
            })
            .collect()
    }
}

/// Container totals: `(total, available)` bytes.
pub trait SpaceProbe: Sync {
    fn container(&self) -> Option<(u64, u64)>;
}

/// `statfs` on the data volume (on APFS its totals are the container's).
pub struct RealSpace;

impl SpaceProbe for RealSpace {
    fn container(&self) -> Option<(u64, u64)> {
        for p in ["/System/Volumes/Data", "/"] {
            let p = Path::new(p);
            if let (Some(total), Some(free)) = (
                crate::fs_gate::fs_space::total_bytes(p),
                crate::fs_gate::fs_space::available_bytes(p),
            ) {
                return Some((total, free));
            }
        }
        None
    }
}

// ---- what to measure ------------------------------------------------

/// Where the pass looks: the data volume, and which folders are listed
/// one level deeper. Paths everywhere else in the pass (rows, accounted
/// locations, the mount table) are *logical*: as a person sees them from
/// `/`. Only [`Mapped`] knows where the data volume is really mounted.
#[derive(Debug, Clone)]
pub struct Layout {
    /// Where the data volume is mounted: `/System/Volumes/Data` on
    /// macOS, `/` where the system and data are not split.
    pub data_root: PathBuf,
    /// Folders (logical paths) that are listed and measured child by
    /// child (depth 2).
    pub expand: Vec<PathBuf>,
}

impl Layout {
    /// This machine: the data volume, with the user's home, `/Library`,
    /// `/opt`, `/private`, `/Applications`, `/Users` and (on macOS)
    /// `/System` and `/System/Library` listed one level deeper.
    pub fn system(home: &Path) -> Layout {
        let mac_data = Path::new("/System/Volumes/Data");
        let mut expand: Vec<PathBuf> = ["/Users", "/Library", "/opt", "/private", "/Applications"]
            .iter()
            .map(PathBuf::from)
            .collect();
        expand.push(home.to_path_buf());
        let data_root = if crate::fs_gate::is_dir(mac_data) {
            expand.push(PathBuf::from("/System"));
            expand.push(PathBuf::from("/System/Library"));
            mac_data.to_path_buf()
        } else {
            expand.extend(["/home", "/var", "/usr"].iter().map(PathBuf::from));
            PathBuf::from("/")
        };
        Layout { data_root, expand }
    }
}

/// The filesystem as the pass sees it: logical paths in, real paths to
/// the wrapped filesystem.
struct Mapped<'a> {
    fs: &'a dyn VolumeFs,
    data_root: &'a Path,
}

impl Mapped<'_> {
    fn real(&self, logical: &Path) -> PathBuf {
        if self.data_root == Path::new("/") {
            logical.to_path_buf()
        } else {
            self.data_root
                .join(logical.strip_prefix("/").unwrap_or(logical))
        }
    }
}

impl VolumeFs for Mapped<'_> {
    fn list(&self, dir: &Path) -> io::Result<Vec<FsEntry>> {
        self.fs.list(&self.real(dir))
    }
    fn list_until(&self, dir: &Path, deadline: Instant) -> io::Result<Vec<FsEntry>> {
        self.fs.list_until(&self.real(dir), deadline)
    }
    fn lstat(&self, path: &Path) -> io::Result<FsStat> {
        self.fs.lstat(&self.real(path))
    }
    fn mounts(&self) -> Vec<MountView> {
        self.fs.mounts()
    }
}

/// One unit of measuring: a folder (whole subtree) or the files directly
/// in one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub path: PathBuf,
    pub files_only: bool,
}

impl Task {
    /// The ledger row's `path`.
    pub fn key(&self) -> String {
        let p = self.path.display().to_string();
        if self.files_only {
            format!("{}{FILES_SUFFIX}", p.trim_end_matches('/'))
        } else {
            p
        }
    }
}

/// The plan: what to measure, plus the rows that only say what was not
/// walked and why.
#[derive(Debug, Default)]
pub struct Plan {
    pub tasks: Vec<Task>,
    pub notes: Vec<Row>,
    /// `expanded` markers this plan consulted (kept when the cycle ends).
    pub markers: Vec<String>,
}

struct PlanCtx<'a> {
    fs: &'a dyn VolumeFs,
    layout: &'a Layout,
    mounts: &'a [MountView],
    accounted: &'a HashSet<PathBuf>,
    prev: &'a HashMap<String, Row>,
    data_dev: u64,
    now: u64,
    plan: Plan,
}

fn note_row(path: &Path, category: Category, now: u64, why: String) -> Row {
    Row {
        path: path.display().to_string(),
        category,
        bytes: None,
        overlap_bytes: 0,
        entries: None,
        unreadable: 0,
        measured_at: now,
        method: METHOD_LISTING.to_string(),
        exactness: Exactness::NotMeasured,
        note: Some(why),
    }
}

fn denied_note(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::PermissionDenied => {
            "permission denied (a folder macOS or the owner protects); size not measured"
                .to_string()
        }
        _ => format!("could not be read ({}); size not measured", e.kind()),
    }
}

impl PlanCtx<'_> {
    fn plan_dir(&mut self, dir: &Path) {
        let entries = match self.fs.list(dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(e) => {
                self.plan.notes.push(note_row(
                    dir,
                    Category::Unreadable,
                    self.now,
                    denied_note(&e),
                ));
                return;
            }
        };
        let mut sorted = entries;
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        let mut has_files = false;
        for entry in sorted {
            let path = dir.join(&entry.name);
            match entry.kind {
                Kind::File => has_files = true,
                Kind::Dir => self.plan_child(&path),
                Kind::Symlink | Kind::Other => {}
            }
        }
        if has_files && !self.accounted.contains(dir) {
            self.plan.tasks.push(Task {
                path: dir.to_path_buf(),
                files_only: true,
            });
        }
    }

    fn plan_child(&mut self, path: &Path) {
        if self.accounted.contains(path) {
            return;
        }
        let st = match self.fs.lstat(path) {
            Ok(s) => s,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(e) => {
                self.plan.notes.push(note_row(
                    path,
                    Category::Unreadable,
                    self.now,
                    denied_note(&e),
                ));
                return;
            }
        };
        if st.dev != self.data_dev {
            // A mount point: another filesystem, never walked from here.
            let view = self
                .mounts
                .iter()
                .find(|m| m.path == path)
                .cloned()
                .unwrap_or(MountView {
                    path: path.to_path_buf(),
                    kind: MountKind::Unknown,
                    used: None,
                });
            let mut row = mount_row(&view, self.now);
            row.method = METHOD_LISTING.to_string();
            self.plan.notes.push(row);
            return;
        }
        let key = Task {
            path: path.to_path_buf(),
            files_only: false,
        }
        .key();
        let expanded = self
            .prev
            .get(&key)
            .is_some_and(|r| r.method == METHOD_EXPANDED);
        if expanded {
            self.plan.markers.push(key);
        }
        if expanded || self.layout.expand.iter().any(|e| e == path) {
            self.plan_dir(path);
        } else {
            self.plan.tasks.push(Task {
                path: path.to_path_buf(),
                files_only: false,
            });
        }
    }
}

/// Lists the top levels and decides the tasks. Cheap: one listing per
/// expanded folder and one `lstat` per child of one.
pub fn build_plan(
    fs: &dyn VolumeFs,
    layout: &Layout,
    mounts: &[MountView],
    accounted: &HashSet<PathBuf>,
    prev: &HashMap<String, Row>,
    now: u64,
) -> Plan {
    let root = Path::new("/");
    let data_dev = fs.lstat(root).map(|s| s.dev).unwrap_or(0);
    let mut ctx = PlanCtx {
        fs,
        layout,
        mounts,
        accounted,
        prev,
        data_dev,
        now,
        plan: Plan::default(),
    };
    ctx.plan_dir(root);
    ctx.plan
}

// ---- measuring one task ---------------------------------------------

/// What one task measured.
#[derive(Debug, Default)]
pub struct Measured {
    pub bytes: u64,
    pub entries: u64,
    /// Folders that could not be read.
    pub unreadable_dirs: Vec<(PathBuf, String)>,
    /// Files that could not be statted (permission).
    pub unreadable_files: u64,
    /// Mount points not entered.
    pub mounts: Vec<PathBuf>,
    /// Something vanished or a folder's modification time moved while it
    /// was being read.
    pub changed: bool,
}

#[derive(Debug)]
pub enum Outcome {
    Done(Measured),
    /// The deadline passed first; nothing is kept.
    Aborted,
    /// The task's own folder could not be listed.
    Unreadable(String),
    /// It was not there any more.
    Gone,
}

fn measure_files_only(fs: &dyn VolumeFs, dir: &Path, deadline: Instant) -> Outcome {
    let entries = match fs.list_until(dir, deadline) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Outcome::Gone,
        Err(e) if e.kind() == io::ErrorKind::TimedOut => return Outcome::Aborted,
        Err(e) => return Outcome::Unreadable(denied_note(&e)),
    };
    let mut m = Measured::default();
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    for e in entries.iter() {
        // Per entry: a `lstat` on a cold, throttled disk can take
        // milliseconds, so a coarser check would run seconds over.
        if Instant::now() >= deadline {
            return Outcome::Aborted;
        }
        match e.kind {
            Kind::File => {
                m.entries += 1;
                match fs.lstat(&dir.join(&e.name)) {
                    Ok(st) => {
                        if st.nlink <= 1 || seen.insert((st.dev, st.ino)) {
                            m.bytes += st.blocks * 512;
                        }
                    }
                    Err(err) if err.kind() == io::ErrorKind::NotFound => m.changed = true,
                    Err(_) => m.unreadable_files += 1,
                }
            }
            Kind::Symlink | Kind::Other => m.entries += 1,
            Kind::Dir => {}
        }
    }
    Outcome::Done(m)
}

/// Measures the subtree at `root`: allocated bytes (`st_blocks`), each
/// hardlinked file once per task, `lstat` only, no contents, no symlink
/// followed, no special file touched, no other device entered, folders in
/// `prune` (already accounted for) skipped.
pub fn measure_dir(
    fs: &dyn VolumeFs,
    root: &Path,
    prune: &HashSet<PathBuf>,
    deadline: Instant,
) -> Outcome {
    let root_stat = match fs.lstat(root) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Outcome::Gone,
        Err(e) => return Outcome::Unreadable(denied_note(&e)),
    };
    let mut m = Measured::default();
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut stack: Vec<(PathBuf, i64)> = vec![(root.to_path_buf(), root_stat.mtime)];
    let mut first = true;
    while let Some((dir, mtime_before)) = stack.pop() {
        if Instant::now() >= deadline {
            return Outcome::Aborted;
        }
        let entries = match fs.list_until(&dir, deadline) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::TimedOut => return Outcome::Aborted,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                m.changed = true;
                first = false;
                continue;
            }
            Err(e) => {
                if first {
                    return Outcome::Unreadable(denied_note(&e));
                }
                first = false;
                m.unreadable_dirs.push((dir, denied_note(&e)));
                continue;
            }
        };
        first = false;
        m.entries += entries.len() as u64;
        for entry in entries {
            // Per entry, for the same reason as above.
            if Instant::now() >= deadline {
                return Outcome::Aborted;
            }
            let path = dir.join(&entry.name);
            match entry.kind {
                Kind::Dir => {
                    if prune.contains(&path) {
                        continue;
                    }
                    match fs.lstat(&path) {
                        Ok(st) if st.dev != root_stat.dev => m.mounts.push(path),
                        Ok(st) => stack.push((path, st.mtime)),
                        Err(e) if e.kind() == io::ErrorKind::NotFound => m.changed = true,
                        Err(e) => m.unreadable_dirs.push((path, denied_note(&e))),
                    }
                }
                Kind::File => match fs.lstat(&path) {
                    Ok(st) => {
                        if st.nlink <= 1 || seen.insert((st.dev, st.ino)) {
                            m.bytes += st.blocks * 512;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => m.changed = true,
                    Err(_) => m.unreadable_files += 1,
                },
                Kind::Symlink | Kind::Other => {}
            }
        }
        match fs.lstat(&dir) {
            Ok(after) if after.mtime != mtime_before => m.changed = true,
            Ok(_) => {}
            Err(_) => m.changed = true,
        }
    }
    Outcome::Done(m)
}

struct Finished {
    task: Task,
    outcome: Outcome,
    started: Duration,
}

fn run_tasks(
    fs: &dyn VolumeFs,
    tasks: &[Task],
    prune: &HashSet<PathBuf>,
    deadline: Instant,
    t0: Instant,
    workers: usize,
) -> Vec<Finished> {
    let next = AtomicUsize::new(0);
    let done: Mutex<Vec<Finished>> = Mutex::new(Vec::new());
    let counters = crate::work_counters::current();
    let workers = workers.clamp(1, tasks.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let counters = counters.clone();
            let (next, done) = (&next, &done);
            scope.spawn(move || {
                crate::work_counters::install(counters);
                crate::fs_gate::sys::lower_current_thread_priority();
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(task) = tasks.get(i) else { break };
                    if Instant::now() >= deadline {
                        break;
                    }
                    let started = t0.elapsed();
                    let outcome = if task.files_only {
                        measure_files_only(fs, &task.path, deadline)
                    } else {
                        measure_dir(fs, &task.path, prune, deadline)
                    };
                    done.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(Finished {
                            task: task.clone(),
                            outcome,
                            started,
                        });
                }
            });
        }
    });
    done.into_inner().unwrap_or_else(|e| e.into_inner())
}

// ---- turning results into rows ----------------------------------------

fn changed_note(m: &Measured) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    let unreadable = m.unreadable_dirs.len() as u64 + m.unreadable_files;
    if unreadable > 0 {
        parts.push(format!(
            "{unreadable} inside could not be read and are not counted here"
        ));
    }
    if m.changed {
        parts.push("something changed while it was being read".to_string());
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

fn rows_for(mounts: &[MountView], finished: &Finished, budget: Duration, now: u64) -> Vec<Row> {
    let key = finished.task.key();
    let mut rows = Vec::new();
    match &finished.outcome {
        Outcome::Done(m) => {
            let unreadable = m.unreadable_dirs.len() as u64 + m.unreadable_files;
            rows.push(Row {
                path: key,
                category: Category::Other,
                bytes: Some(m.bytes),
                overlap_bytes: 0,
                entries: Some(m.entries),
                unreadable,
                measured_at: now,
                method: METHOD_WALK.to_string(),
                exactness: if unreadable > 0 || m.changed {
                    Exactness::Estimated
                } else {
                    Exactness::Exact
                },
                note: changed_note(m),
            });
            for (path, why) in m.unreadable_dirs.iter().take(MAX_NAMED_ROWS) {
                rows.push(Row {
                    path: path.display().to_string(),
                    category: Category::Unreadable,
                    bytes: None,
                    overlap_bytes: 0,
                    entries: None,
                    unreadable: 1,
                    measured_at: now,
                    method: METHOD_WALK.to_string(),
                    exactness: Exactness::NotMeasured,
                    note: Some(why.clone()),
                });
            }
            if m.unreadable_dirs.len() > MAX_NAMED_ROWS {
                rows.push(Row {
                    path: format!(
                        "{} (and {} more folders that could not be read)",
                        finished.task.path.display(),
                        m.unreadable_dirs.len() - MAX_NAMED_ROWS
                    ),
                    category: Category::Unreadable,
                    bytes: None,
                    overlap_bytes: 0,
                    entries: None,
                    unreadable: (m.unreadable_dirs.len() - MAX_NAMED_ROWS) as u64,
                    measured_at: now,
                    method: METHOD_WALK.to_string(),
                    exactness: Exactness::NotMeasured,
                    note: Some(
                        "names beyond the first are not listed; the count is exact".to_string(),
                    ),
                });
            }
            for mount in &m.mounts {
                let view = mounts
                    .iter()
                    .find(|v| v.path == *mount)
                    .cloned()
                    .unwrap_or(MountView {
                        path: mount.clone(),
                        kind: MountKind::Unknown,
                        used: None,
                    });
                rows.push(mount_row(&view, now));
            }
        }
        Outcome::Unreadable(why) => rows.push(Row {
            path: key,
            category: Category::Unreadable,
            bytes: None,
            overlap_bytes: 0,
            entries: None,
            unreadable: 1,
            measured_at: now,
            method: METHOD_WALK.to_string(),
            exactness: Exactness::NotMeasured,
            note: Some(why.clone()),
        }),
        Outcome::Gone => {}
        Outcome::Aborted => {
            // A task that was running from the start of the run and still
            // did not finish took the whole budget by itself.
            if finished.started <= budget / 10 {
                rows.push(if finished.task.files_only {
                    Row {
                        path: key,
                        category: Category::Unreadable,
                        bytes: None,
                        overlap_bytes: 0,
                        entries: None,
                        unreadable: 1,
                        measured_at: now,
                        method: METHOD_OVER_BUDGET.to_string(),
                        exactness: Exactness::NotMeasured,
                        note: Some(
                            "one folder holds more than a run's time budget can list; not measured"
                                .to_string(),
                        ),
                    }
                } else {
                    Row {
                        path: key,
                        category: Category::Other,
                        bytes: None,
                        overlap_bytes: 0,
                        entries: None,
                        unreadable: 0,
                        measured_at: now,
                        method: METHOD_EXPANDED.to_string(),
                        exactness: Exactness::NotMeasured,
                        note: Some(
                            "took a whole run alone; measured as its parts from the next run"
                                .to_string(),
                        ),
                    }
                });
            }
        }
    }
    rows
}

// ---- the run ------------------------------------------------------------

/// Everything a run needs. Tests give it fixtures; `swamp observe` gives
/// it the real filesystem, `diskutil` and `statfs`.
pub struct PassInputs<'a> {
    pub store_dir: &'a Path,
    pub now: u64,
    pub interval: Duration,
    pub budget: Duration,
    /// `swamp observe --volume`: run now whatever the last run's age.
    pub force: bool,
    pub layout: &'a Layout,
    pub fs: &'a dyn VolumeFs,
    pub probe: &'a dyn SystemProbe,
    pub space: &'a dyn SpaceProbe,
    pub accounted: &'a [Accounted],
    /// The `min_free_bytes` setting the disk-full guard uses.
    pub min_free: Option<u64>,
    pub workers: usize,
}

/// What a run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PassOutcome {
    /// Not run, with the one-line reason a person should see (the
    /// disk-full guard, a store marker this build does not understand).
    Skipped(String),
    /// Not due yet: the last complete pass is younger than the interval.
    /// Routine on every scheduled observe, so the command line stays
    /// quiet about it.
    NotDue(String),
    Ran(RunSummary),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummary {
    pub measured: usize,
    pub pending: usize,
    pub complete: bool,
    pub elapsed_ms: u64,
    pub rows: usize,
}

impl RunSummary {
    pub fn line(&self) -> String {
        format!(
            "volume pass: {} folders measured, {} still pending ({}), {} rows, {:.1} s",
            self.measured,
            self.pending,
            if self.complete {
                "cycle complete"
            } else {
                "continues at the next observe"
            },
            self.rows,
            self.elapsed_ms as f64 / 1000.0
        )
    }
}

fn is_walk_derived(r: &Row) -> bool {
    matches!(
        r.category,
        Category::Other | Category::Unreadable | Category::Mount
    )
}

/// Runs the pass once. Never runs on the TUI or report path: the caller
/// is `swamp observe`.
pub fn run(inputs: &PassInputs) -> Result<PassOutcome> {
    let now = inputs.now;
    if crate::growth::store_marker_is_foreign(inputs.store_dir)? {
        return Ok(PassOutcome::Skipped(
            "volume pass skipped: the store's format marker is not this build's generation (a different swamp wrote it); nothing was written".to_string(),
        ));
    }
    let (stored, prev_meta) = crate::growth::read_volume_ledger(inputs.store_dir)?;
    let prev_rows: Vec<Row> = stored.iter().filter_map(Row::from_stored).collect();
    let cycle_open = prev_meta.as_ref().is_some_and(|m| !m.complete);

    if !inputs.force
        && !cycle_open
        && let Some(m) = &prev_meta
        && m.cycle_complete_at > 0
        && Duration::from_secs(now.saturating_sub(m.cycle_complete_at)) < inputs.interval
    {
        return Ok(PassOutcome::NotDue(format!(
            "volume pass not due: the last complete pass is {} old, interval {} h (`swamp observe --volume` runs it now)",
            super::age_text(m.cycle_complete_at, now).trim_end_matches(" ago"),
            inputs.interval.as_secs() / 3600
        )));
    }
    if let crate::disk_guard::DiskDecision::Abort {
        free, threshold, ..
    } = crate::disk_guard::check(inputs.store_dir, inputs.min_free)
    {
        return Ok(PassOutcome::Skipped(format!(
            "volume pass skipped: disk nearly full, {} free is below the {} minimum (min_free_bytes)",
            crate::render::human_bytes_pub(free),
            crate::render::human_bytes_pub(threshold)
        )));
    }

    let started = Instant::now();
    let max_prev = prev_rows.iter().map(|r| r.measured_at).max().unwrap_or(0);
    let cycle_started_at = match (&prev_meta, cycle_open) {
        (Some(m), true) => m.cycle_started_at,
        _ => now.max(max_prev + 1),
    };

    let container = inputs.space.container();
    let facts = system::collect(inputs.probe, now);
    let mapped = Mapped {
        fs: inputs.fs,
        data_root: &inputs.layout.data_root,
    };
    let mounts = mapped.mounts();
    let accounted_set: HashSet<PathBuf> = inputs.accounted.iter().map(|a| a.path.clone()).collect();
    let by_key: HashMap<String, Row> = prev_rows
        .iter()
        .map(|r| (r.path.clone(), r.clone()))
        .collect();
    let plan = build_plan(
        &mapped,
        inputs.layout,
        &mounts,
        &accounted_set,
        &by_key,
        now,
    );

    let pending: Vec<Task> = plan
        .tasks
        .iter()
        .filter(|t| {
            by_key
                .get(&t.key())
                .is_none_or(|r| r.measured_at < cycle_started_at || r.method == METHOD_EXPANDED)
        })
        .cloned()
        .collect();

    // The budget covers the whole run, the system queries and the plan
    // included; only when they alone eat more than half of it does the
    // walk get half a budget of its own, so a tiny budget still moves
    // the cursor.
    let t0 = Instant::now();
    let deadline = (started + inputs.budget).max(t0 + inputs.budget / 2);
    let finished = run_tasks(
        &mapped,
        &pending,
        &accounted_set,
        deadline,
        t0,
        inputs.workers,
    );
    let budget_used_ms = started.elapsed().as_millis() as u64;

    let mut new_rows: Vec<Row> = Vec::new();
    let mut replaced_dirs: Vec<PathBuf> = Vec::new();
    let mut replaced_keys: HashSet<String> = HashSet::new();
    let mut measured = 0usize;
    for f in &finished {
        let rows = rows_for(&mounts, f, inputs.budget, now);
        match &f.outcome {
            Outcome::Done(_) | Outcome::Unreadable(_) | Outcome::Gone => {
                measured += 1;
                replaced_keys.insert(f.task.key());
                if !f.task.files_only {
                    replaced_dirs.push(f.task.path.clone());
                }
            }
            Outcome::Aborted => {
                if !rows.is_empty() {
                    replaced_keys.insert(f.task.key());
                }
            }
        }
        new_rows.extend(rows);
    }

    // Keep what this run did not redo.
    let mut merged: Vec<Row> = prev_rows
        .into_iter()
        .filter(|r| {
            if matches!(
                r.category,
                Category::Catalog
                    | Category::Declared
                    | Category::System
                    | Category::Purgeable
                    | Category::Snapshot
            ) {
                return false; // regenerated below on every run
            }
            if replaced_keys.contains(&r.path) || r.method == METHOD_LISTING {
                return false;
            }
            if matches!(r.category, Category::Unreadable | Category::Mount)
                && replaced_dirs
                    .iter()
                    .any(|d| Path::new(&r.path).starts_with(d))
            {
                return false;
            }
            true
        })
        .collect();
    merged.extend(new_rows);
    merged.extend(accounted_rows(inputs.accounted, &mounts));
    // A mount inside an accounted location is a view of an image stored
    // elsewhere: listed, never added.
    merged.extend(
        mounts
            .iter()
            .filter(|m| accounted_set.iter().any(|a| m.path.starts_with(a)))
            .map(|m| mount_row(m, now)),
    );
    merged.extend(facts.rows.clone());
    merged.extend(plan.notes.clone());

    let fresh = |rows: &[Row]| -> HashSet<String> {
        rows.iter()
            // A marker says a folder is to be split, not that it was measured.
            .filter(|r| r.measured_at >= cycle_started_at && r.method != METHOD_EXPANDED)
            .map(|r| r.path.clone())
            .collect()
    };
    let complete = {
        let fresh = fresh(&merged);
        plan.tasks.iter().all(|t| fresh.contains(&t.key()))
    };
    if complete {
        // Everything current was refreshed this cycle; what was not is a
        // folder that is gone. Structure markers stay.
        let markers: HashSet<&String> = plan.markers.iter().collect();
        merged.retain(|r| {
            !is_walk_derived(r) || r.measured_at >= cycle_started_at || markers.contains(&r.path)
        });
    }
    // One row per path and category, newest wins; then a stable order.
    merged.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then((a.category as u8).cmp(&(b.category as u8)))
            .then(b.measured_at.cmp(&a.measured_at))
    });
    merged.dedup_by(|b, a| a.path == b.path && a.category == b.category);

    let pending_after = {
        let fresh = fresh(&merged);
        plan.tasks
            .iter()
            .filter(|t| !fresh.contains(&t.key()))
            .count()
    };

    let meta = VolumeMetaRow {
        measured_at: now,
        cycle_started_at,
        cycle_complete_at: if complete {
            now
        } else {
            prev_meta.as_ref().map(|m| m.cycle_complete_at).unwrap_or(0)
        },
        complete,
        budget_secs: inputs.budget.as_secs(),
        budget_used_ms,
        statfs_at: now,
        container_total: container.map(|(t, _)| t),
        container_used: container.map(|(t, f)| t.saturating_sub(f)),
        container_free: container.map(|(_, f)| f),
        data_volume_used: facts.data_volume_used,
    };
    let stored_rows: Vec<crate::growth::VolumeLedgerRow> =
        merged.iter().map(Row::to_stored).collect();
    crate::growth::write_volume_ledger(
        inputs.store_dir,
        &stored_rows,
        &meta,
        prev_meta.map(|m| m.measured_at),
    )?;
    Ok(PassOutcome::Ran(RunSummary {
        measured,
        pending: pending_after,
        complete,
        elapsed_ms: started.elapsed().as_millis() as u64,
        rows: merged.len(),
    }))
}

// ---- when the pass may run ----------------------------------------------

/// Why an automatic pass is not allowed at all (not "not due"): said only
/// when the person asked for the pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotAllowed(pub String);

/// The home directory the account database names for this process (not
/// `$HOME`): what [`allowed`] compares `$HOME` with.
pub fn account_home() -> Option<PathBuf> {
    crate::fs_gate::sys::account_home()
}

/// Whether `swamp observe` may run the pass now.
///
/// * Explicit roots replace the configured scope for one invocation, so
///   the accounted part would be that one folder and "everything else"
///   would be nearly the whole disk: never a valid ledger, not even with
///   `--volume`.
/// * The automatic pass measures the account's own home. When `$HOME` is
///   not the home the account database names (a sandbox, a test
///   fixture), nothing runs unless `--volume` asks for it.
/// * `volume_pass_interval_hours = 0` turns the automatic pass off.
pub fn allowed(
    explicit_roots: bool,
    force: bool,
    interval_hours: u64,
    home_env: &Path,
    account_home: Option<&Path>,
) -> Result<(), NotAllowed> {
    if explicit_roots {
        return Err(NotAllowed(
            "explicit roots replace the configured scope, so a machine-wide ledger from this run would count only those roots as accounted".to_string(),
        ));
    }
    if force {
        return Ok(());
    }
    if interval_hours == 0 {
        return Err(NotAllowed(
            "volume_pass_interval_hours is 0 (automatic pass off)".to_string(),
        ));
    }
    match account_home {
        Some(h) if h == home_env => Ok(()),
        _ => Err(NotAllowed(
            "$HOME is not the account's home directory, so this is not the machine's own volume"
                .to_string(),
        )),
    }
}

/// The pass for the observation `swamp observe` just finished: this
/// machine's filesystem, `diskutil`, `statfs`, and the configured
/// interval and budget.
pub fn run_after_observation(
    store_dir: &Path,
    config: &crate::growth::GrowthConfig,
    scope: &crate::scope::EffectiveScope,
    observation: &crate::report::ScopeObservation,
    home: &Path,
    force: bool,
) -> Result<PassOutcome> {
    let accounted = super::accounted_locations(observation, scope, observation.merged.observed_at);
    let layout = Layout::system(home);
    run(&PassInputs {
        store_dir,
        now: crate::entities::now(),
        interval: Duration::from_secs(config.volume_pass_interval_hours.saturating_mul(3600)),
        budget: Duration::from_secs(config.volume_pass_budget_secs),
        force,
        layout: &layout,
        fs: &RealFs,
        probe: &system::RealProbe,
        space: &RealSpace,
        accounted: &accounted,
        min_free: config.min_free_bytes,
        workers: WORKERS,
    })
}
