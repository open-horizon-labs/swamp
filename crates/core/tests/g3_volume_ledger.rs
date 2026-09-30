//! v0.8.0 G3: the volume ledger and the system-volumes line (#169, #170).
//!
//! The pass runs against a virtual filesystem (`FakeFs`), so a denied
//! folder, a sealed system volume, a mounted disk image, a tree that
//! changes mid-pass and a filesystem that is painfully slow are all
//! ordinary fixtures, and no test reads the developer's real disk. Real
//! files are used only where the point is the real syscalls (a FIFO, a
//! socket and a symlink in a temp dir).
//!
//! Each test names the tempting wrong patch it fails.

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
    /// Listing this real path blocks until `release` is set: a hung mount.
    block_on: Mutex<Option<PathBuf>>,
    /// Report 70% of the blocks the FIRST time each file under this prefix
    /// is statted: a walker that undercounts.
    lie_once: Mutex<Option<PathBuf>>,
    lied: Mutex<HashSet<PathBuf>>,
    release: std::sync::atomic::AtomicBool,
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

    fn undercount_first_look_under(&self, logical: &str) {
        *self.lie_once.lock().unwrap() = Some(PathBuf::from(self.lp(logical)));
    }

    fn block_forever_on(&self, logical: &str) {
        *self.block_on.lock().unwrap() = Some(PathBuf::from(self.lp(logical)));
    }

    fn touched_exactly(&self, logical: &str) -> usize {
        let real = PathBuf::from(self.lp(logical));
        self.touched
            .lock()
            .unwrap()
            .iter()
            .filter(|p| **p == real)
            .count()
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
        let blocked = self.block_on.lock().unwrap().as_deref() == Some(dir);
        while blocked && !self.release.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(20));
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
            Some(Node::File(st)) => {
                let mut st = *st;
                let lie = self.lie_once.lock().unwrap().clone();
                if let Some(prefix) = lie
                    && path.starts_with(&prefix)
                    && self.lied.lock().unwrap().insert(path.to_path_buf())
                {
                    st.blocks = st.blocks * 70 / 100;
                }
                Ok(st)
            }
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
    fs: std::sync::Arc<FakeFs>,
    layout: Layout,
    probe: FakeProbe,
    space: FakeSpace,
    accounted: Vec<Accounted>,
}

