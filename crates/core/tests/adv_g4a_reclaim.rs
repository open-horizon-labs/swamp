//! Adversarial tests for v0.8.0 G4a (#175), written by the auditor.
//! Fixture-only: nothing reads the real store or runs a real manager.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use swamp_core::drilldown::{ChildKind, ChildMeasure, UnitChild, rows_total};
use swamp_core::evidence::{
    Evidence, EvidenceSource, FactKind as EvKind, FactSubtype, FactValue, Reason,
};
use swamp_core::external::ExternalUnit;
use swamp_core::fs_gate::spawn::{Program, RunOutput};
use swamp_core::last_used::LastUsed;
use swamp_core::locations::{Provenance, RegenClass, StorageCategory};
use swamp_core::manager_facts::{
    self, FactKind, ManagerFact, ManagerFacts, ProbeRunner, collect_within,
};
use swamp_core::reclaim::{
    HoldKind, KIND_STANDALONE_CARGO_TARGET, ReclaimInput, ReclaimView, RemovalKind, build,
    render_text, scope_statement,
};
use swamp_core::report::{UnownedReason, UnownedRow};
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

fn remainder(bytes: i64, entries: u32) -> UnitChild {
    UnitChild {
        kind: ChildKind::Remainder,
        name: String::new(),
        bytes: Some(bytes),
        measure: ChildMeasure::Complete,
        mtime_max: 0,
        entries,
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

fn toolchains(children: &[(&str, u64)]) -> Vec<ExternalUnit> {
    let mut t = unit(
        "rustup",
        StorageCategory::Installation,
        "/h/.rustup/toolchains",
        children.iter().map(|c| c.1).sum(),
    );
    t.children = children.iter().map(|(n, b)| entry(n, *b as i64)).collect();
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

/// Tempting wrong patch (the PR head): a channel is the text before the
/// first '-', so a dated channel (`nightly-2024-01-01`) never matches its
/// folder `nightly-2024-01-01-aarch64-apple-darwin`, and the default
/// toolchain is counted as regenerable.
#[test]
fn adv_a_dated_nightly_default_toolchain_is_held_out() {
    let units = toolchains(&[
        ("stable-aarch64-apple-darwin", 2 * GB),
        ("nightly-2024-01-01-aarch64-apple-darwin", GB),
    ]);
    let v = view_of(
        &units,
        &[],
        &rustup_default("nightly-2024-01-01"),
        &[present_root("/h/src")],
    );
    let r = row(&v, "/h/.rustup/toolchains");
    assert!(
        r.children[1].hold.is_some() || r.hold.is_some(),
        "the default toolchain nightly-2024-01-01 is not held: held_bytes={} regenerable={}",
        r.held_bytes,
        r.regenerable_bytes
    );
    assert!(r.held_bytes >= GB, "held_bytes={}", r.held_bytes);
}

/// Tempting wrong patch: a version-number default (`1.80.0`) is matched
/// by the text before the first '-' only by luck; a host-less toolchain
/// with a custom name (`my-linked`) must also be held when it is the
/// default, never counted.
#[test]
fn adv_a_custom_named_default_toolchain_with_a_dash_is_held_out() {
    let units = toolchains(&[("stable-aarch64-apple-darwin", 2 * GB), ("my-linked", GB)]);
    let v = view_of(
        &units,
        &[],
        &rustup_default("my-linked"),
        &[present_root("/h/src")],
    );
    let r = row(&v, "/h/.rustup/toolchains");
    assert!(r.held_bytes >= GB, "held_bytes={}", r.held_bytes);
}

/// A default that names no folder listed (the toolchain folder was never
/// listed, or the name form is one swamp cannot map) must hold, never
/// silently vanish. Unknown is held out.
#[test]
fn adv_a_default_that_matches_no_listed_folder_is_not_silently_dropped() {
    let units = toolchains(&[
        ("stable-aarch64-apple-darwin", 2 * GB),
        ("beta-aarch64-apple-darwin", GB),
    ]);
    let v = view_of(
        &units,
        &[],
        &rustup_default("stable-x86_64-apple-darwin"),
        &[present_root("/h/src")],
    );
    let r = row(&v, "/h/.rustup/toolchains");
    let text = render_text(&v);
    assert!(
        text.contains("stable-x86_64-apple-darwin"),
        "the default the settings name is not mentioned anywhere:\n{text}"
    );
}

fn mise_installs(children: &[(&str, u64)]) -> ExternalUnit {
    let mut u = unit(
        "mise",
        StorageCategory::Installation,
        "/h/.local/share/mise/installs",
        children.iter().map(|c| c.1).sum(),
    );
    u.children = children.iter().map(|(n, b)| entry(n, *b as i64)).collect();
    u
}

fn mise_global(tool: &str) -> ManagerFacts {
    facts(vec![
        pass(),
        fact(
            "mise",
            "global-tools",
            FactKind::ActiveDefault,
            Some(tool),
            "listed in the global configuration /h/.config/mise/config.toml",
        ),
        fact("mise", "global-tools", FactKind::Checked, None, ""),
        fact("mise", "prune-dry-run", FactKind::Checked, None, ""),
    ])
}

/// Tempting wrong patch (the PR head): the global tool name joins the
/// install folder by string equality. mise backends name tools
/// `npm:prettier`, `cargo:ripgrep`, `aqua:cli/cli`; their install folders
/// are `npm-prettier`, `cargo-ripgrep`, `aqua-cli-cli`. A global backend
/// tool must be held, not counted as regenerable.
#[test]
fn adv_a_mise_global_backend_tool_is_held_out() {
    for (tool, folder) in [
        ("npm:prettier", "npm-prettier"),
        ("cargo:ripgrep", "cargo-ripgrep"),
        ("aqua:cli/cli", "aqua-cli-cli"),
    ] {
        let u = mise_installs(&[("node", GB), (folder, 2 * GB)]);
        let v = view_of(&[u], &[], &mise_global(tool), &[present_root("/h/src")]);
        let r = row(&v, "/h/.local/share/mise/installs");
        assert!(
            r.held_bytes >= 2 * GB,
            "{tool} (folder {folder}) is a global tool but held_bytes={} regenerable={}",
            r.held_bytes,
            r.regenerable_bytes
        );
    }
}

/// Tempting wrong patch (the PR head): Homebrew prints a tap formula by
/// its full name (`user/tap/foo`, the same `full_name` its autoremove
/// uses), while the Cellar folder is `foo`. An on-request tap formula
/// must still be held.
#[test]
fn adv_a_tap_qualified_on_request_formula_is_held_out() {
    let foo = unit(
        "homebrew-devtools",
        StorageCategory::Installation,
        "/opt/homebrew/Cellar/foo",
        GB,
    );
    let mf = facts(vec![
        pass(),
        fact(
            "brew",
            "installed-on-request",
            FactKind::InstalledOnRequest,
            Some("user/tap/foo"),
            "installed on request",
        ),
        fact("brew", "installed-on-request", FactKind::Checked, None, ""),
        fact("brew", "autoremove-dry-run", FactKind::Checked, None, ""),
    ]);
    let v = view_of(&[foo], &[], &mf, &[present_root("/h/src")]);
    let r = row(&v, "/opt/homebrew/Cellar/foo");
    assert_eq!(r.held_bytes, GB, "hold={:?}", r.hold);
}

/// Real Homebrew prints its dry-run header through `oh1`, which prefixes
/// `==> ` (Library/Homebrew/cleanup.rb `oh1 "#{verb} ... unneeded ..."`,
/// utils/formatter.rb `arrow` -> `prefix("==>", ...)`). The tempting wrong
/// patch (the PR head) matches only the bare sentence, so every real run
/// is "could not be read".
#[test]
fn adv_brew_autoremove_parses_the_real_oh1_header() {
    let real = b"==> Would autoremove 2 unneeded formulae:\nlibevent\nunbound\n";
    let got = manager_facts::parse_brew_autoremove(real);
    assert!(got.is_ok(), "real brew output refused: {got:?}");
    assert_eq!(got.unwrap().1, vec!["libevent", "unbound"]);
}

/// Tap formulae come back as full names from autoremove too; they must
/// parse (they do: '/' is allowed) and must join their Cellar folder.
#[test]
fn adv_brew_autoremove_crlf_and_trailing_warnings() {
    let crlf = b"Would autoremove 1 unneeded formula:\r\nlibevent\r\n";
    assert_eq!(
        manager_facts::parse_brew_autoremove(crlf).unwrap().1,
        vec!["libevent"]
    );
    let after = b"Would autoremove 1 unneeded formula:\nlibevent\n\nWarning: something\n";
    assert_eq!(
        manager_facts::parse_brew_autoremove(after).unwrap().1,
        vec!["libevent"]
    );
}

/// A manager statement observed long ago must show its age: the
/// tempting wrong patch (the PR head) drops `observed_at` from the
/// quote, so a year-old verdict reads exactly like today's.
#[test]
fn adv_an_old_manager_statement_shows_its_age() {
    let mk = |at: u64| {
        let mut f = fact(
            "mise",
            "prune-dry-run",
            FactKind::ReportsPrunable,
            Some("poetry@2.1.3"),
            "mise poetry@2.1.3 is prunable: no tracked config",
        );
        f.observed_at = at;
        let mut checked = fact("mise", "prune-dry-run", FactKind::Checked, None, "");
        checked.observed_at = at;
        let mut g = fact("mise", "global-tools", FactKind::Checked, None, "");
        g.observed_at = at;
        let mut p = pass();
        p.observed_at = at;
        facts(vec![p, f, checked, g])
    };
    let u = || mise_installs(&[("poetry", GB)]);
    let fresh = render_text(&view_of(&[u()], &[], &mk(NOW), &[present_root("/h/src")]));
    let old = render_text(&view_of(
        &[u()],
        &[],
        &mk(NOW - 400 * 86_400),
        &[present_root("/h/src")],
    ));
    assert!(fresh.contains("is prunable"), "{fresh}");
    assert_ne!(
        fresh, old,
        "a 400-day-old statement renders identically to a fresh one"
    );
}

/// Every rendered string, text and JSON, over the fixtures above, carries
/// no verdict word and no em dash.
#[test]
fn adv_no_verdict_words_in_holds_and_notes() {
    let units = toolchains(&[("stable-aarch64-apple-darwin", GB)]);
    let v = view_of(&units, &[], &ManagerFacts::default(), &[]);
    let text = render_text(&v).to_lowercase();
    let json = serde_json::to_string(&v).unwrap().to_lowercase();
    for w in [
        "unused",
        "stale",
        "orphan",
        "removable",
        "candidate",
        "safe",
        "obsolete",
        "\u{2014}",
    ] {
        assert!(!text.contains(w), "{w} in text:\n{text}");
        assert!(!json.contains(w), "{w} in json");
    }
}
