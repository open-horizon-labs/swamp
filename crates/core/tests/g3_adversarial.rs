//! Adversarial audit of v0.8.0 G3 (PR #195). The harness is copied from
//! g3_volume_ledger.rs; a test here that fails on the PR head is a finding.
#![allow(dead_code, unused_imports)]

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use swamp_core::growth::{read_volume_ledger, write_volume_ledger};
use swamp_core::volume_ledger::pass::{
    FILES_SUFFIX, FsEntry, FsStat, Kind, Layout, PassInputs, PassOutcome, SpaceProbe, VolumeFs,
    allowed, measure_dir, run,
};
use swamp_core::volume_ledger::system::{
    ProbeError, SystemProbe, parse_apfs_list, parse_data_volume_info, parse_snapshots,
};
use swamp_core::volume_ledger::{
    Accounted, Category, Exactness, MountKind, MountView, RESIDUAL_NAME, Row, account,
    accounted_rows, read_account, render_disk_view,
};

const NOW: u64 = 1_800_000_000;

// ---- a virtual filesystem ------------------------------------------------

#[derive(Clone)]
enum Node {
    Dir {
        dev: u64,
        mtime: i64,
        kids: Vec<(OsString, Kind)>,
    },
    File(FsStat),
}

type Hook = (PathBuf, Box<dyn Fn(&FakeFs) + Send>);

#[derive(Default)]
struct FakeFs {
    nodes: Mutex<HashMap<PathBuf, Node>>,
    denied: Mutex<HashSet<PathBuf>>,
    mount_table: Mutex<Vec<MountView>>,
    /// Every path the pass listed or statted.
    touched: Mutex<Vec<PathBuf>>,
    /// Sleep on every `list` under this prefix.
    slow: Mutex<Option<(PathBuf, Duration)>>,
    /// Run after the listing of a path, once.
    hooks: Mutex<Vec<Hook>>,
    lists: AtomicU64,
    stats: AtomicU64,
}

fn name_of(path: &Path) -> OsString {
    path.file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default()
}

impl FakeFs {
    /// A machine whose data volume is mounted at `/data`, as
    /// `/System/Volumes/Data` is on a Mac. Paths given to the helpers
    /// below are logical (as seen from `/`); `raw_*` helpers take real
    /// ones, for the sealed system volume outside `/data`.
    fn new() -> FakeFs {
        let fs = FakeFs::default();
        fs.mkdir_real("/", 1);
        fs.mkdir_real("/data", 1);
        fs
    }

    fn lp(&self, logical: &str) -> String {
        if logical == "/" {
            "/data".to_string()
        } else {
            format!("/data{logical}")
        }
    }

    fn set_mounts(&self, mounts: Vec<MountView>) {
        *self.mount_table.lock().unwrap() = mounts;
    }

    fn add_kid(&self, path: &Path, kind: Kind) {
        let Some(parent) = path.parent() else { return };
        let mut nodes = self.nodes.lock().unwrap();
        if let Some(Node::Dir { kids, .. }) = nodes.get_mut(parent) {
            let n = name_of(path);
            kids.retain(|(k, _)| *k != n);
            kids.push((n, kind));
        }
    }

    /// A directory, creating missing ancestors on the same device.
    fn mkdir(&self, path: &str, dev: u64) {
        self.mkdir_real(&self.lp(path), dev);
    }

    fn mkdir_real(&self, path: &str, dev: u64) {
        let path = PathBuf::from(path);
        let mut missing = Vec::new();
        let mut cur = Some(path.as_path());
        {
            let nodes = self.nodes.lock().unwrap();
            while let Some(p) = cur {
                if nodes.contains_key(p) {
                    break;
                }
                missing.push(p.to_path_buf());
                cur = p.parent();
            }
        }
        for p in missing.into_iter().rev() {
            self.nodes.lock().unwrap().insert(
                p.clone(),
                Node::Dir {
                    dev,
                    mtime: 100,
                    kids: Vec::new(),
                },
            );
            self.add_kid(&p, Kind::Dir);
        }
    }

    fn file(&self, path: &str, blocks: u64, ino: u64, nlink: u64) {
        self.file_real(&self.lp(path), blocks, ino, nlink);
    }

