//! Scope-wide hardlink accounting is explicit: ordinary incremental
//! observations preserve the previous unique-byte value as needing
//! reconciliation, while
//! a full all-family observation recomputes it and persists the result.

#![cfg(unix)]

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use swamp_core::fs_events::{
    FsEventsPlan, FsEventsRequest, FsEventsSource, RefreshRefusal, testing::CannedSource,
};
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::report::{self, ObservationParts};
use swamp_core::scope::{ScanConfig, resolve_effective_scope};
use swamp_core::work_counters::{self, WorkCounters};

static SERIAL: Mutex<()> = Mutex::new(());
static NO_TOO_SOON: std::sync::Once = std::sync::Once::new();

fn disable_too_soon_floor() {
    NO_TOO_SOON.call_once(|| unsafe {
        // Set once before this test binary reads the refresh interval.
        std::env::set_var("SWAMP_FSEVENTS_MIN_INTERVAL_SECS", "0");
    });
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
        .output()
        .expect("run git fixture command");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn make_scope(home: &Path, roots: &[PathBuf]) -> swamp_core::scope::EffectiveScope {
    let registry = Registry::with_builtins();
    let config = ScanConfig {
        defaults: false,
        include: roots.iter().map(|p| p.display().to_string()).collect(),
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .collect(),
        ..Default::default()
    };
    resolve_effective_scope(
        &Environment::fixture(home.to_path_buf(), Default::default(), Platform::MacOS),
        &config,
        &[],
        &registry,
        1_000,
    )
}

struct ScopedLiveSource {
    changed_dirs: Vec<PathBuf>,
    event_id: u64,
}

impl FsEventsSource for ScopedLiveSource {
    fn replay(&self, request: &FsEventsRequest) -> FsEventsPlan {
        let root = fs::canonicalize(&request.root).unwrap_or_else(|_| request.root.clone());
        let changed_dirs = self
            .changed_dirs
            .iter()
            .filter_map(|p| fs::canonicalize(p).ok())
            .filter(|p| p.starts_with(&root))
            .collect();
        let device = fs::metadata(&root).ok().map(|m| m.dev());
        FsEventsPlan::from_live(changed_dirs, self.event_id, device)
    }
}

fn source(changed_dirs: Vec<PathBuf>, event_id: u64) -> ScopedLiveSource {
    ScopedLiveSource {
        changed_dirs,
        event_id,
    }
}

fn initial_source(roots: &[PathBuf]) -> CannedSource {
    #[cfg(unix)]
    let device = roots.first().map(|p| fs::metadata(p).unwrap().dev());
    #[cfg(not(unix))]
    let device = None;
    CannedSource(FsEventsPlan {
        incremental: false,
        refusal: Some(RefreshRefusal::NoStoredEventId),
        changed_dirs: Vec::new(),
        current_event_id: 1,
        device,
        live: false,
        consume: None,
    })
}

fn observe(
    scope: &swamp_core::scope::EffectiveScope,
    store: &Path,
    full: bool,
    source: &dyn swamp_core::fs_events::FsEventsSource,
) -> report::ScopeObservation {
    report::observe_scope(
        scope,
        ObservationParts::ALL,
        None,
        None,
        false,
        Some(store),
        None,
        true,
        false,
        false,
        full,
        source,
        30,
        24 * 60 * 60,
    )
    .expect("observe disposable scope")
}

fn unique_allocated_bytes(roots: &[PathBuf]) -> u64 {
    fn visit(path: &Path, seen: &mut HashSet<(u64, u64)>, bytes: &mut u64) {
        let Ok(meta) = fs::symlink_metadata(path) else {
            return;
        };
        if meta.file_type().is_symlink() {
            return;
        }
        if meta.is_dir() {
            let Ok(entries) = fs::read_dir(path) else {
                return;
            };
            for entry in entries.flatten() {
                visit(&entry.path(), seen, bytes);
            }
        } else if meta.is_file() && seen.insert((meta.dev(), meta.ino())) {
            *bytes += meta.blocks() * 512;
        }
    }

    let mut seen = HashSet::new();
    let mut bytes = 0;
    for root in roots {
        visit(root, &mut seen, &mut bytes);
    }
    bytes
}

#[test]
fn full_scope_reconciles_cross_root_hardlinks_independent_of_order_and_roundtrips() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    git(&a, &["init", "-q"]);
    git(&b, &["init", "-q"]);
    fs::write(a.join("shared.bin"), vec![b'x'; 32 * 1024]).unwrap();
    fs::hard_link(a.join("shared.bin"), b.join("shared.bin")).unwrap();
    fs::write(a.join("only-a"), b"a").unwrap();
    fs::write(b.join("only-b"), b"b").unwrap();
    let roots = vec![a.clone(), b.clone()];
    let store = tempfile::tempdir().unwrap();
    let scope = make_scope(&home, &roots);

    let first = observe(&scope, store.path(), true, &initial_source(&roots));
    let expected = unique_allocated_bytes(&roots);
    let estimate = first
        .merged
        .reconciliation
        .unique_estimate
        .as_ref()
        .unwrap();
    assert_eq!(estimate.bytes, expected);
    assert!(!estimate.needs_reconciliation);
    let sharing = estimate.sharing.as_ref().unwrap();
    assert_eq!(sharing.groups.len(), 1);
    assert_eq!(sharing.groups[0].containers.len(), 2);
    assert!(!sharing.groups[0].unresolved_links);
    assert_eq!(
        sharing.groups[0].bytes,
        fs::metadata(a.join("shared.bin")).unwrap().blocks() * 512
    );

    let stored = report::report_scope_from_store(&scope, store.path()).unwrap();
    let roundtripped = stored
        .report
        .reconciliation
        .unique_estimate
        .expect("reconciliation estimate persisted");
    assert_eq!(roundtripped.bytes, estimate.bytes);
    assert_eq!(roundtripped.sharing, estimate.sharing);
    assert_eq!(roundtripped.reconciled_at, estimate.reconciled_at);
    assert_eq!(
        roundtripped.needs_reconciliation,
        estimate.needs_reconciliation
    );

    let reversed_store = tempfile::tempdir().unwrap();
    let reversed = vec![b, a];
    let reversed_scope = make_scope(&home, &reversed);
    let reverse_report = observe(
        &reversed_scope,
        reversed_store.path(),
        true,
        &initial_source(&reversed),
    );
    assert_eq!(
        reverse_report
            .merged
            .reconciliation
            .unique_estimate
            .unwrap()
            .bytes,
        expected,
        "scope input order must not affect unique-byte accounting"
    );
}

