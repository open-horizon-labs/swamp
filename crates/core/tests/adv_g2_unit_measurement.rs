//! Adversarial audit of v0.8.0 G2 (PR #189): last-used facts, depth-2
//! drilldown, standalone Cargo target directories, structured overlap
//! fields. Every fixture is a disposable tempdir with tool homes injected
//! through each detector's own environment variable; nothing here reads
//! the developer's real tool homes or store.
//!
//! Each test names the wrong-but-plausible implementation it is aimed at.
//! A test that fails on the PR head is a finding and is kept failing.

use std::collections::HashMap;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use swamp_core::drilldown::{ChildKind, ChildMeasure, UnitChild, children_of, rows_total};
use swamp_core::external::{ExternalUnit, discover_and_measure};
use swamp_core::last_used::LastUsedSource;
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::report::{DirRollup, UnownedReason};
use swamp_core::scope::{EffectiveScope, ScanConfig, resolve_effective_scope};

/// 2026-07-08 UTC.
const OLD: i64 = 1_783_468_800;
/// 2026-09-19 UTC.
const NEWER: i64 = 1_789_776_000;

fn set_times(path: &Path, atime: i64, mtime: i64) {
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    let times = [
        libc::timespec {
            tv_sec: atime as _,
            tv_nsec: 0,
        },
        libc::timespec {
            tv_sec: mtime as _,
            tv_nsec: 0,
        },
    ];
    // SAFETY: NUL-terminated path and two timespecs.
    let rc = unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            c.as_ptr(),
            times.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    assert_eq!(rc, 0, "utimensat {}", path.display());
}

fn write(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![7u8; bytes]).unwrap();
}

fn scope_for(detector: &str, vars: &[(&str, &Path)], home: &Path) -> EffectiveScope {
    let env_vars: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.display().to_string()))
        .collect();
    let env = Environment::fixture(home.to_path_buf(), env_vars, Platform::MacOS);
    let registry = Registry::with_builtins();
    let cfg = ScanConfig {
        defaults: false,
        include: Vec::new(),
        exclude: Vec::new(),
        disabled_detectors: Vec::new(),
        enabled_detectors: vec![detector.to_string()],
    };
    resolve_effective_scope(&env, &cfg, &[], &registry, 1)
}

fn measure(scope: &EffectiveScope, store: &Path) -> Vec<ExternalUnit> {
    discover_and_measure(
        scope,
        Some(store),
        true,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap()
}

/// Runs `f` on a thread and fails (instead of hanging the suite) when it
/// does not return in `secs`: a FIFO opened for reading blocks forever.
fn within<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(secs))
        .expect("did not finish in time: something opened a FIFO or walked without bound")
}

struct Rustup {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    rustup: PathBuf,
    scope: EffectiveScope,
    store: tempfile::TempDir,
}

fn rustup_fixture() -> Rustup {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let rustup = home.join("rustup-home");
    fs::create_dir_all(rustup.join("toolchains")).unwrap();
    let scope = scope_for("rustup", &[("RUSTUP_HOME", &rustup)], &home);
    Rustup {
        _tmp: tmp,
        home,
        rustup,
        scope,
        store: tempfile::tempdir().unwrap(),
    }
}

fn toolchains_of(units: &[ExternalUnit]) -> ExternalUnit {
    units
        .iter()
        .find(|u| u.path.ends_with("toolchains"))
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "no toolchains unit: {:?}",
                units.iter().map(|u| &u.path).collect::<Vec<_>>()
            )
        })
}

impl Rustup {
    fn toolchains(&self) -> ExternalUnit {
        toolchains_of(&measure(&self.scope, self.store.path()))
    }
}

// ---------------------------------------------------------------------
// Last used: key files
// ---------------------------------------------------------------------

#[test]
fn adv_a_bin_that_is_a_symlink_to_a_fresh_directory_is_never_followed() {
    // Wrong patch: `metadata()` (follows) on the `bin` entry, so a
    // toolchain whose `bin` points at a freshly used directory elsewhere
    // inherits that directory's key files' access time.
    let fx = rustup_fixture();
    let fresh = fx.home.join("fresh/bin");
    write(&fresh.join("tool"), 100);
    set_times(&fresh.join("tool"), NEWER, NEWER);
    let tc = fx.rustup.join("toolchains/linked");
    write(&tc.join("lib/x"), 1_000);
    std::os::unix::fs::symlink(&fresh, tc.join("bin")).unwrap();
    let unit = fx.toolchains();
    assert_eq!(unit.last_used.at, None, "{:?}", unit.last_used);
}

#[test]
fn adv_a_bin_with_only_directories_is_no_record() {
    // Wrong patch: a directory's own atime standing in when `bin` has no
    // regular file.
    let fx = rustup_fixture();
    let tc = fx.rustup.join("toolchains/dirs-only");
    fs::create_dir_all(tc.join("bin/sub")).unwrap();
    write(&tc.join("bin/sub/deeper"), 10);
    set_times(&tc.join("bin/sub"), NEWER, NEWER);
    set_times(&tc.join("bin"), NEWER, NEWER);
    let unit = fx.toolchains();
    assert_eq!(unit.last_used.at, None, "{:?}", unit.last_used);
    assert_eq!(unit.last_used.source, LastUsedSource::None);
}

