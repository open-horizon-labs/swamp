//! v0.8.0 G4a verification (audit/v080-g4ab): manager fact ages.
//! hand, manager answers come from a fake runner, and nothing here reads
//! the developer's real store, brew, mise or rustup.
//!
//! Each test names the tempting wrong patch it fails.

use std::path::PathBuf;

use swamp_core::drilldown::{ChildKind, ChildMeasure, UnitChild};
use swamp_core::external::ExternalUnit;
use swamp_core::last_used::LastUsed;
use swamp_core::locations::{Provenance, StorageCategory};
use swamp_core::manager_facts::{FactKind, ManagerFact, ManagerFacts};
use swamp_core::reclaim::{ReclaimInput, ReclaimView, build, render_text};
use swamp_core::report::UnownedRow;
use swamp_core::roots::{DeclaredRoot, DeclaredState};

const NOW: u64 = 1_790_000_000;
const GB: u64 = 1_000_000_000;
fn unit(detector: &str, category: StorageCategory, path: &str, bytes: u64) -> ExternalUnit {
    ExternalUnit {
        detector_id: detector.to_string(),
        detector_name: format!("{detector} (fixture)"),
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

fn entry(name: &str, bytes: i64) -> UnitChild {
    UnitChild {
        kind: ChildKind::Entry,
        name: name.to_string(),
        bytes: Some(bytes),
        measure: ChildMeasure::Complete,
        mtime_max: 0,
        entries: 0,
        not_measured: 0,
        last_used: LastUsed::default(),
    }
}

fn present_root(path: &str) -> DeclaredRoot {
    DeclaredRoot {
        path: PathBuf::from(path),
        state: DeclaredState::Present {
            bytes: Some(1),
            complete: true,
        },
    }
}

fn fact(
    manager: &str,
    probe: &str,
    kind: FactKind,
    subject: Option<&str>,
    text: &str,
) -> ManagerFact {
    ManagerFact {
        manager: manager.to_string(),
        probe: probe.to_string(),
        kind,
        subject: subject.map(str::to_string),
        text: text.to_string(),
        observed_at: NOW,
    }
}

fn pass() -> ManagerFact {
    fact("", "", FactKind::Pass, None, "")
}

fn facts(rows: Vec<ManagerFact>) -> ManagerFacts {
    ManagerFacts {
        observed: rows.iter().any(|f| f.kind == FactKind::Pass),
        facts: rows,
    }
}

fn view_of(
    units: &[ExternalUnit],
    unowned: &[UnownedRow],
    mf: &ManagerFacts,
    roots: &[DeclaredRoot],
) -> ReclaimView {
    build(&ReclaimInput {
        units,
        interiors: &[],
        unowned,
        manager_facts: mf,
        declared_roots: roots,
        explicit_scope: false,
        projects: 3,
        observed_at: NOW,
    })
}

fn row<'a>(v: &'a ReclaimView, path: &str) -> &'a swamp_core::reclaim::ReclaimRow {
    v.rows
        .iter()
        .find(|r| r.path == path)
        .unwrap_or_else(|| panic!("no row for {path}"))
}

fn rustup_units() -> Vec<ExternalUnit> {
    let mut t = unit(
        "rustup",
        StorageCategory::Installation,
        "/h/.rustup/toolchains",
        3 * GB,
    );
    t.children = vec![
        entry("stable-aarch64-apple-darwin", 2 * GB as i64),
        entry("nightly-aarch64-apple-darwin", GB as i64),
    ];
    vec![
        unit("rustup", StorageCategory::LocalState, "/h/.rustup", 1_000),
        t,
    ]
}

fn rustup_default(name: &str) -> ManagerFacts {
    facts(vec![
        pass(),
        fact(
            "rustup",
            "settings-default",
            FactKind::ActiveDefault,
            Some(name),
            &format!("default_toolchain \"{name}\" in settings.toml"),
        ),
        fact("rustup", "settings-default", FactKind::Checked, None, ""),
    ])
}

/// The tempting wrong patch: the toolchains unit is one regenerable row,
/// so the default toolchain's bytes are counted as reclaimable, or the
/// default is matched only by exact directory name (`stable` never equals
/// `stable-aarch64-apple-darwin`).

fn aged(mut mf: ManagerFacts, secs: i64) -> ManagerFacts {
    for f in &mut mf.facts {
        f.observed_at = (NOW as i64 - secs) as u64;
    }
    mf
}

/// Tempting wrong patch (the fix round's): age only the quote lines and
/// keep trusting a hold from an older pass. A default read 30 days before
/// the units may no longer be the default (the person switched to
/// nightly since), so the new default is presented as regenerable. The
/// review asked for an older hold to fail closed (unknown) or at least say
/// it is older.
#[test]
fn ver_a_hold_from_an_older_pass_fails_closed_or_says_its_age() {
    let units = rustup_units();
    let v = view_of(
        &units,
        &[],
        &aged(rustup_default("stable"), 30 * 86_400),
        &[present_root("/h/src")],
    );
    let r = row(&v, "/h/.rustup/toolchains");
    let whole_unknown = r.held_bytes == 3 * GB;
    let says_age = r.children[0]
        .hold
        .as_ref()
        .is_some_and(|h| h.label.contains("older") || h.label.contains("days"));
    assert!(
        whole_unknown || says_age,
        "a 30-day-old default holds as current: held={} hold={:?}\n{}",
        r.held_bytes,
        r.children[0].hold,
        render_text(&v)
    );
}

/// A quote two hours older than the listing is "older", and the text
/// must not say "quoted 0 days before this listing".
#[test]
fn ver_a_quote_hours_old_does_not_read_zero_days() {
    let mut u = unit(
        "mise",
        StorageCategory::Installation,
        "/h/.local/share/mise/installs",
        GB,
    );
    u.children = vec![entry("poetry", GB as i64)];
    let mk = |kind, subject: Option<&str>, text: &str, probe: &str| ManagerFact {
        manager: "mise".into(),
        probe: probe.into(),
        kind,
        subject: subject.map(str::to_string),
        text: text.into(),
        observed_at: NOW - 2 * 3_600,
    };
    let mf = facts(vec![
        pass(),
        mk(
            FactKind::ReportsPrunable,
            Some("poetry@2.1.3"),
            "mise poetry@2.1.3 is prunable: x",
            "prune-dry-run",
        ),
        mk(FactKind::Checked, None, "", "prune-dry-run"),
        mk(FactKind::Checked, None, "", "global-tools"),
    ]);
    let text = render_text(&view_of(&[u], &[], &mf, &[present_root("/h/src")]));
    assert!(!text.contains("quoted 0 days"), "{text}");
}