#[test]
fn incremental_observation_keeps_prior_estimate_stale_until_full_reconciliation() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    disable_too_soon_floor();
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    git(&a, &["init", "-q"]);
    git(&b, &["init", "-q"]);
    for root in [&a, &b] {
        fs::write(root.join("README.md"), b"fixture\n").unwrap();
        fs::write(root.join(".gitignore"), b"target/\n").unwrap();
        git(root, &["add", "README.md", ".gitignore"]);
        git(root, &["commit", "-q", "-m", "fixture"]);
    }
    fs::create_dir_all(a.join("target")).unwrap();
    fs::create_dir_all(b.join("target")).unwrap();
    fs::create_dir_all(a.join("target/unchanged-tree")).unwrap();
    for i in 0..96 {
        fs::write(
            a.join("target/unchanged-tree").join(format!("f-{i}")),
            vec![i as u8; 512],
        )
        .unwrap();
    }
    fs::write(a.join("target/shared.bin"), vec![b'x'; 16 * 1024]).unwrap();
    fs::hard_link(a.join("target/shared.bin"), b.join("target/shared.bin")).unwrap();
    let roots = vec![a.clone(), b.clone()];
    let scope = make_scope(&home, &roots);
    let store = tempfile::tempdir().unwrap();

    let baseline = observe(&scope, store.path(), true, &initial_source(&roots));
    let prior = baseline.merged.reconciliation.unique_estimate.unwrap();
    assert!(!prior.needs_reconciliation);

    // No filesystem event: both roots are unchanged, so the incremental
    // observer should do no traversal and must not run the scope census.
    let incremental_source = source(Vec::new(), 2);
    let (incremental, work): (report::ScopeObservation, WorkCounters) =
        work_counters::measured(|| observe(&scope, store.path(), false, &incremental_source));

    let carried = incremental.merged.reconciliation.unique_estimate.unwrap();
    assert_eq!(
        carried.bytes, prior.bytes,
        "fast refresh preserves prior estimate"
    );
    assert!(carried.needs_reconciliation);
    assert_eq!(carried.sharing, prior.sharing);
    assert!(
        work.dirs_listed == 0
            && work.files_statted == 0
            && work.header_bytes_read == 0
            && work.subprocess_spawns == 0,
        "incremental pass traversed unchanged roots or ran the census: {work:?}; notes={:?}",
        incremental.merged.notes
    );
    let roundtripped = report::report_scope_from_store(&scope, store.path())
        .unwrap()
        .report
        .reconciliation
        .unique_estimate
        .expect("incremental estimate persisted");
    assert_eq!(roundtripped.bytes, carried.bytes);
    assert_eq!(roundtripped.reconciled_at, carried.reconciled_at);
    assert!(roundtripped.needs_reconciliation);
    assert_eq!(roundtripped.sharing, prior.sharing);

    // A real localized event in B's target container must not force a
    // root-wide walk of A. The source canonicalizes and filters events
    // against each request root, as a live watcher does.
    fs::remove_file(b.join("target/shared.bin")).unwrap();
    fs::write(b.join("target/shared.bin"), vec![b'y'; 24 * 1024]).unwrap();
    let target = fs::canonicalize(b.join("target")).unwrap();
    let (changed, changed_work) =
        work_counters::measured(|| observe(&scope, store.path(), false, &source(vec![target], 3)));
    let changed_estimate = changed.merged.reconciliation.unique_estimate.unwrap();
    assert_eq!(changed_estimate.bytes, prior.bytes);
    assert!(changed_estimate.needs_reconciliation);
    assert_eq!(changed_estimate.sharing, prior.sharing);
    assert!(
        changed
            .merged
            .notes
            .iter()
            .any(|n| n.contains("mode=incremental")),
        "the changed-container observation should remain incremental: {:?}",
        changed.merged.notes
    );
    assert!(
        changed_work.files_statted < 96,
        "localized event traversed unchanged target contents: {changed_work:?}"
    );

    // Replacing and then removing a link changes the unique allocation.
    // Only explicit full observations recompute that aggregate.
    let expected_after_replace = unique_allocated_bytes(&roots);
    let reconciled = observe(&scope, store.path(), true, &source(Vec::new(), 4));
    let current = reconciled.merged.reconciliation.unique_estimate.unwrap();
    assert_eq!(current.bytes, expected_after_replace);
    assert!(!current.needs_reconciliation);
    assert!(current.sharing.as_ref().unwrap().groups.is_empty());

    fs::remove_file(a.join("target/shared.bin")).unwrap();
    let expected_after_remove = unique_allocated_bytes(&roots);
    let removed = observe(&scope, store.path(), true, &source(Vec::new(), 5));
    let current = removed.merged.reconciliation.unique_estimate.unwrap();
    assert_eq!(current.bytes, expected_after_remove);
    assert!(!current.needs_reconciliation);
}

