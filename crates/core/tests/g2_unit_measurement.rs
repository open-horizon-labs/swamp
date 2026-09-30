//! v0.8.0 G2: last-used facts, depth-2 drilldown and the store change that
//! carries them (#176, #178, #185, #186). Disposable `tempfile` fixtures
//! throughout, with every tool home injected through the detector's own
//! environment variable; nothing here reads the developer's real
//! `~/.rustup` or `~/.cargo`.
//!
//! Each test names the tempting wrong patch it fails.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use swamp_core::drilldown::{ChildKind, ChildMeasure, rows_total};
use swamp_core::external::{ExternalUnit, discover_and_measure};
use swamp_core::last_used::LastUsedSource;
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::report::{ObservationParts, observe_scope, report_scope_from_store};
use swamp_core::scope::{EffectiveScope, ScanConfig, resolve_effective_scope};

/// 2026-07-08 UTC: an access time long before the fixture's modification.
const OLD: i64 = 1_783_468_800;
/// 2026-09-19 UTC.
const NEWER: i64 = 1_789_776_000;

fn set_times(path: &Path, atime: i64, mtime: i64) {
    use std::os::unix::ffi::OsStrExt;
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
    // SAFETY: `c` is a NUL-terminated path and `times` is two timespecs.
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

fn atime_of(path: &Path) -> i64 {
    use std::os::unix::fs::MetadataExt;
    fs::symlink_metadata(path).unwrap().atime()
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

fn unit_ending<'a>(units: &'a [ExternalUnit], suffix: &str) -> &'a ExternalUnit {
    units
        .iter()
        .find(|u| u.path.ends_with(suffix))
        .unwrap_or_else(|| {
            panic!(
                "no unit ending {suffix}: {:?}",
                units.iter().map(|u| &u.path).collect::<Vec<_>>()
            )
        })
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
    let scope = scope_for("rustup", &[("RUSTUP_HOME", &rustup)], &home);
    Rustup {
        _tmp: tmp,
        home,
        rustup,
        scope,
        store: tempfile::tempdir().unwrap(),
    }
}

impl Rustup {
    fn units(&self) -> Vec<ExternalUnit> {
        measure(&self.scope, self.store.path())
    }
    fn toolchains(&self) -> ExternalUnit {
        unit_ending(&self.units(), "toolchains").clone()
    }
}

#[test]
fn last_used_is_the_key_file_access_time_never_a_modification_time() {
    // Tempting wrong patch: `last_used = mtime_max`, the number the walk
    // already has. Here the file was modified long after it was last read.
    let fx = rustup_fixture();
    let tc = fx.rustup.join("toolchains/stable-x");
    write(&tc.join("bin/rustc"), 10_000);
    write(&tc.join("lib/libstd.rlib"), 10_000);
    set_times(&tc.join("bin/rustc"), OLD, NEWER);
    // A non-key file read more recently must not count either.
    set_times(&tc.join("lib/libstd.rlib"), NEWER + 5_000, NEWER + 5_000);
    let unit = fx.toolchains();
    assert_eq!(unit.last_used.at, Some(OLD as u64));
    assert_eq!(unit.last_used.source, LastUsedSource::FileAtime);
    assert!(unit.mtime_max as i64 >= NEWER, "the modification is real");
    assert_ne!(unit.last_used.at, Some(unit.mtime_max));
    // The depth-2 row for that toolchain says the same, by its own name.
    let child = unit
        .children
        .iter()
        .find(|c| c.name == "stable-x")
        .expect("the toolchain is a drilldown row");
    assert_eq!(child.last_used.at, Some(OLD as u64));
}

#[test]
fn a_unit_with_no_key_file_shows_no_record_not_a_date_from_its_modification_time() {
    // Tempting wrong patch: falling back to mtime when there is no
    // access time to read.
    let fx = rustup_fixture();
    write(
        &fx.rustup.join("toolchains/docs-only/share/doc/index.html"),
        4_000,
    );
    let unit = fx.toolchains();
    assert!(unit.mtime_max > 0, "it has a modification time");
    assert_eq!(unit.last_used.at, None);
    assert_eq!(unit.last_used.source, LastUsedSource::None);
    assert!(
        unit.last_used
            .describe(unit.observed_at)
            .ends_with("no record")
    );
}