#[test]
fn adv_a_fifo_and_a_socket_in_bin_are_never_opened_or_counted() {
    // Wrong patch: opening entries of `bin` (a read on a FIFO blocks) or
    // counting a non-regular file's atime.
    let fx = rustup_fixture();
    let bin = fx.rustup.join("toolchains/odd/bin");
    fs::create_dir_all(&bin).unwrap();
    let fifo = std::ffi::CString::new(bin.join("pipe").as_os_str().as_bytes()).unwrap();
    // SAFETY: NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);
    let _sock = std::os::unix::net::UnixListener::bind(bin.join("sock")).unwrap();
    set_times(&bin.join("pipe"), NEWER, NEWER);
    let scope = fx.scope.clone();
    let store = fx.store.path().to_path_buf();
    let unit = within(30, move || toolchains_of(&measure(&scope, &store)));
    assert_eq!(unit.last_used.at, None, "{:?}", unit.last_used);
}

#[test]
fn adv_a_huge_bin_is_bounded_and_never_a_sampled_newest() {
    // Wrong patch: an unbounded probe of every entry of every `bin`, or
    // reporting the newest of the first N entries as the unit's value.
    let fx = rustup_fixture();
    let bin = fx.rustup.join("toolchains/huge/bin");
    fs::create_dir_all(&bin).unwrap();
    for i in 0..10_000 {
        let p = bin.join(format!("f{i:05}"));
        fs::write(&p, b"").unwrap();
        set_times(&p, OLD, OLD);
    }
    // The newest one sorts last, beyond the shallow-list cap.
    set_times(&bin.join("f09999"), NEWER, NEWER);
    let started = Instant::now();
    let (units, work) = swamp_core::work_counters::measured(|| measure(&fx.scope, fx.store.path()));
    let unit = toolchains_of(&units);
    assert!(started.elapsed() < Duration::from_secs(60));
    assert!(
        unit.last_used.at.is_none() || unit.last_used.at == Some(NEWER as u64),
        "a sample's newest was reported as the unit's: {:?}",
        unit.last_used
    );
    // Bounded: the walk itself stats each file once; the probe adds at
    // most one listing cap more.
    assert!(work.files_statted < 40_000, "{work:?}");
}

// ---------------------------------------------------------------------
// Last used: Cargo's tracker
// ---------------------------------------------------------------------

fn cargo_fixture() -> (tempfile::TempDir, PathBuf, EffectiveScope) {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let cargo = home.join("cargo-home");
    write(&cargo.join("registry/src/idx/a/lib.rs"), 4_000);
    write(&cargo.join("registry/cache/idx/a.crate"), 4_000);
    let scope = scope_for("cargo-home", &[("CARGO_HOME", &cargo)], &home);
    (tmp, cargo, scope)
}

fn tracker(cargo: &Path, wal: bool, ts: i64) -> rusqlite::Connection {
    let db = rusqlite::Connection::open(cargo.join(".global-cache")).unwrap();
    if wal {
        let mode: String = db
            .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }
    for table in ["registry_crate", "registry_src", "git_db", "git_checkout"] {
        db.execute(
            &format!("CREATE TABLE {table} (name TEXT NOT NULL, timestamp INTEGER NOT NULL)"),
            [],
        )
        .unwrap();
    }
    db.execute(
        "INSERT INTO registry_src (name, timestamp) VALUES ('x', ?1)",
        [ts],
    )
    .unwrap();
    db
}

fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn src_unit(units: &[ExternalUnit]) -> &ExternalUnit {
    units
        .iter()
        .find(|u| u.path.ends_with("registry/src"))
        .expect("registry/src unit")
}

#[test]
fn adv_a_wal_tracker_with_a_writer_mid_transaction_reads_the_committed_value_and_leaves_no_sidecar()
{
    // Wrong patch: opening read-write (creating -wal/-shm), or reading the
    // writer's uncommitted row.
    let (_tmp, cargo, scope) = cargo_fixture();
    let writer = tracker(&cargo, true, 1_788_000_000);
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    writer
        .execute(
            "INSERT INTO registry_src (name, timestamp) VALUES ('y', 1788999999)",
            [],
        )
        .unwrap();
    let before = listing(&cargo);
    let store = tempfile::tempdir().unwrap();
    let units = measure(&scope, store.path());
    assert_eq!(listing(&cargo), before, "reading changed the cargo home");
    let got = src_unit(&units).last_used.clone();
    assert!(
        got.at == Some(1_788_000_000) || got.at.is_none(),
        "an uncommitted row was read: {got:?}"
    );
    writer.execute_batch("ROLLBACK").unwrap();
    drop(writer);
}