    fn file_real(&self, path: &str, blocks: u64, ino: u64, nlink: u64) {
        let path = PathBuf::from(path);
        let parent = path.parent().unwrap().to_str().unwrap().to_string();
        let dev = self.dev_of(Path::new(&parent)).unwrap_or(1);
        self.mkdir_real(&parent, dev);
        self.nodes.lock().unwrap().insert(
            path.clone(),
            Node::File(FsStat {
                dev,
                ino,
                nlink,
                blocks,
                mtime: 100,
            }),
        );
        self.add_kid(&path, Kind::File);
    }

    fn special(&self, path: &str, kind: Kind) {
        let path = PathBuf::from(self.lp(path));
        self.mkdir_real(path.parent().unwrap().to_str().unwrap(), 1);
        self.add_kid(&path, kind);
    }

    fn dev_of(&self, path: &Path) -> Option<u64> {
        match self.nodes.lock().unwrap().get(path) {
            Some(Node::Dir { dev, .. }) => Some(*dev),
            _ => None,
        }
    }

    fn deny(&self, path: &str) {
        let real = self.lp(path);
        self.mkdir_real(
            &real,
            self.dev_of(Path::new(&real).parent().unwrap()).unwrap_or(1),
        );
        self.denied.lock().unwrap().insert(PathBuf::from(real));
    }

    fn remove(&self, path: &str) {
        let path = PathBuf::from(self.lp(path));
        self.nodes.lock().unwrap().remove(&path);
        if let Some(parent) = path.parent()
            && let Some(Node::Dir { kids, mtime, .. }) = self.nodes.lock().unwrap().get_mut(parent)
        {
            let n = name_of(&path);
            kids.retain(|(k, _)| *k != n);
            *mtime += 1;
        }
    }

    fn bump_mtime(&self, path: &str) {
        let real = self.lp(path);
        if let Some(Node::Dir { mtime, .. }) = self.nodes.lock().unwrap().get_mut(Path::new(&real))
        {
            *mtime += 5;
        }
    }

    fn after_listing(&self, path: &str, f: impl Fn(&FakeFs) + Send + 'static) {
        self.hooks
            .lock()
            .unwrap()
            .push((PathBuf::from(self.lp(path)), Box::new(f)));
    }

    /// Slow every listing under this logical prefix.
    fn slow_under(&self, logical_prefix: &str, delay: Duration) {
        *self.slow.lock().unwrap() = Some((PathBuf::from(self.lp(logical_prefix)), delay));
    }

    fn touched_under(&self, logical_prefix: &str) -> usize {
        self.touched_real_under(&self.lp(logical_prefix))
    }

    fn touched_real_under(&self, prefix: &str) -> usize {
        self.touched
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.starts_with(prefix) && p.as_path() != Path::new(prefix))
            .count()
    }
}

impl VolumeFs for FakeFs {
    fn list(&self, dir: &Path) -> io::Result<Vec<FsEntry>> {
        self.lists.fetch_add(1, Ordering::SeqCst);
        self.touched.lock().unwrap().push(dir.to_path_buf());
        if let Some((prefix, d)) = self.slow.lock().unwrap().clone()
            && dir
                .to_string_lossy()
                .starts_with(prefix.to_string_lossy().as_ref())
        {
            std::thread::sleep(d);
        }
        if self.denied.lock().unwrap().contains(dir) {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        let out = match self.nodes.lock().unwrap().get(dir) {
            Some(Node::Dir { kids, .. }) => Ok(kids
                .iter()
                .map(|(name, kind)| FsEntry {
                    name: name.clone(),
                    kind: *kind,
                })
                .collect()),
            _ => Err(io::Error::from(io::ErrorKind::NotFound)),
        };
        let mut hooks = self.hooks.lock().unwrap();
        if let Some(i) = hooks.iter().position(|(p, _)| p == dir) {
            let (_, hook) = hooks.remove(i);
            drop(hooks);
            hook(self);
        }
        out
    }

    fn lstat(&self, path: &Path) -> io::Result<FsStat> {
        self.stats.fetch_add(1, Ordering::SeqCst);
        self.touched.lock().unwrap().push(path.to_path_buf());
        match self.nodes.lock().unwrap().get(path) {
            Some(Node::Dir { dev, mtime, .. }) => Ok(FsStat {
                dev: *dev,
                ino: 0,
                nlink: 2,
                blocks: 0,
                mtime: *mtime,
            }),
            Some(Node::File(st)) => Ok(*st),
            None => Err(io::Error::from(io::ErrorKind::NotFound)),
        }
    }

    fn mounts(&self) -> Vec<MountView> {
        self.mount_table.lock().unwrap().clone()
    }
}

// ---- fixtures for the other collaborators -------------------------------

struct FakeSpace(Option<(u64, u64)>);

impl SpaceProbe for FakeSpace {
    fn container(&self) -> Option<(u64, u64)> {
        self.0
    }
}

struct FakeProbe {
    apfs: Result<String, ProbeError>,
    info: Result<String, ProbeError>,
    snapshots: Result<String, ProbeError>,
    calls: AtomicU64,
}

impl SystemProbe for FakeProbe {
    fn apfs_list(&self) -> Result<String, ProbeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.apfs.clone()
    }
    fn info_data_volume(&self) -> Result<String, ProbeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.info.clone()
    }
    fn local_snapshots(&self) -> Result<String, ProbeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.snapshots.clone()
    }
}