#[test]
fn a_directorys_own_access_time_is_never_the_last_used_time() {
    // Tempting wrong patch: stat the `bin` directory (or the toolchain
    // directory) and report its atime. A directory listing moves it.
    let fx = rustup_fixture();
    let tc = fx.rustup.join("toolchains/stable-x");
    write(&tc.join("bin/rustc"), 10_000);
    set_times(&tc.join("bin/rustc"), OLD, OLD);
    set_times(&tc.join("bin"), NEWER, NEWER);
    set_times(&tc, NEWER, NEWER);
    set_times(&fx.rustup.join("toolchains"), NEWER, NEWER);
    assert_eq!(fx.toolchains().last_used.at, Some(OLD as u64));
    // And the other way: a recently read file under an old directory.
    set_times(&tc.join("bin/rustc"), NEWER, OLD);
    set_times(&tc.join("bin"), OLD, OLD);
    set_times(&tc, OLD, OLD);
    assert_eq!(fx.toolchains().last_used.at, Some(NEWER as u64));
}

#[test]
fn reading_a_key_file_moves_last_used_and_listing_a_directory_does_not() {
    // The #176 acceptance test. Tempting wrong patch: a scan that opens
    // files (or a directory atime), so the two cases are indistinguishable.
    let fx = rustup_fixture();
    let tc = fx.rustup.join("toolchains/stable-x");
    let rustc = tc.join("bin/rustc");
    write(&rustc, 10_000);
    // Modified after it was last read, so a read is a real access on any
    // mount that keeps access times (relatime and APFS both update then).
    set_times(&rustc, OLD, NEWER);
    assert_eq!(fx.toolchains().last_used.at, Some(OLD as u64));

    for _ in fs::read_dir(tc.join("bin")).unwrap() {}
    for _ in fs::read_dir(fx.rustup.join("toolchains")).unwrap() {}
    assert_eq!(
        fx.toolchains().last_used.at,
        Some(OLD as u64),
        "a directory listing is not a use"
    );
    assert_eq!(atime_of(&rustc), OLD);

    let _ = fs::read(&rustc).unwrap();
    if atime_of(&rustc) == OLD {
        eprintln!("this mount does not update access times on read; nothing more to prove here");
        return;
    }
    let after = fx.toolchains().last_used.at.unwrap();
    assert!(after as i64 > OLD, "a file read is a use: {after}");
    assert_eq!(after as i64, atime_of(&rustc));
}

#[test]
fn measuring_never_opens_a_key_file() {
    // Tempting wrong patch: reading a file's contents (or `--version`)
    // to learn its time. A read on an old-atime file would move the very
    // timestamp being measured, and would show up as header bytes.
    let fx = rustup_fixture();
    let tc = fx.rustup.join("toolchains/stable-x");
    write(&tc.join("bin/rustc"), 50_000);
    write(&tc.join("bin/cargo"), 50_000);
    set_times(&tc.join("bin/rustc"), OLD, NEWER);
    set_times(&tc.join("bin/cargo"), OLD + 1, NEWER);
    let (units, work) = swamp_core::work_counters::measured(|| fx.units());
    assert_eq!(
        unit_ending(&units, "toolchains").last_used.at,
        Some(OLD as u64 + 1)
    );
    assert_eq!(work.header_bytes_read, 0);
    assert_eq!(work.subprocess_spawns, 0);
    assert_eq!(atime_of(&tc.join("bin/rustc")), OLD, "not opened");
    assert_eq!(atime_of(&tc.join("bin/cargo")), OLD + 1, "not opened");
}

#[test]
fn a_symlink_in_bin_is_never_a_key_file() {
    // Tempting wrong patch: lstat every entry of `bin`, so a symlink to a
    // freshly run binary elsewhere (Homebrew's, or a rustup proxy) makes
    // an idle toolchain look used.
    let fx = rustup_fixture();
    let tc = fx.rustup.join("toolchains/stable-x");
    write(&tc.join("bin/rustc"), 10_000);
    set_times(&tc.join("bin/rustc"), OLD, NEWER);
    let fresh = fx.home.join("fresh-binary");
    write(&fresh, 100);
    std::os::unix::fs::symlink(&fresh, tc.join("bin/alias")).unwrap();
    assert_eq!(fx.toolchains().last_used.at, Some(OLD as u64));
}