#[test]
fn adv_a_wal_tracker_with_no_sidecars_is_read_without_creating_any() {
    // Wrong patch: a read-only open that still lets SQLite create the
    // `-shm`/`-wal` sidecars beside Cargo's database (the module doc
    // promises nothing is written). Cargo's own tracker closes cleanly, so
    // this is the ordinary at-rest state of a WAL-mode tracker.
    let (_tmp, cargo, scope) = cargo_fixture();
    drop(tracker(&cargo, true, 1_788_000_000));
    let before = listing(&cargo);
    assert!(
        !before
            .iter()
            .any(|n| n.ends_with("-wal") || n.ends_with("-shm")),
        "{before:?}"
    );
    let store = tempfile::tempdir().unwrap();
    let units = measure(&scope, store.path());
    let after = listing(&cargo);
    assert_eq!(after, before, "reading created files beside the tracker");
    // Report what the read gave (either is honest; a wrong date is not).
    let got = src_unit(&units).last_used.clone();
    assert!(got.at == Some(1_788_000_000) || got.at.is_none(), "{got:?}");
}

#[test]
fn adv_a_wal_tracker_in_a_read_only_cargo_home_is_no_record_or_the_value_never_a_crash() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, cargo, scope) = cargo_fixture();
    drop(tracker(&cargo, true, 1_788_000_000));
    let before = listing(&cargo);
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o555)).unwrap();
    let store = tempfile::tempdir().unwrap();
    let units = measure(&scope, store.path());
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(listing(&cargo), before);
    let got = src_unit(&units).last_used.clone();
    assert!(got.at == Some(1_788_000_000) || got.at.is_none(), "{got:?}");
}

#[test]
fn adv_an_empty_or_text_typed_tracker_is_no_record() {
    // Wrong patch: `unwrap_or(0)` turning an unreadable value into 1970,
    // or parsing a text timestamp partially.
    let (_tmp, cargo, scope) = cargo_fixture();
    fs::write(cargo.join(".global-cache"), b"").unwrap();
    let store = tempfile::tempdir().unwrap();
    assert_eq!(src_unit(&measure(&scope, store.path())).last_used.at, None);
    fs::remove_file(cargo.join(".global-cache")).unwrap();
    let db = rusqlite::Connection::open(cargo.join(".global-cache")).unwrap();
    db.execute_batch(
        "CREATE TABLE registry_src (name TEXT, timestamp TEXT);
         INSERT INTO registry_src VALUES ('x', '2026-09-06T00:00:00Z');",
    )
    .unwrap();
    drop(db);
    assert_eq!(src_unit(&measure(&scope, store.path())).last_used.at, None);
}

#[test]
fn adv_a_tool_native_record_in_the_future_is_not_shown_as_a_fact() {
    // FINDING candidate. Wrong patch: trusting any positive tool-native
    // value. A tracker written in milliseconds (or by a machine with a
    // bad clock) says the unit was last used in the year 58,000; the row
    // then states that as a fact with the tool's name on it.
    let (_tmp, cargo, scope) = cargo_fixture();
    drop(tracker(&cargo, false, 1_788_000_000_000));
    let store = tempfile::tempdir().unwrap();
    let units = measure(&scope, store.path());
    let unit = src_unit(&units);
    let now = unit.observed_at;
    let text = unit.last_used.describe(now);
    assert!(
        unit.last_used.at.is_none_or(|at| at <= now + 86_400),
        "a last-used date in the future is stated as a fact: {text} ({:?})",
        unit.last_used
    );
}

// ---------------------------------------------------------------------
// Xcode info.plist
// ---------------------------------------------------------------------

fn plutil_available() -> bool {
    Path::new("/usr/bin/plutil").exists()
}

fn xml_plist(body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n{body}\n</dict>\n</plist>\n"
    )
}