fn apfs_xml(volumes: &[(&str, &str, Option<u64>)]) -> String {
    let mut vols = String::new();
    for (name, role, used) in volumes {
        let roles = if role.is_empty() {
            "<array/>".to_string()
        } else {
            format!("<array><string>{role}</string></array>")
        };
        let cap = used
            .map(|u| format!("<key>CapacityInUse</key><integer>{u}</integer>"))
            .unwrap_or_default();
        vols.push_str(&format!(
            "<dict><key>Name</key><string>{name}</string>{cap}\
             <key>DeviceIdentifier</key><string>disk3s{}</string>\
             <key>Roles</key>{roles}</dict>",
            name.len()
        ));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\"><dict><key>Containers</key><array>\
         <dict><key>ContainerReference</key><string>disk9</string><key>Volumes</key><array>\
         <dict><key>Name</key><string>iOS Simulator</string><key>CapacityInUse</key><integer>99999999999</integer>\
         <key>DeviceIdentifier</key><string>disk9s1</string><key>Roles</key><array/></dict></array></dict>\
         <dict><key>ContainerReference</key><string>disk3</string><key>Volumes</key><array>{vols}</array></dict>\
         </array></dict></plist>"
    )
}

fn info_xml(purgeable: Option<u64>) -> String {
    let p = purgeable
        .map(|n| format!("<key>APFSPurgeableSpace</key><integer>{n}</integer>"))
        .unwrap_or_default();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
         <key>APFSContainerReference</key><string>disk3</string>\
         <key>CapacityInUse</key><integer>1</integer>{p}</dict></plist>"
    )
}

const SYSTEM_BYTES: u64 = 14_000_000;
const PREBOOT_BYTES: u64 = 22_000_000;
const DATA_BYTES: u64 = 40_000_000;

fn good_probe() -> FakeProbe {
    FakeProbe {
        apfs: Ok(apfs_xml(&[
            ("Macintosh HD", "System", Some(SYSTEM_BYTES)),
            ("Preboot", "Preboot", Some(PREBOOT_BYTES)),
            ("Data", "Data", Some(DATA_BYTES)),
        ])),
        info: Ok(info_xml(None)),
        snapshots: Ok("Snapshots for volume group containing disk /:\n".to_string()),
        calls: AtomicU64::new(0),
    }
}

fn layout() -> Layout {
    Layout {
        data_root: PathBuf::from("/data"),
        expand: [
            "/Users",
            "/Users/me",
            "/Library",
            "/opt",
            "/private",
            "/Applications",
            "/System",
            "/System/Library",
        ]
        .iter()
        .map(PathBuf::from)
        .collect(),
    }
}

