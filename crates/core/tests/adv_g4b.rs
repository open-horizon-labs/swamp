#![allow(dead_code, unused_imports)]
//! Adversarial audit of G4b (#206): the headline. Fixture-only:
//! every store and report here is built by hand or in a temp directory;
//! nothing reads the developer's real store or disk.
//!
//! Each test names the tempting wrong patch it fails.

use std::path::PathBuf;

use swamp_core::external::ExternalUnit;
use swamp_core::growth::{VolumeMetaRow, write_volume_ledger};
use swamp_core::headline::{
    Category, Disk, Headline, Input, ScopeKind, build, category_of_unit, group_digits, relation,
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

fn rows_sum(h: &Headline) -> u128 {
    h.categories.iter().map(|c| c.bytes as u128).sum()
}

/// Tempting wrong patch: a previous-scope report divides the old scope's
/// developer storage by today's disk and prints a percent anyway. The
/// brief: previous scope degrades with a clear line and no percent.
#[test]
fn adv_previous_scope_has_no_percent() {
    let units = known_units();
    let r = report(30 * GB, &[]);
    let ledger = known_ledger(200 * GB, &units, 30 * GB, NOW - 60);
    let h = headline_of(&units, &r, &ledger, ScopeKind::Previous { roots: 3 });
    assert!(
        !h.line.contains("% of used"),
        "previous scope prints a percent: {}",
        h.line
    );
    assert!(
        h.to_json()["percent_of_used"].is_null(),
        "{}",
        h.to_json()["percent_of_used"]
    );
}

/// Tempting wrong patch: on overflow each row and the total are capped
/// separately, and the text still says the rows add up to the figure.
#[test]
fn adv_capped_totals_never_claim_the_rows_add_up() {
    let units = vec![
        unit(
            "rustup",
            "rustup",
            StorageCategory::Installation,
            "/a",
            u64::MAX / 2 + 1,
        ),
        unit("uv", "uv", StorageCategory::Cache, "/b", u64::MAX / 2 + 1),
    ];
    let r = report(0, &[]);
    let h = headline_of(&units, &r, &LedgerReading::NotMeasured, ScopeKind::Current);
    let text = h.render_text(NOW);
    let claims = text.contains("the rows add up to");
    assert!(
        !claims || rows_sum(&h) == h.developer_bytes as u128,
        "rows sum {} but developer {} and text says:\n{text}",
        rows_sum(&h),
        h.developer_bytes
    );
}

/// Tempting wrong patch: the "more than used" FLAG is pushed into `flags`
/// and also returned by `disk_state_sentence`, so text prints it twice.
#[test]
fn adv_exceeds_used_flag_is_printed_once() {
    let units = known_units();
    let r = report(300 * GB, &[]);
    let ledger = reading(vec![], meta(Some(100 * GB), Some(90 * GB), NOW - 60));
    let h = headline_of(&units, &r, &ledger, ScopeKind::Current);
    assert!(!h.line.contains('%'), "{}", h.line);
    let text = h.render_text(NOW);
    let n = text
        .lines()
        .filter(|l| l.contains("FLAG") && l.contains("more than the disk's used bytes"))
        .count();
    assert_eq!(n, 1, "the exceeds-used flag is printed {n} times:\n{text}");
}

/// Tempting wrong patch: only a ledger older than the observation is
/// labelled; an observation days older than the ledger (mixed ages the
/// other way) divides old developer storage by a fresh disk silently.
#[test]
fn adv_observation_much_older_than_ledger_is_labelled() {
    let units = known_units();
    let mut r = report(30 * GB, &[]);
    r.observed_at = NOW - 10 * 86_400;
    let ledger = known_ledger(200 * GB, &units, 30 * GB, NOW - 60);
    let h = headline_of(&units, &r, &ledger, ScopeKind::Current);
    let ages = h.ages_sentence(NOW);
    assert!(
        h.line.contains("% of used") && ages.contains("older"),
        "no label for a 10 d older observation: line={} ages={ages}",
        h.line
    );
}

/// Property: over random units, the category rows add to the headline
/// exactly, text and JSON carry the same bytes, the percent is floor.
#[test]
fn adv_property_rows_sum_text_json_agree() {
    let kinds = [
        ("rustup", StorageCategory::Installation),
        ("uv", StorageCategory::Cache),
        ("claude-code", StorageCategory::LocalState),
        ("docker-desktop", StorageCategory::LocalState),
        ("huggingface", StorageCategory::Models),
        (HOMEBREW_OTHER_DETECTOR_ID, StorageCategory::Installation),
        ("no-such-detector", StorageCategory::Unclassified),
        ("npm", StorageCategory::Downloads),
    ];
    let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut rnd = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    for _ in 0..2000 {
        let n = (rnd() % 12) as usize;
        let units: Vec<ExternalUnit> = (0..n)
            .map(|i| {
                let (d, c) = kinds[(rnd() % kinds.len() as u64) as usize];
                unit(d, d, c, &format!("/u/{i}"), rnd() % (3 * GB) + rnd() % 1000)
            })
            .collect();
        let targets: Vec<(String, u64)> = (0..(rnd() % 3))
            .map(|i| (format!("/h/src/t{i}"), rnd() % GB))
            .collect();
        let tref: Vec<(&str, u64)> = targets.iter().map(|(p, b)| (p.as_str(), *b)).collect();
        let tsum: u64 = targets.iter().map(|t| t.1).sum();
        let r = report(tsum + rnd() % (50 * GB), &tref);
        let used = rnd() % (400 * GB) + 1;
        let ledger = reading(vec![], meta(Some(used), Some(used), NOW - 60));
        let h = headline_of(&units, &r, &ledger, ScopeKind::Current);
        assert_eq!(rows_sum(&h), h.developer_bytes as u128);
        let j = h.to_json();
        assert_eq!(j["developer_bytes"].as_u64(), Some(h.developer_bytes));
        let text = h.render_text(NOW);
        assert!(
            text.contains(
                "totals use exact stored bytes; displayed rows are rounded independently"
            ),
            "{text}"
        );
        let jrows: u64 = j["categories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["bytes"].as_u64().unwrap())
            .sum();
        assert_eq!(jrows, h.developer_bytes);
        if let Some(p) = j["percent_of_used"].as_f64() {
            let floor = (h.developer_bytes as u128 * 1000 / used as u128) as f64 / 10.0;
            assert_eq!(p, floor);
            assert!(
                h.line.contains(&format!(
                    "{}% of used",
                    swamp_core::headline::tenths_text((floor * 10.0).round() as u32)
                )),
                "{}",
                h.line
            );
            assert!(h.developer_bytes <= used);
        } else {
            assert!(
                h.developer_bytes > used,
                "no percent though dev {} <= used {used}",
                h.developer_bytes
            );
        }
    }
}

/// Tempting wrong patch: a hostile ledger (measured_at huge but not in the
/// future relative to a hostile clock) overflows `measured_at + 1 day`.
#[test]
fn adv_hostile_timestamps_do_not_panic() {
    let units = known_units();
    let mut r = report(30 * GB, &[]);
    for (obs, at, now) in [
        (u64::MAX, u64::MAX - 10, u64::MAX),
        (u64::MAX, 0, 0),
        (0, u64::MAX, u64::MAX),
        (1, u64::MAX - 100, u64::MAX - 1000),
    ] {
        r.observed_at = obs;
        let ledger = reading(vec![], meta(Some(200 * GB), Some(GB), at));
        let h = build(&Input {
            units: &units,
            report: &r,
            ledger: &ledger,
            scope: ScopeKind::Current,
            observed_at: obs,
            now,
        });
        let _ = h.render_text(now);
        let _ = h.ages_sentence(now);
        let _ = h.to_json();
    }
}
