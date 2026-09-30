//! v0.8.0 G4a: the Reclaim view (#175). Fixture-only: units are built by
//! hand, manager answers come from a fake runner, and nothing here reads
//! the developer's real store, brew, mise or rustup.
//!
//! Each test names the tempting wrong patch it fails.

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

fn fact(manager: &str, probe: &str, kind: FactKind, subject: Option<&str>, text: &str) -> ManagerFact {
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

fn standalone(path: &str, bytes: u64) -> UnownedRow {
    UnownedRow {
        measurement: None,
        path_or_object: path.to_string(),
        bytes,
        reason: UnownedReason::StandaloneCargoTarget,
        shared_bytes: None,
        note: Some("standalone Cargo target".to_string()),
        docker_kind: None,
        created_at: None,
        containers: Vec::new(),
        shared_with: Vec::new(),
        dangling: false,
        evidence: Vec::new(),
    }
}

// ---------------------------------------------------------------------
// Consumer evidence and its scope
// ---------------------------------------------------------------------

/// The tempting wrong patch: "no declared consumer among the checked
/// projects" is rendered as "no consumers" / "needs no consumer". A tool
/// used only by a project outside the declared roots looks exactly like
/// that, so the row states what it found and where it looked, and the
/// listing says the evidence is incomplete when a declared root could not
/// be read.
#[test]
fn a_tool_used_only_outside_the_declared_roots_is_never_presented_as_needing_no_consumer() {
    let toolchains = unit(
        "rustup",
        StorageCategory::Installation,
        "/h/.rustup/toolchains",
        5 * GB,
    );
    for roots in [
        vec![DeclaredRoot {
            path: PathBuf::from("/Volumes/work/src"),
            state: DeclaredState::Missing,
        }],
        vec![DeclaredRoot {
            path: PathBuf::from("/Volumes/work/src"),
            state: DeclaredState::Unreadable {
                reason: "denied".into(),
            },
        }],
        vec![
            present_root("/h/src"),
            DeclaredRoot {
                path: PathBuf::from("/Volumes/work/src"),
                state: DeclaredState::Missing,
            },
        ],
    ] {
        let v = view_of(
            std::slice::from_ref(&toolchains),
            &[],
            &facts(vec![pass()]),
            &roots,
        );
        let r = row(&v, "/h/.rustup/toolchains");
        assert!(!v.scope.complete, "{roots:?}");
        assert!(r.consumers.declared.is_empty());
        assert!(
            r.consumers.summary.contains("none found among 3 projects"),
            "{}",
            r.consumers.summary
        );
        assert!(r.consumers.summary.contains("(incomplete)"));
        assert!(v.scope.statement.contains("incomplete"));
        assert!(v.scope.statement.contains("/Volumes/work/src"));
        let text = render_text(&v);
        let json = serde_json::to_string(&v).unwrap();
        for bad in [
            "no consumer",
            "no consumers",
            "needs no consumer",
            "nothing needs",
            "not needed",
        ] {
            assert!(!text.to_lowercase().contains(bad), "{bad}: {text}");
            assert!(!json.to_lowercase().contains(bad), "{bad}");
        }
        assert_eq!(v.totals.scope_statement, v.scope.statement);
    }
}

/// The tempting wrong patch: an unreadable declared root is skipped and
/// the statement reports the rest as if it were the whole. This goes
/// through the real `declared_roots` state of a directory the process
/// cannot open.
#[test]
fn a_real_unreadable_declared_root_makes_the_scope_incomplete() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid takes no arguments and only reads.
    if unsafe { libc::geteuid() } == 0 {
        return; // root can open anything; the state is unreachable
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("locked");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000)).unwrap();
    let env = swamp_core::locations::Environment::fixture(
        tmp.path().to_path_buf(),
        HashMap::new(),
        swamp_core::locations::Platform::MacOS,
    );
    let cfg = swamp_core::scope::ScanConfig {
        defaults: false,
        include: vec![root.display().to_string()],
        ..Default::default()
    };
    let scope = swamp_core::scope::resolve_effective_scope(
        &env,
        &cfg,
        &[],
        &swamp_core::locations::Registry::with_builtins(),
        1,
    );
    let declared = swamp_core::roots::declared_roots(&scope, &[]);
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(declared.len(), 1, "{declared:?}");
    let s = scope_statement(1, &declared, false);
    assert!(!s.complete, "{declared:?}");
    assert!(s.statement.contains("unreadable"), "{}", s.statement);
}

/// The tempting wrong patch: an explicit root keeps the declared-roots
/// wording, which claims a check against roots that were never consulted.
#[test]
fn an_explicit_root_says_the_declared_roots_were_not_used() {
    let s = scope_statement(7, &[], true);
    assert!(!s.complete);
    assert!(s.statement.contains("root named on the command line"));
    assert!(s.statement.contains("declared roots were not used"));
}