#[test]
fn adv_derived_data_plist_binary_xml_corrupt_empty_and_wrong_type() {
    use swamp_core::external_associations::read_plist_facts;
    if !plutil_available() {
        eprintln!("no plutil on this host");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let body = "\t<key>LastAccessedDate</key>\n\t<date>2026-09-06T15:39:21Z</date>\n\t<key>WorkspacePath</key>\n\t<string>/nowhere/at/all.xcodeproj</string>";
    let xml = tmp.path().join("xml.plist");
    fs::write(&xml, xml_plist(body)).unwrap();
    let facts = read_plist_facts(&xml).unwrap();
    assert_eq!(facts.last_accessed, Some(1_788_709_161));
    assert_eq!(
        facts.workspace_path.as_deref(),
        Some("/nowhere/at/all.xcodeproj")
    );

    // Binary plist: Xcode's real on-disk format.
    let bin = tmp.path().join("bin.plist");
    fs::copy(&xml, &bin).unwrap();
    let status = std::process::Command::new("/usr/bin/plutil")
        .args(["-convert", "binary1"])
        .arg(&bin)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(&fs::read(&bin).unwrap()[..6], b"bplist");
    assert_eq!(read_plist_facts(&bin).unwrap(), facts, "binary == xml");

    // Corrupt and empty: an error or nothing, never a date.
    for (name, bytes) in [
        ("corrupt.plist", b"bplist00\xff\xff\xff".to_vec()),
        ("empty.plist", Vec::new()),
    ] {
        let p = tmp.path().join(name);
        fs::write(&p, bytes).unwrap();
        match read_plist_facts(&p) {
            Err(_) => {}
            Ok(f) => assert_eq!(f.last_accessed, None, "{name}"),
        }
    }
    // Wrong type: an integer or a string under the key is no record.
    for body in [
        "\t<key>LastAccessedDate</key>\n\t<integer>1788709161</integer>",
        "\t<key>LastAccessedDate</key>\n\t<string>2026-09-06T15:39:21Z</string>",
    ] {
        let p = tmp.path().join("typed.plist");
        fs::write(&p, xml_plist(body)).unwrap();
        assert_eq!(read_plist_facts(&p).unwrap().last_accessed, None, "{body}");
    }
}

#[test]
fn adv_a_derived_data_date_in_the_future_is_not_kept_as_a_fact() {
    // FINDING candidate, same as the Cargo one: Xcode's record copied
    // from a machine with a wrong clock says 2099.
    use swamp_core::external_associations::parse_last_accessed_from_plist_xml;
    let at = parse_last_accessed_from_plist_xml(&xml_plist(
        "\t<key>LastAccessedDate</key>\n\t<date>2099-01-01T00:00:00Z</date>",
    ));
    let now = swamp_core::entities::now();
    let shown = swamp_core::last_used::resolve(at.map(|t| ("xcode-derived-data", t)), None);
    assert!(
        shown.at.is_none_or(|t| t <= now + 86_400),
        "a 2099 last-used is stated as a fact: {}",
        shown.describe(now)
    );
}

// ---------------------------------------------------------------------
// Drilldown
// ---------------------------------------------------------------------

/// Deterministic pseudo-random numbers (no extra dependency).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[test]
fn adv_random_trees_with_hardlinks_between_children_sum_exactly_to_the_unit() {
    // Wrong patch: a remainder computed from the per-directory rollups
    // (which count a hardlinked file under each parent) with no
    // adjustment row, so the rows exceed the unit's total.
    for seed in 1..=12u64 {
        let fx = rustup_fixture();
        let mut rng = Lcg(seed);
        let tcs = 2 + rng.below(20);
        let mut files: Vec<PathBuf> = Vec::new();
        for t in 0..tcs {
            let depth = 1 + rng.below(3);
            let mut dir = fx.rustup.join(format!("toolchains/t{t:02}"));
            for d in 0..depth {
                dir = dir.join(format!("d{d}"));
            }
            for f in 0..(1 + rng.below(4)) {
                let p = dir.join(format!("f{f}"));
                write(&p, (1 + rng.below(40)) as usize * 1_024);
                files.push(p);
            }
        }
        // Loose files directly inside the unit.
        for f in 0..rng.below(3) {
            write(&fx.rustup.join(format!("toolchains/loose{f}")), 3_000);
        }
        // Hardlinks between children.
        for h in 0..rng.below(6) {
            let src = files[rng.below(files.len() as u64) as usize].clone();
            let t = rng.below(tcs);
            let dst = fx.rustup.join(format!("toolchains/t{t:02}/link{h}"));
            if !dst.exists() {
                fs::hard_link(&src, &dst).unwrap();
            }
        }
        let unit = fx.toolchains();
        assert!(!unit.children.is_empty(), "seed {seed}");
        assert_eq!(
            rows_total(&unit.children),
            unit.bytes as i64,
            "seed {seed}: rows {:?} vs total {}",
            unit.children,
            unit.bytes
        );
        for c in &unit.children {
            if c.kind == ChildKind::Entry && c.measure == ChildMeasure::Complete {
                assert!(c.bytes.is_some_and(|b| b >= 0), "seed {seed}: {c:?}");
            }
        }
    }
}

#[test]
fn adv_unicode_and_newline_child_names_are_rows_exactly_as_on_disk() {
    let fx = rustup_fixture();
    for (name, size) in [("é-tool", 30_000usize), ("a\nb", 20_000), ("日本", 10_000)] {
        write(&fx.rustup.join("toolchains").join(name).join("f"), size);
    }
    let unit = fx.toolchains();
    for name in ["é-tool", "a\nb", "日本"] {
        assert!(
            unit.children.iter().any(|c| c.name == name),
            "{name:?} missing: {:?}",
            unit.children.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
    }
    assert_eq!(rows_total(&unit.children), unit.bytes as i64);
}

#[test]
fn adv_two_non_utf8_child_names_stay_two_rows_and_still_add_up() {
    // Wrong patch: keying children by a lossy name, so `x\xff` and `x\xfe`
    // (both `x\u{FFFD}`) merge, double or drop.
    use std::os::unix::ffi::OsStringExt;
    let fx = rustup_fixture();
    let tc = fx.rustup.join("toolchains");
    for (raw, size) in [(b"x\xff".to_vec(), 40_000usize), (b"x\xfe".to_vec(), 8_000)] {
        let dir = tc.join(std::ffi::OsString::from_vec(raw));
        if let Err(e) = fs::create_dir_all(&dir) {
            // APFS refuses names that are not UTF-8 (EILSEQ); Linux CI
            // exercises this.
            eprintln!("this filesystem refuses non-UTF-8 names: {e}");
            return;
        }
        fs::write(dir.join("f"), vec![1u8; size]).unwrap();
    }
    let scope = fx.scope.clone();
    let store = fx.store.path().to_path_buf();
    let unit = within(60, move || toolchains_of(&measure(&scope, &store)));
    let entries: Vec<&UnitChild> = unit
        .children
        .iter()
        .filter(|c| c.kind == ChildKind::Entry)
        .collect();
    assert_eq!(entries.len(), 2, "{:?}", unit.children);
    assert_eq!(rows_total(&unit.children), unit.bytes as i64);
    let mut sizes: Vec<i64> = entries.iter().filter_map(|c| c.bytes).collect();
    sizes.sort();
    assert!(
        sizes[1] >= 40_000,
        "the big one kept its own size: {sizes:?}"
    );
}

#[test]
fn adv_symlink_fifo_and_socket_children_are_never_opened_or_followed() {
    // Wrong patch: following a child symlink into a big tree elsewhere,
    // or opening a FIFO child.
    let fx = rustup_fixture();
    let tc = fx.rustup.join("toolchains");
    write(&tc.join("real/f"), 10_000);
    let big = fx.home.join("elsewhere-big");
    write(&big.join("huge"), 2_000_000);
    std::os::unix::fs::symlink(&big, tc.join("link-to-big")).unwrap();
    let fifo = std::ffi::CString::new(tc.join("pipe").as_os_str().as_bytes()).unwrap();
    // SAFETY: NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);
    let _sock = std::os::unix::net::UnixListener::bind(tc.join("sock")).unwrap();
    let scope = fx.scope.clone();
    let store = fx.store.path().to_path_buf();
    let unit = within(30, move || toolchains_of(&measure(&scope, &store)));
    assert!(
        unit.bytes < 1_000_000,
        "a symlink was followed: {}",
        unit.bytes
    );
    assert!(
        !unit
            .children
            .iter()
            .any(|c| c.name == "link-to-big" && c.bytes.unwrap_or(0) > 0),
        "{:?}",
        unit.children
    );
    assert_eq!(rows_total(&unit.children), unit.bytes as i64);
}

fn rollup(rel: &str, own: u64, entries: u32) -> DirRollup {
    DirRollup {
        worktree_id: "x".into(),
        track: None,
        rel_path: rel.into(),
        parent_rel_path: if rel.is_empty() {
            None
        } else {
            Some(String::new())
        },
        allocated_total: own,
        own_allocated: own,
        file_count: 1,
        entry_count: entries,
        symlink_count: 0,
        mod_time_min: 1,
        complete: true,
        growth_bytes: None,
    }
}

#[test]
fn adv_top_n_ties_are_ordered_by_name_whatever_the_input_order() {
    // Wrong patch: an unstable sort on bytes alone, so equal-size folders
    // swap between passes and the stored drilldown churns.
    let unit = Path::new("/u");
    let names = ["m", "b", "z", "a", "q", "c"];
    let mut first: Option<Vec<String>> = None;
    for rot in 0..names.len() {
        let mut dirs = vec![rollup("", 0, names.len() as u32)];
        for i in 0..names.len() {
            dirs.push(rollup(names[(i + rot) % names.len()], 100, 1));
        }
        let kids = children_of(unit, dirs, 600, 3);
        let shown: Vec<String> = kids
            .iter()
            .filter(|c| c.kind == ChildKind::Entry)
            .map(|c| c.name.clone())
            .collect();
        assert_eq!(shown, vec!["a", "b", "c"], "rotation {rot}");
        if let Some(f) = &first {
            assert_eq!(&shown, f);
        }
        first = Some(shown);
        assert_eq!(rows_total(&kids), 600);
    }
}

#[test]
fn adv_a_child_deleted_mid_walk_never_breaks_the_identity() {
    // Race-dependent (fails about 1 run in 6 on APFS); the deterministic
    // form is `adv_an_incomplete_pass_shows_a_drilldown_...` below.
    // Wrong patch: a remainder computed from a separate listing taken at
    // another moment. Here children churn between two measurements; each
    // measurement must still add up to its own total.
    let fx = rustup_fixture();
    for i in 0..30 {
        write(&fx.rustup.join(format!("toolchains/c{i:02}/f")), 5_000);
    }
    let root = fx.rustup.join("toolchains");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let churn = {
        let stop = stop.clone();
        let root = root.clone();
        std::thread::spawn(move || {
            let mut i = 0u64;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let d = root.join(format!("c{:02}", i % 30));
                let _ = fs::remove_dir_all(&d);
                let _ = fs::create_dir_all(&d);
                let _ = fs::write(d.join("f"), vec![0u8; 5_000]);
                i += 1;
            }
        })
    };
    for _ in 0..5 {
        let unit = fx.toolchains();
        assert_eq!(
            rows_total(&unit.children),
            unit.bytes as i64,
            "note {:?} rows {:?}",
            unit.note,
            unit.children
        );
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    churn.join().unwrap();
}

// ---------------------------------------------------------------------
// Standalone Cargo target directories
// ---------------------------------------------------------------------

const SIG: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

struct Quiet;
impl swamp_core::fs_events::FsEventsSource for Quiet {
    fn replay(
        &self,
        _: &swamp_core::fs_events::FsEventsRequest,
    ) -> swamp_core::fs_events::FsEventsPlan {
        swamp_core::fs_events::FsEventsPlan::from_live(Vec::new(), 1000, None)
    }
}

fn report(root: &Path, store: &Path) -> swamp_core::Report {
    swamp_core::report::report_full_mode_scoped(
        root,
        None,
        false,
        Some(store),
        Some("1h"),
        true,
        false,
        false,
        true,
        &Quiet,
        &[],
        false,
    )
    .unwrap()
}

fn target_with_tag(dir: &Path, tag: &[u8]) {
    write(&dir.join("debug/deps/libfoo.rlib"), 200_000);
    fs::write(dir.join("CACHEDIR.TAG"), tag).unwrap();
    fs::write(dir.join(".rustc_info.json"), b"{}").unwrap();
}

fn standalone_rows(r: &swamp_core::Report) -> Vec<PathBuf> {
    r.unowned
        .iter()
        .filter(|u| u.reason == UnownedReason::StandaloneCargoTarget)
        .map(|u| PathBuf::from(&u.path_or_object))
        .collect()
}

#[test]
fn adv_cachedir_tag_variants() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let cases: Vec<(&str, Vec<u8>, bool)> = vec![
        ("lf", [SIG, b"\n# cargo\n"].concat(), true),
        ("crlf", [SIG, b"\r\n# cargo\r\n"].concat(), true),
        ("trailing-space", [SIG, b"   \n"].concat(), true),
        (
            "only-first-line",
            [SIG, b"\ngarbage\x00\xff"].concat(),
            true,
        ),
        (
            "bom",
            [b"\xef\xbb\xbf".as_slice(), SIG, b"\n"].concat(),
            false,
        ),
        (
            "wrong-sig",
            b"Signature: 8a477f597d28d172789f06886806bc56\n".to_vec(),
            false,
        ),
        ("truncated-sig", SIG[..30].to_vec(), false),
    ];
    for (name, tag, _) in &cases {
        target_with_tag(&root.join(name), tag);
    }
    let store = tempfile::tempdir().unwrap();
    let r = report(&root, store.path());
    let found = standalone_rows(&r);
    for (name, _, want) in &cases {
        assert_eq!(
            found.contains(&root.join(name)),
            *want,
            "{name}: standalone rows {found:?}"
        );
    }
}

#[test]
fn adv_rustc_info_json_empty_huge_or_a_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let tag = [SIG, b"\n"].concat();
    let empty = root.join("empty-info");
    target_with_tag(&empty, &tag);
    fs::write(empty.join(".rustc_info.json"), b"").unwrap();
    let huge = root.join("huge-info");
    target_with_tag(&huge, &tag);
    fs::File::create(huge.join(".rustc_info.json"))
        .unwrap()
        .set_len(512 * 1024 * 1024)
        .unwrap();
    let dir = root.join("dir-info");
    target_with_tag(&dir, &tag);
    fs::remove_file(dir.join(".rustc_info.json")).unwrap();
    fs::create_dir(dir.join(".rustc_info.json")).unwrap();
    let store = tempfile::tempdir().unwrap();
    let (r, work) = swamp_core::work_counters::measured(|| report(&root, store.path()));
    let found = standalone_rows(&r);
    assert!(found.contains(&empty), "{found:?}");
    assert!(found.contains(&huge), "{found:?}");
    assert!(!found.contains(&dir), "{found:?}");
    // Recognition never reads the json (only its lstat).
    assert!(work.header_bytes_read < 4_096, "{work:?}");
}