/// One declared layout per kind: mise `installs/<tool>/<version>/bin`,
/// pyenv `versions/<v>/bin`, Homebrew `Cellar/<formula>/<v>/bin`, Android
/// `cmdline-tools/<v>/bin`, ESP-IDF `tools/<tool>/<v>/<tool>/bin`.
#[test]
fn every_declared_layout_reads_its_own_key_file_access_time() {
    let cases: [(&str, &str, &str, &str); 5] = [
        (
            "mise",
            "MISE_DATA_DIR",
            "installs",
            "installs/node/22.1.0/bin/node",
        ),
        (
            "pyenv",
            "PYENV_ROOT",
            "versions",
            "versions/3.12.9/bin/python",
        ),
        (
            "homebrew",
            "HOMEBREW_PREFIX",
            "Cellar",
            "Cellar/llvm/20.1/bin/clang",
        ),
        (
            "android",
            "ANDROID_HOME",
            "cmdline-tools",
            "cmdline-tools/latest/bin/sdkmanager",
        ),
        (
            "espressif",
            "IDF_TOOLS_PATH",
            "tools",
            "tools/xtensa-esp-elf/esp-14/xtensa-esp-elf/bin/gcc",
        ),
    ];
    for (detector, var, unit_dir, key_file) in cases {
        let tmp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(tmp.path()).unwrap();
        let base = home.join("tool-home");
        write(&base.join(key_file), 8_000);
        // A non-key file in the same unit, read more recently.
        write(&base.join(unit_dir).join("README"), 1_000);
        set_times(&base.join(key_file), OLD, NEWER);
        set_times(&base.join(unit_dir).join("README"), NEWER + 9, NEWER + 9);
        let scope = scope_for(detector, &[(var, &base)], &home);
        let store = tempfile::tempdir().unwrap();
        let units = measure(&scope, store.path());
        let unit = unit_ending(&units, unit_dir);
        assert_eq!(
            unit.last_used.at,
            Some(OLD as u64),
            "{detector}: {:?}",
            unit.last_used
        );
        assert_eq!(
            unit.last_used.source,
            LastUsedSource::FileAtime,
            "{detector}"
        );
    }
}

fn cargo_tracker(home: &Path, rows: &[(&str, i64)]) {
    let db = rusqlite::Connection::open(home.join(".global-cache")).unwrap();
    for table in ["registry_crate", "registry_src", "git_db", "git_checkout"] {
        db.execute(
            &format!("CREATE TABLE {table} (name TEXT NOT NULL, timestamp INTEGER NOT NULL)"),
            [],
        )
        .unwrap();
    }
    for (table, ts) in rows {
        db.execute(
            &format!("INSERT INTO {table} (name, timestamp) VALUES ('x', ?1)"),
            [ts],
        )
        .unwrap();
    }
}

#[test]
fn cargo_units_use_their_own_tracker_table_and_a_missing_tracker_is_no_record() {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let cargo = home.join("cargo-home");
    for rel in [
        "registry/cache/idx/a.crate",
        "registry/src/idx/a/lib.rs",
        "git/db/repo/HEAD",
        "git/checkouts/repo/abc/lib.rs",
    ] {
        write(&cargo.join(rel), 4_000);
    }
    let scope = scope_for("cargo-home", &[("CARGO_HOME", &cargo)], &home);
    let store = tempfile::tempdir().unwrap();

    // No tracker at all: no record. Tempting wrong patch: the folded
    // modification time, or any access time of the extracted sources.
    let units = measure(&scope, store.path());
    for suffix in ["registry/src", "registry/cache", "git/db", "git/checkouts"] {
        assert_eq!(unit_ending(&units, suffix).last_used.at, None, "{suffix}");
    }

    cargo_tracker(
        &cargo,
        &[
            ("registry_crate", 1_788_000_000),
            ("registry_src", 1_788_100_000),
            ("git_db", 1_788_200_000),
            ("git_checkout", 1_788_300_000),
        ],
    );
    let units = measure(&scope, store.path());
    for (suffix, want) in [
        ("registry/cache", 1_788_000_000u64),
        ("registry/src", 1_788_100_000),
        ("git/db", 1_788_200_000),
        ("git/checkouts", 1_788_300_000),
    ] {
        let unit = unit_ending(&units, suffix);
        assert_eq!(unit.last_used.at, Some(want), "{suffix}");
        assert_eq!(
            unit.last_used.source,
            LastUsedSource::ToolNative("cargo-global-cache".to_string())
        );
    }

    // A tracker that is not SQLite: no record, no crash, no date.
    fs::write(cargo.join(".global-cache"), b"not a database").unwrap();
    let units = measure(&scope, store.path());
    assert_eq!(unit_ending(&units, "registry/src").last_used.at, None);
}