#[test]
fn every_listing_states_the_scope_in_the_documented_words() {
    let v = view_of(&[], &[], &ManagerFacts::default(), &[present_root("/h/src")]);
    assert_eq!(
        v.scope.statement,
        "consumer evidence checked against 3 projects in 1 declared root"
    );
    assert!(render_text(&v).contains(&v.scope.statement));
}

// ---------------------------------------------------------------------
// Active defaults and requested installs
// ---------------------------------------------------------------------

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
#[test]
fn the_active_default_toolchain_is_held_out_by_channel_and_by_exact_name() {
    for spelling in ["stable", "stable-aarch64-apple-darwin"] {
        let units = rustup_units();
        let v = view_of(&units, &[], &rustup_default(spelling), &[present_root("/h/src")]);
        let r = row(&v, "/h/.rustup/toolchains");
        let stable = &r.children[0];
        assert_eq!(
            stable.hold.as_ref().map(|h| h.kind),
            Some(HoldKind::ActiveDefault),
            "{spelling}"
        );
        assert!(r.children[1].hold.is_none(), "nightly is not the default");
        assert_eq!(r.held_bytes, 2 * GB, "{spelling}");
        assert_eq!(r.regenerable_bytes, GB, "{spelling}");
        let text = render_text(&v);
        assert!(text.contains("active default"), "{text}");
        assert!(text.contains("held out of the regenerable total"), "{text}");
        assert!(!text.contains("candidate"), "{text}");
    }
}

/// The tempting wrong patch: when the manager's own record could not be
/// read, treat the absence of a default as "there is none". The standing
/// is unknown, and unknown is held out.
#[test]
fn an_unread_default_is_unknown_and_held_out_never_assumed_absent() {
    let units = rustup_units();
    for mf in [
        ManagerFacts::default(), // never observed
        facts(vec![
            pass(),
            fact(
                "rustup",
                "settings-default",
                FactKind::NotObserved,
                None,
                "settings.toml could not be read (permission denied)",
            ),
        ]),
    ] {
        let v = view_of(&units, &[], &mf, &[present_root("/h/src")]);
        let r = row(&v, "/h/.rustup/toolchains");
        let hold = r.hold.as_ref().expect("held");
        assert_eq!(hold.kind, HoldKind::Unknown);
        assert!(hold.whole_unit);
        assert_eq!(r.held_bytes, r.bytes);
        assert_eq!(r.regenerable_bytes, 0);
        assert!(render_text(&v).contains("unknown ("));
    }
}

/// The tempting wrong patch: only a rustup default is special-cased. A
/// mise global tool and a Homebrew formula installed on request are held
/// the same way, from the managers' own records.
#[test]
fn a_mise_global_tool_and_an_on_request_formula_are_held_out() {
    let mut installs = unit(
        "mise",
        StorageCategory::Installation,
        "/h/.local/share/mise/installs",
        3 * GB,
    );
    installs.children = vec![entry("node", GB as i64), entry("python", 2 * GB as i64)];
    let node = unit(
        "homebrew-devtools",
        StorageCategory::Installation,
        "/opt/homebrew/Cellar/node",
        GB,
    );
    let go = unit(
        "homebrew-devtools",
        StorageCategory::Installation,
        "/opt/homebrew/Cellar/go",
        GB,
    );
    let other = unit(
        "homebrew-other",
        StorageCategory::Installation,
        "/opt/homebrew",
        2 * GB,
    );
    let mf = facts(vec![
        pass(),
        fact("mise", "global-tools", FactKind::ActiveDefault, Some("node"), "listed in the global configuration /h/.config/mise/config.toml"),
        fact("mise", "global-tools", FactKind::Checked, None, ""),
        fact("mise", "prune-dry-run", FactKind::Checked, None, ""),
        fact("brew", "installed-on-request", FactKind::InstalledOnRequest, Some("go"), "installed on request"),
        fact("brew", "installed-on-request", FactKind::InstalledOnRequest, Some("ripgrep"), "installed on request"),
        fact("brew", "installed-on-request", FactKind::Checked, None, ""),
        fact("brew", "autoremove-dry-run", FactKind::Checked, None, ""),
    ]);
    let v = view_of(&[installs, node, go, other], &[], &mf, &[present_root("/h/src")]);
    let mise = row(&v, "/h/.local/share/mise/installs");
    assert_eq!(mise.held_bytes, GB, "only the global tool is held");
    assert_eq!(
        mise.children[0].hold.as_ref().map(|h| h.kind),
        Some(HoldKind::ActiveDefault)
    );
    assert!(mise.children[1].hold.is_none());
    let go = row(&v, "/opt/homebrew/Cellar/go");
    assert_eq!(go.hold.as_ref().map(|h| h.kind), Some(HoldKind::InstalledOnRequest));
    assert_eq!(go.held_bytes, GB);
    let node_brew = row(&v, "/opt/homebrew/Cellar/node");
    assert!(node_brew.hold.is_none(), "a dependency is not held");
    assert_eq!(node_brew.regenerable_bytes, GB);
    // The remainder unit cannot tell which of its formulae were requested.
    let other = row(&v, "/opt/homebrew");
    let h = other.hold.as_ref().expect("held whole");
    assert!(h.whole_unit);
    assert_eq!(other.held_bytes, 2 * GB);
    assert!(h.label.contains("not separable"));
}