/// A small machine: a sealed system volume (real paths outside `/data`,
/// never to be listed), a data volume that keeps `/System/Library/AssetsV2`
/// and `/usr/local`, a home with a declared source root, protected
/// folders, mounted disk images and the dmg that backs one of them.
fn world() -> FakeFs {
    let fs = FakeFs::new();
    // The sealed system volume: real paths, device 2. Listing or statting
    // anything here means the pass walked it.
    fs.mkdir_real("/System", 2);
    fs.file_real("/System/Library/CoreServices/big", 5_000, 900, 1);
    fs.file_real("/usr/bin/tool", 4_000, 901, 1);
    // What the data volume keeps under the same names.
    fs.file("/System/Library/AssetsV2/runtime.dmg", 2_000, 10, 1);
    fs.file("/usr/local/lib.a", 100, 11, 1);
    // Home.
    fs.file("/Users/me/src/code.rs", 7_777, 20, 1);
    fs.file("/Users/me/Library/Caches/thing/blob", 300, 21, 1);
    fs.deny("/Users/me/Library/Mail");
    fs.deny("/Users/me/Pictures");
    fs.file("/Users/me/notes.txt", 10, 22, 1);
    fs.file("/Users/me/Downloads/big.iso", 900, 23, 1);
    // Mounted images: views of files stored elsewhere (device 9).
    fs.mkdir("/Library/Developer/CoreSimulator/Volumes", 1);
    fs.mkdir("/Library/Developer/CoreSimulator/Volumes/iOS_1", 9);
    fs.file(
        "/Library/Developer/CoreSimulator/Volumes/iOS_1/Runtime",
        10_000,
        30,
        1,
    );
    fs.file("/Library/Developer/CoreSimulator/Caches/c", 500, 31, 1);
    fs.mkdir("/Volumes", 1);
    fs.mkdir("/Volumes/Share", 7);
    fs.mkdir("/Volumes/Recovery", 6);
    fs.set_mounts(vec![
        MountView {
            path: PathBuf::from("/Library/Developer/CoreSimulator/Volumes/iOS_1"),
            kind: MountKind::OwnStorage,
            used: Some(9_000 * 512),
        },
        MountView {
            path: PathBuf::from("/Volumes/Share"),
            kind: MountKind::Remote,
            used: None,
        },
        MountView {
            path: PathBuf::from("/Volumes/Recovery"),
            kind: MountKind::SameContainer,
            used: None,
        },
    ]);
    fs.file("/opt/homebrew/bin/brew", 400, 40, 1);
    fs.file("/Applications/Xcode.app/x", 1_000, 41, 1);
    // Special files and symlinks: never statted, never opened.
    fs.special("/private/tmp/pipe", Kind::Other);
    fs.special("/private/tmp/link", Kind::Symlink);
    fs.file("/private/tmp/file", 50, 50, 1);
    fs.file("/private/var/x", 60, 51, 1);
    fs
}

fn accounted_src() -> Vec<Accounted> {
    vec![Accounted {
        path: PathBuf::from("/Users/me/src"),
        bytes: 7_777 * 512,
        category: Category::Declared,
        subset_of_enclosing: false,
        measured_at: NOW,
        incomplete: false,
        note: None,
    }]
}

struct Setup {
    store: tempfile::TempDir,
    fs: FakeFs,
    layout: Layout,
    probe: FakeProbe,
    space: FakeSpace,
    accounted: Vec<Accounted>,
}

impl Setup {
    fn new() -> Setup {
        Setup {
            store: tempfile::tempdir().unwrap(),
            fs: world(),
            layout: layout(),
            probe: good_probe(),
            space: FakeSpace(Some((
                10_000_000_000,
                10_000_000_000 - (SYSTEM_BYTES + PREBOOT_BYTES + DATA_BYTES),
            ))),
            accounted: accounted_src(),
        }
    }

    fn run_at(
        &self,
        now: u64,
        force: bool,
        budget: Duration,
        min_free: Option<u64>,
    ) -> PassOutcome {
        run(&PassInputs {
            store_dir: self.store.path(),
            now,
            interval: Duration::from_secs(24 * 3600),
            budget,
            force,
            layout: &self.layout,
            fs: &self.fs,
            probe: &self.probe,
            space: &self.space,
            accounted: &self.accounted,
            min_free,
            workers: 1,
        })
        .unwrap()
    }

    fn pass(&self) -> PassOutcome {
        self.run_at(NOW, true, Duration::from_secs(60), Some(0))
    }

    fn rows(&self) -> Vec<Row> {
        read_account(self.store.path())
            .unwrap()
            .map(|a| a.rows)
            .unwrap_or_default()
    }

    fn row(&self, path: &str) -> Option<Row> {
        self.rows().into_iter().find(|r| r.path == path)
    }
}