#[test]
fn a_drilldown_adds_up_to_the_unit_and_a_folder_that_cannot_be_read_is_not_measured() {
    // Tempting wrong patches: a remainder taken from the unit's stored
    // total instead of the walk's, and an unreadable folder shown as 0B.
    use std::os::unix::fs::PermissionsExt;
    let fx = rustup_fixture();
    for (i, size) in [40_000usize, 30_000, 20_000, 10_000].iter().enumerate() {
        write(
            &fx.rustup.join(format!("toolchains/tc-{i}/lib/blob")),
            *size,
        );
    }
    write(&fx.rustup.join("toolchains/loose-file"), 5_000);
    let locked = fx.rustup.join("toolchains/locked");
    write(&locked.join("lib/blob"), 10_000);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let unit = fx.toolchains();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

    let entries: Vec<_> = unit
        .children
        .iter()
        .filter(|c| c.kind == ChildKind::Entry)
        .collect();
    let not_measured = entries.iter().find(|c| c.name == "locked");
    let not_measured = not_measured.expect("the unreadable folder is listed, not dropped");
    assert_eq!(not_measured.measure, ChildMeasure::NotMeasured);
    assert_eq!(not_measured.bytes, None, "not measured is not 0");
    assert_eq!(rows_total(&unit.children), unit.bytes as i64);
    // Largest first.
    let sizes: Vec<i64> = entries.iter().filter_map(|c| c.bytes).collect();
    assert!(sizes.windows(2).all(|w| w[0] >= w[1]), "{sizes:?}");
    // The loose file is inside the remainder, not lost.
    let rest = unit
        .children
        .iter()
        .find(|c| c.kind == ChildKind::Remainder)
        .expect("a remainder row for what is not listed");
    assert!(rest.bytes.unwrap() >= 5_000);
}

#[test]
fn a_large_child_count_lists_the_top_rows_and_one_remainder_and_still_adds_up() {
    let fx = rustup_fixture();
    for i in 0..120u64 {
        write(
            &fx.rustup.join(format!("toolchains/t{i:03}/bin/rustc")),
            1_000 + (i as usize) * 100,
        );
    }
    let unit = fx.toolchains();
    assert!(unit.children.len() <= swamp_core::drilldown::DRILLDOWN_TOP_N + 2);
    assert_eq!(rows_total(&unit.children), unit.bytes as i64);
}

fn store_scope(vars: &[(&str, &Path)], root: &Path, home: &Path) -> EffectiveScope {
    let env_vars: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.display().to_string()))
        .collect();
    let env = Environment::fixture(home.to_path_buf(), env_vars, Platform::MacOS);
    let registry = Registry::with_builtins();
    let cfg = ScanConfig {
        defaults: false,
        include: vec![root.display().to_string()],
        exclude: Vec::new(),
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .filter(|id| id != "rustup")
            .collect(),
        enabled_detectors: Vec::new(),
    };
    resolve_effective_scope(&env, &cfg, &[], &registry, 1_000)
}

fn observe(scope: &EffectiveScope, store: &Path) -> swamp_core::report::ScopeObservation {
    observe_scope(
        scope,
        ObservationParts::ALL,
        None,
        None,
        false,
        Some(store),
        None,
        true,
        true,
        false,
        false,
        swamp_core::fs_events::platform_source().as_ref(),
        30,
        24 * 3600,
    )
    .expect("observe_scope")
}

struct Stored {
    _tmp: tempfile::TempDir,
    store: PathBuf,
    scope: EffectiveScope,
    rustc: PathBuf,
}

