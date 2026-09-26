//! What an ordinary refresh of an unchanged build container costs.
//!
//! The claim in `docs/build-artifacts.md` is "zero listings and zero
//! manifest reads for an unchanged container". These tests assert the
//! counters rather than the wall clock, so the claim survives a faster
//! machine, and they assert the *other* direction too: without a trusted
//! window the container is identified again, at full price. A reuse test
//! with no matching re-identification test proves only that the cache
//! returns something.

use std::fs;
use std::path::{Path, PathBuf};
use swamp_core::build_adapters::{
    BuildContainer, BuildCtx, ContainerCache, FoldedDir, FoldedIndex, identify_all,
    registry::Registry,
};
use swamp_core::fs_events::EventCoverage;

const STORED_AT: u64 = 1_000;
const NOW: u64 = 2_000;

/// A Node project big enough that re-identification is visibly
/// expensive: 300 installed packages, each with a manifest.
fn node_project(root: &Path) -> Vec<(PathBuf, u64, u64)> {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("package.json"), br#"{"name":"app"}"#).unwrap();
    let nm = root.join("node_modules");
    let mut dirs = vec![(nm.clone(), 100_000, 900_000u64)];
    for i in 0..300 {
        let p = nm.join(format!("pkg-{i:03}"));
        fs::create_dir_all(&p).unwrap();
        fs::write(
            p.join("package.json"),
            format!("{{\"name\":\"pkg-{i:03}\",\"version\":\"1.0.{i}\"}}"),
        )
        .unwrap();
        dirs.push((p, 300, 900_000));
    }
    let dist = root.join("dist");
    fs::create_dir_all(&dist).unwrap();
    dirs.push((dist, 5_000, 900_000));
    dirs
}

fn index(dirs: &[(PathBuf, u64, u64)]) -> FoldedIndex {
    FoldedIndex::from_dirs(dirs.iter().map(|(p, b, m)| FoldedDir {
        path: p.clone(),
        allocated_total: *b,
        mtime_max: *m,
        complete: true,
    }))
}

fn run(
    root: &Path,
    dirs: &[(PathBuf, u64, u64)],
    coverage: &EventCoverage,
    cache: &ContainerCache,
) -> Vec<swamp_core::artifact::NestedArtifact> {
    let idx = index(dirs);
    let ctx = BuildCtx::new(NOW, &idx, coverage, cache);
    let candidates: Vec<PathBuf> = dirs.iter().map(|(p, _, _)| p.clone()).collect();
    identify_all(
        &Registry::with_builtins(),
        &[(root.to_path_buf(), candidates)],
        &[],
        &ctx,
    )
}

#[test]
fn unchanged_container_lists_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("app");
    let dirs = node_project(&root);

    let none = EventCoverage::untrusted();
    let cold = ContainerCache::disabled();
    let (first, cost_cold) =
        swamp_core::work_counters::measured(|| run(&root, &dirs, &none, &cold));
    assert!(first.len() > 300, "the fixture identified {}", first.len());
    assert!(
        cost_cold.header_bytes_read > 0,
        "a cold pass must actually read the package manifests, or the warm pass proves nothing"
    );

    // Same bytes, a trusted window covering the whole project, and the
    // previous pass's units in hand.
    let trusted = EventCoverage::trusted(root.clone(), Vec::new(), STORED_AT);
    let warm = ContainerCache::from_previous(first.clone(), STORED_AT);
    let (second, cost_warm) =
        swamp_core::work_counters::measured(|| run(&root, &dirs, &trusted, &warm));

    assert_eq!(
        cost_warm.dirs_listed, 0,
        "an unchanged container must not re-list a directory"
    );
    assert_eq!(
        cost_warm.header_bytes_read, 0,
        "an unchanged container must not re-read a manifest"
    );
    assert_eq!(
        cost_warm.files_statted, 0,
        "an unchanged container must not re-stat a member"
    );
    assert!(
        cost_warm.containers_reused > 0,
        "nothing was replayed, so the zero counters above mean nothing"
    );
    assert_eq!(
        first.len(),
        second.len(),
        "the replay must produce the same rows, not fewer"
    );
    for (a, b) in first.iter().zip(&second) {
        assert_eq!((&a.id, a.bytes, &a.role), (&b.id, b.bytes, &b.role));
    }
}

#[test]
fn without_trusted_coverage_the_container_is_identified_again() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("app");
    let dirs = node_project(&root);
    let none = EventCoverage::untrusted();
    let cold = ContainerCache::disabled();
    let first = run(&root, &dirs, &none, &cold);

    // The cache is full and the bytes are unchanged -- but there is no
    // window, so there is no evidence, so there is no reuse.
    let warm = ContainerCache::from_previous(first.clone(), STORED_AT);
    let (_second, cost) = swamp_core::work_counters::measured(|| run(&root, &dirs, &none, &warm));
    assert_eq!(
        cost.containers_reused, 0,
        "a full cache with no window is not evidence that nothing changed"
    );
    assert!(
        cost.header_bytes_read > 0,
        "re-identification must actually re-read, not silently serve the cache"
    );
}