#[test]
fn partial_family_observation_persists_needs_reconciliation_status() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("project");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    fs::write(root.join("README.md"), b"scope fixture\n").unwrap();
    let roots = vec![root.clone()];
    let scope = make_scope(&home, &roots);
    let store = tempfile::tempdir().unwrap();

    let full = observe(&scope, store.path(), true, &initial_source(&roots));
    assert!(
        !full
            .merged
            .reconciliation
            .unique_estimate
            .unwrap()
            .needs_reconciliation
    );

    let partial = report::observe_scope(
        &scope,
        ObservationParts::WALK_ONLY,
        None,
        None,
        false,
        Some(store.path()),
        None,
        true,
        false,
        false,
        false,
        &source(Vec::new(), 2),
        30,
        24 * 60 * 60,
    )
    .expect("partial walk-only observation");
    assert!(
        partial
            .merged
            .reconciliation
            .unique_estimate
            .unwrap()
            .needs_reconciliation
    );

    let stored = report::report_scope_from_store(&scope, store.path()).unwrap();
    let persisted = stored
        .report
        .reconciliation
        .unique_estimate
        .expect("old aggregate remains available but explicitly marked");
    assert!(persisted.needs_reconciliation);
}

#[test]
fn nested_external_hardlink_is_included_alongside_project_files() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    let cargo_home = project.join(".cargo");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&cargo_home).unwrap();
    git(&project, &["init", "-q"]);
    let shared = project.join("shared.bin");
    fs::write(&shared, vec![b's'; 32 * 1024]).unwrap();
    fs::hard_link(&shared, cargo_home.join("config.toml")).unwrap();
    fs::create_dir_all(cargo_home.join("bin")).unwrap();
    fs::write(cargo_home.join("bin/cargo"), vec![b'c'; 4096]).unwrap();

    let mut env_vars = std::collections::HashMap::new();
    env_vars.insert("CARGO_HOME".to_string(), cargo_home.display().to_string());
    let env = Environment::fixture(home.clone(), env_vars, Platform::MacOS);
    let registry = Registry::with_builtins();
    let config = ScanConfig {
        defaults: false,
        include: vec![project.display().to_string()],
        enabled_detectors: vec!["cargo-home".into()],
        ..Default::default()
    };
    let scope = resolve_effective_scope(&env, &config, &[], &registry, 1_000);
    assert!(
        scope
            .external_pruned_subtrees
            .iter()
            .any(|p| p.path == cargo_home),
        "the project walker must hand the nested Cargo home to the external detector"
    );

    let canonical_project = fs::canonicalize(&project).unwrap();
    let roots = vec![canonical_project.clone()];
    let store = tempfile::tempdir().unwrap();
    let observation = observe(&scope, store.path(), true, &initial_source(&roots));
    assert!(
        observation
            .external_units
            .iter()
            .any(|u| { u.path == fs::canonicalize(&cargo_home).unwrap() }),
        "fixture must actually measure the nested external unit"
    );
    assert_eq!(
        observation
            .merged
            .reconciliation
            .unique_estimate
            .unwrap()
            .bytes,
        unique_allocated_bytes(&[canonical_project]),
        "the unique census must include the detector-owned nested tree exactly once"
    );
}

