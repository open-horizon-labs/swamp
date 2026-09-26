//! R12: `swamp report` is a pure read of stored Parquet rows; `swamp
//! observe` is the only scanner.
//!
//! Three adversarial claims, not a happy path:
//!
//! 1. `report::report_scope_from_store` on a scope `observe_scope` has
//!    already covered does **zero** directory listings, file stats,
//!    header-byte reads, or subprocess spawns (`work_counters`) -- the
//!    tempting shortcut this contract forbids is "read the store, but
//!    still walk to double-check/annotate something".
//! 2. A scope that has never been observed refuses with the exact
//!    documented shape (`report::NoObservation`) rather than silently
//!    falling back to a walk.
//! 3. What `report_scope_from_store` returns is byte-for-byte the same
//!    `Report` `observe_scope` produced (a round trip through stored
//!    Parquet tables, not a re-derivation).
//!
//! Disposable `tempfile` fixtures only.

use std::path::{Path, PathBuf};
use std::process::Command;
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::report::{self, ObservationParts};
use swamp_core::scope::{ScanConfig, resolve_effective_scope};
use swamp_core::work_counters::{self, WorkCounters};

/// The work counters are process-global (scoped counting is
/// thread-local, but the pool `report::observe_scope`'s own walk spawns
/// still bumps the shared global this file does not read) -- serialize
/// this file's tests so one's observation pass is never measured by
/// another's `measured` scope through a stray pool thread.
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
        .unwrap_or_else(|e| panic!("run git {args:?}: {e}"));
    assert!(out.status.success(), "git {args:?} failed");
}

/// One synthetic checkout with a build-output artifact, so the report
/// carries more than an empty project list.
fn make_checkout(root: &Path) -> PathBuf {
    let dir = root.join("repo");
    std::fs::create_dir_all(&dir).unwrap();
    run_git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("README.md"), b"x").unwrap();
    std::fs::write(dir.join(".gitignore"), b"target/\n").unwrap();
    run_git(&dir, &["add", "README.md", ".gitignore"]);
    run_git(&dir, &["commit", "-q", "-m", "init"]);
    std::fs::create_dir_all(dir.join("target/debug")).unwrap();
    std::fs::write(dir.join("target/debug/seed"), vec![b'x'; 8192]).unwrap();
    dir
}

fn only_include(src: &Path) -> ScanConfig {
    let registry = Registry::with_builtins();
    ScanConfig {
        defaults: false,
        include: vec![src.display().to_string()],
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .collect(),
        ..Default::default()
    }
}

#[test]
fn report_scope_from_store_does_no_io_after_observe() {
    let _lock = serial();
    let tmp = tempfile::tempdir().unwrap();
    let root = swamp_core::fs_gate::canonicalize(tmp.path()).unwrap_or(tmp.path().to_path_buf());
    let src = root.join("src");
    make_checkout(&src);
    let store = root.join("store");
    std::fs::create_dir_all(&store).unwrap();

    let env = Environment::fixture(root.clone(), Default::default(), Platform::MacOS);
    let registry = Registry::with_builtins();
    let cfg = only_include(&src);
    let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1_000);

    // `swamp observe`: the only pass allowed to do any of this work.
    let observation = report::observe_scope(
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
    assert!(
        !observation.merged.projects.is_empty(),
        "fixture must produce at least one project"
    );

    // `swamp report`: read only, measured.
    let (result, work) =
        work_counters::measured(|| report::report_scope_from_store(&scope, &store));
    let snapshot = result.expect("a scope observe_scope just covered must have a stored snapshot");

    assert_eq!(
        work,
        WorkCounters::default(),
        "report_scope_from_store must do zero directory listings, file \
         stats, header-byte reads or subprocess spawns: {work:?}"
    );

    // Round trip: what came back through the stored tables is
    // byte-for-byte (field-for-field: `serde_json::Value`'s map is a
    // `BTreeMap`, so key order in a `HashMap`-backed field like
    // `series_by_key` cannot hide a real difference or fake one) what
    // `observe_scope` itself produced, not a re-derivation that merely
    // looks similar.
    let before = serde_json::to_value(&observation.merged).unwrap();
    let after = serde_json::to_value(&snapshot.report).unwrap();
    assert_eq!(
        before, after,
        "the stored snapshot must round-trip the observed Report exactly"
    );
    assert_eq!(
        serde_json::to_value(&observation.external_units).unwrap(),
        serde_json::to_value(&snapshot.external_units).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&observation.agent_units).unwrap(),
        serde_json::to_value(&snapshot.agent_units).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&observation.coverage).unwrap(),
        serde_json::to_value(&snapshot.coverage).unwrap()
    );

    // Rendering off the round-tripped snapshot must be identical to
    // rendering off the freshly observed report: the same view, from
    // either source, is the same text.
    let rendered_before = swamp_core::render::render_overview_sorted(
        &observation.merged,
        true,
        false,
        false,
        swamp_core::render::OverviewSort::Growth,
        false,
    );
    let rendered_after = swamp_core::render::render_overview_sorted(
        &snapshot.report,
        true,
        false,
        false,
        swamp_core::render::OverviewSort::Growth,
        false,
    );
    assert_eq!(rendered_before, rendered_after);
}

/// A scope nothing has ever observed refuses with the documented shape
/// -- never a walk to produce something to show.
#[test]
fn report_scope_from_store_refuses_when_never_observed() {
    let _lock = serial();
    let tmp = tempfile::tempdir().unwrap();
    let root = swamp_core::fs_gate::canonicalize(tmp.path()).unwrap_or(tmp.path().to_path_buf());
    let src = root.join("src");
    make_checkout(&src);
    let store = root.join("store");
    std::fs::create_dir_all(&store).unwrap();

    let env = Environment::fixture(root.clone(), Default::default(), Platform::MacOS);
    let registry = Registry::with_builtins();
    let cfg = only_include(&src);
    let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1_000);

    let (result, work) =
        work_counters::measured(|| report::report_scope_from_store(&scope, &store));
    let err = result.expect_err("an unobserved scope must refuse, not walk to answer anyway");
    assert!(
        err.to_string().contains("no observation yet")
            && err.to_string().contains("run `swamp observe`"),
        "unexpected message: {err}"
    );
    assert_eq!(
        work,
        WorkCounters::default(),
        "a refusal must not have walked anything either: {work:?}"
    );
}
