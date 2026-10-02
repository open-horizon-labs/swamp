//! v0.8.0 G4b (#167, #170): the developer-storage headline. Fixture-only:
//! every store and report here is built by hand or in a temp directory;
//! nothing reads the developer's real store or disk.
//!
//! Each test names the tempting wrong patch it fails.

use std::path::PathBuf;

use swamp_core::external::ExternalUnit;
use swamp_core::growth::{VolumeMetaRow, write_volume_ledger};
use swamp_core::headline::{
    Category, Disk, Headline, Input, ScopeKind, build, category_of_unit, relation,
    render_reclaim_header,
};
use swamp_core::last_used::LastUsed;
use swamp_core::locations::homebrew::HOMEBREW_OTHER_DETECTOR_ID;
use swamp_core::locations::{HeadlineGroup, Provenance, StorageCategory, remainder_of};
use swamp_core::manager_facts::ManagerFacts;
use swamp_core::reclaim::{ReclaimInput, build as build_reclaim};
use swamp_core::report::{Report, UnownedReason, UnownedRow};
use swamp_core::volume_ledger::{
    Accounted, Category as LedgerCategory, Exactness, LedgerReading, MountKind, MountView, Row,
    account, accounted_rows, read_reading,
};

const NOW: u64 = 1_790_000_000;
const GB: u64 = 1_000_000_000;

fn unit(
    detector: &str,
    name: &str,
    category: StorageCategory,
    path: &str,
    bytes: u64,
) -> ExternalUnit {
    ExternalUnit {
        detector_id: detector.to_string(),
        detector_name: name.to_string(),
        category,
        provenance: Provenance::BuiltinConvention,
        path: PathBuf::from(path),
        bytes,
        mtime_max: 0,
        hardlinked: false,
        growth_bytes: None,
        regrowth_count: 0,
        observed_at: NOW,
        consumers: Vec::new(),
        note: None,
        evidence: Vec::new(),
        bytes_counted_elsewhere: 0,
        overlap_count: 0,
        last_used: LastUsed::default(),
        children: Vec::new(),
    }
}

fn standalone(path: &str, bytes: u64) -> UnownedRow {
    UnownedRow {
        measurement: None,
        path_or_object: path.to_string(),
        bytes,
        reason: UnownedReason::StandaloneCargoTarget,
        shared_bytes: None,
        note: None,
        docker_kind: None,
        created_at: None,
        containers: Vec::new(),
        shared_with: Vec::new(),
        dangling: false,
        evidence: Vec::new(),
    }
}

fn report(walked: u64, targets: &[(&str, u64)]) -> Report {
    let mut r = Report::empty(PathBuf::from("/h/src"));
    r.observed_at = NOW - 240;
    r.reconciliation.walked_total = walked;
    r.reconciliation.unowned = targets.iter().fold(0u64, |a, t| a.saturating_add(t.1));
    r.unowned = targets.iter().map(|(p, b)| standalone(p, *b)).collect();
    r
}

fn meta(used: Option<u64>, data: Option<u64>, measured_at: u64) -> VolumeMetaRow {
    VolumeMetaRow {
        measured_at,
        cycle_started_at: 1,
        cycle_complete_at: measured_at,
        complete: true,
        budget_secs: 120,
        budget_used_ms: 1000,
        statfs_at: measured_at,
        container_total: Some(500 * GB),
        container_used: used,
        container_free: Some(100 * GB),
        data_volume_used: data,
    }
}

fn lrow(path: &str, cat: LedgerCategory, bytes: Option<u64>) -> Row {
    Row {
        path: path.to_string(),
        category: cat,
        bytes,
        overlap_bytes: 0,
        entries: None,
        unreadable: 0,
        measured_at: NOW - 3 * 3600,
        method: "walk: allocated bytes, lstat only".to_string(),
        exactness: if bytes.is_some() {
            Exactness::Exact
        } else {
            Exactness::NotMeasured
        },
        note: None,
    }
}

fn reading(rows: Vec<Row>, m: VolumeMetaRow) -> LedgerReading {
    LedgerReading::Measured(Box::new(account(&rows, &m)))
}

fn headline_of(
    units: &[ExternalUnit],
    r: &Report,
    ledger: &LedgerReading,
    scope: ScopeKind,
) -> Headline {
    build(&Input {
        units,
        report: r,
        ledger,
        scope,
        observed_at: r.observed_at,
        now: NOW,
    })
}

/// The fixture whose parts are known:
/// projects 28 GB + standalone 2 GB (walked 30 GB), rustup 5, uv cache 3,
/// Claude Code home 1, Docker data 20, Hugging Face models 1, Homebrew
/// other 12 (a remainder: not counted), simulator volumes 40 of which 40
/// are mounted images (not counted).
fn known_units() -> Vec<ExternalUnit> {
    vec![
        unit(
            "rustup",
            "rustup",
            StorageCategory::Installation,
            "/h/.rustup/toolchains",
            5 * GB,
        ),
        unit("uv", "uv", StorageCategory::Cache, "/h/.cache/uv", 3 * GB),
        unit(
            "claude-code",
            "Claude Code",
            StorageCategory::LocalState,
            "/h/.claude",
            GB,
        ),
        unit(
            "docker-desktop",
            "Docker Desktop",
            StorageCategory::LocalState,
            "/h/Library/Containers/docker",
            20 * GB,
        ),
        unit(
            "huggingface",
            "Hugging Face",
            StorageCategory::Models,
            "/h/.cache/huggingface/hub",
            GB,
        ),
        unit(
            HOMEBREW_OTHER_DETECTOR_ID,
            "Homebrew (other)",
            StorageCategory::Installation,
            "/opt/homebrew",
            12 * GB,
        ),
        unit(
            "core-simulator",
            "CoreSimulator",
            StorageCategory::Installation,
            "/Library/Developer/CoreSimulator/Volumes",
            40 * GB,
        ),
    ]
}

/// The ledger the same observation writes: the source root's walked
/// total and every unit, through the real row builder, with the
/// simulator images mounted (their bytes are the image files).
fn known_ledger(used: u64, units: &[ExternalUnit], walked: u64, measured_at: u64) -> LedgerReading {
    let mut list: Vec<Accounted> = vec![Accounted {
        path: PathBuf::from("/h/src"),
        bytes: walked,
        category: LedgerCategory::Declared,
        subset_of_enclosing: false,
        measured_at,
        incomplete: false,
        note: None,
    }];
    for u in units {
        list.push(Accounted {
            path: u.path.clone(),
            bytes: u.bytes,
            category: LedgerCategory::Catalog,
            subset_of_enclosing: false,
            measured_at,
            incomplete: false,
            note: None,
        });
    }
    let mounts = vec![MountView {
        path: PathBuf::from("/Library/Developer/CoreSimulator/Volumes/iOS_1"),
        kind: MountKind::OwnStorage,
        used: Some(40 * GB),
    }];
    let mut rows = accounted_rows(&list, &mounts);
    rows.push(lrow("/Users/x/Movies", LedgerCategory::Other, Some(9 * GB)));
    rows.push(lrow(
        "/System/Library/AssetsV2",
        LedgerCategory::Other,
        Some(25 * GB),
    ));
    rows.push(lrow(
        "APFS volume Preboot (disk3s2)",
        LedgerCategory::System,
        Some(10 * GB),
    ));
    rows.push(lrow(
        "APFS volume VM (disk3s6)",
        LedgerCategory::System,
        Some(6 * GB),
    ));
    rows.push(lrow("/Users/x/Pictures", LedgerCategory::Unreadable, None));
    reading(rows, meta(Some(used), Some(used - 16 * GB), measured_at))
}