// ---------------------------------------------------------------------
// Sums, ties, missing values
// ---------------------------------------------------------------------

/// The tempting wrong patch: children are the top N and the rest is
/// dropped, or a hardlink adjustment is left out, so the rows and the
/// unit disagree. Rows sum to the unit total, and the totals partition.
#[test]
fn rows_sum_to_the_unit_total_and_the_totals_partition() {
    let mut u = unit("uv", StorageCategory::Cache, "/h/.cache/uv", 130);
    let mut adj = remainder(-10, 0);
    adj.kind = ChildKind::Adjustment;
    u.children = vec![entry("a", 100), remainder(40, 3), adj];
    assert_eq!(rows_total(&u.children), 130);
    let mut t = unit(
        "rustup",
        StorageCategory::Installation,
        "/h/.rustup/toolchains",
        3 * GB,
    );
    t.children = vec![entry("stable-aarch64-apple-darwin", 3 * GB as i64)];
    let state = unit("codex", StorageCategory::LocalState, "/h/.codex", 500);
    let unk = unit("x", StorageCategory::Unclassified, "/h/Library/Caches", 700);
    let v = view_of(
        &[u, t, state, unk],
        &[standalone("/tmp/target", 900)],
        &rustup_default("stable"),
        &[present_root("/h/src")],
    );
    for r in &v.rows {
        let sum: i64 = r.children.iter().filter_map(|c| c.bytes).sum();
        if !r.children.is_empty() {
            assert_eq!(sum, r.bytes as i64, "{}", r.path);
        }
        assert_eq!(
            r.regenerable_bytes + r.held_bytes
                + match r.regeneration.class {
                    RegenClass::NotRegenerable | RegenClass::NotEstablished => r.bytes,
                    _ => 0,
                },
            r.bytes,
            "{}",
            r.path
        );
    }
    let t = &v.totals;
    assert_eq!(
        t.bytes,
        t.regenerable_bytes + t.held_bytes + t.not_regenerable_bytes + t.not_established_bytes
    );
    assert_eq!(t.bytes, v.rows.iter().map(|r| r.bytes).sum::<u64>());
    assert_eq!(
        t.per_kind.iter().map(|k| k.bytes).sum::<u64>(),
        t.bytes,
        "per-kind bytes add up to the total"
    );
    assert_eq!(t.count, v.rows.len());
    assert_eq!(t.held_bytes, 3 * GB, "the only toolchain is the default");
    assert_eq!(t.not_regenerable_bytes, 500);
    assert_eq!(t.not_established_bytes, 700);
}

/// The tempting wrong patch: a folder that could not be read is
/// `unwrap_or(0)` in the row. It is `not measured`, in text and in JSON.
#[test]
fn a_folder_that_was_not_measured_is_never_shown_as_zero() {
    let mut u = unit("x", StorageCategory::Cache, "/h/.cache/x", 100);
    let mut locked = entry("locked", 0);
    locked.bytes = None;
    locked.measure = ChildMeasure::NotMeasured;
    u.children = vec![entry("open", 100), locked];
    let v = view_of(&[u], &[], &ManagerFacts::default(), &[present_root("/h/src")]);
    let r = row(&v, "/h/.cache/x");
    let locked = &r.children[1];
    assert_eq!(locked.bytes, None);
    assert!(locked.text.contains("not measured"), "{}", locked.text);
    let text = render_text(&v);
    let line = text.lines().find(|l| l.contains("locked")).unwrap();
    assert!(line.contains("not measured"), "{line}");
    assert!(!line.contains("0B"), "{line}");
    let json = serde_json::to_value(&v).unwrap();
    assert!(json["rows"][0]["children"][1]["bytes"].is_null());
}