#[test]
fn an_event_inside_the_container_costs_a_reidentification() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("app");
    let dirs = node_project(&root);
    let none = EventCoverage::untrusted();
    let cold = ContainerCache::disabled();
    let first = run(&root, &dirs, &none, &cold);
    let warm = ContainerCache::from_previous(first.clone(), STORED_AT);

    // One file changed, deep inside. The container's own directory stamp
    // would not have moved; the window says it changed, and that is the
    // difference the gate exists for.
    let noisy = EventCoverage::trusted(
        root.clone(),
        vec![root.join("node_modules/pkg-042/index.js")],
        STORED_AT,
    );
    let (_second, cost) = swamp_core::work_counters::measured(|| run(&root, &dirs, &noisy, &warm));
    assert!(
        cost.header_bytes_read > 0,
        "an event under node_modules must cost that container a re-identification"
    );

    // And the *other* containers are still replayed: a change in
    // node_modules is not a change in dist.
    assert!(
        cost.containers_reused > 0,
        "a change in one container must not invalidate its siblings"
    );
}

#[test]
fn a_manifest_budget_bounds_a_large_installed_tree() {
    // 300 packages, a 200-manifest budget: the cost is bounded by the
    // budget, and the shortfall is a stated number rather than silence.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("app");
    let dirs = node_project(&root);
    let none = EventCoverage::untrusted();
    let cold = ContainerCache::disabled();
    let units = run(&root, &dirs, &none, &cold);
    let nm = units
        .iter()
        .find(|u| u.path == root.join("node_modules"))
        .expect("the installed tree");
    assert!(
        nm.coverage
            .limits
            .iter()
            .any(|l| l.contains("sized but not identified")),
        "the unread packages are counted in a stated limit: {:?}",
        nm.coverage.limits
    );
    let unknown = units
        .iter()
        .filter(|u| u.variant.unknowns.iter().any(|x| x == "package"))
        .count();
    assert_eq!(
        unknown, 100,
        "300 packages minus a 200-manifest budget is 100 explicit unknowns"
    );
}

#[test]
fn a_shared_store_container_is_replayed_on_the_same_gate() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("gradle-home");
    let caches = store.join("caches");
    fs::create_dir_all(caches.join("modules-2")).unwrap();
    let dirs: Vec<(PathBuf, u64, u64)> = vec![
        (store.clone(), 10_000, 900_000),
        (caches.clone(), 9_000, 900_000),
        (caches.join("modules-2"), 8_000, 900_000),
    ];
    let idx = index(&dirs);
    let shared = vec![BuildContainer::shared_store("gradle", store.clone())];

    let none = EventCoverage::untrusted();
    let cold = ContainerCache::disabled();
    let first = identify_all(
        &Registry::with_builtins(),
        &[],
        &shared,
        &BuildCtx::new(NOW, &idx, &none, &cold),
    );
    assert!(!first.is_empty());

    let trusted = EventCoverage::trusted(store.clone(), Vec::new(), STORED_AT);
    let warm = ContainerCache::from_previous(first.clone(), STORED_AT);
    let (second, cost) = swamp_core::work_counters::measured(|| {
        identify_all(
            &Registry::with_builtins(),
            &[],
            &shared,
            &BuildCtx::new(NOW, &idx, &trusted, &warm),
        )
    });
    assert_eq!(cost.dirs_listed, 0);
    assert!(cost.containers_reused > 0);
    assert_eq!(first.len(), second.len());
}

/// The recorded cost report for
/// `.oh/sessions/2026-09-21-build-adapters-node-jvm.md`: a cold pass, an
/// unchanged pass, and a one-container-changed pass over the same
/// fixture. Prints with `--nocapture`; the assertions are the shape, and
/// the numbers in the session note are one machine's observation.
#[test]
fn cost_report_cold_unchanged_and_one_container_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("app");
    let dirs = node_project(&root);
    let measure = |coverage: &EventCoverage, cache: &ContainerCache| {
        let t = std::time::Instant::now();
        let (units, counters) =
            swamp_core::work_counters::measured(|| run(&root, &dirs, coverage, cache));
        (units, counters, t.elapsed())
    };

    let none = EventCoverage::untrusted();
    let cold_cache = ContainerCache::disabled();
    let (cold_units, cold, t_cold) = measure(&none, &cold_cache);

    let warm_cache = ContainerCache::from_previous(cold_units.clone(), STORED_AT);
    let trusted = EventCoverage::trusted(root.clone(), Vec::new(), STORED_AT);
    let (_, warm, t_warm) = measure(&trusted, &warm_cache);

    let one_changed = EventCoverage::trusted(
        root.clone(),
        vec![root.join("node_modules/pkg-007/index.js")],
        STORED_AT,
    );
    let (_, partial, t_partial) = measure(&one_changed, &warm_cache);

    println!("--- BUILD ADAPTER COST REPORT ---");
    println!(
        "fixture: 300-package node_modules + dist, {} identified units",
        cold_units.len()
    );
    for (name, c, t) in [
        ("cold (no cache, no window)", &cold, t_cold),
        ("unchanged (trusted window)", &warm, t_warm),
        ("one container changed", &partial, t_partial),
    ] {
        println!(
            "{name}: {t:?} dirs_listed={} files_statted={} manifest_bytes={} \
             containers_reused={} containers_identified={}",
            c.dirs_listed,
            c.files_statted,
            c.header_bytes_read,
            c.containers_reused,
            c.containers_identified
        );
    }
    println!("--- END ---");

    assert_eq!(warm.header_bytes_read, 0);
    assert!(partial.header_bytes_read > 0);
    assert!(
        partial.header_bytes_read <= cold.header_bytes_read,
        "a one-container change must not cost more than a cold pass"
    );
    assert!(
        partial.containers_reused > 0,
        "the unaffected containers must still be replayed"
    );
}