fn ran(o: &PassOutcome) -> &swamp_core::volume_ledger::pass::RunSummary {
    match o {
        PassOutcome::Ran(s) => s,
        PassOutcome::Skipped(why) | PassOutcome::NotDue(why) => {
            panic!("expected a run, got: {why}")
        }
    }
}

// ---- adversarial: denied folders -------------------------------------------

// ======================= auditor's tests =======================

use swamp_core::growth::{VolumeLedgerRow, VolumeMetaRow};

fn meta(container_used: Option<u64>, data_used: Option<u64>) -> VolumeMetaRow {
    VolumeMetaRow {
        measured_at: NOW,
        cycle_started_at: NOW,
        cycle_complete_at: NOW,
        complete: true,
        budget_secs: 60,
        budget_used_ms: 1,
        statfs_at: NOW,
        container_total: container_used.map(|u| u.saturating_mul(2)),
        container_used,
        container_free: container_used,
        data_volume_used: data_used,
    }
}

fn other(path: &str, bytes: u64) -> Row {
    Row {
        path: path.to_string(),
        category: Category::Other,
        bytes: Some(bytes),
        overlap_bytes: 0,
        entries: Some(1),
        unreadable: 0,
        measured_at: NOW,
        method: "walk: allocated bytes, lstat only".to_string(),
        exactness: Exactness::Exact,
        note: None,
    }
}

const GB: u64 = 1_000_000_000;

#[test]
fn adv_future_measured_at_row_does_not_wedge_the_cursor() {
    // Tempting wrong patch (the PR's): `cycle_started_at = now.max(max_prev + 1)`.
    // One row from the future (clock set back, a restored store) puts the
    // cursor after `now`, so no row this run writes is ever "fresh": the
    // cycle never completes and every run restarts at the first task.
    let s = Setup::new();
    ran(&s.pass());
    let (mut rows, m) = read_volume_ledger(s.store.path()).unwrap();
    let m = m.unwrap();
    rows[0].measured_at = NOW + 365 * 86_400;
    write_volume_ledger(s.store.path(), &rows, &m, Some(m.measured_at)).unwrap();
    let mut done = false;
    for i in 1..=3 {
        if ran(&s.run_at(NOW + i, true, Duration::from_secs(60), Some(0))).complete {
            done = true;
            break;
        }
    }
    assert!(
        done,
        "a single future-dated row wedges the cycle: never complete in 3 ample runs"
    );
}

#[test]
fn adv_elimination_estimate_does_not_absorb_a_gap_when_nothing_was_unreadable() {
    // Tempting wrong patch (the PR's): plugging `data_used - measured` in as
    // "not measured" whatever the walk missed. Here no folder was unreadable,
    // yet 99% of the Data volume is attributed to "not measured" and the
    // residual reports a perfect 0 within 1%: a walk bug that skips 99 GB is
    // invisible.
    let rows = vec![other("/Users", GB)];
    let a = account(&rows, &meta(Some(100 * GB), Some(100 * GB)));
    assert_eq!(a.not_measured.count, 0);
    let est = a.not_measured.estimate_bytes.unwrap_or(0);
    let flagged = a.residual.within_one_percent == Some(false);
    assert!(
        est == 0 || flagged,
        "0 unreadable folders but {est} bytes filed as 'not measured', residual {:?} within 1% = {:?}",
        a.residual.bytes,
        a.residual.within_one_percent
    );
}

#[test]
fn adv_residual_outside_tolerance_is_flagged_in_the_text() {
    // Tempting wrong patch (the PR's): printing the residual percentage but
    // never saying the parts do not reconcile; only JSON has the flag.
    let rows = vec![other("/Users", 10 * GB)];
    let a = account(&rows, &meta(Some(100 * GB), None));
    assert_eq!(a.residual.within_one_percent, Some(false));
    let text = render_disk_view(Some(&a), NOW);
    let l = text.to_lowercase();
    assert!(
        l.contains("tolerance")
            || l.contains("do not add up")
            || l.contains("not reconcile")
            || l.contains("outside 1%")
            || l.contains("more than 1%"),
        "a 90% residual is shown without any flag:\n{text}"
    );
}