/// The tempting wrong patch: `sort_by_key(Reverse(bytes))` keeps input
/// order among equal sizes, so two runs over the same facts (units
/// arriving in a different order) print differently.
#[test]
fn sorting_is_deterministic_with_ties() {
    let make = |order: &[&str]| -> Vec<ExternalUnit> {
        order
            .iter()
            .map(|p| unit("uv", StorageCategory::Cache, p, GB))
            .collect()
    };
    let a = view_of(
        &make(&["/h/c", "/h/a", "/h/b"]),
        &[],
        &ManagerFacts::default(),
        &[present_root("/h/src")],
    );
    let b = view_of(
        &make(&["/h/b", "/h/c", "/h/a"]),
        &[],
        &ManagerFacts::default(),
        &[present_root("/h/src")],
    );
    let paths = |v: &ReclaimView| v.rows.iter().map(|r| r.path.clone()).collect::<Vec<_>>();
    assert_eq!(paths(&a), vec!["/h/a", "/h/b", "/h/c"]);
    assert_eq!(paths(&a), paths(&b));
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
    let big = unit("uv", StorageCategory::Cache, "/h/z", 2 * GB);
    let mut mixed = make(&["/h/c", "/h/a"]);
    mixed.push(big);
    let m = view_of(&mixed, &[], &ManagerFacts::default(), &[present_root("/h/src")]);
    assert_eq!(paths(&m)[0], "/h/z", "bytes descending first");
}

/// The tempting wrong patch: a unit with no record falls back to its
/// modification time, or renders an epoch date.
#[test]
fn no_record_is_never_rendered_as_a_date() {
    let mut u = unit("uv", StorageCategory::Cache, "/h/.cache/uv", GB);
    u.mtime_max = NOW - 86_400;
    u.children = vec![entry("wheels", GB as i64)];
    let v = view_of(&[u], &[], &ManagerFacts::default(), &[present_root("/h/src")]);
    let r = row(&v, "/h/.cache/uv");
    assert_eq!(r.last_used_text, "no record");
    assert_eq!(r.children[0].last_used_text.as_deref(), Some("no record"));
    let text = render_text(&v);
    assert!(text.contains("last used: no record"));
    for month in ["Jan ", "Feb ", "Mar ", "Apr ", "May ", "Jun ", "Jul ", "Aug ", "Sep ", "Oct ", "Nov ", "Dec ", "1970"] {
        assert!(!text.contains(month), "{month} in\n{text}");
    }
    let json = serde_json::to_value(&v).unwrap();
    assert!(json["rows"][0]["last_used"]["at"].is_null());
}

/// A tool-native record shows with its source label; the access time
/// stays in the JSON (the #176 precedence rule, carried through).
#[test]
fn a_last_used_fact_shows_its_source() {
    let mut u = unit("xcode", StorageCategory::BuildOutput, "/h/DerivedData", GB);
    u.last_used = LastUsed::default().with_tool_native(
        swamp_core::last_used::XCODE_DERIVED_DATA,
        NOW - 86_400 * 24,
    );
    let v = view_of(&[u], &[], &ManagerFacts::default(), &[present_root("/h/src")]);
    let r = row(&v, "/h/DerivedData");
    assert!(r.last_used_text.contains("Xcode DerivedData record"), "{}", r.last_used_text);
}

// ---------------------------------------------------------------------
// Manager statements
// ---------------------------------------------------------------------

/// The tempting wrong patch: the manager's report is paraphrased into a
/// swamp verdict ("unneeded, safe to remove"). The quote is the manager's
/// sentence, verbatim, with who said it.
#[test]
fn manager_statements_are_verbatim_and_attributed_never_swamps_own() {
    let cellar = unit(
        "homebrew-devtools",
        StorageCategory::Installation,
        "/opt/homebrew/Cellar/llvm@20",
        GB,
    );
    let other = unit(
        "homebrew-other",
        StorageCategory::Installation,
        "/opt/homebrew",
        2 * GB,
    );
    let mut mise = unit(
        "mise",
        StorageCategory::Installation,
        "/h/.local/share/mise/installs",
        GB,
    );
    mise.children = vec![entry("poetry", GB as i64)];
    let header = "Would autoremove 2 unneeded formulae:";
    let prune = "mise poetry@2.1.3 is prunable: no tracked config or tool stub requires poetry";
    let mf = facts(vec![
        pass(),
        fact("brew", "autoremove-dry-run", FactKind::ReportsUnneeded, Some("llvm@20"), header),
        fact("brew", "autoremove-dry-run", FactKind::ReportsUnneeded, Some("libevent"), header),
        fact("brew", "autoremove-dry-run", FactKind::Checked, None, ""),
        fact("brew", "installed-on-request", FactKind::Checked, None, ""),
        fact("mise", "prune-dry-run", FactKind::ReportsPrunable, Some("poetry@2.1.3"), prune),
        fact("mise", "prune-dry-run", FactKind::Checked, None, ""),
        fact("mise", "global-tools", FactKind::Checked, None, ""),
    ]);
    let v = view_of(&[cellar, other, mise], &[], &mf, &[present_root("/h/src")]);
    let llvm = row(&v, "/opt/homebrew/Cellar/llvm@20");
    assert_eq!(llvm.manager.len(), 1);
    assert_eq!(llvm.manager[0].quote, header);
    assert_eq!(
        llvm.manager[0].attribution,
        "Homebrew reports unneeded (brew autoremove)"
    );
    let other = row(&v, "/opt/homebrew");
    assert_eq!(other.manager[0].subject, "libevent");
    let poetry = &row(&v, "/h/.local/share/mise/installs").children[0];
    assert_eq!(poetry.manager[0].quote, prune, "verbatim");
    assert_eq!(
        poetry.manager[0].attribution,
        "mise reports prunable (mise prune --dry-run)"
    );
    let text = render_text(&v);
    assert!(text.contains("Homebrew reports unneeded (brew autoremove): \"Would autoremove 2 unneeded formulae:\""));
    assert!(text.contains(prune));
    for verdict in ["safe to remove", "safe to delete", "can be removed", "should remove"] {
        assert!(!text.to_lowercase().contains(verdict), "{verdict}");
    }
}