fn category(h: &Headline, c: Category) -> (usize, u64) {
    let r = h.categories.iter().find(|r| r.category == c).unwrap();
    (r.count, r.bytes)
}

// ---------------------------------------------------------------------
// The exact strings, on a fixture whose parts are known
// ---------------------------------------------------------------------

#[test]
fn the_known_fixture_produces_the_exact_headline_and_rows() {
    let units = known_units();
    let r = report(30 * GB, &[("/h/src/tmp/t1", GB), ("/h/src/tmp/t2", GB)]);
    let ledger = known_ledger(200 * GB, &units, 30 * GB, NOW - 3 * 3600);
    let h = headline_of(&units, &r, &ledger, ScopeKind::Current);
    // 28 + 2 + 5 + 3 + 1 + 20 + 1 = 60 GB of 200 GB used: 30.0%.
    assert_eq!(
        h.line,
        "Developer storage: 60.0GB across 1 project and 7 tool locations (30.0% of used)"
    );
    assert_eq!(h.developer_bytes, 60 * GB);
    assert_eq!(category(&h, Category::Projects), (1, 28 * GB));
    assert_eq!(category(&h, Category::ToolchainsAndSdks), (1, 5 * GB));
    assert_eq!(category(&h, Category::Caches), (1, 3 * GB));
    assert_eq!(category(&h, Category::AgentStorage), (1, GB));
    assert_eq!(category(&h, Category::ContainersAndVms), (1, 20 * GB));
    assert_eq!(category(&h, Category::StandaloneCargoTargets), (2, 2 * GB));
    assert_eq!(category(&h, Category::OtherDeveloperUnits), (1, GB));
    let text = h.render_text(NOW);
    let want = "\
Developer storage: 60.0GB across 1 project and 7 tool locations (30.0% of used)
  observed 4m ago; disk ledger measured 3h ago
  projects                       28.0GB     1 project
  toolchains and SDKs             5.0GB     1 location
  caches                          3.0GB     1 location
  agent storage                   1.0GB     1 location
  containers and VMs             20.0GB     1 location
  standalone Cargo targets        2.0GB     2 locations
  other developer units           1.0GB     1 location
  totals use exact stored bytes; displayed rows are rounded independently
  not counted: 12.0GB in Homebrew (other) (the rest of a location after its developer tooling)
  not counted: 40.0GB of mounted disk images (views of image files; the disk cost is the image files themselves, listed under Everything else, not in developer storage)
Everything else (measured, not developer storage): 34.0GB across 2 folders
      25.0GB  /System/Library/AssetsV2  (exact)
       9.0GB  /Users/x/Movies  (exact)
System volumes: 16.0GB (Preboot, VM) (separate volumes that share the container's free space)
Not measured: 1 directory (protected folders); the unexplained part of the Data volume, up to 78.0GB, may be inside them (an estimate, not part of any check)
  /Users/x/Pictures (not measured)
Walk spot audit: not run (not run this pass)
";
    assert_eq!(text, want, "\n{text}");
}

// ---------------------------------------------------------------------
// Sum identity and rounding
// ---------------------------------------------------------------------

/// Tempting wrong patch: the rows are rounded first and added (or the
/// headline is rounded and the rows are not), so the rows disagree with
/// the headline by a few bytes or a rounding step. The rows here are odd
/// byte counts on purpose.
#[test]
fn the_rows_sum_to_the_headline_exactly_whatever_the_rounding() {
    let units = vec![
        unit(
            "rustup",
            "rustup",
            StorageCategory::Installation,
            "/a",
            1_234_567_891,
        ),
        unit("uv", "uv", StorageCategory::Cache, "/b", 987_654_321),
        unit(
            "claude-code",
            "Claude Code",
            StorageCategory::LocalState,
            "/c",
            55_555_555,
        ),
        unit(
            "mystery-tool",
            "Mystery",
            StorageCategory::Unclassified,
            "/d",
            77_777_777,
        ),
    ];
    let r = report(3_333_333_333, &[("/h/src/t", 11_111_111)]);
    let h = headline_of(&units, &r, &LedgerReading::NotMeasured, ScopeKind::Current);
    let sum: u64 = h.categories.iter().map(|c| c.bytes).sum();
    assert_eq!(sum, h.developer_bytes);
    let count: usize = h.categories.iter().map(|c| c.count).sum();
    assert_eq!(count, h.locations);
    let text = h.render_text(NOW);
    assert!(
        text.contains("totals use exact stored bytes; displayed rows are rounded independently"),
        "{text}"
    );
    assert_eq!(h.to_json()["developer_bytes"], h.developer_bytes);
    assert_eq!(
        h.to_json()["categories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["bytes"].as_u64().unwrap())
            .sum::<u64>(),
        h.developer_bytes
    );
}

/// Tempting wrong patch: the percent is rounded to nearest, so 99.96%
/// reads 100.0% and 57.46% reads 57.5%. It is rounded down, in integers.
#[test]
fn the_percent_is_rounded_down_never_up() {
    let units = vec![unit(
        "rustup",
        "rustup",
        StorageCategory::Installation,
        "/a",
        5746,
    )];
    let r = report(0, &[]);
    let ledger = reading(vec![], meta(Some(10_000), Some(9_000), NOW - 60));
    let h = headline_of(&units, &r, &ledger, ScopeKind::Current);
    assert!(h.line.ends_with("(57.4% of used)"), "{}", h.line);
    let units = vec![unit(
        "rustup",
        "rustup",
        StorageCategory::Installation,
        "/a",
        9996,
    )];
    let h = headline_of(&units, &r, &ledger, ScopeKind::Current);
    assert!(h.line.ends_with("(99.9% of used)"), "{}", h.line);
}

// ---------------------------------------------------------------------
// The percent's denominator and its refusals
// ---------------------------------------------------------------------

/// Tempting wrong patch: the percent divides by the Data volume's used
/// bytes (what `du` sees) instead of the container's (what `df` calls
/// used). Here Data is 150 GB and the container 200 GB: 60 GB is 30.0%,
/// not 40.0%.
#[test]
fn the_percent_is_of_the_container_not_the_data_volume() {
    let units = known_units();
    let r = report(30 * GB, &[("/h/src/tmp/t1", GB), ("/h/src/tmp/t2", GB)]);
    let ledger = known_ledger(200 * GB, &units, 30 * GB, NOW - 60);
    let h = headline_of(&units, &r, &ledger, ScopeKind::Current);
    assert!(h.line.contains("(30.0% of used)"), "{}", h.line);
    assert!(!h.line.contains("40.0%"));
    if let Disk::Measured(m) = &h.disk {
        assert_eq!(m.container_used, Some(200 * GB));
    } else {
        panic!("measured");
    }
}

/// Tempting wrong patch: developer storage larger than the disk's used
/// bytes prints "120.0% of used" (or clamps to 100%) without saying the
/// measurement is wrong.
#[test]
fn developer_storage_larger_than_used_is_flagged_and_has_no_percent() {
    let units = vec![unit(
        "rustup",
        "rustup",
        StorageCategory::Installation,
        "/a",
        60 * GB,
    )];
    let r = report(0, &[]);
    let ledger = reading(vec![], meta(Some(50 * GB), Some(40 * GB), NOW - 60));
    let h = headline_of(&units, &r, &ledger, ScopeKind::Current);
    assert!(!h.line.contains('%'), "{}", h.line);
    let text = h.render_text(NOW);
    assert!(
        text.contains("FLAG: developer storage (60.0GB) is more than the disk's used bytes"),
        "{text}"
    );
    assert!(
        !text.contains("120.0%") && !text.contains("100.0%"),
        "{text}"
    );
    let j = h.to_json();
    assert!(j["percent_of_used"].is_null());
    assert_eq!(j["disk"]["exceeds_used"], true);
    assert!(!h.flags.is_empty());
}

/// Tempting wrong patch: a zero used figure divides by zero (a panic in
/// debug, infinity or NaN in JSON).
#[test]
fn zero_used_bytes_gives_no_percent_and_no_panic() {
    let units = vec![unit(
        "rustup",
        "rustup",
        StorageCategory::Installation,
        "/a",
        GB,
    )];
    let r = report(0, &[]);
    let ledger = reading(vec![], meta(Some(0), Some(0), NOW - 60));
    let h = headline_of(&units, &r, &ledger, ScopeKind::Current);
    assert!(!h.line.contains('%'));
    assert!(
        h.render_text(NOW)
            .contains("reports 0 bytes used; no percent")
    );
    let v = h.to_json();
    assert!(v["percent_of_used"].is_null());
    // And a ledger with no container figure at all.
    let ledger = reading(vec![], meta(None, None, NOW - 60));
    let h = headline_of(&units, &r, &ledger, ScopeKind::Current);
    assert!(!h.line.contains('%'));
    assert!(
        h.render_text(NOW)
            .contains("container's size is not available")
    );
}

/// Tempting wrong patch: the headline reads the ledger through the same
/// path as the disk view and panics or invents a percent when the file is
/// absent, corrupt, from a newer swamp, or dated in the future.
#[test]
fn a_missing_corrupt_newer_or_future_dated_ledger_degrades_with_a_line_and_no_percent() {
    let units = vec![unit(
        "rustup",
        "rustup",
        StorageCategory::Installation,
        "/a",
        GB,
    )];
    let r = report(0, &[]);
    let dir = tempfile::tempdir().unwrap();
    let mut states: Vec<(&str, LedgerReading)> = Vec::new();
    // Absent.
    states.push(("absent", read_reading(dir.path())));
    // Corrupt: the files exist and are not Parquet.
    let bad = tempfile::tempdir().unwrap();
    std::fs::write(
        bad.path().join("volume_ledger.parquet"),
        b"not parquet at all",
    )
    .unwrap();
    std::fs::write(bad.path().join("volume_ledger_meta.parquet"), b"\0\0\0\0").unwrap();
    states.push(("corrupt", read_reading(bad.path())));
    // Newer: a row whose category this build does not know.
    let newer = tempfile::tempdir().unwrap();
    let mut future_row = lrow("/x", LedgerCategory::Other, Some(1));
    future_row.measured_at = NOW - 60;
    let mut stored = future_row.clone().to_stored_for_test();
    stored.category = "quantum-storage".to_string();
    write_volume_ledger(
        newer.path(),
        &[stored],
        &meta(Some(GB), Some(GB), NOW - 60),
        None,
    )
    .unwrap();
    states.push(("newer", read_reading(newer.path())));
    // Future-dated: written a day from now.
    let fut = tempfile::tempdir().unwrap();
    write_volume_ledger(
        fut.path(),
        &[future_row.to_stored_for_test()],
        &meta(Some(100 * GB), Some(90 * GB), NOW + 86_400),
        None,
    )
    .unwrap();
    states.push(("future", read_reading(fut.path())));

    let want = [
        (
            "absent",
            "disk ledger: not measured yet; run swamp observe --volume",
        ),
        ("corrupt", "disk ledger: could not be read"),
        ("newer", "disk ledger: written by a newer swamp"),
        ("future", "disk ledger: dated in the future"),
    ];
    for ((name, ledger), (wname, sentence)) in states.iter().zip(want) {
        assert_eq!(*name, wname);
        let h = headline_of(&units, &r, ledger, ScopeKind::Current);
        assert!(!h.line.contains('%'), "{name}: {}", h.line);
        let text = h.render_text(NOW);
        assert!(text.contains(sentence), "{name}: {text}");
        assert!(h.to_json()["percent_of_used"].is_null(), "{name}");
        // The developer bytes do not depend on the ledger being readable.
        assert_eq!(h.developer_bytes, GB, "{name}");
    }
}

/// Tempting wrong patch: a ledger written long before the observation is
/// used as if it were current, with no word about the gap.
#[test]
fn a_ledger_much_older_than_the_observation_says_so() {
    let units = vec![unit(
        "rustup",
        "rustup",
        StorageCategory::Installation,
        "/a",
        GB,
    )];
    let mut r = report(0, &[]);
    r.observed_at = NOW - 2 * 3600;
    let ledger = reading(vec![], meta(Some(10 * GB), Some(9 * GB), NOW - 4 * 86_400));
    let h = headline_of(&units, &r, &ledger, ScopeKind::Current);
    let text = h.render_text(NOW);
    assert!(
        text.contains("observed 2h ago; disk ledger measured 4d ago; the ledger is 3 d older than this observation"),
        "{text}"
    );
    // A ledger a few hours older is not flagged.
    let ledger = reading(vec![], meta(Some(10 * GB), Some(9 * GB), NOW - 3 * 3600));
    let h = headline_of(&units, &r, &ledger, ScopeKind::Current);
    assert!(!h.render_text(NOW).contains("older than this observation"));
}

// ---------------------------------------------------------------------
// What is and is not developer storage
// ---------------------------------------------------------------------

/// Tempting wrong patch: the remainder unit is selected by id string
/// (`detector_id == "homebrew-other"`) or by its display name, so a
/// second remainder detector (or a renamed one) is counted as developer
/// storage. Selection is by `Detector::remainder_of`.
#[test]
fn the_remainder_is_selected_by_capability_not_by_id_or_name() {
    // A unit that merely SAYS it is Homebrew (other) is counted; the
    // detector that declares itself a remainder is not, whatever it is
    // called.
    let units = vec![
        unit(
            "rustup",
            "Homebrew (other)",
            StorageCategory::Installation,
            "/a",
            4 * GB,
        ),
        unit(
            HOMEBREW_OTHER_DETECTOR_ID,
            "the rest of the prefix",
            StorageCategory::Installation,
            "/opt/homebrew",
            9 * GB,
        ),
    ];
    let r = report(0, &[]);
    let h = headline_of(&units, &r, &LedgerReading::NotMeasured, ScopeKind::Current);
    assert_eq!(h.developer_bytes, 4 * GB);
    assert_eq!(h.not_counted.remainder_bytes, 9 * GB);
    assert!(remainder_of(HOMEBREW_OTHER_DETECTOR_ID).is_some());
    // And no detector id is spelled in the headline's source: the
    // selection cannot be an id comparison.
    let src = include_str!("../src/headline.rs");
    for d in swamp_core::locations::Registry::with_builtins().detectors() {
        assert!(
            !src.contains(&format!("\"{}\"", d.id())),
            "headline.rs names detector id {:?}",
            d.id()
        );
    }
}

/// Tempting wrong patch: a unit that is both a project worktree and
/// inside a detector location is added twice (the location's bytes plus
/// the project's), or the worktree is subtracted a second time. Units
/// already exclude the worktrees counted under projects
/// (`bytes_counted_elsewhere`), so the sum is walked + unit bytes.
#[test]
fn a_worktree_inside_a_detector_location_is_counted_once() {
    let mut codex = unit(
        "codex",
        "Codex",
        StorageCategory::LocalState,
        "/h/.codex",
        10 * GB,
    );
    // 4 GB of worktrees live inside /h/.codex and are in the walked total
    // of the source roots (20 GB); the unit's own bytes exclude them.
    codex.bytes_counted_elsewhere = 4 * GB;
    codex.overlap_count = 1;
    let r = report(20 * GB, &[]);
    let h = headline_of(
        &[codex],
        &r,
        &LedgerReading::NotMeasured,
        ScopeKind::Current,
    );
    assert_eq!(h.developer_bytes, 30 * GB);
    assert_eq!(category(&h, Category::Projects).1, 20 * GB);
    assert_eq!(category(&h, Category::AgentStorage).1, 10 * GB);
}

/// Tempting wrong patch: standalone Cargo targets are added on top of the
/// walked total that already holds them.
#[test]
fn standalone_cargo_targets_are_not_counted_twice() {
    let r = report(30 * GB, &[("/h/src/x/target", 6 * GB)]);
    let h = headline_of(&[], &r, &LedgerReading::NotMeasured, ScopeKind::Current);
    assert_eq!(h.developer_bytes, 30 * GB);
    assert_eq!(category(&h, Category::Projects).1, 24 * GB);
    assert_eq!(category(&h, Category::StandaloneCargoTargets), (1, 6 * GB));
    // Targets larger than the recorded walk (a store that did not record
    // its walked total) cannot make projects wrap around below zero.
    let r = report(GB, &[("/h/src/x/target", 6 * GB)]);
    let h = headline_of(&[], &r, &LedgerReading::NotMeasured, ScopeKind::Current);
    assert_eq!(category(&h, Category::Projects).1, 0);
    assert_eq!(h.developer_bytes, 6 * GB);
}

/// Tempting wrong patch: the mounted simulator images are counted at
/// their mounted size (42.8 GB here), though the disk cost is the image
/// files. With a ledger the headline takes them out and says so; without
/// one it counts what the observation measured.
#[test]
fn mounted_disk_images_are_not_counted_when_the_ledger_knows_them() {
    let units = known_units();
    let r = report(30 * GB, &[("/h/src/tmp/t1", GB), ("/h/src/tmp/t2", GB)]);
    let with = headline_of(
        &units,
        &r,
        &known_ledger(200 * GB, &units, 30 * GB, NOW - 60),
        ScopeKind::Current,
    );
    assert_eq!(with.developer_bytes, 60 * GB);
    assert_eq!(with.not_counted.mounted_image_bytes, 40 * GB);
    let without = headline_of(&units, &r, &LedgerReading::NotMeasured, ScopeKind::Current);
    assert_eq!(without.developer_bytes, 100 * GB);
    assert_eq!(without.not_counted.mounted_image_bytes, 0);
}

/// Tempting wrong patch: an agent tool's home (a finer view of a folder
/// counted whole elsewhere) is mistaken for a mounted-image overlap on
/// the same path, and its bytes are taken out of the unit.
#[test]
fn an_agent_home_overlap_is_not_a_mounted_image_overlap() {
    let units = vec![unit(
        "claude-code",
        "Claude Code",
        StorageCategory::LocalState,
        "/h/.claude",
        5 * GB,
    )];
    let list = vec![
        Accounted {
            path: PathBuf::from("/h/.claude"),
            bytes: 5 * GB,
            category: LedgerCategory::Catalog,
            subset_of_enclosing: false,
            measured_at: NOW - 60,
            incomplete: false,
            note: None,
        },
        Accounted {
            path: PathBuf::from("/h/.claude"),
            bytes: 2 * GB,
            category: LedgerCategory::Catalog,
            subset_of_enclosing: true,
            measured_at: NOW - 60,
            incomplete: false,
            note: Some("agent sessions, caches and logs".into()),
        },
    ];
    let rows = accounted_rows(&list, &[]);
    let ledger = reading(rows, meta(Some(50 * GB), Some(40 * GB), NOW - 60));
    let h = headline_of(&units, &report(0, &[]), &ledger, ScopeKind::Current);
    assert_eq!(h.developer_bytes, 5 * GB);
    assert_eq!(h.not_counted.mounted_image_bytes, 0);
}

/// Tempting wrong patch: a unit whose detector or category this build
/// does not place is dropped from the headline (or panics). It lands in
/// "other developer units", and every category maps somewhere.
#[test]
fn a_unit_nothing_classifies_falls_into_other_developer_units() {
    let units = vec![unit(
        "detector-from-the-future",
        "Future",
        StorageCategory::Unclassified,
        "/h/.future",
        7 * GB,
    )];
    let h = headline_of(
        &units,
        &report(0, &[]),
        &LedgerReading::NotMeasured,
        ScopeKind::Current,
    );
    assert_eq!(category(&h, Category::OtherDeveloperUnits), (1, 7 * GB));
    assert_eq!(h.developer_bytes, 7 * GB);
}

/// A new storage category, a new headline group or a new breakdown row
/// must be placed on purpose: the match names every one (no wildcard),
/// and these lists must name them all.
#[test]
fn every_catalog_kind_maps_to_a_row_and_a_new_kind_fails_until_it_does() {
    use StorageCategory::*;
    // Compile-time: adding a variant breaks this match.
    fn placed(c: StorageCategory) -> Category {
        match c {
            Installation | Environments => Category::ToolchainsAndSdks,
            Downloads | Cache | BuildOutput => Category::Caches,
            LocalState | Models | Unclassified => Category::OtherDeveloperUnits,
        }
    }
    let all = [
        Installation,
        Downloads,
        Cache,
        LocalState,
        Environments,
        BuildOutput,
        Models,
        Unclassified,
    ];
    for c in all {
        assert_eq!(category_of_unit(None, c), placed(c), "{c:?}");
    }
    // A group claim wins over the category.
    for c in all {
        assert_eq!(
            category_of_unit(Some(HeadlineGroup::AgentStorage), c),
            Category::AgentStorage
        );
        assert_eq!(
            category_of_unit(Some(HeadlineGroup::Containers), c),
            Category::ContainersAndVms
        );
    }
    // Every breakdown row is displayed, in one fixed order.
    assert_eq!(Category::ALL.len(), 7);
    for c in Category::ALL {
        match c {
            Category::Projects
            | Category::ToolchainsAndSdks
            | Category::Caches
            | Category::AgentStorage
            | Category::ContainersAndVms
            | Category::StandaloneCargoTargets
            | Category::OtherDeveloperUnits => {}
        }
    }
    // Every agent adapter's detector claims agent storage; the container
    // runtime claims containers; nothing else claims a group.
    let adapters = swamp_core::agents::registry::Registry::with_builtins().ids();
    let mut claiming: Vec<&str> = Vec::new();
    for d in swamp_core::locations::Registry::with_builtins().detectors() {
        match d.headline_group() {
            Some(HeadlineGroup::AgentStorage) => claiming.push(d.id()),
            Some(HeadlineGroup::Containers) => assert_eq!(d.id(), "docker-desktop"),
            None => {}
        }
    }
    for id in &adapters {
        assert!(
            claiming.contains(id),
            "agent adapter {id} has no agent-storage claim"
        );
    }
    for id in &claiming {
        assert!(
            adapters.contains(id) || *id == "claude-code-scratch",
            "{id} claims agent storage but is not an agent adapter"
        );
    }
}

// ---------------------------------------------------------------------
// Consistency: the headline, the Reclaim totals and the disk view
// ---------------------------------------------------------------------

/// Tempting wrong patch: the headline is computed from its own reading of
/// the units, and drifts from the Reclaim totals and the disk view's
/// accounted part. On one store all three agree, by definition:
///   developer = projects + reclaim(regenerable + held + not regenerable
///               + not established) - remainder units - mounted images
///   disk view accounted = developer + remainder units
#[test]
fn the_headline_the_reclaim_totals_and_the_disk_view_agree() {
    let units = known_units();
    let r = report(30 * GB, &[("/h/src/tmp/t1", GB), ("/h/src/tmp/t2", GB)]);
    let ledger = known_ledger(200 * GB, &units, 30 * GB, NOW - 60);
    let h = headline_of(&units, &r, &ledger, ScopeKind::Current);
    let facts = ManagerFacts::default();
    let view = build_reclaim(&ReclaimInput {
        units: &units,
        interiors: &[],
        unowned: &r.unowned,
        manager_facts: &facts,
        declared_roots: &[],
        explicit_scope: false,
        projects: 0,
        observed_at: r.observed_at,
    });
    let rel = relation(&h, &view.totals);
    assert!(rel.holds, "{rel:?}");
    assert_eq!(rel.developer_from_parts, h.developer_bytes);
    assert_eq!(view.totals.remainder_bytes, 12 * GB);
    // The four classes add up to the view's own total.
    let t = &view.totals;
    assert_eq!(
        t.regenerable_bytes + t.held_bytes + t.not_regenerable_bytes + t.not_established_bytes,
        t.bytes
    );
    // The disk view's accounted part, from the real row builder.
    let Disk::Measured(m) = &h.disk else {
        panic!("measured")
    };
    assert!(m.accounted_check.agrees, "{:?}", m.accounted_check);
    assert_eq!(
        m.accounted_check.disk_view_accounted,
        h.developer_bytes + h.not_counted.remainder_bytes
    );
    // The text says how the parts add up.
    let text = render_reclaim_header(&h, &view.totals, NOW);
    assert!(
        text.starts_with(
            "Developer storage: 60.0GB across 1 project and 7 tool locations (30.0% of used)"
        ),
        "{text}"
    );
    assert!(
        text.contains("Developer storage 60.0GB = projects 28.0GB + this view's units"),
        "{text}"
    );
    assert!(
        text.lines().count() <= 5,
        "the reclaim preamble is compact:\n{text}"
    );
    assert!(
        !text.contains("Everything else"),
        "disk detail belongs in its own view:\n{text}"
    );
    assert!(
        !text.contains("System volumes:"),
        "disk detail belongs in its own view:\n{text}"
    );

    let mut warning_headline = h.clone();
    warning_headline
        .flags
        .push("test critical flag remains visible".into());
    if let Disk::Measured(measured) = &mut warning_headline.disk {
        measured.audit.flag = true;
        measured.accounted_check.agrees = false;
        measured.accounted_check.disk_view_accounted = measured
            .accounted_check
            .developer_plus_remainder
            .saturating_sub(1);
        measured.accounted_check.difference = -1;
    }
    let warning_text = render_reclaim_header(&warning_headline, &view.totals, NOW);
    assert!(
        warning_text.contains("FLAG: test critical flag remains visible"),
        "{warning_text}"
    );
    assert!(
        warning_text.contains("FLAG: walk spot audit disagrees"),
        "{warning_text}"
    );
    assert!(warning_text.contains("disk view check:"), "{warning_text}");
}

/// Tempting wrong patch: a relation that quietly holds because it is
/// defined as its own remainder. Break one part and it must not hold.
#[test]
fn a_relation_that_does_not_add_up_says_so() {
    let units = known_units();
    let r = report(30 * GB, &[("/h/src/tmp/t1", GB), ("/h/src/tmp/t2", GB)]);
    let ledger = known_ledger(200 * GB, &units, 30 * GB, NOW - 60);
    let h = headline_of(&units, &r, &ledger, ScopeKind::Current);
    let facts = ManagerFacts::default();
    // The Reclaim view was built from a different observation (one unit
    // fewer): the relation must fail loudly.
    let fewer: Vec<ExternalUnit> = units[..3].to_vec();
    let view = build_reclaim(&ReclaimInput {
        units: &fewer,
        interiors: &[],
        unowned: &r.unowned,
        manager_facts: &facts,
        declared_roots: &[],
        explicit_scope: false,
        projects: 0,
        observed_at: r.observed_at,
    });
    let rel = relation(&h, &view.totals);
    assert!(!rel.holds);
    assert!(rel.text(h.developer_bytes).contains("these do not add up"));
}

/// Tempting wrong patch: the accounted-part check compares the sum with
/// a tolerance wide enough to hide a missing unit. A ledger whose
/// accounted rows are missing one unit disagrees, by exactly that unit.
#[test]
fn a_ledger_missing_a_unit_disagrees_with_the_headline_by_that_unit() {
    let units = known_units();
    let r = report(30 * GB, &[("/h/src/tmp/t1", GB), ("/h/src/tmp/t2", GB)]);
    let partial: Vec<ExternalUnit> = units
        .iter()
        .filter(|u| u.bytes != 5 * GB)
        .cloned()
        .collect();
    let ledger = known_ledger(200 * GB, &partial, 30 * GB, NOW - 60);
    let h = headline_of(&units, &r, &ledger, ScopeKind::Current);
    let Disk::Measured(m) = &h.disk else { panic!() };
    assert!(!m.accounted_check.agrees);
    assert_eq!(m.accounted_check.difference, -(5 * GB as i64));
}

// ---------------------------------------------------------------------
// The walk's independent spot audit
// ---------------------------------------------------------------------

fn audit_row(folder: &str, ledger: u64, audited: u64, outside: bool) -> Row {
    Row {
        path: format!("spot audit: {folder}"),
        category: LedgerCategory::Audit,
        bytes: Some(audited),
        overlap_bytes: 0,
        entries: Some(ledger),
        unreadable: 0,
        measured_at: NOW - 60,
        method: "spot audit: naive lstat sum".to_string(),
        exactness: if outside {
            Exactness::Estimated
        } else {
            Exactness::Exact
        },
        note: None,
    }
}

/// Tempting wrong patch: the headline shows a percent and no warning while
/// the spot audit says the walk undercounts (the only check that can catch
/// it), or it shows the warning for an audit inside its tolerance. The
/// warning is a line of the headline, in the text and in the JSON.
#[test]
fn a_walk_the_spot_audit_disagrees_with_is_warned_about_in_the_headline() {
    let units = vec![unit(
        "rustup",
        "rustup",
        StorageCategory::Installation,
        "/a",
        10 * GB,
    )];
    let r = report(0, &[]);
    let flagged = reading(
        vec![audit_row("/Users/x/Library", 100 * GB, 130 * GB, true)],
        meta(Some(200 * GB), Some(150 * GB), NOW - 60),
    );
    let h = headline_of(&units, &r, &flagged, ScopeKind::Current);
    let text = h.render_text(NOW);
    assert!(
        text.contains("  FLAG: walk spot audit disagrees: see swamp report --view disk"),
        "{text}"
    );
    assert_eq!(
        h.to_json()["audit_warning"],
        "FLAG: walk spot audit disagrees: see swamp report --view disk"
    );
    assert_eq!(h.to_json()["disk"]["audit"]["flag"], true);
    // Inside the tolerance: audited, said so, no warning.
    let fine = reading(
        vec![audit_row(
            "/Users/x/Library",
            100 * GB,
            100 * GB + 1000,
            false,
        )],
        meta(Some(200 * GB), Some(150 * GB), NOW - 60),
    );
    let h = headline_of(&units, &r, &fine, ScopeKind::Current);
    let text = h.render_text(NOW);
    assert!(!text.contains("spot audit disagrees"), "{text}");
    assert!(
        text.contains("Walk spot-audited: 1 folder, max difference 0.0%"),
        "{text}"
    );
    assert!(h.to_json()["audit_warning"].is_null());
}

// ---------------------------------------------------------------------
// Scope statements
// ---------------------------------------------------------------------

/// Tempting wrong patch: a report scoped to one root named on the
/// command line prints a percent of the machine's used bytes for what is
/// one folder, and a previous-scope report does not say so.
#[test]
fn an_explicit_root_and_a_previous_scope_are_stated() {
    let units = known_units();
    let r = report(30 * GB, &[]);
    let ledger = known_ledger(200 * GB, &units, 30 * GB, NOW - 60);
    let one = headline_of(&[], &r, &ledger, ScopeKind::ExplicitRoot);
    assert!(!one.line.contains("% of used"), "{}", one.line);
    assert!(
        one.line.contains("only the root named on the command line"),
        "{}",
        one.line
    );
    assert!(
        one.render_text(NOW)
            .contains("covers only the root named on the command line")
    );
    let prev = headline_of(&units, &r, &ledger, ScopeKind::Previous { roots: 3 });
    let text = prev.render_text(NOW);
    assert!(
        text.contains(
            "covers the previous scope (3 roots), so no percent of used is shown; new roots are not observed yet, run swamp observe"
        ),
        "{text}"
    );
    assert_eq!(prev.to_json()["previous_scope_roots"], 3);
}

// ---------------------------------------------------------------------
// Numbers, JSON, arithmetic
// ---------------------------------------------------------------------

/// Tempting wrong patch: the JSON is a second computation, and the text
/// and JSON disagree on a figure.
#[test]
fn text_and_json_carry_the_same_numbers() {
    let units = known_units();
    let r = report(30 * GB, &[("/h/src/tmp/t1", GB), ("/h/src/tmp/t2", GB)]);
    let ledger = known_ledger(200 * GB, &units, 30 * GB, NOW - 3 * 3600);
    let h = headline_of(&units, &r, &ledger, ScopeKind::Current);
    let j = h.to_json();
    let text = h.render_text(NOW);
    assert_eq!(j["line"].as_str().unwrap(), text.lines().next().unwrap());
    assert_eq!(j["developer_bytes"], 60 * GB);
    assert_eq!(j["percent_of_used"], 30.0);
    assert_eq!(j["disk"]["percent_of_used_tenths"], 300);
    let sum: u64 = j["categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["bytes"].as_u64().unwrap())
        .sum();
    assert_eq!(sum, j["developer_bytes"].as_u64().unwrap());
    for c in j["categories"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["count"].as_u64() > Some(0))
    {
        let label = c["label"].as_str().unwrap();
        let human = swamp_core::render::human_bytes_pub(c["bytes"].as_u64().unwrap());
        let line = text
            .lines()
            .find(|l| l.trim_start().starts_with(label))
            .unwrap();
        assert!(line.contains(&human), "{line} vs {human}");
    }
    // The text says the ages; the JSON keeps the times, so two reads of
    // one store are identical however far apart they run.
    assert!(text.contains("observed 4m ago; disk ledger measured 3h ago"));
    assert_eq!(j["measured_at"]["observed_at"], r.observed_at);
    assert_eq!(j["measured_at"]["ledger_measured_at"], NOW - 3 * 3600);
    assert!(j.get("ages").is_none());
    assert_eq!(h.to_json(), h.to_json());
}

/// Tempting wrong patch: sums use `+` on stored u64s, which panics in a
/// debug build (and wraps to nonsense in release) on a corrupt store.
#[test]
fn huge_stored_values_neither_panic_nor_wrap() {
    let units = vec![
        unit(
            "rustup",
            "rustup",
            StorageCategory::Installation,
            "/a",
            u64::MAX,
        ),
        unit("uv", "uv", StorageCategory::Cache, "/b", u64::MAX),
        unit(
            HOMEBREW_OTHER_DETECTOR_ID,
            "Homebrew (other)",
            StorageCategory::Installation,
            "/c",
            u64::MAX,
        ),
        unit("huggingface", "HF", StorageCategory::Models, "/d", u64::MAX),
    ];
    let mut r = report(u64::MAX, &[("/h/src/t", u64::MAX), ("/h/src/u", u64::MAX)]);
    r.reconciliation.attributed = u64::MAX;
    let rows = vec![lrow("/x", LedgerCategory::Other, Some(u64::MAX))];
    let ledger = reading(rows, meta(Some(u64::MAX), Some(u64::MAX), NOW - 60));
    let h = headline_of(&units, &r, &ledger, ScopeKind::Current);
    assert_eq!(h.developer_bytes, u64::MAX);
    assert!(
        h.flags.iter().any(|f| f.contains("64 bits")),
        "{:?}",
        h.flags
    );
    let _ = h.render_text(NOW);
    let _ = h.to_json();
    let facts = ManagerFacts::default();
    let view = build_reclaim(&ReclaimInput {
        units: &units,
        interiors: &[],
        unowned: &r.unowned,
        manager_facts: &facts,
        declared_roots: &[],
        explicit_scope: false,
        projects: 0,
        observed_at: r.observed_at,
    });
    let _ = relation(&h, &view.totals);
}

// ---------------------------------------------------------------------
// No work, no verdicts
// ---------------------------------------------------------------------

/// Tempting wrong patch: the headline lists a directory, stats a file or
/// asks a tool for a number "just to be current".
#[test]
fn building_and_rendering_the_headline_does_no_work() {
    let units = known_units();
    let r = report(30 * GB, &[("/h/src/tmp/t1", GB)]);
    let ledger = known_ledger(200 * GB, &units, 30 * GB, NOW - 60);
    let (_, work) = swamp_core::work_counters::measured(|| {
        let h = headline_of(&units, &r, &ledger, ScopeKind::Current);
        let _ = h.render_text(NOW);
        let _ = h.to_json();
        let _ = h.line_tiers();
    });
    assert_eq!(work, swamp_core::work_counters::WorkCounters::default());
    // Reading the stored ledger is two Parquet reads and nothing else.
    let dir = tempfile::tempdir().unwrap();
    write_volume_ledger(
        dir.path(),
        &[lrow("/x", LedgerCategory::Other, Some(5)).to_stored_for_test()],
        &meta(Some(GB), Some(GB), NOW - 60),
        None,
    )
    .unwrap();
    let (_, work) = swamp_core::work_counters::measured(|| read_reading(dir.path()));
    assert_eq!(work, swamp_core::work_counters::WorkCounters::default());
}

/// The words a report never says as a verdict, and the em dash, in every
/// state the headline can be in.
#[test]
fn no_state_of_the_headline_carries_a_verdict_word_or_an_em_dash() {
    let units = known_units();
    let r = report(30 * GB, &[("/h/src/tmp/t1", GB)]);
    let states = [
        headline_of(&units, &r, &LedgerReading::NotMeasured, ScopeKind::Current),
        headline_of(
            &units,
            &r,
            &LedgerReading::Unreadable("broken\nsecond line".into()),
            ScopeKind::Current,
        ),
        headline_of(
            &units,
            &r,
            &LedgerReading::Newer { unknown_rows: 2 },
            ScopeKind::Current,
        ),
        headline_of(
            &units,
            &r,
            &known_ledger(200 * GB, &units, 30 * GB, NOW - 5 * 86_400),
            ScopeKind::Previous { roots: 2 },
        ),
        headline_of(
            &units,
            &r,
            &known_ledger(50 * GB, &units, 30 * GB, NOW - 60),
            ScopeKind::Current,
        ),
        headline_of(
            &units,
            &r,
            &known_ledger(200 * GB, &units, 30 * GB, NOW - 60),
            ScopeKind::ExplicitRoot,
        ),
    ];
    for h in &states {
        let blob = format!("{}\n{}", h.render_text(NOW), h.to_json());
        let lower = blob.to_lowercase();
        for word in [
            "unused",
            "obsolete",
            "stale",
            "safe",
            "orphan",
            "candidate",
            "removable",
        ] {
            let bad = lower
                .split(|c: char| !c.is_ascii_alphabetic())
                .any(|w| w == word);
            assert!(!bad, "verdict word {word:?} in\n{blob}");
        }
        assert!(!blob.contains('\u{2014}'), "em dash in\n{blob}");
        for tier in h.line_tiers() {
            assert!(!tier.contains('\u{2014}'));
        }
    }
}

trait ToStored {
    fn to_stored_for_test(&self) -> swamp_core::growth::VolumeLedgerRow;
}

impl ToStored for Row {
    fn to_stored_for_test(&self) -> swamp_core::growth::VolumeLedgerRow {
        swamp_core::growth::VolumeLedgerRow {
            path: self.path.clone(),
            category: self.category.as_str().to_string(),
            allocated_bytes: self.bytes,
            overlap_bytes: self.overlap_bytes,
            entry_count: self.entries,
            unreadable_count: self.unreadable,
            measured_at: self.measured_at,
            method: self.method.clone(),
            exactness: self.exactness.as_str().to_string(),
            note: self.note.clone(),
        }
    }
}

/// Tempting wrong patch: the ledger keeps one row per path, and an agent
/// tool's home that is also a catalog unit's path (a bigger "view" than the
/// unit's net bytes) wins the path: the row is marked "counted elsewhere"
/// and the bytes are counted nowhere. The view folds into the unit's row;
/// the disk view's accounted bytes equal developer storage plus the
/// remainder, and when they do not, a plain line says so.
#[test]
fn an_agent_home_view_at_a_unit_path_does_not_swallow_the_unit() {
    let units = vec![unit(
        "codex",
        "Codex",
        StorageCategory::LocalState,
        "/h/.codex",
        7 * GB,
    )];
    let list = vec![
        Accounted {
            path: PathBuf::from("/h/.codex"),
            bytes: 7 * GB,
            category: LedgerCategory::Catalog,
            subset_of_enclosing: false,
            measured_at: NOW - 60,
            incomplete: false,
            note: None,
        },
        // The view is LARGER than the unit (the unit excludes a worktree).
        Accounted {
            path: PathBuf::from("/h/.codex"),
            bytes: 7 * GB + 120_000_000,
            category: LedgerCategory::Catalog,
            subset_of_enclosing: true,
            measured_at: NOW - 60,
            incomplete: false,
            note: Some("agent sessions, caches and logs".into()),
        },
    ];
    let rows = accounted_rows(&list, &[]);
    assert_eq!(rows.iter().filter(|r| r.path == "/h/.codex").count(), 1);
    let row = rows.iter().find(|r| r.path == "/h/.codex").unwrap();
    assert_eq!(row.bytes, Some(7 * GB));
    assert_eq!(row.overlap_bytes, 0, "the unit's bytes must be counted");
    assert!(row.note.as_deref().unwrap().contains("agent sessions"));
    let ledger = reading(rows, meta(Some(50 * GB), Some(40 * GB), NOW - 60));
    let h = headline_of(&units, &report(0, &[]), &ledger, ScopeKind::Current);
    let Disk::Measured(m) = &h.disk else { panic!() };
    assert!(m.accounted_check.agrees, "{:?}", m.accounted_check);
    assert!(h.accounted_sentence().is_none());
    // A disagreement is said in the text and the JSON.
    let other = reading(vec![], meta(Some(50 * GB), Some(40 * GB), NOW - 60));
    let h = headline_of(&units, &report(0, &[]), &other, ScopeKind::Current);
    let text = h.render_text(NOW);
    assert!(text.contains("disk view check: the ledger's accounted bytes (0B) differ from developer storage plus the remainder units (7.0GB) by -7.0GB"), "{text}");
    assert!(h.to_json()["accounted_check_line"].is_string());
}

/// Tempting wrong patch: a folder shared by many owners (~/Library/Caches,
/// category unclassified) is counted as developer storage with no word
/// about who else owns its files.
#[test]
fn a_mixed_owner_folder_is_counted_and_named_as_mixed() {
    let units = vec![
        unit(
            "builtin-defaults",
            "Built-in",
            StorageCategory::Unclassified,
            "/h/Library/Caches",
            36 * GB,
        ),
        unit("rustup", "rustup", StorageCategory::Installation, "/a", GB),
    ];
    let h = headline_of(
        &units,
        &report(0, &[]),
        &LedgerReading::NotMeasured,
        ScopeKind::Current,
    );
    assert_eq!(h.developer_bytes, 37 * GB);
    assert_eq!(h.mixed_owners.len(), 1);
    let text = h.render_text(NOW);
    assert!(
        text.contains("other developer units include 36.0GB in /h/Library/Caches: other, mixed owners (not only developer tools)"),
        "{text}"
    );
}

/// The one-line headline names projects and tool locations apart.
#[test]
fn the_first_line_counts_projects_and_tool_locations_separately() {
    let units = vec![unit(
        "rustup",
        "rustup",
        StorageCategory::Installation,
        "/a",
        GB,
    )];
    let mut r = report(2 * GB, &[]);
    r.projects = Vec::new();
    let h = headline_of(&units, &r, &LedgerReading::NotMeasured, ScopeKind::Current);
    assert!(
        h.line.ends_with("across 1 project and 1 tool location"),
        "{}",
        h.line
    );
    let h = headline_of(
        &[],
        &report(0, &[]),
        &LedgerReading::NotMeasured,
        ScopeKind::Current,
    );
    assert!(h.line.ends_with("across 0 tool locations"), "{}", h.line);
}

/// Adversarial audit (disk view check). Tempting wrong patch: blame every
/// disagreement on "a different observation". Here the ledger's accounted
/// rows were written from THIS observation (same `measured_at` as the
/// report), and the difference comes from a unit on another volume: the
/// ledger lists it apart (never added), the headline counts it. The
/// sentence must not name observation age as the cause.
#[test]
fn adv_disk_check_does_not_blame_observation_age_when_the_observation_is_the_same() {
    let r = report(30 * GB, &[]);
    let units = vec![unit(
        "uv-cache",
        "uv",
        StorageCategory::Cache,
        "/Volumes/ext/uv",
        7 * GB,
    )];
    let list = vec![
        Accounted {
            path: PathBuf::from("/h/src"),
            bytes: 30 * GB,
            category: LedgerCategory::Declared,
            subset_of_enclosing: false,
            measured_at: r.observed_at,
            incomplete: false,
            note: None,
        },
        Accounted {
            path: PathBuf::from("/Volumes/ext/uv"),
            bytes: 7 * GB,
            category: LedgerCategory::Catalog,
            subset_of_enclosing: false,
            measured_at: r.observed_at,
            incomplete: false,
            note: None,
        },
    ];
    let mounts = vec![MountView {
        path: PathBuf::from("/Volumes/ext"),
        kind: MountKind::Unknown,
        used: None,
    }];
    let rows = accounted_rows(&list, &mounts);
    let ledger = reading(rows, meta(Some(200 * GB), Some(190 * GB), r.observed_at));
    let h = headline_of(&units, &r, &ledger, ScopeKind::Current);
    let Disk::Measured(m) = &h.disk else { panic!() };
    eprintln!("difference = {}", m.accounted_check.difference);
    if let Some(s) = h.accounted_sentence() {
        assert!(
            !s.contains("different observation"),
            "same observation, cause is another volume, sentence says: {s}"
        );
    }
}

#[test]
fn disk_check_names_the_other_volume_and_blames_age_only_when_the_times_differ() {
    let r = report(30 * GB, &[]);
    let units = vec![unit(
        "uv-cache",
        "uv",
        StorageCategory::Cache,
        "/Volumes/ext/uv",
        7 * GB,
    )];
    let list = vec![
        Accounted {
            path: PathBuf::from("/h/src"),
            bytes: 30 * GB,
            category: LedgerCategory::Declared,
            subset_of_enclosing: false,
            measured_at: r.observed_at,
            incomplete: false,
            note: None,
        },
        Accounted {
            path: PathBuf::from("/Volumes/ext/uv"),
            bytes: 7 * GB,
            category: LedgerCategory::Catalog,
            subset_of_enclosing: false,
            measured_at: r.observed_at,
            incomplete: false,
            note: None,
        },
    ];
    let mounts = vec![MountView {
        path: PathBuf::from("/Volumes/ext"),
        kind: MountKind::Unknown,
        used: None,
    }];
    let rows = accounted_rows(&list, &mounts);
    let ledger = reading(
        rows.clone(),
        meta(Some(200 * GB), Some(190 * GB), r.observed_at),
    );
    let h = headline_of(&units, &r, &ledger, ScopeKind::Current);
    let s = h
        .accounted_sentence()
        .expect("a 7GB disagreement is stated");
    // Tempting wrong patch: the fixed "may come from a different
    // observation" clause, or no named part at all.
    assert!(s.contains("7.0GB is on another volume"), "{s}");
    assert!(!s.contains("different time"), "{s}");
    // The same ledger measured at another time says so.
    let late = reading(
        rows,
        meta(Some(200 * GB), Some(190 * GB), r.observed_at + 60),
    );
    let h2 = headline_of(&units, &r, &late, ScopeKind::Current);
    let s2 = h2.accounted_sentence().unwrap();
    assert!(s2.contains("different time"), "{s2}");
}