#[test]
fn adv_huge_ledger_values_do_not_panic_the_reading() {
    // Tempting wrong patch (the PR's): plain `+`/`sum()` and `used as i64`
    // over stored values. A corrupt or foreign ledger with huge values must
    // not panic `report` (debug) or wrap to nonsense (release).
    let rows = vec![other("/a", u64::MAX / 2 + 1), other("/b", u64::MAX / 2 + 1)];
    let r = std::panic::catch_unwind(|| {
        let a = account(&rows, &meta(Some(u64::MAX - 1), Some(u64::MAX)));
        render_disk_view(Some(&a), NOW)
    });
    assert!(r.is_ok(), "account() panicked on large stored values");
}

#[test]
fn adv_non_utf8_sibling_names_are_two_rows_not_one() {
    // Tempting wrong patch (the PR's): keying rows by `path.display()`.
    // Two folders whose names differ only in invalid UTF-8 bytes (possible
    // on Linux ext4) get the same key; dedup keeps one and the other's
    // bytes vanish.
    use std::os::unix::ffi::OsStrExt;
    let s = Setup::new();
    for (b, ino, blocks) in [(0xffu8, 7001u64, 1000u64), (0xfe, 7002, 3000)] {
        let name = std::ffi::OsStr::from_bytes(&[b'x', b]).to_os_string();
        let real = PathBuf::from("/data/Users/me").join(&name);
        s.fs.nodes.lock().unwrap().insert(
            real.clone(),
            Node::Dir {
                dev: 1,
                mtime: 100,
                kids: vec![],
            },
        );
        s.fs.add_kid(&real, Kind::Dir);
        let f = real.join("f");
        s.fs.nodes.lock().unwrap().insert(
            f.clone(),
            Node::File(FsStat {
                dev: 1,
                ino,
                nlink: 1,
                blocks,
                mtime: 100,
            }),
        );
        s.fs.add_kid(&f, Kind::File);
    }
    ran(&s.pass());
    let a = read_account(s.store.path()).unwrap().unwrap();
    let got: u64 = a
        .rows
        .iter()
        .filter(|r| r.category == Category::Other && r.path.starts_with("/Users/me/x"))
        .filter_map(|r| r.bytes)
        .sum();
    assert_eq!(
        got,
        4000 * 512,
        "rows: {:?}",
        a.rows
            .iter()
            .filter(|r| r.path.starts_with("/Users/me/x"))
            .collect::<Vec<_>>()
    );
}

#[test]
fn adv_a_second_data_volume_in_the_container_is_not_dropped() {
    // Tempting wrong patch (the PR's): "the first volume with role Data is
    // this machine's Data volume, and every Data volume is skipped". A
    // container with two macOS installs has two; the other install's bytes
    // vanish and data_volume_used may be the wrong one.
    let apfs = "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>Containers</key><array>\
        <dict><key>ContainerReference</key><string>disk3</string><key>Volumes</key><array>\
        <dict><key>Name</key><string>Other Data</string><key>CapacityInUse</key><integer>7000</integer><key>DeviceIdentifier</key><string>disk3s1</string><key>Roles</key><array><string>Data</string></array></dict>\
        <dict><key>Name</key><string>Data</string><key>CapacityInUse</key><integer>5000</integer><key>DeviceIdentifier</key><string>disk3s5</string><key>Roles</key><array><string>Data</string></array></dict>\
        </array></dict></array></dict></plist>";
    let info = "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>APFSContainerReference</key><string>disk3</string>\
        <key>DeviceIdentifier</key><string>disk3s5</string><key>CapacityInUse</key><integer>5000</integer></dict></plist>";
    let probe = FakeProbe {
        apfs: Ok(apfs.into()),
        info: Ok(info.into()),
        snapshots: Ok(String::new()),
        calls: AtomicU64::new(0),
    };
    let facts = swamp_core::volume_ledger::system::collect(&probe, NOW);
    assert_eq!(
        facts.data_volume_used,
        Some(5000),
        "picked the other install's Data volume"
    );
    assert!(
        facts.rows.iter().any(|r| r.bytes == Some(7000)),
        "the other Data volume's 7000 bytes are in no row: {:?}",
        facts.rows
    );
}

