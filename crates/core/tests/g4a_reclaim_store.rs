//! v0.8.0 G4a: what the Reclaim view reads from the store (#175).
//! Disposable `tempfile` fixtures; the manager rows are written by the
//! same function `swamp observe` uses, never by a real manager.
//!
//! Each test names the tempting wrong patch it fails.

use std::path::{Path, PathBuf};
use std::process::Command;
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::manager_facts::{FactKind, ManagerFact};
use swamp_core::reclaim::{ReclaimInput, build};
use swamp_core::report::{self, ObservationParts};
use swamp_core::scope::{EffectiveScope, ScanConfig, resolve_effective_scope};
use swamp_core::work_counters::{self, WorkCounters};

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn run_git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
}

struct Fixture {
    _tmp: tempfile::TempDir,
    store: PathBuf,
    scope: EffectiveScope,
}

fn observed() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = swamp_core::fs_gate::canonicalize(tmp.path()).unwrap_or(tmp.path().to_path_buf());
    let src = root.join("src");
    let repo = src.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), b"x").unwrap();
    run_git(&repo, &["add", "README.md"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);
    let store = root.join("store");
    std::fs::create_dir_all(&store).unwrap();
    let registry = Registry::with_builtins();
    let cfg = ScanConfig {
        defaults: false,
        include: vec![src.display().to_string()],
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .collect(),
        ..Default::default()
    };
    let env = Environment::fixture(root.clone(), Default::default(), Platform::MacOS);
    let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1_000);
    report::observe_scope(
        &scope,
        ObservationParts::ALL,
        None,
        None,
        false,
        Some(&store),
        None,
        true,
        true,
        false,
        false,
        swamp_core::fs_events::platform_source().as_ref(),
        30,
        24 * 3600,
    )
    .expect("observe_scope");
    Fixture {
        _tmp: tmp,
        store,
        scope,
    }
}

fn input<'a>(snapshot: &'a swamp_core::growth::ReportSnapshot) -> ReclaimInput<'a> {
    ReclaimInput {
        units: &snapshot.external_units,
        interiors: &snapshot.store_interiors,
        unowned: &snapshot.report.unowned,
        manager_facts: &snapshot.manager_facts,
        declared_roots: &[],
        explicit_scope: false,
        projects: snapshot.report.projects.len(),
        observed_at: snapshot.report.observed_at,
    }
}

fn fact(kind: FactKind, subject: Option<&str>, text: &str) -> ManagerFact {
    ManagerFact {
        manager: if kind == FactKind::Pass { "" } else { "brew" }.to_string(),
        probe: if kind == FactKind::Pass {
            ""
        } else {
            "autoremove-dry-run"
        }
        .to_string(),
        kind,
        subject: subject.map(str::to_string),
        text: text.to_string(),
        observed_at: 1_790_000_000,
    }
}

/// The tempting wrong patch: a store without the manager table (every
/// store an older swamp wrote, or one nothing has observed managers for)
/// is an error, or reads as "no manager reported anything". It reads as
/// not observed yet, with no failure.
#[test]
fn a_store_without_the_manager_table_reads_as_not_observed_yet() {
    let _lock = serial();
    let fx = observed();
    assert!(!fx.store.join("manager_facts.parquet").exists());
    let snapshot = report::report_scope_from_store(&fx.scope, &fx.store).expect("stored snapshot");
    assert!(!snapshot.manager_facts.observed);
    assert!(snapshot.manager_facts.facts.is_empty());
    let v = build(&input(&snapshot));
    assert!(v.totals.count == v.rows.len());
}

/// The tempting wrong patch: the read path opens the manager tool. Reading
/// the snapshot with the table present, and building the view from it,
/// lists nothing, stats nothing and starts nothing.
#[test]
fn reading_the_stored_view_does_no_listing_no_stat_and_no_spawn() {
    let _lock = serial();
    let fx = observed();
    swamp_core::growth::write_manager_fact_table(
        &fx.store,
        &[
            fact(FactKind::Pass, None, ""),
            fact(
                FactKind::ReportsUnneeded,
                Some("libevent"),
                "Would autoremove 1 unneeded formulae:",
            ),
            fact(FactKind::Checked, None, ""),
        ],
    )
    .unwrap();
    let ((snapshot, view), work) = work_counters::measured(|| {
        let snapshot =
            report::report_scope_from_store(&fx.scope, &fx.store).expect("stored snapshot");
        let view = build(&input(&snapshot));
        (snapshot, view)
    });
    assert_eq!(
        work,
        WorkCounters::default(),
        "the Reclaim view is a stored read: {work:?}"
    );
    assert!(snapshot.manager_facts.observed);
    assert_eq!(snapshot.manager_facts.facts.len(), 3);
    let libevent = snapshot
        .manager_facts
        .facts
        .iter()
        .find(|f| f.subject.as_deref() == Some("libevent"))
        .unwrap();
    assert_eq!(libevent.text, "Would autoremove 1 unneeded formulae:");
    assert_eq!(libevent.kind, FactKind::ReportsUnneeded);
    let _ = view;
}

/// The tempting wrong patch: a row of a kind this build does not know
/// (a newer swamp's) fails the whole read. The label parser rejects it, so
/// the reader skips that row and reads the rest.
#[test]
fn a_kind_this_build_does_not_know_is_not_a_kind() {
    assert_eq!(FactKind::from_label("from-the-future"), None);
    assert_eq!(FactKind::from_label(""), None);
}

/// The tempting wrong patch: the pass rewrites a store a newer swamp
/// owns. It refuses, like every other writer.
#[test]
fn the_pass_refuses_to_write_into_a_store_a_newer_swamp_owns() {
    let _lock = serial();
    let fx = observed();
    std::fs::write(fx.store.join("housekeeping.version"), "99\n").unwrap();
    let err =
        swamp_core::growth::write_manager_fact_table(&fx.store, &[fact(FactKind::Pass, None, "")])
            .expect_err("a newer store is not modified");
    assert!(err.to_string().contains("newer swamp"), "{err}");
    assert!(!fx.store.join("manager_facts.parquet").exists());
}