// ---------------------------------------------------------------------
// The scheduled manager pass
// ---------------------------------------------------------------------

struct Fake {
    answers: Mutex<HashMap<&'static str, io::Result<RunOutput>>>,
    seen: Mutex<Vec<String>>,
    settings: Option<String>,
}

impl Fake {
    fn new() -> Self {
        Fake {
            answers: Mutex::new(HashMap::new()),
            seen: Mutex::new(Vec::new()),
            settings: None,
        }
    }
    fn answer(self, key: &'static str, r: io::Result<RunOutput>) -> Self {
        self.answers.lock().unwrap().insert(key, r);
        self
    }
}

fn out(code: Option<i32>, stdout: &[u8], stderr: &[u8], timed_out: bool) -> RunOutput {
    RunOutput {
        code,
        stdout: stdout.to_vec(),
        stderr: stderr.to_vec(),
        timed_out,
    }
}

impl ProbeRunner for Fake {
    fn run(&self, program: Program, args: &[&str], _t: Duration) -> io::Result<RunOutput> {
        let key = format!("{} {}", program.binary(), args.join(" "));
        self.seen.lock().unwrap().push(key.clone());
        let map = self.answers.lock().unwrap();
        for (k, v) in map.iter() {
            if key.starts_with(k) {
                return match v {
                    Ok(o) => Ok(o.clone()),
                    Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
                };
            }
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "no such program"))
    }
    fn read_settings(&self, _p: &std::path::Path) -> Result<String, String> {
        self.settings.clone().ok_or_else(|| "no such file".into())
    }
}

fn manager_units() -> Vec<ExternalUnit> {
    vec![
        unit("homebrew-other", StorageCategory::Installation, "/opt/homebrew", GB),
        unit("mise", StorageCategory::Installation, "/h/.local/share/mise/installs", GB),
    ]
}

fn kinds(rows: &[ManagerFact], manager: &str) -> Vec<(String, FactKind)> {
    rows.iter()
        .filter(|f| f.manager == manager)
        .map(|f| (f.probe.clone(), f.kind))
        .collect()
}

/// The tempting wrong patch: a missing binary, a time-out, garbage or a
/// flood becomes an error that fails the observation, or, worse, an empty
/// list read as "the manager reported nothing". Each is one `not
/// observed` row that says why, and the view says the report was not
/// observed rather than failing.
#[test]
fn a_manager_that_is_missing_slow_garbled_or_flooding_is_a_coverage_note() {
    let huge = vec![b'a'; manager_facts::MAX_OUTPUT + 1];
    let cases: Vec<(&str, Fake, &str)> = vec![
        (
            "missing",
            Fake::new(),
            "is not installed or not on PATH",
        ),
        (
            "timeout",
            Fake::new()
                .answer("brew", Ok(out(None, b"", b"", true)))
                .answer("mise", Ok(out(None, b"", b"", true))),
            "did not answer within",
        ),
        (
            "garbage",
            Fake::new()
                .answer("brew", Ok(out(Some(0), &[0xff, 0xfe, 0x00, 0x01], b"", false)))
                .answer("mise", Ok(out(Some(0), b"\x00\x01garbage", b"", false))),
            "could not be read",
        ),
        (
            "flood",
            Fake::new()
                .answer("brew", Ok(out(Some(0), &huge, b"", false)))
                .answer("mise", Ok(out(Some(0), &huge, b"", false))),
            "could not be read",
        ),
        (
            "failed",
            Fake::new()
                .answer("brew", Ok(out(Some(1), b"", b"Error", false)))
                .answer("mise", Ok(out(Some(1), b"", b"", false))),
            "exited with status 1",
        ),
    ];
    for (name, fake, why) in cases {
        let rows = collect_within(
            &manager_units(),
            &fake,
            NOW,
            Duration::from_secs(60),
            Duration::from_secs(1),
        );
        let observed: Vec<&ManagerFact> = rows
            .iter()
            .filter(|f| f.kind == FactKind::NotObserved)
            .collect();
        assert!(!observed.is_empty(), "{name}");
        assert!(observed.iter().all(|f| f.text.contains(why)), "{name}: {observed:?}");
        assert!(
            !rows.iter().any(|f| matches!(
                f.kind,
                FactKind::ReportsUnneeded | FactKind::ReportsPrunable | FactKind::InstalledOnRequest
            )),
            "{name}: a failed probe reports nothing"
        );
        // The view says so, and holds the unread standing out.
        let mf = facts(rows);
        let v = view_of(&manager_units(), &[], &mf, &[present_root("/h/src")]);
        assert!(
            v.coverage_notes.iter().any(|n| n.contains("not observed")),
            "{name}: {:?}",
            v.coverage_notes
        );
        let mise = row(&v, "/h/.local/share/mise/installs");
        assert_eq!(mise.hold.as_ref().map(|h| h.kind), Some(HoldKind::Unknown), "{name}");
        assert_eq!(mise.regenerable_bytes, 0, "{name}");
    }
}