#[test]
fn external_project_hardlink_is_counted_when_external_root_is_disjoint() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    let cargo_home = tmp.path().join("cargo-home");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&project).unwrap();
    git(&project, &["init", "-q"]);
    let project_file = project.join("shared.bin");
    let cargo_file = cargo_home.join("config.toml");
    fs::create_dir_all(&cargo_home).unwrap();
    fs::write(&project_file, vec![b'p'; 24 * 1024]).unwrap();
    fs::hard_link(&project_file, &cargo_file).unwrap();
    fs::write(cargo_home.join("outside.bin"), vec![b'o'; 8 * 1024]).unwrap();

    let mut env_vars = std::collections::HashMap::new();
    env_vars.insert("CARGO_HOME".to_string(), cargo_home.display().to_string());
    let env = Environment::fixture(home.clone(), env_vars, Platform::MacOS);
    let registry = Registry::with_builtins();
    let config = ScanConfig {
        defaults: false,
        include: vec![project.display().to_string()],
        enabled_detectors: vec!["cargo-home".into()],
        ..Default::default()
    };
    let scope = resolve_effective_scope(&env, &config, &[], &registry, 1_000);
    let canonical_project = fs::canonicalize(&project).unwrap();
    let canonical_cargo_home = fs::canonicalize(&cargo_home).unwrap();
    let roots = vec![canonical_project.clone()];
    let store = tempfile::tempdir().unwrap();
    let observation = observe(&scope, store.path(), true, &initial_source(&roots));
    assert!(
        observation
            .external_units
            .iter()
            .any(|u| { u.path.starts_with(&canonical_cargo_home) })
    );
    assert_eq!(
        observation
            .merged
            .reconciliation
            .unique_estimate
            .unwrap()
            .bytes,
        unique_allocated_bytes(&[canonical_project, canonical_cargo_home]),
        "one hardlinked inode shared by project and external roots contributes once"
    );
}

#[test]
fn a_known_missing_root_does_not_block_an_observed_set_reconciliation() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    let absent = tmp.path().join("unused-root");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&project).unwrap();
    git(&project, &["init", "-q"]);
    fs::write(project.join("payload"), vec![b'z'; 12 * 1024]).unwrap();
    let roots = vec![project.clone(), absent.clone()];
    let scope = make_scope(&home, &roots);
    let store = tempfile::tempdir().unwrap();

    let observation = observe(&scope, store.path(), true, &initial_source(&roots));
    let estimate = observation
        .merged
        .reconciliation
        .unique_estimate
        .expect("the present observed root is reconcilable");
    assert!(!estimate.needs_reconciliation);
    assert_eq!(
        estimate.bytes,
        unique_allocated_bytes(&[fs::canonicalize(project).unwrap()])
    );
    assert!(observation.coverage.iter().any(|c| {
        c.path == absent && matches!(c.status, swamp_core::coverage::RegionStatus::Missing)
    }));
}