fn stored_fixture() -> Stored {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let rustup = home.join("rustup-home");
    let root = home.join("src");
    fs::create_dir_all(&root).unwrap();
    let rustc = rustup.join("toolchains/stable-x/bin/rustc");
    write(&rustc, 20_000);
    write(&rustup.join("toolchains/stable-x/lib/libstd.rlib"), 30_000);
    set_times(&rustc, OLD, NEWER);
    let store = home.join("store");
    fs::create_dir_all(&store).unwrap();
    let scope = store_scope(&[("RUSTUP_HOME", &rustup)], &root, &home);
    Stored {
        _tmp: tmp,
        store,
        scope,
        rustc,
    }
}

#[test]
fn stored_facts_round_trip_and_reading_a_report_never_walks_or_probes() {
    // Tempting wrong patch: computing last-used (or the drilldown) when
    // the report is read, which makes `swamp report` walk.
    let fx = stored_fixture();
    let observed = observe(&fx.scope, &fx.store);
    let want = unit_ending(&observed.external_units, "toolchains").clone();
    assert_eq!(want.last_used.at, Some(OLD as u64));
    assert!(!want.children.is_empty());

    let (snapshot, work) = swamp_core::work_counters::measured(|| {
        report_scope_from_store(&fx.scope, &fx.store).expect("the stored observation reads")
    });
    assert_eq!(work.dirs_listed, 0, "a report read lists no directory");
    assert_eq!(work.files_statted, 0, "a report read stats no file");
    assert_eq!(work.subprocess_spawns, 0);
    let got = unit_ending(&snapshot.external_units, "toolchains");
    assert_eq!(got.last_used, want.last_used);
    assert_eq!(got.children, want.children);
    assert_eq!(got.bytes_counted_elsewhere, want.bytes_counted_elsewhere);
    assert_eq!(got.overlap_count, want.overlap_count);
    // The key file is untouched by both passes.
    assert_eq!(atime_of(&fx.rustc), OLD);
}

/// The v0.7.5 column list of `external_units.parquet`. v0.7.5 resets the
/// store on any marker it does not know, so v0.8.0 keeps this table's schema
/// exactly and puts what it adds in sibling tables.
const V075_EXTERNAL_UNITS_COLUMNS: [&str; 24] = [
    "scope_key",
    "id",
    "source_id",
    "source_name",
    "category",
    "path",
    "bytes",
    "complete",
    "mtime_max",
    "linkage_state",
    "linkage_basis",
    "project_id",
    "protected",
    "protect_reason",
    "consequence",
    "observed_at",
    "growth_bytes",
    "regrowth_count",
    "provenance_kind",
    "provenance_value",
    "hardlinked",
    "tool_home",
    "relative_path",
    "action",
];

fn parquet_columns(path: &Path) -> Vec<String> {
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    ParquetRecordBatchReaderBuilder::try_new(fs::File::open(path).unwrap())
        .unwrap()
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect()
}