/// The tempting wrong patch: a probe that ran but listed nothing is
/// stored as no rows, which reads as "not observed".
#[test]
fn a_manager_that_lists_nothing_is_checked_not_absent() {
    let fake = Fake::new()
        .answer("brew autoremove", Ok(out(Some(0), b"", b"", false)))
        .answer("brew list", Ok(out(Some(0), b"", b"", false)))
        .answer("mise prune", Ok(out(Some(0), b"", b"mise pruned configuration links [dryrun]\n", false)))
        .answer("mise ls", Ok(out(Some(0), b"{}", b"mise WARN update available\n", false)));
    let rows = collect_within(
        &manager_units(),
        &fake,
        NOW,
        Duration::from_secs(60),
        Duration::from_secs(1),
    );
    for (manager, probes) in [("brew", 2), ("mise", 2)] {
        let k = kinds(&rows, manager);
        assert_eq!(
            k.iter().filter(|(_, kind)| *kind == FactKind::Checked).count(),
            probes,
            "{manager}: {k:?}"
        );
        assert!(!k.iter().any(|(_, kind)| *kind == FactKind::NotObserved));
    }
    assert!(rows.iter().any(|f| f.kind == FactKind::Pass));
}

/// The tempting wrong patch: an exhausted budget still starts the next
/// command. With no time left the probe does not run and says so.
#[test]
fn a_spent_budget_starts_nothing() {
    let fake = Fake::new();
    let rows = collect_within(
        &manager_units(),
        &fake,
        NOW,
        Duration::ZERO,
        Duration::from_secs(1),
    );
    assert!(fake.seen.lock().unwrap().is_empty(), "no command may start");
    assert!(
        rows.iter()
            .filter(|f| f.kind == FactKind::NotObserved)
            .all(|f| f.text.contains("time budget"))
    );
}

/// The real answers of this machine's tools, in their real shape: the
/// pass stores facts, the quote is the manager's own line, and the query
/// asked is a dry run.
#[test]
fn the_pass_asks_only_dry_runs_and_stores_the_answers_it_read() {
    let fake = Fake::new()
        .answer(
            "brew autoremove --dry-run",
            Ok(out(
                Some(0),
                b"Would autoremove 4 unneeded formulae:\nlibevent\nlibnghttp2\nunbound\nusage\n",
                b"",
                false,
            )),
        )
        .answer(
            "brew list --formula --installed-on-request",
            Ok(out(Some(0), b"ansible\natuin\n", b"", false)),
        )
        .answer(
            "mise prune --dry-run",
            Ok(out(
                Some(0),
                b"",
                b"mise pruned configuration links [dryrun]\nmise poetry@2.1.3 is prunable: no tracked config or tool stub requires poetry\nmise poetry@2.1.3 [dryrun]  uninstall\n",
                false,
            )),
        )
        .answer(
            "mise ls --global --json",
            Ok(out(
                Some(0),
                br#"{"node":[{"version":"24.1","requested_version":"latest","source":{"type":"mise.toml","path":"/h/.config/mise/config.toml"}}]}"#,
                b"mise WARN mise version 2026.9.18 available\n",
                false,
            )),
        );
    let rows = collect_within(
        &manager_units(),
        &fake,
        NOW,
        Duration::from_secs(60),
        Duration::from_secs(1),
    );
    let mut seen = fake.seen.lock().unwrap().clone();
    seen.sort();
    assert_eq!(
        seen,
        vec![
            "brew autoremove --dry-run",
            "brew list --formula --installed-on-request",
            "mise ls --global --json",
            "mise prune --dry-run",
        ],
        "exactly the four read-only questions, and no real run"
    );
    let unneeded: Vec<&str> = rows
        .iter()
        .filter(|f| f.kind == FactKind::ReportsUnneeded)
        .filter_map(|f| f.subject.as_deref())
        .collect();
    assert_eq!(unneeded, vec!["libevent", "libnghttp2", "unbound", "usage"]);
    let prune = rows.iter().find(|f| f.kind == FactKind::ReportsPrunable).unwrap();
    assert_eq!(prune.subject.as_deref(), Some("poetry@2.1.3"));
    assert_eq!(
        prune.text,
        "mise poetry@2.1.3 is prunable: no tracked config or tool stub requires poetry"
    );
    assert!(rows.iter().any(|f| f.kind == FactKind::ActiveDefault && f.subject.as_deref() == Some("node")));
}