#[test]
fn adv_a_corrupt_ledger_file_does_not_wedge_every_future_pass() {
    // Tempting wrong patch (the PR's): `read_volume_ledger(..)?` at the top of
    // the pass. A torn/corrupt ledger (disk error, foreign writer) makes every
    // later observe print "volume pass failed" forever; nothing rebuilds it.
    let s = Setup::new();
    ran(&s.pass());
    std::fs::write(s.store.path().join("volume_ledger.parquet"), b"PAR1 torn").unwrap();
    let r = run(&PassInputs {
        store_dir: s.store.path(),
        now: NOW + 10,
        interval: Duration::from_secs(86_400),
        budget: Duration::from_secs(60),
        force: true,
        layout: &s.layout,
        fs: &s.fs,
        probe: &s.probe,
        space: &s.space,
        accounted: &s.accounted,
        min_free: Some(0),
        workers: 1,
    });
    assert!(
        r.is_ok(),
        "pass cannot recover from a corrupt ledger: {:?}",
        r.err()
    );
    assert!(read_account(s.store.path()).is_ok());
}

#[test]
fn adv_rows_written_but_meta_not_is_consistent_on_the_next_run() {
    // Crash between the two renames (rows replaced, meta old): the next run
    // must proceed (not refuse as a concurrent writer) and leave both files
    // consistent (meta newer than every row).
    let s = Setup::new();
    ran(&s.pass());
    let (rows, m) = read_volume_ledger(s.store.path()).unwrap();
    let m = m.unwrap();
    let mut newer = rows.clone();
    for r in &mut newer {
        r.measured_at = NOW + 5;
    }
    // Simulate: rows file from the crashed run, meta untouched.
    let meta_path = s.store.path().join("volume_ledger_meta.parquet");
    let saved = std::fs::read(&meta_path).unwrap();
    write_volume_ledger(s.store.path(), &newer, &m, Some(m.measured_at)).unwrap();
    std::fs::write(&meta_path, saved).unwrap();
    ran(&s.run_at(NOW + 6, true, Duration::from_secs(60), Some(0)));
    let (rows2, m2) = read_volume_ledger(s.store.path()).unwrap();
    let m2 = m2.unwrap();
    assert!(rows2.iter().all(|r| r.measured_at <= m2.measured_at));
    assert!(m2.complete);
}

#[test]
fn adv_hardlinks_across_two_folders_are_counted_once_in_the_ledger() {
    // Tempting wrong patch (the PR's, documented as "once per task"): the
    // hardlink set is per task, so one 1000-block file linked into two
    // top-level folders is counted twice in "everything else".
    let s = Setup::new();
    s.fs.file("/Users/me/h1/f", 1000, 8888, 2);
    s.fs.file("/Users/me/h2/f", 1000, 8888, 2);
    ran(&s.pass());
    let a = read_account(s.store.path()).unwrap().unwrap();
    let h: u64 = a
        .rows
        .iter()
        .filter(|r| r.path.starts_with("/Users/me/h"))
        .filter_map(|r| r.bytes)
        .sum();
    assert_eq!(
        h,
        1000 * 512,
        "hardlinked bytes counted {}x",
        h / (1000 * 512)
    );
}

#[test]
fn adv_accounted_root_spelled_through_the_data_volume_mount_is_still_pruned() {
    // Tempting wrong patch (the PR's): pruning by exact PathBuf equality. A
    // declared root spelled `/System/Volumes/Data/Users/me/src` (here the
    // fixture's data root `/data`) is not recognised as `/Users/me/src`, so
    // its bytes are walked again and counted twice.
    let mut s = Setup::new();
    s.accounted[0].path = PathBuf::from("/data/Users/me/src");
    s.layout = Layout {
        data_root: PathBuf::from("/data"),
        ..layout()
    };
    ran(&s.pass());
    let a = read_account(s.store.path()).unwrap().unwrap();
    let walked: u64 = a
        .rows
        .iter()
        .filter(|r| r.category == Category::Other && r.path.starts_with("/Users/me/src"))
        .filter_map(|r| r.bytes)
        .sum();
    assert_eq!(
        walked, 0,
        "the declared root was walked again: {walked} bytes counted twice"
    );
}

#[test]
fn adv_a_zero_budget_still_moves_the_cursor() {
    // `volume_pass_budget_secs = 0` is accepted by config; the deadline is
    // then `started`, no task ever runs, and the pass never completes.
    let s = Setup::new();
    let mut complete = false;
    for i in 0..5 {
        if ran(&s.run_at(NOW + i, true, Duration::ZERO, Some(0))).complete {
            complete = true;
            break;
        }
    }
    assert!(
        complete,
        "budget 0: five runs, no progress (config accepts 0 silently)"
    );
}