#[test]
fn adv_a_target_nested_inside_an_unowned_directory_is_one_row_counted_once() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let outer = root.join("scratch");
    write(&outer.join("notes/big.bin"), 300_000);
    let inner = outer.join("deep/target-x");
    target_with_tag(&inner, &[SIG, b"\n"].concat());
    let store = tempfile::tempdir().unwrap();
    let r = report(&root, store.path());
    let found = standalone_rows(&r);
    assert_eq!(found, vec![inner.clone()], "unowned: {:?}", r.unowned);
    let unowned: u64 = r.unowned.iter().map(|u| u.bytes).sum();
    assert_eq!(r.reconciliation.walked_total, unowned, "{:?}", r.unowned);
}

#[test]
fn adv_a_symlinked_target_dir_is_never_a_second_row() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let real = root.join("real-target");
    target_with_tag(&real, &[SIG, b"\n"].concat());
    std::os::unix::fs::symlink(&real, root.join("link-target")).unwrap();
    let store = tempfile::tempdir().unwrap();
    let r = report(&root, store.path());
    assert_eq!(standalone_rows(&r), vec![real]);
}

#[test]
fn adv_the_declared_root_itself_is_never_planned_for_trash_as_a_standalone_target() {
    // FINDING candidate. Wrong patch: recognizing the signature on any
    // folded directory, including the root the human declared, so the
    // whole declared root becomes one "standalone Cargo target" plan unit.
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap().join("tgt");
    target_with_tag(&root, &[SIG, b"\n"].concat());
    let store = tempfile::tempdir().unwrap();
    let r = report(&root, store.path());
    let planned = swamp_core::actions::propose(&r, None, std::slice::from_ref(&root), "test");
    assert!(
        planned.is_err(),
        "the declared root was planned as one Trash unit: {:?}",
        planned.map(|u| u.iter().map(|x| x.path().to_path_buf()).collect::<Vec<_>>())
    );
}