fn store_snapshot(store: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![store.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if e.file_type().unwrap().is_dir() {
                stack.push(p);
            } else if !p.ends_with("store-write.lock") {
                out.push((p.clone(), fs::read(&p).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn v080_leaves_the_marker_and_the_v075_unit_schema_alone() {
    // Tempting wrong patch: widening external_units.parquet and bumping the
    // marker, which makes an installed v0.7.5 reset the store on every
    // alternation with this version.
    let fx = stored_fixture();
    observe(&fx.scope, &fx.store);
    assert_eq!(
        fs::read_to_string(fx.store.join("housekeeping.version")).unwrap(),
        "2\n"
    );
    let cols = parquet_columns(&fx.store.join("external_units.parquet"));
    assert_eq!(cols, V075_EXTERNAL_UNITS_COLUMNS);
    assert!(fx.store.join("unit_meta.parquet").is_file());
    assert!(fx.store.join("unit_children.parquet").is_file());
}

#[test]
fn a_store_a_v075_swamp_wrote_reads_without_the_sibling_tables() {
    let fx = stored_fixture();
    observe(&fx.scope, &fx.store);
    fs::remove_file(fx.store.join("unit_meta.parquet")).unwrap();
    fs::remove_file(fx.store.join("unit_children.parquet")).unwrap();
    let snapshot = report_scope_from_store(&fx.scope, &fx.store).expect("still readable");
    let unit = unit_ending(&snapshot.external_units, "toolchains");
    assert_eq!(unit.last_used, Default::default(), "no last-used, no error");
    assert!(unit.children.is_empty());
    assert_eq!(unit.overlap_count, 0);
}

#[test]
fn sibling_rows_from_another_pass_are_not_shown_as_current_facts() {
    // A v0.7.5 pass rewrites external_units and leaves unit_meta behind.
    let fx = stored_fixture();
    observe(&fx.scope, &fx.store);
    let meta = fx.store.join("unit_meta.parquet");
    let children = fx.store.join("unit_children.parquet");
    let before = fs::read(&meta).unwrap();
    // Age the sibling tables: a different observed_at than the units'.
    use parquet::arrow::ArrowWriter;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    for table in [&meta, &children] {
        let builder =
            ParquetRecordBatchReaderBuilder::try_new(fs::File::open(table).unwrap()).unwrap();
        let schema = builder.schema().clone();
        let at = schema.index_of("observed_at").unwrap();
        let batches: Vec<_> = builder.build().unwrap().map(|b| b.unwrap()).collect();
        let mut w =
            ArrowWriter::try_new(fs::File::create(table).unwrap(), schema.clone(), None).unwrap();
        for b in batches {
            let mut cols = b.columns().to_vec();
            let old = cols[at]
                .as_any()
                .downcast_ref::<arrow_array::UInt64Array>()
                .unwrap();
            cols[at] = std::sync::Arc::new(arrow_array::UInt64Array::from(
                old.iter().map(|v| v.map(|x| x + 1)).collect::<Vec<_>>(),
            ));
            w.write(&arrow_array::RecordBatch::try_new(schema.clone(), cols).unwrap())
                .unwrap();
        }
        w.close().unwrap();
    }
    assert_ne!(fs::read(&meta).unwrap(), before);
    let snapshot = report_scope_from_store(&fx.scope, &fx.store).unwrap();
    let unit = unit_ending(&snapshot.external_units, "toolchains");
    assert_eq!(unit.last_used, Default::default());
    assert!(unit.children.is_empty());
}

#[test]
fn a_newer_marker_is_read_but_never_reset_or_written() {
    // Tempting wrong patch: reset on any marker difference (the old rule,
    // whose comment said "older"), which lets an older swamp wipe a newer
    // one's store and the newer one wipe it back.
    let fx = stored_fixture();
    observe(&fx.scope, &fx.store);
    let marker = fx.store.join("housekeeping.version");
    fs::write(&marker, "9\n").unwrap();
    let before = store_snapshot(&fx.store);
    assert!(
        report_scope_from_store(&fx.scope, &fx.store).is_ok(),
        "a newer store is read, not refused"
    );
    let err = observe_scope(
        &fx.scope,
        ObservationParts::ALL,
        None,
        None,
        false,
        Some(&fx.store),
        None,
        true,
        true,
        false,
        false,
        swamp_core::fs_events::platform_source().as_ref(),
        30,
        24 * 3600,
    )
    .err()
    .expect("observe refuses to write a newer store");
    assert!(
        err.to_string().contains("newer swamp; not modifying it"),
        "{err}"
    );
    assert_eq!(store_snapshot(&fx.store), before, "not one byte changed");
}

#[test]
fn an_older_marker_still_resets_and_keeps_human_files() {
    let fx = stored_fixture();
    observe(&fx.scope, &fx.store);
    let marker = fx.store.join("housekeeping.version");
    fs::write(&marker, "1\n").unwrap();
    fs::write(fx.store.join("config.toml"), b"# keep me\n").unwrap();
    let ledger = fx.store.join("ledger.parquet");
    fs::write(&ledger, b"ledger state").unwrap();
    let asked = fx.store.join("first-run-asked");
    fs::write(&asked, b"asked\n").unwrap();
    assert!(report_scope_from_store(&fx.scope, &fx.store).is_err());
    observe(&fx.scope, &fx.store);
    assert_eq!(fs::read_to_string(&marker).unwrap(), "2\n");
    assert_eq!(fs::read(fx.store.join("config.toml")).unwrap(), b"# keep me\n");
    assert_eq!(fs::read(&ledger).unwrap(), b"ledger state");
    assert_eq!(fs::read(&asked).unwrap(), b"asked\n");
    let snapshot = report_scope_from_store(&fx.scope, &fx.store).unwrap();
    assert_eq!(
        unit_ending(&snapshot.external_units, "toolchains").last_used.at,
        Some(OLD as u64)
    );
}

#[test]
fn structured_overlap_fields_survive_the_store_and_sum_with_the_parent() {
    // Tempting wrong patch: keeping the overlap only in the free-text
    // note, which the Reclaim view would have to parse.
    use swamp_core::external::{NestedWorktree, discover_and_measure_with_worktrees};
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let rustup = home.join("rustup-home");
    write(&rustup.join("toolchains/a/bin/rustc"), 10_000);
    let inner = rustup.join("toolchains/a/wt");
    write(&inner.join("target/blob"), 200_000);
    let scope = scope_for("rustup", &[("RUSTUP_HOME", &rustup)], &home);
    let store = tempfile::tempdir().unwrap();
    let whole = measure(&scope, store.path());
    let whole_bytes = unit_ending(&whole, "toolchains").bytes;
    let units = discover_and_measure_with_worktrees(
        &scope,
        &[NestedWorktree {
            path: inner.clone(),
            reported_bytes: 200_000,
        }],
        Some(store.path()),
        true,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    let unit = unit_ending(&units, "toolchains");
    assert_eq!(unit.overlap_count, 1);
    assert_eq!(unit.bytes_counted_elsewhere, 200_000);
    assert_eq!(unit.note, None);
    assert!(unit.bytes < whole_bytes);
    assert!(
        unit.overlap_note()
            .unwrap()
            .contains("counted under projects")
    );
}

/// DerivedData's `info.plist` through the real bounded `plutil` read, in
/// the shapes a real machine has: XML, binary, corrupt, absent, and a
/// plist with no `LastAccessedDate`. Xcode and `plutil` are macOS-only.
#[cfg(target_os = "macos")]
mod derived_data_plist {
    use super::*;
    use swamp_core::external_associations::read_plist_facts;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>LastAccessedDate</key>
	<date>2026-09-06T15:39:21Z</date>
	<key>WorkspacePath</key>
	<string>/Users/dev/src/App/App.xcworkspace</string>
</dict>
</plist>
"#;

    fn plist_dir() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = fs::canonicalize(tmp.path()).unwrap();
        (tmp, dir)
    }

    #[test]
    fn an_xml_plist_gives_both_facts() {
        let (_tmp, dir) = plist_dir();
        let p = dir.join("info.plist");
        fs::write(&p, XML).unwrap();
        let facts = read_plist_facts(&p).unwrap();
        assert_eq!(facts.last_accessed, Some(1_788_709_161));
        assert_eq!(
            facts.workspace_path.as_deref(),
            Some("/Users/dev/src/App/App.xcworkspace")
        );
    }

    #[test]
    fn a_binary_plist_reads_the_same_as_its_xml_form() {
        let (_tmp, dir) = plist_dir();
        let xml = dir.join("xml.plist");
        let bin = dir.join("info.plist");
        fs::write(&xml, XML).unwrap();
        let status = std::process::Command::new("plutil")
            .args(["-convert", "binary1", "-o"])
            .arg(&bin)
            .arg(&xml)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(fs::read(&bin).unwrap().starts_with(b"bplist"));
        assert_eq!(
            read_plist_facts(&bin).unwrap(),
            read_plist_facts(&xml).unwrap()
        );
    }

    #[test]
    fn a_corrupt_or_missing_plist_is_a_named_error_never_a_date() {
        let (_tmp, dir) = plist_dir();
        let corrupt = dir.join("info.plist");
        fs::write(&corrupt, b"bplist00 truncated \x00\x01\x02 garbage").unwrap();
        assert!(read_plist_facts(&corrupt).is_err());
        assert!(read_plist_facts(&dir.join("absent.plist")).is_err());
        let empty = dir.join("empty.plist");
        fs::write(&empty, b"").unwrap();
        assert!(read_plist_facts(&empty).is_err());
    }

    #[test]
    fn a_plist_without_the_date_has_no_last_accessed() {
        let (_tmp, dir) = plist_dir();
        let p = dir.join("info.plist");
        fs::write(
            &p,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>WorkspacePath</key><string>/x</string></dict></plist>
"#,
        )
        .unwrap();
        let facts = read_plist_facts(&p).unwrap();
        assert_eq!(facts.last_accessed, None);
        assert_eq!(facts.workspace_path.as_deref(), Some("/x"));
    }
}
