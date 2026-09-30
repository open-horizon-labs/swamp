//! v0.8.0 G3 (#169, #170) through the built binary: `report --view disk`
//! and the `disk` object are pure reads of the stored ledger, and
//! `observe` runs the volume pass only where it is valid.
//!
//! Every test uses a temp store and a temp `HOME`; none runs the pass
//! against the developer's real disk.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use swamp_core::growth::{VolumeLedgerRow, VolumeMetaRow, write_volume_ledger};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_swamp"))
}

fn swamp(store: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .env("SWAMP_DIR", store)
        .env("HOME", home)
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("run swamp")
}

fn row(
    path: &str,
    category: &str,
    bytes: Option<u64>,
    age: u64,
    exactness: &str,
) -> VolumeLedgerRow {
    VolumeLedgerRow {
        path: path.into(),
        category: category.into(),
        allocated_bytes: bytes,
        overlap_bytes: 0,
        entry_count: None,
        unreadable_count: 0,
        measured_at: swamp_core::entities::now() - age,
        method: "walk: allocated bytes, lstat only".into(),
        exactness: exactness.into(),
        note: None,
    }
}

fn write_fixture_ledger(store: &Path) {
    let rows = vec![
        row(
            "/Users/x/.cargo",
            "catalog",
            Some(3_000_000_000),
            60,
            "exact",
        ),
        row(
            "/Users/x/src",
            "declared",
            Some(14_000_000_000),
            60,
            "exact",
        ),
        row(
            "/Users/x/Movies",
            "other",
            Some(9_000_000_000),
            3 * 3600,
            "exact",
        ),
        row(
            "/Users/x/Pictures",
            "unreadable",
            None,
            3 * 3600,
            "not_measured",
        ),
        row(
            "APFS volume Preboot (disk3s2)",
            "system",
            Some(10_000_000_000),
            60,
            "exact",
        ),
    ];
    let meta = VolumeMetaRow {
        measured_at: swamp_core::entities::now() - 3 * 3600,
        cycle_started_at: 1,
        cycle_complete_at: swamp_core::entities::now() - 3 * 3600,
        complete: true,
        budget_secs: 120,
        budget_used_ms: 4_000,
        statfs_at: swamp_core::entities::now() - 3 * 3600,
        container_total: Some(500_000_000_000),
        container_used: Some(40_000_000_000),
        container_free: Some(460_000_000_000),
        data_volume_used: Some(30_000_000_000),
    };
    write_volume_ledger(store, &rows, &meta, None).unwrap();
}

#[test]
fn view_disk_without_a_ledger_says_so_and_is_not_an_error() {
    // Tempting wrong patch: exit 2 with `no_observation` (the ledger is
    // separate from the observation), or measuring on demand.
    let store = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let out = swamp(store.path(), home.path(), &["report", "--view", "disk"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("not measured yet; run `swamp observe --volume`"),
        "{text}"
    );
    let out = swamp(
        store.path(),
        home.path(),
        &["report", "--view", "disk", "--json"],
    );
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["view"], "disk");
    assert_eq!(v["result"]["measured"], false);
    assert!(
        std::fs::read_dir(store.path()).unwrap().next().is_none(),
        "a read writes nothing"
    );
}

#[test]
fn view_disk_reads_the_stored_ledger_with_ages_and_the_named_residual() {
    let store = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    write_fixture_ledger(store.path());
    let out = swamp(store.path(), home.path(), &["report", "--view", "disk"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    for needle in [
        "Disk: ",
        "measured 3 h ago",
        "Accounted (catalog and declared locations, counted once)",
        "Everything else (measured, 1 folders)",
        "/Users/x/Movies",
        "System volumes:",
        "Preboot",
        "Not measured: 1 folders could not be read",
        "/Users/x/Pictures",
        "Full Disk Access",
        "Unattributed: allocation not explained by any measured part",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in\n{text}");
    }
    let out = swamp(
        store.path(),
        home.path(),
        &["report", "--view", "disk", "--json"],
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let r = &v["result"];
    assert_eq!(r["measured"], true);
    assert_eq!(r["accounted"]["bytes"], 17_000_000_000u64);
    assert_eq!(r["system_volumes"]["bytes"], 10_000_000_000u64);
    assert_eq!(r["not_measured"]["count"], 1);
    // 40 GB used = 17 + 9 + 10 + estimate (30 - 26 = 4) + residual 0.
    assert_eq!(r["not_measured"]["estimate_bytes"], 4_000_000_000u64);
    assert_eq!(r["residual"]["bytes"], 0);
    assert_eq!(r["residual"]["bookkeeping_balanced"], true);
    assert_eq!(
        r["residual"]["within_one_percent"], false,
        "the measured parts alone do not explain the Data volume"
    );
}

#[test]
fn the_full_report_json_carries_a_disk_object_and_reading_it_runs_nothing() {
    let store = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("f"), b"x").unwrap();
    let out = swamp(
        store.path(),
        home.path(),
        &["observe", root.to_str().unwrap()],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("volume pass"),
        "an explicit-root observe must not run (or announce) the pass: {stdout}"
    );
    let out = swamp(
        store.path(),
        home.path(),
        &["report", root.to_str().unwrap(), "--json"],
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["disk"]["measured"], false,
        "no ledger yet: said, not an error"
    );
    write_fixture_ledger(store.path());
    let out = swamp(
        store.path(),
        home.path(),
        &["report", root.to_str().unwrap(), "--json"],
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["disk"]["measured"], true);
    assert_eq!(v["disk"]["container"]["used"], 40_000_000_000u64);
}

#[test]
fn observe_with_explicit_roots_refuses_the_pass_even_when_forced() {
    // Tempting wrong patch: running the pass under explicit roots, whose
    // scope replaces the configured one: the ledger would call one folder
    // "accounted" and nearly the whole disk "everything else".
    let store = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    let out = swamp(
        store.path(),
        home.path(),
        &["observe", "--volume", root.to_str().unwrap()],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("volume pass skipped: explicit roots replace the configured scope"),
        "{stdout}"
    );
    assert!(!store.path().join("volume_ledger.parquet").exists());
}

#[test]
fn a_plain_observe_under_a_sandboxed_home_does_not_measure_the_real_disk() {
    // Tempting wrong patch: running the automatic pass whenever the ledger
    // is absent, which would walk the real `/` under every test and
    // fixture that observes with a temp `$HOME`.
    let store = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("src/p")).unwrap();
    let out = swamp(store.path(), home.path(), &["observe"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains("volume pass"));
    assert!(!store.path().join("volume_ledger.parquet").exists());
    assert!(!store.path().join("volume_ledger_meta.parquet").exists());
}