#[test]
fn adv_a_target_inside_a_linked_git_worktree_is_the_projects_not_standalone() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let main = root.join("main");
    let git = |args: &[&str], cwd: &Path| {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .status()
                .unwrap()
                .success()
        )
    };
    fs::create_dir_all(&main).unwrap();
    git(&["init", "-q"], &main);
    fs::write(main.join(".gitignore"), b"target/\n").unwrap();
    git(&["add", "."], &main);
    git(&["commit", "-qm", "x"], &main);
    git(&["worktree", "add", "-q", "../wt"], &main);
    let t = root.join("wt/target");
    target_with_tag(&t, &[SIG, b"\n"].concat());
    let store = tempfile::tempdir().unwrap();
    let r = report(&root, store.path());
    assert!(standalone_rows(&r).is_empty(), "{:?}", r.unowned);
}

#[test]
fn adv_a_target_held_open_by_another_process_reads_as_in_use_and_protection_refuses() {
    use swamp_core::evidence::{FactKind, FactStatus, FactValue};
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let target = root.join("ct");
    target_with_tag(&target, &[SIG, b"\n"].concat());
    let store = tempfile::tempdir().unwrap();
    let r = report(&root, store.path());

    // A stand-in for `cargo build` writing into CARGO_TARGET_DIR.
    let held = target.join("debug/deps/libfoo.rlib");
    let mut child = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg("exec 3>>\"$1\"; exec sleep 30")
        .arg("sh")
        .arg(&held)
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let units = swamp_core::actions::propose(&r, None, std::slice::from_ref(&target), "test");
    let protected = swamp_core::actions::propose_checking_protection(
        &r,
        None,
        std::slice::from_ref(&target),
        "test",
        std::slice::from_ref(&root),
    );
    let _ = child.kill();
    let _ = child.wait();
    let units = units.unwrap();
    let uses: Vec<_> = units[0]
        .evidence()
        .iter()
        .filter(|e| e.kind == FactKind::CurrentUse)
        .collect();
    assert!(
        uses.iter()
            .any(|e| matches!(e.status, FactStatus::Known(FactValue::Bool(true)))),
        "another process's open handle was not seen: {uses:?}"
    );
    assert!(
        protected.is_err(),
        "a target under a protected path was planned"
    );
}