#[test]
fn adv_reading_a_200k_row_ledger_does_zero_work() {
    let s = Setup::new();
    ran(&s.pass());
    let (_, m) = read_volume_ledger(s.store.path()).unwrap();
    let m = m.unwrap();
    let rows: Vec<VolumeLedgerRow> = (0..200_000)
        .map(|i| other(&format!("/x/{i}"), i))
        .map(|r| VolumeLedgerRow {
            path: r.path,
            category: "other".into(),
            allocated_bytes: r.bytes,
            overlap_bytes: 0,
            entry_count: Some(1),
            unreadable_count: 0,
            measured_at: NOW,
            method: r.method,
            exactness: "exact".into(),
            note: None,
        })
        .collect();
    write_volume_ledger(s.store.path(), &rows, &m, Some(m.measured_at)).unwrap();
    let (_, work) = swamp_core::work_counters::measured(|| {
        let a = read_account(s.store.path()).unwrap();
        let _ = render_disk_view(a.as_ref(), NOW);
        let _ = swamp_core::volume_ledger::disk_json(a.as_ref(), NOW);
    });
    assert_eq!(
        (work.dirs_listed, work.files_statted, work.subprocess_spawns),
        (0, 0, 0)
    );
}

#[test]
fn adv_plist_edge_cases_are_notes_not_numbers_or_panics() {
    for xml in [
        // purgeable as a string, CapacityInUse negative, unicode name
        "<plist><dict><key>APFSContainerReference</key><string>disk3</string><key>CapacityInUse</key><integer>-5</integer><key>APFSPurgeableSpace</key><string>12</string></dict></plist>",
        // billion-laughs style entities
        "<?xml version=\"1.0\"?><!DOCTYPE p [<!ENTITY a \"aaaaaaaaaa\"><!ENTITY b \"&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;\"><!ENTITY c \"&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;\"><!ENTITY d \"&c;&c;&c;&c;&c;&c;&c;&c;&c;&c;\"><!ENTITY e \"&d;&d;&d;&d;&d;&d;&d;&d;&d;&d;\"><!ENTITY f \"&e;&e;&e;&e;&e;&e;&e;&e;&e;&e;\"><!ENTITY g \"&f;&f;&f;&f;&f;&f;&f;&f;&f;&f;\">]><plist><dict><key>CapacityInUse</key><string>&g;</string></dict></plist>",
        "<plist><integer>99999999999999999999999</integer></plist>",
        "",
    ] {
        let t = Instant::now();
        let r = std::panic::catch_unwind(|| parse_data_volume_info(xml));
        assert!(r.is_ok(), "panic on {xml:.60}");
        if let Ok(Ok(i)) = r {
            assert_eq!(i.in_use, None);
            assert_eq!(i.purgeable, None);
        }
        assert!(t.elapsed() < Duration::from_secs(2));
    }
    // 12 volumes, unicode names, one locked (no CapacityInUse).
    let mut vols: Vec<(String, &str, Option<u64>)> = (0..11)
        .map(|i| (format!("Vol ü{i}"), "", Some(1000 + i)))
        .collect();
    vols.push(("Locked 🔒".into(), "", None));
    vols.push(("Data".into(), "Data", Some(5)));
    let v: Vec<(&str, &str, Option<u64>)> =
        vols.iter().map(|(a, b, c)| (a.as_str(), *b, *c)).collect();
    let c = parse_apfs_list(&apfs_xml(&v), Some("disk3")).unwrap();
    assert_eq!(c.volumes.len(), 13);
    assert_eq!(c.volumes.iter().filter(|v| v.in_use.is_none()).count(), 1);
}

/// Helper for the v0.7.5 survival check: writes a ledger into $ADV_LEDGER_OUT.
#[test]
#[ignore]
fn adv_emit_ledger_for_compat_check() {
    let s = Setup::new();
    ran(&s.pass());
    let out = PathBuf::from(std::env::var("ADV_LEDGER_OUT").unwrap());
    for f in ["volume_ledger.parquet", "volume_ledger_meta.parquet"] {
        std::fs::copy(s.store.path().join(f), out.join(f)).unwrap();
    }
}