impl Setup {
    fn new() -> Setup {
        Setup {
            store: tempfile::tempdir().unwrap(),
            fs: std::sync::Arc::new(world()),
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
            fs: self.fs.clone(),
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

/// Runs `f` until it reports success, at most six times. The tests below
/// depend on a pass actually getting to start its first task; a host that
/// freezes the whole process for longer than a budget (seen once in 25
/// runs with eight CPU burners on a shared machine) makes that a coin flip
/// that no wall-clock margin fixes, so such a run is repeated. What each
/// closure asserts about a run that did get to work is not relaxed.
fn attempt<T>(what: &str, mut f: impl FnMut(u64) -> Option<T>) -> T {
    for n in 0..6 {
        if let Some(v) = f(n) {
            return v;
        }
    }
    panic!("{what}: six runs in a row did not get to work (host stalled?)");
}

// ---- adversarial: denied folders -------------------------------------------

#[test]
fn a_folder_that_cannot_be_read_is_not_measured_never_zero_and_never_dropped() {
    // Tempting wrong patch: `read_dir(..).map(..).unwrap_or(0)` or a
    // silent `continue` on the error, which reports the protected folder
    // as an empty one (or as nothing at all).
    let s = Setup::new();
    assert!(ran(&s.pass()).complete);
    for denied in ["/Users/me/Pictures", "/Users/me/Library/Mail"] {
        let row = s
            .row(denied)
            .unwrap_or_else(|| panic!("{denied} was dropped"));
        assert_eq!(row.category, Category::Unreadable, "{denied}");
        assert_eq!(row.bytes, None, "{denied} must not carry a size");
        assert_eq!(row.exactness, Exactness::NotMeasured);
        assert_ne!(row.bytes, Some(0));
    }
    let a = read_account(s.store.path()).unwrap().unwrap();
    assert_eq!(a.not_measured.count, 2);
    assert!(a.not_measured.names.iter().any(|n| n.ends_with("Pictures")));
    assert!(a.not_measured.names.iter().any(|n| n.ends_with("Mail")));
    // The folder holding one is a lower bound, and says so.
    let library = s.row("/Users/me/Library").unwrap();
    assert_eq!(library.exactness, Exactness::Estimated);
    assert_eq!(library.unreadable, 1);
    assert!(
        library
            .note
            .as_deref()
            .unwrap()
            .contains("could not be read")
    );
    // The report names them and says why, without asking for anything.
    let text = render_disk_view(Some(&a), NOW);
    assert!(text.contains("Not measured: 2 folders"));
    assert!(text.contains("Pictures") && text.contains("Mail"));
    assert!(text.contains("Full Disk Access"));
}

#[test]
fn special_files_and_symlinks_are_never_statted_or_followed() {
    // Tempting wrong patch: `lstat`ing (or worse, opening) every entry the
    // listing returns, which touches a FIFO or socket and follows nothing
    // only by luck. Kind comes from the directory read; only regular
    // files and directories are statted.
    let s = Setup::new();
    ran(&s.pass());
    assert_eq!(s.fs.touched_under("/private/tmp/pipe"), 0);
    assert_eq!(s.fs.touched_under("/private/tmp/link"), 0);
    let tmp = s.row("/private/tmp").unwrap();
    assert_eq!(
        tmp.exactness,
        Exactness::Exact,
        "a FIFO and a link are not a change"
    );
    assert_eq!(tmp.bytes, Some(50 * 512));
}

// ---- adversarial: the firmlink case ---------------------------------------

#[test]
fn a_system_path_the_data_volume_keeps_is_measured_not_skipped_as_sealed() {
    // Tempting wrong patch: skipping `/System` (and `/usr`) as "the sealed
    // system volume". On a real Mac 24.9 GiB of simulator runtime images
    // live in /System/Library/AssetsV2, on the DATA volume, and `st_dev`
    // cannot tell it from the sealed part (every path under `/` reports
    // one device), so the pass walks the data volume from its own mount
    // point and never lists the sealed one.
    let s = Setup::new();
    ran(&s.pass());
    let assets = s.row("/System/Library/AssetsV2").expect("AssetsV2 skipped");
    assert_eq!(assets.category, Category::Other);
    assert_eq!(assets.bytes, Some(2_000 * 512));
    let usr = s.row("/usr").expect("/usr/local is on the data volume");
    assert_eq!(usr.bytes, Some(100 * 512));
    // The sealed volume is never listed or statted; its bytes are the
    // System line from diskutil.
    assert_eq!(s.fs.touched_real_under("/System"), 0);
    assert_eq!(s.fs.touched_real_under("/usr"), 0);
    assert!(s.row("/System/Library/CoreServices").is_none());
    let a = read_account(s.store.path()).unwrap().unwrap();
    assert!(a.system_volumes.bytes >= SYSTEM_BYTES);
}

// ---- adversarial: mounted image counted twice -----------------------------

#[test]
fn a_mounted_image_and_its_backing_file_are_counted_once() {
    // Tempting wrong patch: adding the mounted volume's size (or a unit
    // that an observation walked through the mount points) on top of the
    // .dmg that backs it: ~44 GB double counted on the reporter's Mac.
    // Here the observation's unit for the mount directory counted the
    // mounted files, and the dmg is inside AssetsV2's row.
    let mut s = Setup::new();
    s.accounted.push(Accounted {
        path: PathBuf::from("/Library/Developer/CoreSimulator/Volumes"),
        bytes: 9_000 * 512,
        category: Category::Catalog,
        subset_of_enclosing: false,
        measured_at: NOW,
        incomplete: false,
        note: None,
    });
    ran(&s.pass());
    let a = read_account(s.store.path()).unwrap().unwrap();
    let unit = s
        .row("/Library/Developer/CoreSimulator/Volumes")
        .expect("the unit is listed");
    assert_eq!(
        unit.overlap_bytes,
        9_000 * 512,
        "its bytes are the mounted view"
    );
    assert_eq!(
        a.accounted.bytes,
        7_777 * 512,
        "only the source root is added"
    );
    // Listed as a view, never added.
    let mount = a
        .mounted_views
        .iter()
        .find(|r| r.path.ends_with("iOS_1"))
        .expect("the mounted image is listed");
    assert_eq!(mount.category, Category::Mount);
    assert_eq!(mount.bytes, Some(9_000 * 512));
    assert_eq!(
        s.fs.touched_under("/Library/Developer/CoreSimulator/Volumes/iOS_1"),
        0,
        "the walk never enters a mount"
    );
    // Everything else: the backing file once (inside AssetsV2), the
    // mounted image's own 10_000 blocks never.
    let else_expected: u64 = [2_000, 100, 300, 10, 900, 500, 400, 1_000, 50, 60]
        .iter()
        .sum::<u64>()
        * 512;
    assert_eq!(a.everything_else.bytes, else_expected);
    let text = render_disk_view(Some(&a), NOW);
    assert!(text.contains("Mounted disk images (not added"));
}

#[test]
fn mounts_found_by_the_walk_are_views_a_network_share_and_a_sibling_volume_have_no_size() {
    // Tempting wrong patch: `statfs` on every mount point and adding it.
    // On APFS that is the whole CONTAINER's used bytes (423 GB for a
    // Recovery volume on the reporter's Mac); on a network share it is not
    // this disk at all.
    let s = Setup::new();
    ran(&s.pass());
    let a = read_account(s.store.path()).unwrap().unwrap();
    let by = |suffix: &str| {
        a.mounted_views
            .iter()
            .find(|r| r.path.ends_with(suffix))
            .unwrap_or_else(|| panic!("{suffix} not listed"))
    };
    assert_eq!(by("iOS_1").bytes, Some(9_000 * 512));
    assert_eq!(by("Share").bytes, None);
    assert_eq!(by("Share").exactness, Exactness::NotMeasured);
    assert!(
        by("Share")
            .note
            .as_deref()
            .unwrap()
            .contains("network share")
    );
    assert_eq!(by("Recovery").bytes, None);
    assert!(
        by("Recovery")
            .note
            .as_deref()
            .unwrap()
            .contains("system volume lines")
    );
    // None of them is in what was measured.
    assert!(a.everything_else.bytes < 6_000 * 512);
    assert_eq!(
        a.not_measured.count, 2,
        "mounts are not 'could not be read'"
    );
}

// ---- adversarial: the identity ---------------------------------------------

#[test]
fn parts_add_up_to_the_container_with_a_named_residual_for_clones_sparse_and_hardlinks() {
    // Tempting wrong patch: counting `st_size` (a sparse file is huge),
    // counting each hardlink, or letting the sum "come out" by dropping
    // the remainder instead of naming it. Clones (two files, one set of
    // extents) cannot be seen per file: the overcount must surface as a
    // negative, named residual.
    let s = Setup::new();
    // A hardlink pair (one inode), a clone (another inode, same extents)
    // and a sparse file whose allocated size is tiny.
    s.fs.file("/private/var/lab/a", 100, 700, 2);
    s.fs.file("/private/var/lab/a-link", 100, 700, 2);
    s.fs.file("/private/var/lab/clone-of-a", 100, 701, 1);
    s.fs.file("/private/var/lab/sparse", 3, 702, 1);
    ran(&s.pass());
    let var = s.row("/private/var").unwrap();
    assert_eq!(
        var.bytes,
        Some((60 + 100 + 100 + 3) * 512),
        "hardlink once, clone twice, sparse by allocated blocks"
    );
    // The disk holds the clone's extents once: 100 blocks less than the
    // files add up to. The Data volume's own figure says so.
    let measured_data: u64 = read_account(s.store.path())
        .unwrap()
        .map(|a| a.accounted.bytes + a.everything_else.bytes)
        .unwrap();
    let physical_data = measured_data - 100 * 512;
    let container_used = physical_data + SYSTEM_BYTES + PREBOOT_BYTES;
    let mut probe = good_probe();
    probe.apfs = Ok(apfs_xml(&[
        ("Macintosh HD", "System", Some(SYSTEM_BYTES)),
        ("Preboot", "Preboot", Some(PREBOOT_BYTES)),
        ("Data", "Data", Some(physical_data)),
    ]));
    let s2 = Setup {
        probe,
        space: FakeSpace(Some((10_000_000_000, 10_000_000_000 - container_used))),
        ..Setup::new_with_world(s.fs)
    };
    ran(&s2.pass());
    let a = read_account(s2.store.path()).unwrap().unwrap();
    assert_eq!(a.container.used, Some(container_used));
    let residual = a.residual.bytes.unwrap();
    assert_eq!(
        residual,
        -(100 * 512),
        "the clone overcount is the residual"
    );
    assert_eq!(a.residual.name, RESIDUAL_NAME);
    let parts = a.accounted.bytes
        + a.everything_else.bytes
        + a.system_volumes.bytes
        + a.not_measured.estimate_bytes.unwrap_or(0);
    assert_eq!(parts as i64 + residual, container_used as i64);
    assert_eq!(a.residual.within_one_percent, Some(true));
    let text = render_disk_view(Some(&a), NOW);
    assert!(text.contains("Unattributed: allocation not explained by any measured part"));
}

impl Setup {
    fn new_with_world(fs: std::sync::Arc<FakeFs>) -> Setup {
        Setup { fs, ..Setup::new() }
    }
}

#[test]
fn what_could_not_be_read_is_an_estimate_by_elimination_and_the_parts_sum_within_one_percent() {
    // Tempting wrong patch: leaving the unreadable share out of the sum
    // (so a 14% gap appears as "unattributed"), or presenting the
    // estimate as a measurement.
    let s = Setup::new();
    ran(&s.pass());
    let a = read_account(s.store.path()).unwrap().unwrap();
    let measured = a.accounted.bytes + a.everything_else.bytes;
    assert!(
        DATA_BYTES > measured,
        "the fixture's Data volume is larger than what was read"
    );
    assert_eq!(a.not_measured.estimate_bytes, Some(DATA_BYTES - measured));
    let container_used = a.container.used.unwrap();
    let parts = a.accounted.bytes
        + a.everything_else.bytes
        + a.system_volumes.bytes
        + a.not_measured.estimate_bytes.unwrap();
    // The fixture's container holds exactly what diskutil says its
    // volumes hold, so nothing is left over, and the identity is exact.
    assert_eq!(a.residual.bytes, Some(container_used as i64 - parts as i64));
    assert_eq!(a.residual.bytes, Some(0));
    assert_eq!(a.residual.bookkeeping_balanced, Some(true));
    // The measured parts alone do NOT explain the Data volume: the protected
    // folders' share is an estimate, never a passing check.
    assert_eq!(a.residual.within_one_percent, Some(false));
    let text = render_disk_view(Some(&a), NOW);
    assert!(text.contains("Protected folders: not measured (2 folders)"));
    assert!(text.contains("an estimate"));
    // System volumes are separate volumes; the mounted image container
    // (disk9) is never one of them.
    let names: Vec<&str> = a
        .system_volumes
        .volumes
        .iter()
        .map(|r| r.path.as_str())
        .collect();
    assert_eq!(
        a.system_volumes.bytes,
        SYSTEM_BYTES + PREBOOT_BYTES,
        "{names:?}"
    );
    assert!(!names.iter().any(|n| n.contains("iOS Simulator")));
}

// ---- adversarial: the tree changes during the pass -----------------------

#[test]
fn a_tree_that_changes_during_the_pass_is_estimated_not_claimed_exact() {
    // Tempting wrong patch: `unwrap()` on the second lstat (a panic or an
    // abort when a file vanishes), or reporting a moving tree as exact.
    let s = Setup::new();
    s.fs.file("/opt/homebrew/lib/gone.dylib", 8_000, 60, 1);
    s.fs.file("/opt/homebrew/lib/stays.dylib", 100, 61, 1);
    s.fs.after_listing("/opt/homebrew/lib", |fs| {
        fs.remove("/opt/homebrew/lib/gone.dylib");
    });
    let outcome = s.pass();
    assert!(
        ran(&outcome).complete,
        "a vanishing file must not stop the pass"
    );
    let brew = s.row("/opt/homebrew").unwrap();
    assert_eq!(brew.exactness, Exactness::Estimated);
    assert!(brew.note.as_deref().unwrap().contains("changed"));
    assert_eq!(
        brew.bytes,
        Some((400 + 100) * 512),
        "the vanished file adds nothing"
    );
}

#[test]
fn a_folder_whose_modification_time_moves_while_it_is_read_is_estimated() {
    let s = Setup::new();
    s.fs.after_listing("/opt/homebrew/bin", |fs| fs.bump_mtime("/opt/homebrew/bin"));
    ran(&s.pass());
    assert_eq!(
        s.row("/opt/homebrew").unwrap().exactness,
        Exactness::Estimated
    );
    // An untouched task stays exact.
    assert_eq!(
        s.row("/Applications/Xcode.app").unwrap().exactness,
        Exactness::Exact
    );
}

// ---- adversarial: time budget, cursor -------------------------------------

fn many_small_folders(fs: &FakeFs, n: usize) {
    for i in 0..n {
        fs.file(
            &format!("/Users/me/d{i:03}/a"),
            10 + i as u64,
            1_000 + i as u64,
            1,
        );
        fs.file(&format!("/Users/me/d{i:03}/b"), 20, 5_000 + i as u64, 1);
    }
}

/// Totals per folder, whatever the granularity a run split it into (a
/// folder that filled a whole run alone is measured as its parts, which
/// sum to the same bytes and entries).
fn comparable(rows: &[Row]) -> Vec<(String, Option<u64>, Option<u64>)> {
    let mut by: std::collections::BTreeMap<String, (Option<u64>, Option<u64>)> = Default::default();
    for r in rows
        .iter()
        .filter(|r| r.category == Category::Other || r.category == Category::Unreadable)
        .filter(|r| r.method != "expanded" && r.method != "pending")
    {
        let key = r
            .path
            .strip_suffix("/(files directly here)")
            .unwrap_or(&r.path)
            .to_string();
        let e = by.entry(key).or_insert((None, None));
        e.0 = match (e.0, r.bytes) {
            (None, b) => b,
            (a, None) => a,
            (Some(a), Some(b)) => Some(a + b),
        };
        e.1 = match (e.1, r.entries) {
            (None, b) => b,
            (a, None) => a,
            (Some(a), Some(b)) => Some(a + b),
        };
    }
    by.into_iter().map(|(k, (b, n))| (k, b, n)).collect()
}

#[test]
fn a_slow_filesystem_stops_at_the_budget_and_the_cursor_resumes_to_identical_totals() {
    // Tempting wrong patches: ignoring the deadline (a slow disk hangs
    // `observe`), or restarting from the top every run (a slow disk never
    // finishes). Resuming must also land on exactly the totals an
    // uninterrupted pass gives.
    let fast = Setup::new();
    many_small_folders(&fast.fs, 300);
    assert!(ran(&fast.pass()).complete);
    let expected = comparable(&fast.rows());

    let slow = Setup::new();
    many_small_folders(&slow.fs, 300);
    slow.fs.slow_under("/Users/me/d", Duration::from_millis(20));
    let budget = Duration::from_millis(2_000);
    let mut runs = 0;
    let mut cycle_start: Option<u64> = None;
    let mut sizes: Vec<usize> = Vec::new();
    loop {
        runs += 1;
        assert!(runs < 200, "the pass never finished");
        let t = Instant::now();
        let outcome = slow.run_at(NOW + runs, true, budget, Some(0));
        let took = t.elapsed();
        // Never hangs: the run ends within a generous multiple of the
        // budget even when the machine running the tests stalls a write.
        assert!(took < Duration::from_secs(15), "run {runs} took {took:?}");
        let summary = ran(&outcome).clone();
        let (_, meta) = read_volume_ledger(slow.store.path()).unwrap();
        let meta = meta.unwrap();
        // The cursor is persisted and stable across the cycle.
        match cycle_start {
            None => cycle_start = Some(meta.cycle_started_at),
            Some(c) => assert_eq!(meta.cycle_started_at, c, "the cycle restarted"),
        }
        sizes.push(
            slow.rows()
                .iter()
                .filter(|r| r.category != Category::Audit)
                .count(),
        );
        // The walk itself stopped at the budget (not counting the write).
        assert!(
            meta.budget_used_ms < budget.as_millis() as u64 * 10,
            "run {runs}: {} ms against a {budget:?} budget",
            meta.budget_used_ms
        );
        if summary.complete {
            assert!(meta.complete);
            break;
        }
        assert!(!meta.complete, "an unfinished pass says so");
        assert!(summary.pending > 0);
    }
    assert!(
        runs >= 3,
        "the budget was meant to split the pass ({runs} runs)"
    );
    assert!(
        sizes.windows(2).all(|w| w[0] <= w[1]),
        "rows only accumulate: {sizes:?}"
    );
    // A host that froze the whole process for longer than a budget while a
    // folder was the first thing a worker did makes that folder "too big
    // for one run": a legitimate row, but not a sample of the walk. Such
    // folders (there can only be a handful) are left out of both sides.
    let stalled: HashSet<String> = slow
        .rows()
        .iter()
        .filter(|r| r.method == "over_budget" || r.method == "stuck")
        .map(|r| {
            r.path
                .trim_end_matches("/(files directly here)")
                .to_string()
        })
        .collect();
    assert!(stalled.len() <= 3, "{stalled:?}");
    let keep = |v: Vec<(String, Option<u64>, Option<u64>)>| -> Vec<_> {
        v.into_iter().filter(|r| !stalled.contains(&r.0)).collect()
    };
    assert_eq!(keep(comparable(&slow.rows())), keep(expected));
}

#[test]
fn a_partial_pass_leaves_a_partial_ledger_with_honest_ages() {
    // Tempting wrong patch: writing nothing until the whole pass finished,
    // or stamping every row with the last run's time.
    let s = Setup::new();
    many_small_folders(&s.fs, 300);
    s.fs.slow_under("/Users/me/d", Duration::from_millis(20));
    let mid = attempt("first run", |n| {
        let first = s.run_at(NOW + n, true, Duration::from_millis(2_000), Some(0));
        assert!(!ran(&first).complete);
        let mid = read_account(s.store.path()).unwrap().unwrap();
        assert!(!mid.complete);
        (mid.everything_else.folders > 0).then_some(mid)
    });
    let text = render_disk_view(Some(&mid), NOW + 1);
    assert!(text.contains("has not finished"));
    // Finish it later: rows measured in the first run keep the first time.
    let done = loop {
        let n = s.run_at(NOW + 3_600, true, Duration::from_millis(2_000), Some(0));
        if ran(&n).complete {
            break n;
        }
    };
    assert!(ran(&done).complete);
    let ages: HashSet<u64> = s
        .rows()
        .iter()
        .filter(|r| r.category == Category::Other)
        .map(|r| r.measured_at)
        .collect();
    assert!(
        ages.contains(&NOW),
        "first-run rows keep their own time: {ages:?}"
    );
    assert!(ages.contains(&(NOW + 3_600)));
}

#[test]
fn a_folder_that_alone_exceeds_the_budget_is_measured_as_its_parts_next_run() {
    // Tempting wrong patch: retrying the same giant folder from scratch
    // every run (a livelock: it never fits in one budget).
    let fast = Setup::new();
    for i in 0..160 {
        fast.fs
            .file(&format!("/Users/me/huge/sub{i:02}/f"), 40 + i, 3_000 + i, 1);
    }
    assert!(ran(&fast.pass()).complete);
    let whole = fast.row("/Users/me/huge").unwrap().bytes.unwrap();

    let s = Setup::new();
    for i in 0..160 {
        s.fs.file(&format!("/Users/me/huge/sub{i:02}/f"), 40 + i, 3_000 + i, 1);
    }
    s.fs.slow_under("/Users/me/huge", Duration::from_millis(30));
    let budget = Duration::from_millis(2_000);
    let mut runs = 0;
    loop {
        runs += 1;
        assert!(runs < 60, "a folder larger than the budget never finished");
        if ran(&s.run_at(NOW + runs, true, budget, Some(0))).complete {
            break;
        }
    }
    let rows = s.rows();
    assert!(
        rows.iter()
            .any(|r| r.path == "/Users/me/huge" && r.method == "expanded"),
        "the giant folder is recorded as expanded"
    );
    let parts: u64 = rows
        .iter()
        .filter(|r| r.category == Category::Other && r.path.starts_with("/Users/me/huge/sub"))
        .filter_map(|r| r.bytes)
        .sum();
    assert_eq!(
        parts, whole,
        "its parts add up to the whole an uninterrupted pass measured"
    );
}

#[test]
fn an_unchanged_second_pass_gives_identical_bytes() {
    // Tempting wrong patch: a second pass that quietly reuses stale rows
    // (so the answer cannot be wrong, and cannot notice a change either),
    // or one whose totals drift on an unchanged tree. It is not faster:
    // there is no event replay for the whole disk yet, and the docs say
    // so; the budget bounds it instead.
    let s = Setup::new();
    ran(&s.pass());
    let before = comparable(&s.rows());
    let again = s.run_at(NOW + 30 * 3_600, false, Duration::from_secs(60), Some(0));
    assert!(ran(&again).complete);
    assert_eq!(comparable(&s.rows()), before);
    let ages: HashSet<u64> = s
        .rows()
        .iter()
        .filter(|r| r.category == Category::Other)
        .map(|r| r.measured_at)
        .collect();
    assert_eq!(
        ages,
        HashSet::from([NOW + 30 * 3_600]),
        "and every row was measured again"
    );
}

#[test]
fn the_pass_is_not_due_until_the_interval_has_passed_and_an_unfinished_one_always_continues() {
    let s = Setup::new();
    ran(&s.pass());
    match s.run_at(NOW + 3_600, false, Duration::from_secs(60), Some(0)) {
        PassOutcome::NotDue(why) => assert!(why.contains("interval 24 h"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        s.run_at(NOW + 25 * 3_600, false, Duration::from_secs(60), Some(0)),
        PassOutcome::Ran(_)
    ));
    // An unfinished cycle continues at the next observe whatever the age.
    let u = Setup::new();
    many_small_folders(&u.fs, 40);
    u.fs.slow_under("/Users/me/d", Duration::from_millis(20));
    assert!(!ran(&u.run_at(NOW, true, Duration::from_millis(100), Some(0))).complete);
    assert!(matches!(
        u.run_at(NOW + 60, false, Duration::from_millis(100), Some(0)),
        PassOutcome::Ran(_)
    ));
}

// ---- adversarial: the disk-full guard, the store marker -----------------------

#[test]
fn the_disk_full_guard_skips_the_pass_and_says_why_in_one_line() {
    // Tempting wrong patch: running the whole-disk pass on a volume that
    // is nearly full, where the ledger write itself could fail.
    let s = Setup::new();
    let (outcome, work) = swamp_core::work_counters::measured(|| {
        s.run_at(NOW, true, Duration::from_secs(60), Some(u64::MAX))
    });
    match outcome {
        PassOutcome::Skipped(line) => {
            assert!(line.contains("disk nearly full"), "{line}");
            assert!(!line.contains('\n'), "one line");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        work.dirs_listed + work.files_statted + work.subprocess_spawns,
        0
    );
    assert_eq!(
        s.probe.calls.load(Ordering::SeqCst),
        0,
        "no system query either"
    );
    let (rows, meta) = read_volume_ledger(s.store.path()).unwrap();
    assert!(rows.is_empty() && meta.is_none(), "nothing written");
}

#[test]
fn a_store_marker_from_another_generation_gets_no_ledger_written() {
    // Tempting wrong patch: adding files to a store a different swamp
    // owns. The ledger's files are new and separate, but a store this
    // build does not understand (a NEWER swamp's, or one an older swamp
    // left that has not been reset) is left exactly as it was.
    for marker in ["99\n", "1\n", "not-a-generation\n"] {
        let s = Setup::new();
        std::fs::write(s.store.path().join("housekeeping.version"), marker).unwrap();
        match s.pass() {
            PassOutcome::Skipped(line) => assert!(line.contains("format marker"), "{line}"),
            other => panic!("{marker:?}: {other:?}"),
        }
        assert!(!s.store.path().join("volume_ledger.parquet").exists());
        assert!(!s.store.path().join("volume_ledger_meta.parquet").exists());
        assert_eq!(
            std::fs::read(s.store.path().join("housekeeping.version")).unwrap(),
            marker.as_bytes()
        );
    }
}

#[test]
fn the_ledger_files_are_new_and_a_store_format_reset_leaves_them_alone() {
    // Tempting wrong patch: putting the ledger into a table the format
    // reset owns (or one v0.7.5 resets), so every reset, and every
    // install of another version, wipes a pass that took minutes. The
    // ledger is a measurement, not derived from another table.
    let s = Setup::new();
    ran(&s.pass());
    let rows = s.store.path().join("volume_ledger.parquet");
    let meta = s.store.path().join("volume_ledger_meta.parquet");
    assert!(rows.is_file() && meta.is_file());
    let before = (std::fs::read(&rows).unwrap(), std::fs::read(&meta).unwrap());
    std::fs::write(s.store.path().join("housekeeping.version"), b"1\n").unwrap();
    let dir = swamp_core::fs_gate::StoreDir::at(s.store.path()).unwrap();
    assert!(dir.reset_incompatible_format().unwrap());
    assert_eq!(
        (std::fs::read(&rows).unwrap(), std::fs::read(&meta).unwrap()),
        before,
        "a reset must not touch the ledger"
    );
    // It still reads, and the next pass continues from it.
    assert!(read_account(s.store.path()).unwrap().is_some());
}

#[test]
fn an_absent_or_unreadable_ledger_is_tolerated_by_the_reading() {
    let s = Setup::new();
    assert!(read_account(s.store.path()).unwrap().is_none());
    let text = render_disk_view(None, NOW);
    assert!(text.contains("not measured yet; run `swamp observe --volume`"));
    // A ledger file that is not Parquet is an error the caller words, not
    // a panic, and not an overwrite.
    std::fs::write(s.store.path().join("volume_ledger_meta.parquet"), b"junk").unwrap();
    assert!(read_account(s.store.path()).is_err());
}

#[test]
fn the_writer_lock_is_free_while_the_pass_walks() {
    // Tempting wrong patch: taking the observation writer lock for the
    // whole pass, which would hold up the TUI's refresh and every other
    // observer for as long as the disk takes to walk. Only the two small
    // ledger writes take it.
    let s = Setup::new();
    let store = s.store.path().to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    s.fs.after_listing("/Applications/Xcode.app", move |_| {
        // Runs on a pass worker, mid-walk.
        let store = store.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let dir = swamp_core::fs_gate::StoreDir::at(&store).unwrap();
            let _held = dir.lock_observation_writes().unwrap();
            let _ = done_tx.send(());
        });
        let got = done_rx.recv_timeout(Duration::from_secs(5)).is_ok();
        let _ = tx.lock().unwrap().send(got);
    });
    ran(&s.pass());
    assert_eq!(
        rx.try_recv().ok(),
        Some(true),
        "another writer could not take the lock while the pass was walking"
    );
}

// ---- adversarial: reading does no work ------------------------------------

#[test]
fn reading_the_ledger_lists_nothing_stats_nothing_and_spawns_nothing() {
    // Tempting wrong patch: `report --view disk` (or the TUI) refreshing a
    // stale ledger, or statting the recorded paths "to be sure they are
    // still there".
    let s = Setup::new();
    ran(&s.pass());
    let (text, work) = swamp_core::work_counters::measured(|| {
        let a = read_account(s.store.path()).unwrap();
        let text = render_disk_view(a.as_ref(), NOW + 3 * 3_600);
        let json = swamp_core::volume_ledger::disk_json(a.as_ref(), NOW + 3 * 3_600, None);
        assert_eq!(json["measured"], true);
        text
    });
    assert_eq!(work.dirs_listed, 0);
    assert_eq!(work.files_statted, 0);
    assert_eq!(work.subprocess_spawns, 0);
    assert!(text.contains("measured 3 h ago"), "{text}");
    // Per-row ages are shown.
    assert!(text.contains("measured 3 h ago)"));
}

#[test]
fn the_report_reads_the_top_five_of_everything_else_largest_first() {
    let s = Setup::new();
    for i in 0..8u64 {
        s.fs.file(&format!("/Users/me/zz{i}/f"), 3_000 + i * 100, 9_000 + i, 1);
    }
    ran(&s.pass());
    let a = read_account(s.store.path()).unwrap().unwrap();
    assert_eq!(a.everything_else.top.len(), 5);
    let sizes: Vec<u64> = a
        .everything_else
        .top
        .iter()
        .map(|r| r.bytes.unwrap())
        .collect();
    assert!(sizes.windows(2).all(|w| w[0] >= w[1]), "{sizes:?}");
    assert_eq!(a.everything_else.top[0].path, "/Users/me/zz7");
    let json = swamp_core::volume_ledger::disk_json(Some(&a), NOW, None);
    assert_eq!(json["everything_else"]["top"].as_array().unwrap().len(), 5);
    for key in [
        "container",
        "accounted",
        "system_volumes",
        "purgeable",
        "snapshots",
        "not_measured",
        "residual",
        "rows",
        "notes",
    ] {
        assert!(json.get(key).is_some(), "disk json lacks {key}");
    }
    assert!(
        json["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r.get("measured_at").is_some() && r.get("exactness").is_some())
    );
}

// ---- adversarial: the system queries -------------------------------------------

fn run_with(probe: FakeProbe) -> (Setup, swamp_core::volume_ledger::Account) {
    let mut s = Setup::new();
    s.probe = probe;
    ran(&s.pass());
    let a = read_account(s.store.path()).unwrap().unwrap();
    (s, a)
}

#[test]
fn diskutil_and_tmutil_failing_in_every_way_become_coverage_notes_never_zero_or_an_error() {
    // Tempting wrong patch: `.unwrap_or_default()` on the query (a
    // system-volumes line of 0 B), or `?` that fails the whole pass when
    // `diskutil` is missing, hangs or answers with junk.
    let failures: Vec<(ProbeError, &str)> = vec![
        (ProbeError::Missing("diskutil"), "was not found"),
        (ProbeError::TimedOut("diskutil"), "did not answer within"),
        (
            ProbeError::Failed("`diskutil` did not run: permission denied".into()),
            "permission denied",
        ),
    ];
    for (err, needle) in failures {
        let probe = FakeProbe {
            apfs: Err(err.clone()),
            info: Err(err.clone()),
            snapshots: Err(match &err {
                ProbeError::Missing(_) => ProbeError::Missing("tmutil"),
                other => other.clone(),
            }),
            calls: AtomicU64::new(0),
        };
        let (_s, a) = run_with(probe);
        assert_eq!(a.system_volumes.bytes, 0);
        assert!(a.system_volumes.volumes.is_empty(), "no invented volume");
        assert_eq!(a.container.data_volume_used, None);
        assert_eq!(
            a.not_measured.estimate_bytes, None,
            "no estimate without the Data volume figure"
        );
        let notes = a.notes.join(" | ");
        assert!(notes.contains(needle), "{needle}: {notes}");
        assert!(a.purgeable.as_ref().unwrap().bytes.is_none());
        assert_eq!(
            a.purgeable.as_ref().unwrap().exactness,
            Exactness::NotMeasured
        );
        assert_eq!(a.snapshots.as_ref().unwrap().bytes, None);
        // The residual takes what the missing line would have held, named.
        assert_eq!(a.residual.name, RESIDUAL_NAME);
    }
}

#[test]
fn garbage_and_partial_property_lists_are_notes_not_numbers() {
    for garbage in [
        "<html>not a plist</html>",
        "",
        "\0\0\0",
        "<plist version=\"1.0\"><array/></plist>",
        "<plist version=\"1.0\"><dict><key>Containers</key><string>x</string></dict></plist>",
    ] {
        let probe = FakeProbe {
            apfs: Ok(garbage.to_string()),
            info: Ok(garbage.to_string()),
            snapshots: Ok("garbage without dots or newlines \u{1}".into()),
            calls: AtomicU64::new(0),
        };
        let (_s, a) = run_with(probe);
        assert!(a.system_volumes.volumes.is_empty(), "{garbage:?}");
        assert!(!a.notes.is_empty(), "{garbage:?}");
        assert_eq!(a.container.data_volume_used, None);
    }
    // A volume without a size is not measured; a missing purgeable key
    // says the answer had none.
    let probe = FakeProbe {
        apfs: Ok(apfs_xml(&[
            ("Macintosh HD", "System", None),
            ("Data", "Data", Some(DATA_BYTES)),
        ])),
        info: Ok(info_xml(None)),
        snapshots: Ok(
            "com.apple.TimeMachine.2026-09-01-010101.local\ncom.apple.os.update-ABC\n".into(),
        ),
        calls: AtomicU64::new(0),
    };
    let (_s, a) = run_with(probe);
    assert_eq!(a.system_volumes.bytes, 0);
    let notes = a.notes.join(" | ");
    assert!(notes.contains("gave no size for this volume"), "{notes}");
    assert!(
        a.purgeable
            .as_ref()
            .and_then(|r| r.note.as_deref())
            .is_some_and(|n| n.contains("did not report purgeable")),
        "the purgeable line carries its own note"
    );
    let snaps = a.snapshots.unwrap();
    assert_eq!(snaps.entries, Some(2));
    assert_eq!(snaps.bytes, None, "tmutil lists names, not sizes");
}

#[test]
fn purgeable_space_is_read_when_present_and_never_added() {
    // Tempting wrong patch: adding purgeable bytes as another part, which
    // counts space that is already inside the folders above twice.
    let mut probe = good_probe();
    probe.info = Ok(info_xml(Some(3_000_000)));
    probe.snapshots = Ok(String::new());
    let (_s, a) = run_with(probe);
    let p = a.purgeable.clone().unwrap();
    assert_eq!(p.bytes, Some(3_000_000));
    let container_used = a.container.used.unwrap() as i64;
    let parts = (a.accounted.bytes
        + a.everything_else.bytes
        + a.system_volumes.bytes
        + a.not_measured.estimate_bytes.unwrap()) as i64;
    assert_eq!(
        a.residual.bytes,
        Some(container_used - parts),
        "purgeable is not one of the parts"
    );
    assert_eq!(a.snapshots.unwrap().bytes, Some(0));
}

#[test]
fn only_the_container_with_the_data_volume_is_the_system_volumes() {
    let c = parse_apfs_list(
        &apfs_xml(&[("Data", "Data", Some(5)), ("VM", "VM", Some(7))]),
        None,
    )
    .unwrap();
    assert_eq!(c.reference, "disk3");
    assert_eq!(c.volumes.len(), 2);
    assert!(parse_apfs_list(&apfs_xml(&[("Backup", "Backup", Some(5))]), None).is_err());
    let info = parse_data_volume_info(&info_xml(Some(9))).unwrap();
    assert_eq!(info.purgeable, Some(9));
    assert_eq!(info.container_reference.as_deref(), Some("disk3"));
    assert_eq!(
        parse_snapshots("Snapshots for volume group containing disk /:\ncom.apple.os.update-X\n"),
        vec!["com.apple.os.update-X"]
    );
}

#[test]
fn the_spawn_allow_list_rejects_every_other_argv_and_counts_no_spawn() {
    // Tempting wrong patch: `Program::Diskutil` allowed with any
    // arguments because it "only reads": `diskutil apfs deleteVolume` and
    // `tmutil deletelocalsnapshots` are one argument away.
    use swamp_core::fs_gate::spawn::{Program, run as spawn_run};
    for (program, args) in [
        (Program::Diskutil, vec!["apfs", "deleteVolume", "disk3s5"]),
        (Program::Diskutil, vec!["eraseDisk", "APFS", "x", "disk9"]),
        (Program::Diskutil, vec!["info", "-plist", "/"]),
        (Program::Diskutil, vec!["list"]),
        (
            Program::Tmutil,
            vec!["deletelocalsnapshots", "2026-01-01-000000"],
        ),
        (Program::Tmutil, vec!["listlocalsnapshots", "/Users"]),
        (Program::Tmutil, vec!["destinationinfo"]),
    ] {
        let (r, work) = swamp_core::work_counters::measured(|| {
            spawn_run(program, args.clone(), Duration::from_secs(5))
        });
        assert!(r.is_err(), "{program:?} {args:?} must be refused");
        assert_eq!(work.subprocess_spawns, 0);
    }
}

// ---- adversarial: real files ---------------------------------------------------

#[test]
fn the_real_walker_never_opens_special_files_never_follows_links_and_counts_hardlinks_once() {
    // Tempting wrong patches: `File::open`/`metadata()` (follows links, and
    // blocks forever on a FIFO with no writer), `read_dir` recursion that
    // follows a symlink out of the tree, and per-name (not per-inode)
    // accounting of hardlinks.
    use std::os::unix::fs::MetadataExt;
    use swamp_core::volume_ledger::pass::{Outcome, RealFs};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("tree");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("huge"), vec![1u8; 4 << 20]).unwrap();
    std::fs::write(root.join("a"), vec![2u8; 8192]).unwrap();
    std::fs::hard_link(root.join("a"), root.join("sub/a-link")).unwrap();
    std::fs::write(root.join("sub/b"), vec![3u8; 4096]).unwrap();
    let sparse = std::fs::File::create(root.join("sparse")).unwrap();
    sparse.set_len(64 << 20).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
    std::os::unix::fs::symlink(outside.join("huge"), root.join("escape-file")).unwrap();
    swamp_core::fs_gate::sys::make_fifo_for_test(&root.join("pipe")).unwrap();
    let _socket = std::os::unix::net::UnixListener::bind(root.join("s")).unwrap();

    let expected: u64 = ["a", "sub/b", "sparse"]
        .iter()
        .map(|p| std::fs::symlink_metadata(root.join(p)).unwrap().blocks() * 512)
        .sum();
    assert!(
        expected < 1 << 20,
        "a sparse file's allocation is not its length ({expected})"
    );

    let (tx, rx) = std::sync::mpsc::channel();
    let r = root.clone();
    std::thread::spawn(move || {
        let out = measure_dir(
            &RealFs,
            &r,
            &HashSet::new(),
            Instant::now() + Duration::from_secs(30),
        );
        let _ = tx.send(out);
    });
    let out = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("the walk blocked (a FIFO was opened?)");
    match out {
        Outcome::Done(m) => {
            assert_eq!(
                m.bytes, expected,
                "hardlink once, sparse by allocation, no link followed"
            );
            assert!(!m.changed);
            assert!(m.unreadable_dirs.is_empty());
            assert!(m.mounts.is_empty());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_real_unreadable_directory_is_not_measured() {
    use std::os::unix::fs::PermissionsExt;
    use swamp_core::volume_ledger::pass::{Outcome, RealFs};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("t");
    let denied = root.join("protected");
    std::fs::create_dir_all(&denied).unwrap();
    std::fs::write(denied.join("secret"), vec![9u8; 100_000]).unwrap();
    std::fs::write(root.join("ok"), b"x").unwrap();
    std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable_anyway = std::fs::read_dir(&denied).is_ok(); // running as root
    let out = measure_dir(
        &RealFs,
        &root,
        &HashSet::new(),
        Instant::now() + Duration::from_secs(10),
    );
    std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o700)).unwrap();
    if readable_anyway {
        return;
    }
    match out {
        Outcome::Done(m) => {
            assert_eq!(m.unreadable_dirs.len(), 1);
            assert!(m.unreadable_dirs[0].0.ends_with("protected"));
        }
        other => panic!("{other:?}"),
    }
}

// ---- accounting: overlaps, gating ----------------------------------------------

fn acc(path: &str, bytes: u64, subset: bool, cat: Category) -> Accounted {
    Accounted {
        path: PathBuf::from(path),
        bytes,
        category: cat,
        subset_of_enclosing: subset,
        measured_at: NOW,
        incomplete: false,
        note: None,
    }
}

#[test]
fn units_nested_in_units_add_up_but_a_view_of_a_unit_is_added_once() {
    // Tempting wrong patches: (a) subtracting every nested unit from its
    // parent's total, when an observation already measured the parent
    // WITHOUT the units under it (`/opt/homebrew` 5.7 GB + the formulae
    // and Android packages under it = the folder's 25 GB, matching `du`);
    // (b) summing an agent tool's sessions on top of its home, which is
    // also a catalog location and already holds them.
    let rows = accounted_rows(
        &[
            acc("/opt/homebrew", 5_000, false, Category::Catalog),
            acc("/opt/homebrew/Cellar/llvm", 1_000, false, Category::Catalog),
            acc("/h/.claude", 1_000, false, Category::Catalog),
            acc("/h/.claude", 400, true, Category::Catalog),
            acc("/h/.claude/projects", 300, true, Category::Catalog),
            acc("/h/.codex", 700, true, Category::Catalog),
        ],
        &[],
    );
    let added: u64 = rows.iter().map(Row::additive).sum();
    assert_eq!(
        added,
        5_000 + 1_000 + 1_000,
        "agent views inside a unit add nothing"
    );
    let view = rows
        .iter()
        .find(|r| r.path == "/h/.claude/projects")
        .unwrap();
    assert_eq!(view.overlap_bytes, 300);
    assert!(view.note.as_deref().unwrap().contains("counted there"));
    let alone = rows.iter().find(|r| r.path == "/h/.codex").unwrap();
    assert_eq!(
        alone.overlap_bytes, 700,
        "an agent view is never added: the walk measures the whole folder it lives in"
    );
}

#[test]
fn a_unit_walked_through_mount_points_gives_back_the_mounted_bytes() {
    let rows = accounted_rows(
        &[acc("/lib/Volumes", 43_000, false, Category::Catalog)],
        &[
            MountView {
                path: PathBuf::from("/lib/Volumes/a"),
                kind: MountKind::OwnStorage,
                used: Some(17_000),
            },
            MountView {
                path: PathBuf::from("/lib/Volumes/b"),
                kind: MountKind::OwnStorage,
                used: Some(8_000),
            },
            MountView {
                path: PathBuf::from("/lib/Volumes/c"),
                kind: MountKind::SameContainer,
                used: None,
            },
            MountView {
                path: PathBuf::from("/elsewhere/d"),
                kind: MountKind::OwnStorage,
                used: Some(99_000),
            },
        ],
    );
    assert_eq!(rows[0].overlap_bytes, 25_000);
    assert_eq!(rows[0].additive(), 18_000);
    assert!(
        rows[0]
            .note
            .as_deref()
            .unwrap()
            .contains("mounted disk images")
    );
}

#[test]
fn the_pass_needs_the_configured_scope_and_the_accounts_own_home() {
    let home = Path::new("/Users/me");
    // Explicit roots replace the scope: never valid, not even forced.
    assert!(allowed(true, true, 24, home, Some(home)).is_err());
    assert!(allowed(true, false, 24, home, Some(home)).is_err());
    // A sandboxed $HOME: only when asked for.
    assert!(allowed(false, false, 24, Path::new("/tmp/fixture"), Some(home)).is_err());
    assert!(allowed(false, true, 24, Path::new("/tmp/fixture"), Some(home)).is_ok());
    // Interval 0 is the off switch for the automatic pass only.
    assert!(allowed(false, false, 0, home, Some(home)).is_err());
    assert!(allowed(false, true, 0, home, Some(home)).is_ok());
    assert!(allowed(false, false, 24, home, Some(home)).is_ok());
    assert!(allowed(false, false, 24, home, None).is_err());
}

#[test]
fn the_ledger_round_trips_and_refuses_a_write_over_a_concurrent_one() {
    // Tempting wrong patch: two `observe` processes each rewriting the
    // ledger from a stale read.
    let s = Setup::new();
    ran(&s.pass());
    let (rows, meta) = read_volume_ledger(s.store.path()).unwrap();
    let meta = meta.unwrap();
    assert!(write_volume_ledger(s.store.path(), &rows, &meta, Some(meta.measured_at)).is_ok());
    assert!(write_volume_ledger(s.store.path(), &rows, &meta, Some(meta.measured_at + 1)).is_err());
    assert!(write_volume_ledger(s.store.path(), &rows, &meta, None).is_err());
}

#[test]
fn the_files_only_row_covers_files_directly_in_an_expanded_folder() {
    let s = Setup::new();
    ran(&s.pass());
    let files = s
        .row(&format!("/Users/me{FILES_SUFFIX}"))
        .expect("files in the home itself");
    assert_eq!(files.bytes, Some(10 * 512));
    assert!(
        account(
            &s.rows(),
            &read_volume_ledger(s.store.path()).unwrap().1.unwrap()
        )
        .rows
        .len()
            > 5
    );
}

#[test]
fn no_verdict_word_appears_in_the_rendered_reading() {
    let s = Setup::new();
    ran(&s.pass());
    let a = read_account(s.store.path()).unwrap().unwrap();
    let text = render_disk_view(Some(&a), NOW).to_lowercase();
    let json = swamp_core::volume_ledger::disk_json(Some(&a), NOW, None)
        .to_string()
        .to_lowercase();
    for word in [
        ["un", "used"].concat(),
        ["st", "ale"].concat(),
        ["obso", "lete"].concat(),
        ["sa", "fe"].concat(),
        ["orph", "an"].concat(),
    ] {
        assert!(!text.contains(&word), "{word} in text");
        assert!(!json.contains(&word), "{word} in json");
    }
    assert!(!text.contains('\u{2014}'), "no em dashes");
}

// ---- review round: hard deadline, locks, mounts, external volumes ---------

#[test]
fn a_path_that_blocks_forever_ends_the_pass_at_the_hard_deadline_and_the_next_run_continues() {
    // Tempting wrong patch: joining the workers (a scoped thread pool), so
    // one `lstat` on a wedged mount hangs the pass, holds its lock and
    // starves every later run. The pass stops waiting at twice the budget,
    // says where it was stuck, releases its lock and moves the cursor on.
    let s = Setup::new();
    s.fs.block_forever_on("/Applications/Xcode.app");
    let summary = attempt("hard deadline", |n| {
        let t = Instant::now();
        let outcome = s.run_at(NOW + n, true, Duration::from_millis(1_000), Some(0));
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "the pass hung: {:?}",
            t.elapsed()
        );
        let summary = ran(&outcome).clone();
        summary
            .notes
            .iter()
            .any(|n| n.contains("stuck at") && n.contains("Xcode.app"))
            .then_some(summary)
    });
    assert!(!summary.complete);
    assert!(summary.line().contains("stuck at"), "logged with the path");
    // Its lock is free again.
    assert!(
        swamp_core::growth::try_lock_volume_pass(s.store.path())
            .unwrap()
            .is_some(),
        "the pass kept its lock"
    );
    let stuck = s
        .row("/Applications/Xcode.app")
        .expect("the stuck folder is a row");
    assert_eq!(stuck.bytes, None);
    assert_eq!(stuck.exactness, Exactness::NotMeasured);
    // The hang clears; the next run resumes from the cursor and finishes.
    s.fs.release.store(true, Ordering::SeqCst);
    let next = s.run_at(NOW + 1, true, Duration::from_secs(30), Some(0));
    assert!(ran(&next).complete);
}

#[test]
fn the_pass_lock_is_its_own_and_a_second_pass_says_so() {
    // Tempting wrong patch: the pass sharing the observation lock, so a
    // stuck pass makes every scheduled observe say "another observation
    // is running".
    let s = Setup::new();
    let held = swamp_core::growth::try_lock_volume_pass(s.store.path())
        .unwrap()
        .expect("free at first");
    match s.pass() {
        PassOutcome::Skipped(line) => assert!(line.contains("another volume pass"), "{line}"),
        other => panic!("{other:?}"),
    }
    // The observation writer lock is not the pass's.
    let dir = swamp_core::fs_gate::StoreDir::at(s.store.path()).unwrap();
    let _obs = dir.lock_observation_writes().unwrap();
    drop(held);
    assert!(
        swamp_core::growth::try_lock_volume_pass(s.store.path())
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_mount_point_is_never_statted_and_nothing_behind_a_remote_one_is_touched() {
    // Tempting wrong patch: `lstat` on every child to compare devices,
    // which is the call that hangs on a stalled network mount, before the
    // mount table is consulted.
    let s = Setup::new();
    s.fs.mkdir("/Users/me/Remote", 5);
    s.fs.file("/Users/me/Remote/deep", 5, 9_991, 1);
    let mut mounts = s.fs.mounts();
    mounts.push(MountView {
        path: PathBuf::from("/Users/me/Remote"),
        kind: MountKind::Remote,
        used: None,
    });
    s.fs.set_mounts(mounts);
    ran(&s.pass());
    for p in [
        "/Users/me/Remote",
        "/Volumes/Share",
        "/Volumes/Recovery",
        "/Library/Developer/CoreSimulator/Volumes/iOS_1",
    ] {
        assert_eq!(s.fs.touched_exactly(p), 0, "{p} was statted or listed");
        assert_eq!(s.fs.touched_under(p), 0, "{p} was entered");
    }
    let row = s.row("/Users/me/Remote").expect("listed, not dropped");
    assert_eq!(row.category, Category::Mount);
    assert_eq!(row.bytes, None);
}

#[test]
fn a_remote_filesystem_type_is_decided_from_the_mount_table_alone() {
    use swamp_core::volume_ledger::pass::is_remote_fs;
    for t in [
        "smbfs",
        "nfs",
        "afpfs",
        "webdav",
        "macfuse",
        "fuse.sshfs",
        "fuse",
        "cifs",
        "9p",
        "autofs",
    ] {
        assert!(is_remote_fs(t), "{t}");
    }
    for t in ["apfs", "hfs", "ext4", "btrfs", "xfs"] {
        assert!(!is_remote_fs(t), "{t}");
    }
}

#[test]
fn a_declared_root_on_another_volume_is_listed_apart_and_never_added() {
    // Tempting wrong patch: adding every accounted root to the internal
    // container's accounted bytes, so a root on an external disk makes the
    // identity fail (or hides a gap) on the internal one.
    let mut s = Setup::new();
    s.accounted.push(Accounted {
        path: PathBuf::from("/Volumes/Backup/src"),
        bytes: 9_000_000,
        category: Category::Declared,
        subset_of_enclosing: false,
        measured_at: NOW,
        incomplete: false,
        note: None,
    });
    let mut mounts = s.fs.mounts();
    mounts.push(MountView {
        path: PathBuf::from("/Volumes/Backup"),
        kind: MountKind::OwnStorage,
        used: Some(5),
    });
    s.fs.set_mounts(mounts);
    ran(&s.pass());
    let a = read_account(s.store.path()).unwrap().unwrap();
    assert_eq!(
        a.accounted.bytes,
        7_777 * 512,
        "the external root is not added"
    );
    assert_eq!(a.external_volumes.len(), 1);
    assert_eq!(a.external_volumes[0].bytes, Some(9_000_000));
    let text = render_disk_view(Some(&a), NOW);
    assert!(
        text.contains("On other volumes (not part of this container"),
        "{text}"
    );
    assert!(text.contains("/Volumes/Backup/src"));
}

#[test]
fn the_json_rows_are_bounded_by_default_and_the_totals_are_not() {
    // Tempting wrong patch: every ledger row in every `report --json`.
    let s = Setup::new();
    ran(&s.pass());
    let a = read_account(s.store.path()).unwrap().unwrap();
    let all = swamp_core::volume_ledger::disk_json(Some(&a), NOW, None);
    let few = swamp_core::volume_ledger::disk_json(Some(&a), NOW, Some(3));
    assert_eq!(few["rows"].as_array().unwrap().len(), 3);
    assert_eq!(few["rows_truncated"], true);
    assert_eq!(few["rows_total"], all["rows_total"]);
    assert_eq!(
        all["rows"].as_array().unwrap().len() as u64,
        all["rows_total"].as_u64().unwrap()
    );
    assert_eq!(few["accounted"], all["accounted"]);
    assert_eq!(
        few["everything_else"]["bytes"],
        all["everything_else"]["bytes"]
    );
    let largest = few["rows"][0]["allocated_bytes"].as_u64().unwrap();
    assert!(
        all["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["allocated_bytes"].as_u64().unwrap_or(0) <= largest)
    );
}

#[test]
fn an_agent_home_no_unit_encloses_is_measured_by_the_walk_and_never_added_twice() {
    // Tempting wrong patch: pruning the agent home from the walk and adding
    // only the agent sessions' bytes, so everything else in that folder is
    // never measured (or the home is added on top of the walk).
    let mut s = Setup::new();
    s.fs.file("/Users/me/.codex/sessions/a", 700, 9_100, 1);
    s.fs.file("/Users/me/.codex/logs/b", 300, 9_101, 1);
    s.accounted.push(Accounted {
        path: PathBuf::from("/Users/me/.codex"),
        bytes: 700 * 512,
        category: Category::Catalog,
        subset_of_enclosing: true,
        measured_at: NOW,
        incomplete: false,
        note: None,
    });
    ran(&s.pass());
    let a = read_account(s.store.path()).unwrap().unwrap();
    assert_eq!(
        a.accounted.bytes,
        7_777 * 512,
        "the agent view adds nothing"
    );
    let home = s
        .rows()
        .into_iter()
        .find(|r| r.path == "/Users/me/.codex" && r.category == Category::Other)
        .expect("measured by the walk");
    assert_eq!(
        home.bytes,
        Some(1_000 * 512),
        "the whole folder, sessions and logs"
    );
}

#[test]
fn a_pass_that_measured_nothing_says_which_locations_are_not_measured_yet() {
    // Tempting wrong patch: an incomplete pass whose gap is silently filed
    // as an estimate. The cursor knows exactly which locations are pending.
    let s = Setup::new();
    many_small_folders(&s.fs, 150);
    s.fs.slow_under("/Users/me/d", Duration::from_millis(30));
    let a = attempt("first run", |n| {
        let first = s.run_at(NOW + n, true, Duration::from_millis(1_000), Some(0));
        assert!(!ran(&first).complete);
        let a = read_account(s.store.path()).unwrap().unwrap();
        (a.not_measured.not_yet_measured > 0).then_some(a)
    });
    assert_eq!(
        a.not_measured.not_yet_measured,
        a.not_measured.not_yet_measured_names.len()
    );
    let text = render_disk_view(Some(&a), NOW);
    assert!(text.contains("Not measured yet this pass"));
    // A pending row is never a size.
    assert!(
        a.rows
            .iter()
            .filter(|r| r.method == "pending")
            .all(|r| r.bytes.is_none())
    );
}

// ======================= verification round (audit/v080-g3b) =======================

fn ver_file_hash(p: &Path) -> Option<Vec<u8>> {
    std::fs::read(p).ok()
}

#[test]
fn ver_one_unreadable_folder_must_not_make_a_99_percent_undercount_reconcile() {
    // Tempting wrong patch (head 98e0ba5): gate the elimination estimate on
    // "anything unreadable or pending" only. Every real Mac has protected
    // folders, so the estimate is always present and absorbs any walk
    // undercount: the "check that can fail" cannot fail from the walk.
    let mut s = Setup::new();
    // The Data volume claims 100x what the fixture holds; the walk misses 99%.
    s.probe = FakeProbe {
        apfs: Ok(apfs_xml(&[
            ("Macintosh HD", "System", Some(SYSTEM_BYTES)),
            ("Preboot", "Preboot", Some(PREBOOT_BYTES)),
            ("Data", "Data", Some(DATA_BYTES * 100)),
        ])),
        ..good_probe()
    };
    s.space = FakeSpace(Some((
        100_000_000_000,
        100_000_000_000 - (SYSTEM_BYTES + PREBOOT_BYTES + DATA_BYTES * 100),
    )));
    ran(&s.pass());
    let a = read_account(s.store.path()).unwrap().unwrap();
    let measured = a.accounted.bytes + a.everything_else.bytes;
    assert!(measured * 50 < DATA_BYTES * 100, "fixture: walk sees < 2%");
    assert!(
        a.not_measured.count > 0,
        "fixture has Mail and Pictures denied"
    );
    assert_ne!(
        a.residual.within_one_percent,
        Some(true),
        "walk measured {measured} of {} Data bytes, 2 folders unreadable, yet residual {:?} is 'within 1%' and residual_flag={}",
        DATA_BYTES * 100,
        a.residual.bytes,
        a.residual.residual_flag
    );
}

#[test]
fn ver_a_detached_worker_that_finishes_later_writes_nothing_and_a_second_pass_can_run() {
    // Tempting wrong patch: the late worker pushing its result into a
    // ledger write, or the pass lock being held by the detached thread.
    let s = Setup::new();
    s.fs.block_forever_on("/Applications/Xcode.app");
    let t = Instant::now();
    ran(&s.run_at(NOW, true, Duration::from_millis(1_000), Some(0)));
    let took = t.elapsed();
    let rows_p = s.store.path().join("volume_ledger.parquet");
    let meta_p = s.store.path().join("volume_ledger_meta.parquet");
    let (r1, m1) = (ver_file_hash(&rows_p), ver_file_hash(&meta_p));
    // A second pass while the first pass's worker is still stuck.
    let second = s.run_at(NOW + 1, true, Duration::from_millis(1_000), Some(0));
    assert!(matches!(second, PassOutcome::Ran(_)), "{second:?}");
    let (r2, m2) = (ver_file_hash(&rows_p), ver_file_hash(&meta_p));
    s.fs.release.store(true, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(ver_file_hash(&rows_p), r2, "late worker rewrote rows");
    assert_eq!(ver_file_hash(&meta_p), m2, "late worker rewrote meta");
    let _ = (r1, m1);
    // The documented bound is 2 budgets; the assertion allows ten times
    // the hard deadline so a loaded shared host cannot make it flaky. A
    // hang would be minutes, not seconds.
    assert!(
        took < Duration::from_secs(20),
        "took {took:?} against a 2 x 1 s hard deadline"
    );
}

#[test]
fn ver_a_stuck_folder_is_measured_by_the_next_run_once_it_answers() {
    // Tempting wrong patch: recording a stuck task as fresh for the cycle,
    // so after the mount recovers the folder stays "not measured" until the
    // next full cycle (24 h), although the summary says "the next run continues".
    let s = Setup::new();
    s.fs.block_forever_on("/Applications/Xcode.app");
    ran(&s.run_at(NOW, true, Duration::from_millis(1_000), Some(0)));
    s.fs.release.store(true, Ordering::SeqCst);
    ran(&s.run_at(NOW + 1, false, Duration::from_secs(30), Some(0)));
    let row = s.row("/Applications/Xcode.app").expect("row");
    assert!(
        row.bytes.is_some(),
        "after the hang cleared, the next run left Downloads as {:?} / {:?}",
        row.method,
        row.note
    );
}

#[test]
fn ver_logical_path_edge_cases() {
    use swamp_core::volume_ledger::pass::logical_path;
    let d = Path::new("/System/Volumes/Data");
    let lp = |p: &str| logical_path(Path::new(p), d);
    assert_eq!(lp("/System/Volumes/Data"), PathBuf::from("/"));
    assert_eq!(
        lp("/System/Volumes/Data/Users/me"),
        PathBuf::from("/Users/me")
    );
    assert_eq!(
        lp("/System/Volumes/DataX/a"),
        PathBuf::from("/System/Volumes/DataX/a")
    );
    assert_eq!(lp("/Users/me/tmp/x"), PathBuf::from("/Users/me/tmp/x"));
    assert_eq!(lp("/tmp"), PathBuf::from("/private/tmp"));
    assert_eq!(
        lp("/var/folders/x"),
        PathBuf::from("/private/var/folders/x")
    );
    assert_eq!(
        lp("/private/var/folders/x"),
        PathBuf::from("/private/var/folders/x")
    );
    assert_eq!(
        lp("/System/Volumes/Data/tmp/a"),
        PathBuf::from("/private/tmp/a")
    );
}

#[test]
fn ver_logical_path_normalizes_case_of_the_data_mount() {
    // Low: APFS is case-insensitive by default; prune matching is exact.
    use swamp_core::volume_ledger::pass::logical_path;
    let lp = |p: &str| logical_path(Path::new(p), Path::new("/System/Volumes/Data"));
    assert_eq!(
        lp("/system/volumes/data/Users/me"),
        PathBuf::from("/Users/me"),
        "case spelling of the data mount is not normalized"
    );
}

#[test]
fn ver_a_negative_residual_is_flagged_and_huge_values_do_not_panic() {
    use swamp_core::growth::VolumeMetaRow;
    let meta = VolumeMetaRow {
        measured_at: NOW,
        cycle_started_at: NOW,
        cycle_complete_at: NOW,
        complete: true,
        budget_secs: 60,
        budget_used_ms: 1,
        statfs_at: NOW,
        container_total: Some(2_000),
        container_used: Some(1_000),
        container_free: Some(1_000),
        data_volume_used: Some(1_000),
    };
    let mk = |p: &str, b: u64| Row {
        path: p.into(),
        category: Category::Other,
        bytes: Some(b),
        overlap_bytes: 0,
        entries: Some(1),
        unreadable: 0,
        measured_at: NOW,
        method: "walk".into(),
        exactness: Exactness::Exact,
        note: None,
    };
    // Clones counted per file: 1011 measured on 1000 used.
    let a = account(&[mk("/a", 1_011)], &meta);
    assert_eq!(a.residual.bytes, Some(-11));
    assert!(a.residual.residual_flag, "a -1.1% residual must flag");
    let a = account(&[mk("/a", 1_010)], &meta);
    assert!(!a.residual.residual_flag, "exactly 1% is within");
    let a = account(&[mk("/a", u64::MAX), mk("/b", u64::MAX)], &meta);
    assert!(a.residual.residual_flag);
    assert_eq!(a.residual.bytes, Some(i64::MIN));
}

// ---- second review round: spot audit, stuck retry, mounts, planning ----------

fn big_folder(fs: &FakeFs) {
    for i in 0..4u64 {
        fs.file(&format!("/Users/me/big/f{i}"), 25_000, 7_700 + i, 1);
    }
}

#[test]
fn a_walker_that_undercounts_one_folder_by_30_percent_is_flagged_by_the_spot_audit() {
    // Tempting wrong patch: counting the parts adding up to the container
    // as evidence about the walk. Every part is a leftover of the others,
    // so only an independent re-measurement can disagree with the walk.
    let s = Setup::new();
    big_folder(&s.fs);
    s.fs.undercount_first_look_under("/Users/me/big");
    ran(&s.pass());
    let a = read_account(s.store.path()).unwrap().unwrap();
    assert!(a.audit.audit_flag, "{:?}", a.audit);
    let f = a
        .audit
        .folders
        .iter()
        .find(|f| f.path == "/Users/me/big")
        .expect("the largest folder is audited");
    assert_eq!(f.ledger_bytes, 70_000 * 512);
    assert_eq!(f.audited_bytes, 100_000 * 512);
    assert!(f.outside_tolerance && f.percent > 25.0);
    let text = render_disk_view(Some(&a), NOW);
    assert!(
        text.contains("FLAG: walk spot audit disagrees on /Users/me/big: ledger"),
        "{text}"
    );
    let json = swamp_core::volume_ledger::disk_json(Some(&a), NOW, None);
    assert_eq!(json["audit"]["audit_flag"], true);
}

#[test]
fn an_honest_walker_on_a_churning_tree_stays_inside_the_audit_tolerance() {
    let s = Setup::new();
    big_folder(&s.fs);
    // A file lands after the walk listed the folder, before the audit does.
    s.fs.after_listing("/Users/me/big", |fs| {
        fs.file("/Users/me/big/late", 3, 9_999, 1)
    });
    ran(&s.pass());
    let a = read_account(s.store.path()).unwrap().unwrap();
    assert!(!a.audit.audit_flag, "{:?}", a.audit);
    assert!(!a.audit.folders.is_empty() && a.audit.folders.len() <= 5);
    assert!(a.audit.folders.iter().any(|f| f.path == "/Users/me/big"));
    let text = render_disk_view(Some(&a), NOW);
    assert!(
        text.contains("Walk spot-audited:") && text.contains("max difference"),
        "{text}"
    );
}

#[test]
fn the_audit_never_takes_a_folder_with_unreadable_parts_and_is_deterministic_by_day() {
    let s = Setup::new();
    ran(&s.pass());
    let a = read_account(s.store.path()).unwrap().unwrap();
    for f in &a.audit.folders {
        assert!(
            ![
                "/Users/me/Library",
                "/Users/me/Pictures",
                "/Users/me/Library/Mail"
            ]
            .contains(&f.path.as_str()),
            "an unreadable folder was audited: {}",
            f.path
        );
    }
    // The same day picks the same folders.
    let first: Vec<String> = a.audit.folders.iter().map(|f| f.path.clone()).collect();
    let again = s.run_at(NOW + 60, true, Duration::from_secs(60), Some(0));
    assert!(ran(&again).complete);
    let b = read_account(s.store.path()).unwrap().unwrap();
    let second: Vec<String> = b.audit.folders.iter().map(|f| f.path.clone()).collect();
    assert_eq!(first, second);
}

#[test]
fn a_spent_budget_skips_the_audit_with_a_note_and_the_run_still_ends_on_time() {
    let s = Setup::new();
    // The first task of the walk (so it is always reached), and the largest
    // folder (so it is always the first one audited).
    for i in 0..4u64 {
        s.fs.file(&format!("/Applications/AAA.app/f{i}"), 25_000, 7_800 + i, 1);
    }
    // Once the walk has listed the folder, listing it again blocks for far
    // longer than the budget: the audit can never finish.
    let fired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = fired.clone();
    s.fs.after_listing("/Applications/AAA.app", move |fs| {
        flag.store(true, Ordering::SeqCst);
        fs.slow_under("/Applications/AAA.app", Duration::from_secs(30))
    });
    let a = attempt("spent budget", |n| {
        let t = Instant::now();
        ran(&s.run_at(NOW + n, true, Duration::from_millis(1_000), Some(0)));
        // Ten times the budget: the run ends by the deadline, not by the 30 s.
        assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
        if !fired.load(Ordering::SeqCst) {
            return None; // the walk never reached the folder: host stall
        }
        let a = read_account(s.store.path()).unwrap().unwrap();
        assert!(!a.audit.audit_flag);
        assert!(
            a.audit
                .skipped
                .as_deref()
                .is_some_and(|n| n.contains("budget")),
            "{:?}",
            a.audit
        );
        Some(a)
    });
    assert!(render_disk_view(Some(&a), NOW).contains("Walk spot audit: not run"));
}

#[test]
fn a_folder_stuck_three_runs_in_a_row_is_skipped_until_the_next_cycle_and_logged_once() {
    // Tempting wrong patch: retrying a wedged folder forever (every run
    // burns its budget on it) or counting it as measured after one try.
    let s = Setup::new();
    s.fs.block_forever_on("/Applications/Xcode.app");
    let mut logged = 0;
    for i in 0..3u64 {
        let want = if i < 2 { "stuck" } else { "skipped" };
        let row = attempt("stuck run", |n| {
            let out = s.run_at(
                NOW + i * 10 + n,
                true,
                Duration::from_millis(1_000),
                Some(0),
            );
            let notes = ran(&out).notes.clone();
            let row = s.row("/Applications/Xcode.app")?;
            if row.method != want {
                return None; // the run never reached the folder
            }
            logged += notes
                .iter()
                .filter(|n| n.contains("skipped until the next cycle"))
                .count();
            Some(row)
        });
        assert_eq!(row.method, want, "run {i}");
        if i == 2 {
            assert!(row.note.as_deref().unwrap().contains("skipped until 20"));
        }
    }
    assert_eq!(logged, 1, "said once, at the third run");
    // Fresh now: the cycle can finish without it.
    attempt("the cycle finishes without the skipped folder", |n| {
        ran(&s.run_at(NOW + 100 + n, true, Duration::from_secs(30), Some(0)))
            .complete
            .then_some(())
    });
    s.fs.release.store(true, Ordering::SeqCst);
}

#[test]
fn an_automount_spelled_through_the_data_volume_is_never_touched() {
    // Tempting wrong patch: filtering the mount table by path before
    // classifying it, so `auto_home` at /System/Volumes/Data/home is dropped
    // and the walk statts it.
    use swamp_core::volume_ledger::pass::keep_mount;
    let d = Path::new("/System/Volumes/Data");
    assert!(keep_mount(&d.join("home"), "autofs", true));
    assert!(keep_mount(&d.join("home"), "autofs", false));
    assert!(keep_mount(Path::new("/Volumes/x"), "smbfs", false));
    assert!(
        keep_mount(&d.join("Users/me/ext"), "apfs", true),
        "kept, mapped to its normal path"
    );
    assert!(!keep_mount(d, "apfs", true));
    assert!(!keep_mount(Path::new("/System/Volumes/VM"), "apfs", true));
    assert!(!keep_mount(Path::new("/"), "apfs", true));
    let s = Setup::new();
    s.fs.mkdir("/Users/me/auto", 5);
    let mut mounts = s.fs.mounts();
    mounts.push(MountView {
        path: PathBuf::from("/data/Users/me/auto"), // spelled through the data volume
        kind: MountKind::Remote,
        used: None,
    });
    s.fs.set_mounts(mounts);
    ran(&s.pass());
    assert_eq!(s.fs.touched_exactly("/Users/me/auto"), 0);
}

#[test]
fn planning_has_a_time_limit_and_an_incomplete_plan_is_not_a_complete_cycle() {
    // Tempting wrong patch: unbounded planning listings (a slow directory
    // holds the run past its budget) or a truncated plan called complete.
    let s = Setup::new();
    s.fs.slow_under("/Users/me", Duration::from_millis(1_500));
    let out = s.run_at(NOW, true, Duration::from_millis(1_000), Some(0));
    let summary = ran(&out);
    assert!(!summary.complete);
    assert!(
        summary.notes.iter().any(|n| n.contains("planning stopped")),
        "{:?}",
        summary.notes
    );
}

#[test]
fn a_ledger_that_cannot_be_read_because_of_io_is_left_alone_and_only_damage_moves_it_aside() {
    // Tempting wrong patch: any read error means "corrupt": a transient
    // EIO or EMFILE would throw away a good ledger.
    use std::os::unix::fs::PermissionsExt;
    let s = Setup::new();
    ran(&s.pass());
    let rows = s.store.path().join("volume_ledger.parquet");
    std::fs::set_permissions(&rows, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable_anyway = std::fs::read(&rows).is_ok(); // running as root
    let out = s.run_at(NOW + 5, true, Duration::from_secs(30), Some(0));
    std::fs::set_permissions(&rows, std::fs::Permissions::from_mode(0o600)).unwrap();
    if readable_anyway {
        return;
    }
    match out {
        PassOutcome::Skipped(line) => assert!(line.contains("nothing was moved"), "{line}"),
        other => panic!("{other:?}"),
    }
    assert!(rows.exists());
    assert!(!std::fs::read_dir(s.store.path()).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".corrupt-")
    }));
}

#[test]
fn quarantined_ledgers_older_than_a_week_are_removed_at_the_start_of_a_pass() {
    let s = Setup::new();
    let old = s.store.path().join(format!(
        "volume_ledger.parquet.corrupt-{}",
        NOW - 8 * 86_400
    ));
    let recent = s.store.path().join(format!(
        "volume_ledger_meta.parquet.corrupt-{}",
        NOW - 86_400
    ));
    let other = s.store.path().join("notes.corrupt-1");
    for f in [&old, &recent, &other] {
        std::fs::write(f, b"x").unwrap();
    }
    ran(&s.pass());
    assert!(!old.exists(), "older than seven days");
    assert!(recent.exists());
    assert!(
        other.exists(),
        "only the ledger's own quarantine names are touched"
    );
}

#[test]
fn the_audit_picks_five_distinct_folders_when_there_are_enough_and_rotates_by_day() {
    // Tempting wrong patch: a stride that shares a factor with the pool
    // size, which cycles through three folders and audits four.
    let s = Setup::new();
    for i in 0..12u64 {
        s.fs.file(
            &format!("/Users/me/q{i:02}/f"),
            2_000 + i * 10,
            6_000 + i,
            1,
        );
    }
    ran(&s.pass());
    let a = read_account(s.store.path()).unwrap().unwrap();
    let day1: Vec<String> = a.audit.folders.iter().map(|f| f.path.clone()).collect();
    assert_eq!(day1.len(), 5, "{day1:?}");
    let distinct: HashSet<&String> = day1.iter().collect();
    assert_eq!(distinct.len(), 5);
    ran(&s.run_at(NOW + 86_400, true, Duration::from_secs(60), Some(0)));
    let b = read_account(s.store.path()).unwrap().unwrap();
    let day2: Vec<String> = b.audit.folders.iter().map(|f| f.path.clone()).collect();
    assert_eq!(day2.len(), 5);
    assert_eq!(day1[0], day2[0], "the largest is always audited");
    assert_ne!(day1, day2, "the rest rotate with the day");
}