// ---------------------------------------------------------------------
// Words
// ---------------------------------------------------------------------

#[test]
fn adv_every_new_user_facing_string_has_no_verdict_word_and_no_em_dash() {
    let verdicts = [
        ["un", "used"].concat(),
        ["sta", "le"].concat(),
        ["obso", "lete"].concat(),
        ["orph", "an"].concat(),
        ["sa", "fe"].concat(),
    ];
    let now = 1_790_000_000;
    let mut texts: Vec<String> = Vec::new();
    for lu in [
        swamp_core::last_used::resolve(None, Some(1_783_468_800)),
        swamp_core::last_used::resolve(Some(("xcode-derived-data", 1_783_468_800)), None),
        swamp_core::last_used::resolve(Some(("cargo-global-cache", 1_700_000_000)), None),
        swamp_core::last_used::resolve(None, None),
    ] {
        texts.push(lu.describe(now));
    }
    let base = UnitChild {
        kind: ChildKind::Entry,
        name: "n".into(),
        bytes: Some(10),
        measure: ChildMeasure::Complete,
        mtime_max: 1_783_468_800,
        entries: 3,
        not_measured: 2,
        last_used: Default::default(),
    };
    for (kind, measure, bytes) in [
        (ChildKind::Entry, ChildMeasure::Complete, Some(10)),
        (ChildKind::Entry, ChildMeasure::Partial, Some(10)),
        (ChildKind::Entry, ChildMeasure::NotMeasured, None),
        (ChildKind::Remainder, ChildMeasure::Complete, Some(10)),
        (ChildKind::Adjustment, ChildMeasure::Complete, Some(-10)),
    ] {
        let c = UnitChild {
            kind,
            measure,
            bytes,
            ..base.clone()
        };
        texts.push(swamp_core::render::describe_unit_child(&c, now));
        texts.push(swamp_core::render::unit_child_size(&c));
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    target_with_tag(&root.join("t"), &[SIG, b"\n"].concat());
    let store = tempfile::tempdir().unwrap();
    let r = report(&root, store.path());
    for u in &r.unowned {
        texts.extend(u.note.clone());
    }
    let units = swamp_core::actions::propose(&r, None, &[root.join("t")], "test").unwrap();
    texts.extend(units[0].warnings().iter().cloned());
    for text in &texts {
        let lower = text.to_lowercase();
        assert!(!text.contains('\u{2014}'), "em dash in {text:?}");
        for v in &verdicts {
            assert!(!lower.contains(v.as_str()), "{v} in {text:?}");
        }
    }
}

#[test]
fn adv_an_incomplete_pass_shows_a_drilldown_that_adds_up_to_the_total_it_shows() {
    // FINDING. Wrong patch (the PR's): an incomplete pass keeps the last
    // complete total on the unit ("last complete measurement shown") but
    // attaches *this* pass's partial drilldown, so the rows no longer add
    // up to the number beside them. Deterministic form of the mid-walk
    // deletion race above: a child becomes unreadable between passes.
    use std::os::unix::fs::PermissionsExt;
    let fx = rustup_fixture();
    for i in 0..4 {
        write(&fx.rustup.join(format!("toolchains/c{i}/f")), 50_000);
    }
    let first = fx.toolchains();
    assert_eq!(rows_total(&first.children), first.bytes as i64);
    let locked = fx.rustup.join("toolchains/c2");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let second = fx.toolchains();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        rows_total(&second.children),
        second.bytes as i64,
        "note {:?}: rows {:?}",
        second.note,
        second
            .children
            .iter()
            .map(|c| (c.name.clone(), c.kind, c.bytes))
            .collect::<Vec<_>>()
    );
}