/// The settings-file probe for a default toolchain reads the file the
/// detector declared and reports what it holds.
#[test]
fn rustup_default_comes_from_the_settings_file_the_detector_declared() {
    let mut fake = Fake::new();
    fake.settings = Some("version = \"12\"\ndefault_toolchain = \"stable-aarch64-apple-darwin\"\n".into());
    let rows = collect_within(
        &rustup_units(),
        &fake,
        NOW,
        Duration::from_secs(60),
        Duration::from_secs(1),
    );
    assert!(fake.seen.lock().unwrap().is_empty(), "no command runs for a settings file");
    let d = rows.iter().find(|f| f.kind == FactKind::ActiveDefault).unwrap();
    assert_eq!(d.subject.as_deref(), Some("stable-aarch64-apple-darwin"));
    // Unreadable or malformed settings are a note, not a guess.
    for settings in [None, Some("this is [ not toml".to_string())] {
        let mut fake = Fake::new();
        fake.settings = settings;
        let rows = collect_within(
            &rustup_units(),
            &fake,
            NOW,
            Duration::from_secs(60),
            Duration::from_secs(1),
        );
        assert!(rows.iter().any(|f| f.kind == FactKind::NotObserved));
        assert!(!rows.iter().any(|f| f.kind == FactKind::ActiveDefault));
    }
}

// ---------------------------------------------------------------------
// Removal paths, kinds and delivered wording
// ---------------------------------------------------------------------

/// The tempting wrong patch: every row offers Trash. Only a standalone
/// Cargo target (the existing reviewed flow) does; an installation names
/// the tool command that is not built yet; everything else is view only.
#[test]
fn removal_paths_are_only_the_ones_that_exist() {
    let v = view_of(
        &[
            unit("rustup", StorageCategory::Installation, "/h/.rustup/toolchains", GB),
            unit("uv", StorageCategory::Cache, "/h/.cache/uv", GB),
            unit("codex", StorageCategory::LocalState, "/h/.codex", GB),
        ],
        &[standalone("/tmp/cargo-target", GB)],
        &ManagerFacts::default(),
        &[present_root("/h/src")],
    );
    assert_eq!(row(&v, "/tmp/cargo-target").removal.kind, RemovalKind::TrashReviewed);
    assert_eq!(row(&v, "/tmp/cargo-target").kind, KIND_STANDALONE_CARGO_TARGET);
    assert_eq!(
        row(&v, "/h/.rustup/toolchains").removal.text,
        "tool command, not available yet"
    );
    assert_eq!(row(&v, "/h/.cache/uv").removal.text, "view only");
    assert_eq!(row(&v, "/h/.codex").removal.text, "view only");
    assert_eq!(
        row(&v, "/tmp/cargo-target").regeneration.words,
        "rebuild with `cargo build`"
    );
}

/// Local state and models are never presented as re-obtainable.
#[test]
fn local_state_and_models_cannot_be_regenerated() {
    for category in [StorageCategory::LocalState, StorageCategory::Models] {
        let v = view_of(
            &[unit("ollama", category, "/h/.ollama", GB)],
            &[],
            &ManagerFacts::default(),
            &[present_root("/h/src")],
        );
        let r = row(&v, "/h/.ollama");
        assert_eq!(r.regeneration.class, RegenClass::NotRegenerable);
        assert!(r.regeneration.words.contains("cannot be regenerated"));
        assert_eq!(v.totals.regenerable_bytes, 0);
        assert_eq!(v.totals.not_regenerable_bytes, GB);
    }
}

