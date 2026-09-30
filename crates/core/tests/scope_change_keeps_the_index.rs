//! #168/#174 upgrade and add-root behavior: a change to the scope's roots
//! (a declared root added, a default detector added by an upgrade) must not
//! orphan the stored observation. The stored observation is keyed by the
//! PROJECT roots, and when even those changed, the most recent overlapping
//! observation is shown, labelled, from stored tables alone.

use std::path::{Path, PathBuf};
use std::process::Command;
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::report::{self, ObservationParts};
use swamp_core::scope::{EffectiveScope, ScanConfig, resolve_effective_scope};
use swamp_core::work_counters::{self, WorkCounters};

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .output()
        .unwrap();
    assert!(out.status.success());
}

fn checkout(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name).join("repo");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("README.md"), b"x").unwrap();
    git(&dir, &["add", "README.md"]);
    git(&dir, &["commit", "-q", "-m", "init"]);
    root.join(name)
}

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    store: PathBuf,
    env: Environment,
}

fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = swamp_core::fs_gate::canonicalize(tmp.path()).unwrap();
    let store = root.join("store");
    std::fs::create_dir_all(&store).unwrap();
    let mut vars = std::collections::HashMap::new();
    vars.insert(
        "HOMEBREW_PREFIX".into(),
        root.join("no-brew").display().to_string(),
    );
    let env = Environment::fixture(root.clone(), vars, Platform::MacOS);
    Fx {
        _tmp: tmp,
        root,
        store,
        env,
    }
}

/// Only the include roots, plus (when `detectors`) the two default-on
/// Homebrew detectors.
fn scope(f: &Fx, include: &[&Path], detectors: bool) -> EffectiveScope {
    let registry = Registry::with_builtins();
    let disabled: Vec<String> = registry
        .detectors()
        .iter()
        .map(|d| d.id().to_string())
        .filter(|id| !(detectors && id.starts_with("homebrew-")))
        .collect();
    let cfg = ScanConfig {
        defaults: false,
        include: include.iter().map(|p| p.display().to_string()).collect(),
        disabled_detectors: disabled,
        enabled_detectors: if detectors {
            vec!["homebrew-devtools".into(), "homebrew-other".into()]
        } else {
            vec![]
        },
        ..Default::default()
    };
    resolve_effective_scope(&f.env, &cfg, &[], &registry, 1_000)
}

fn observe(f: &Fx, scope: &EffectiveScope) {
    report::observe_scope(
        scope,
        ObservationParts::ALL,
        None,
        None,
        false,
        Some(&f.store),
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
}

/// Tempting wrong patch: key the observation by every root, so adding a
/// default detector on upgrade is a new scope with no observation.
#[test]
fn a_detector_added_by_an_upgrade_keeps_the_same_observation() {
    let _l = serial();
    let f = fx();
    let a = checkout(&f.root, "src");
    let before = scope(&f, &[&a], false);
    observe(&f, &before);
    let after = scope(&f, &[&a], true);
    assert_eq!(
        report::scope_snapshot_key(&before),
        report::scope_snapshot_key(&after)
    );
    let (r, work) = work_counters::measured(|| report::report_scope_from_store(&after, &f.store));
    let snap = r.expect("the existing observation is found");
    assert!(snap.previous_scope.is_none());
    assert_eq!(work, WorkCounters::default());
}

/// Tempting wrong patch: a changed root set is "no observation yet", so the
/// TUI opens empty and walks. The previous overlapping observation is shown
/// from stored tables, labelled, with no I/O.
#[test]
fn adding_a_root_shows_the_previous_scope_labelled_without_a_walk() {
    let _l = serial();
    let f = fx();
    let a = checkout(&f.root, "src");
    let b = checkout(&f.root, "code");
    observe(&f, &scope(&f, &[&a], false));
    let grown = scope(&f, &[&a, &b], false);
    let (r, work) = work_counters::measured(|| report::report_scope_from_store(&grown, &f.store));
    let snap = r.expect("the overlapping observation is shown");
    let prev = snap.previous_scope.expect("labelled as the previous scope");
    assert_eq!(prev.roots, 1);
    assert!(!snap.report.projects.is_empty());
    assert_eq!(work, WorkCounters::default(), "{work:?}");
    // And the declared-roots bytes come from that observation.
    let coverage = report::stored_root_coverage(&grown, &f.store);
    assert!(coverage.iter().any(|c| c.path == a));
    // Removing the added root again is the exact observation, unlabelled.
    let back = scope(&f, &[&a], false);
    assert!(
        report::report_scope_from_store(&back, &f.store)
            .unwrap()
            .previous_scope
            .is_none()
    );
}

/// Tempting wrong patch: fall back to whatever was observed last, however
/// unrelated. A disjoint scope has no observation, but the store is not
/// empty (the TUI must not treat that as "scan").
#[test]
fn an_unrelated_observation_is_not_shown_but_counts_as_an_index() {
    let _l = serial();
    let f = fx();
    let a = checkout(&f.root, "src");
    let other = checkout(&f.root, "elsewhere");
    assert!(!report::store_has_observation(&f.store));
    observe(&f, &scope(&f, &[&a], false));
    assert!(report::store_has_observation(&f.store));
    let unrelated = scope(&f, &[&other], false);
    assert!(report::report_scope_from_store(&unrelated, &f.store).is_err());
}

/// An empty store has no observation and shows none: the first run still
/// scans in the background.
#[test]
fn an_empty_store_has_no_observation() {
    let f = fx();
    let a = checkout(&f.root, "src");
    let s = scope(&f, &[&a], false);
    assert!(report::report_scope_from_store(&s, &f.store).is_err());
    assert!(!report::store_has_observation(&f.store));
}