#[test]
fn adv_the_default_homebrew_devtools_formula_units_read_their_key_file_access_time() {
    // FINDING. Wrong patch (the PR's): the key-file declaration sits on
    // the `homebrew` detector's `Cellar` location only, while the detector
    // that is on by default (#174, `homebrew-devtools`) publishes one unit
    // per formula (`Cellar/<formula>`) and declares nothing. On the
    // maintainer's machine every `/opt/homebrew/Cellar/<formula>` unit
    // (llvm@20, zig, dotnet@9, cmake, ...) says `no record` although each
    // has `<version>/bin/*`.
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let prefix = home.join("brew");
    let clang = prefix.join("Cellar/llvm/20.1/bin/clang");
    write(&clang, 8_000);
    set_times(&clang, OLD, NEWER);
    let scope = scope_for("homebrew-devtools", &[("HOMEBREW_PREFIX", &prefix)], &home);
    let store = tempfile::tempdir().unwrap();
    let units = measure(&scope, store.path());
    let unit = units
        .iter()
        .find(|u| u.path.ends_with("Cellar/llvm"))
        .unwrap_or_else(|| {
            panic!(
                "no per-formula unit: {:?}",
                units
                    .iter()
                    .map(|u| (&u.detector_id, &u.path))
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(
        unit.last_used.at,
        Some(OLD as u64),
        "{} ({}) has bin/clang but says {:?}",
        unit.path.display(),
        unit.detector_id,
        unit.last_used
    );
}

#[test]
fn adv_registering_and_unregistering_a_worktree_inside_a_unit_is_not_growth_and_rows_still_add_up()
{
    // Wrong patches: (1) a drilldown built from the whole walk while the
    // unit's bytes exclude the worktree, so the rows exceed the total;
    // (2) recording the coverage change as storage growth
    // (`coverage-changes-are-not-storage-changes`); (3) an overlap larger
    // than what the unit actually contained. FINDING, pre-existing: the
    // same fixture on release/v0.8.0 (e54312a) already records
    // growth_bytes = Some(-200704) when the worktree is registered.
    use swamp_core::external::{NestedWorktree, discover_and_measure_with_worktrees};
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let rustup = home.join("rustup-home");
    write(&rustup.join("toolchains/a/bin/rustc"), 10_000);
    write(&rustup.join("toolchains/b/lib/x"), 30_000);
    let inner = rustup.join("toolchains/a/wt");
    write(&inner.join("target/blob"), 200_000);
    let scope = scope_for("rustup", &[("RUSTUP_HOME", &rustup)], &home);
    let store = tempfile::tempdir().unwrap();
    let with = |wts: &[NestedWorktree]| {
        toolchains_of(
            &discover_and_measure_with_worktrees(
                &scope,
                wts,
                Some(store.path()),
                true,
                1_000,
                30,
                3600,
                &swamp_core::fs_events::EventCoverage::untrusted(),
            )
            .unwrap(),
        )
    };
    let whole = with(&[]);
    assert_eq!(rows_total(&whole.children), whole.bytes as i64);
    let registered = with(&[NestedWorktree {
        path: inner.clone(),
        reported_bytes: 200_704,
    }]);
    assert_eq!(
        rows_total(&registered.children),
        registered.bytes as i64,
        "rows {:?} vs total {}",
        registered.children,
        registered.bytes
    );
    assert!(registered.bytes_counted_elsewhere <= whole.bytes);
    assert_eq!(
        registered.bytes + registered.bytes_counted_elsewhere,
        whole.bytes,
        "the parts no longer make the whole"
    );
    assert!(
        registered.growth_bytes.is_none_or(|g| g == 0),
        "a coverage change was recorded as growth: {:?}",
        registered.growth_bytes
    );
    let unregistered = with(&[]);
    assert_eq!(unregistered.bytes, whole.bytes);
    assert_eq!(unregistered.bytes_counted_elsewhere, 0);
    assert!(
        unregistered.growth_bytes.is_none_or(|g| g == 0),
        "phantom growth after unregistering: {:?}",
        unregistered.growth_bytes
    );
    assert_eq!(
        rows_total(&unregistered.children),
        unregistered.bytes as i64
    );
}