/// A recorded link (tier two) is listed apart from a declaration (tier
/// one), with its source.
#[test]
fn declared_consumers_and_recorded_links_are_kept_apart() {
    let mut u = unit("xcode", StorageCategory::BuildOutput, "/h/DerivedData", GB);
    let ev = |sub: FactSubtype, who: &str, source: EvidenceSource| {
        Evidence::known(EvKind::Consumer, sub, FactValue::Text(who.into()), source, NOW)
    };
    u.evidence = vec![
        ev(
            FactSubtype::DeclaredConsumer,
            "api",
            EvidenceSource::Lockfile {
                ecosystem: "cargo".into(),
                path: "/h/src/api/Cargo.lock".into(),
            },
        ),
        ev(
            FactSubtype::RecordedLink,
            "/h/src/app.xcworkspace",
            EvidenceSource::BuildMetadata {
                path: "info.plist".into(),
            },
        ),
        Evidence::unknown(
            EvKind::Consumer,
            FactSubtype::DeclaredConsumer,
            EvidenceSource::Inferred { basis: "x".into() },
            NOW,
            Reason::fixed("content-addressed: no entry can be joined"),
        ),
    ];
    let v = view_of(&[u], &[], &ManagerFacts::default(), &[present_root("/h/src")]);
    let c = &row(&v, "/h/DerivedData").consumers;
    assert_eq!(c.declared.len(), 1);
    assert_eq!(c.declared[0].label, "api");
    assert!(c.declared[0].source.contains("Cargo.lock"));
    assert_eq!(c.recorded_links.len(), 1);
    assert_eq!(c.recorded_links[0].label, "/h/src/app.xcworkspace");
    assert_eq!(c.unknown.len(), 1);
    assert!(c.summary.contains("declared by api"));
    assert!(c.summary.contains("recorded links: /h/src/app.xcworkspace"));
    assert!(c.summary.contains("not established: content-addressed"));
}

/// The tempting wrong patch: a verdict word or an em dash slips into a
/// delivered string through a kind nobody rendered in a test. This
/// renders a fixture covering every kind, hold and removal, in text and
/// JSON, and checks the words.
#[test]
fn no_verdict_words_and_no_em_dashes_anywhere_in_the_rendered_view() {
    let mut units: Vec<ExternalUnit> = [
        StorageCategory::Installation,
        StorageCategory::Downloads,
        StorageCategory::Cache,
        StorageCategory::LocalState,
        StorageCategory::Environments,
        StorageCategory::BuildOutput,
        StorageCategory::Models,
        StorageCategory::Unclassified,
    ]
    .iter()
    .enumerate()
    .map(|(i, c)| {
        let mut u = unit("uv", *c, &format!("/h/kind{i}"), GB + i as u64);
        u.children = vec![entry("a", (GB + i as u64) as i64 - 5), remainder(5, 1)];
        u
    })
    .collect();
    units.extend(rustup_units());
    units.extend(manager_units());
    let mf = facts(vec![
        pass(),
        fact("rustup", "settings-default", FactKind::ActiveDefault, Some("stable"), "default_toolchain \"stable\" in settings.toml"),
        fact("rustup", "settings-default", FactKind::Checked, None, ""),
        fact("brew", "autoremove-dry-run", FactKind::ReportsUnneeded, Some("libevent"), "Would autoremove 1 unneeded formulae:"),
        fact("brew", "autoremove-dry-run", FactKind::Checked, None, ""),
        fact("brew", "installed-on-request", FactKind::NotObserved, None, "brew exited with status 1"),
        fact("mise", "prune-dry-run", FactKind::NotObserved, None, "mise is not installed or not on PATH"),
    ]);
    let roots = [
        present_root("/h/src"),
        DeclaredRoot {
            path: PathBuf::from("/gone"),
            state: DeclaredState::Missing,
        },
    ];
    let v = view_of(&units, &[standalone("/tmp/t", GB)], &mf, &roots);
    let mut blob = render_text(&v);
    blob.push_str(&serde_json::to_string_pretty(&v).unwrap());
    assert!(!blob.contains('\u{2014}'), "an em dash");
    assert!(!blob.contains('\u{2013}'), "an en dash");
    let lower = blob.to_lowercase();
    for word in ["unused", "obsolete", "stale", "orphan", "junk", "garbage", "safe"] {
        let bad = lower
            .split(|c: char| !c.is_alphanumeric())
            .any(|w| w == word);
        assert!(!bad, "verdict word {word:?} in\n{blob}");
    }
    for phrase in ["can be deleted", "can be removed", "should delete", "should remove"] {
        assert!(!lower.contains(phrase), "{phrase}");
    }
}

/// The view is computed from stored facts and nothing else: building it
/// lists no directory, stats no file and starts no process.
#[test]
fn building_the_view_does_no_io() {
    let (_, work) = swamp_core::work_counters::measured(|| {
        view_of(
            &rustup_units(),
            &[standalone("/tmp/t", GB)],
            &rustup_default("stable"),
            &[present_root("/h/src")],
        )
    });
    assert_eq!(work, swamp_core::work_counters::WorkCounters::default());
}
