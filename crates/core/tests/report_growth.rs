//! R4: growth since previous observation, exercised end-to-end through
//! `report_with` against a temp store dir and the shared fixture.

#[path = "fixture/mod.rs"]
mod fixture;

use std::fs;
use std::time::Duration;
use swamp_core::entities::now;
use swamp_core::growth::{history_series, series_key, volume_store_dir};
use swamp_core::report::{ArtifactKind, Report, report_with, report_with_observe};

fn node_modules_key_and_bytes(report: &Report, fx: &fixture::Fixture) -> (String, u64) {
    let project = report
        .projects
        .iter()
        .find(|p| p.name == fx.checkout_name)
        .expect("checkout project");
    let worktree = project
        .worktrees
        .iter()
        .find(|w| w.path == fx.checkout)
        .expect("main worktree");
    let artifact = worktree
        .artifacts
        .iter()
        .find(|a| a.path == fx.node_modules)
        .expect("node_modules row");
    let rel_path = artifact
        .path
        .strip_prefix(&worktree.path)
        .expect("artifact is under worktree")
        .display()
        .to_string();
    let kind = format!("{:?}", artifact.kind);
    (
        series_key(&project.project_id, &worktree.worktree_id, &kind, &rel_path),
        artifact.bytes,
    )
}

fn assert_persisted_latest(store: &std::path::Path, root: &std::path::Path, key: &str, bytes: u64) {
    let (series, _) = history_series(&volume_store_dir(store, root), 24 * 60 * 60, 2, now() + 1);
    assert_eq!(
        series.get(key).and_then(|values| values.last().copied()),
        Some(Some(bytes)),
        "current.parquet must persist the latest byte-only update"
    );
}

fn delta_count(volume_dir: &std::path::Path) -> usize {
    fs::read_dir(volume_dir.join("deltas"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "parquet"))
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn byte_only_updates_persist_sequentially_and_noop_without_redundant_delta() {
    let tmp = tempfile::tempdir().expect("tmp root");
    let fx = fixture::build(tmp.path());
    let store = tempfile::tempdir().expect("tmp store");

    let first =
        report_with(&fx.root, None, false, Some(store.path()), Some("24h")).expect("first report");
    let (key, _) = node_modules_key_and_bytes(&first, &fx);
    let volume = volume_store_dir(store.path(), &fx.root);

    // Keep observation timestamps distinct so the reverse-delta series has
    // an unambiguous latest point even on filesystems with coarse clocks.
    std::thread::sleep(Duration::from_millis(1100));
    let first_update_bytes = 1024 * 1024;
    fs::write(
        fx.node_modules.join("persistence-probe"),
        vec![b'a'; first_update_bytes],
    )
    .expect("write first probe");
    let first_update = report_with(&fx.root, None, false, Some(store.path()), Some("24h"))
        .expect("first byte-only update");
    let (_, first_expected) = node_modules_key_and_bytes(&first_update, &fx);
    assert_persisted_latest(store.path(), &fx.root, &key, first_expected);

    let deltas_after_first_update = delta_count(&volume);
    assert!(
        deltas_after_first_update > 0,
        "a byte-only update must append its reverse delta"
    );

    std::thread::sleep(Duration::from_millis(1100));
    let second_update_bytes = 2 * 1024 * 1024;
    fs::write(
        fx.node_modules.join("persistence-probe"),
        vec![b'b'; second_update_bytes],
    )
    .expect("write second probe");
    let second_update = report_with(&fx.root, None, false, Some(store.path()), Some("24h"))
        .expect("second byte-only update");
    let (_, second_expected) = node_modules_key_and_bytes(&second_update, &fx);
    assert_persisted_latest(store.path(), &fx.root, &key, second_expected);

    let deltas_after_second_update = delta_count(&volume);
    assert!(
        deltas_after_second_update > deltas_after_first_update,
        "each sequential byte-only update must append one reverse delta"
    );

    std::thread::sleep(Duration::from_millis(1100));
    let unchanged = report_with(&fx.root, None, false, Some(store.path()), Some("24h"))
        .expect("unchanged report");
    let (_, unchanged_expected) = node_modules_key_and_bytes(&unchanged, &fx);
    assert_persisted_latest(store.path(), &fx.root, &key, unchanged_expected);
    assert_eq!(
        delta_count(&volume),
        deltas_after_second_update,
        "a no-change observation after an update must not append a redundant delta"
    );
}

#[test]
fn growing_one_artifact_shows_growth_there_and_zero_elsewhere() {
    let tmp = tempfile::tempdir().expect("tmp root");
    let fx = fixture::build(tmp.path());
    let store = tempfile::tempdir().expect("tmp store");

    // First observation: persists current-state, no prior baseline.
    let first =
        report_with(&fx.root, None, false, Some(store.path()), Some("1h")).expect("first report");
    let checkout = first
        .projects
        .iter()
        .find(|p| p.name == fx.checkout_name)
        .expect("checkout project");
    let main = checkout
        .worktrees
        .iter()
        .find(|w| w.path == fx.checkout)
        .expect("main worktree");
    for artifact in &main.artifacts {
        assert_eq!(
            artifact.growth_bytes, None,
            "first observation must have no growth baseline: {artifact:?}"
        );
    }

    // Grow node_modules by exactly 5 MiB.
    let grow_bytes = 5 * 1024 * 1024;
    fs::write(fx.node_modules.join("growth-probe"), vec![b'g'; grow_bytes])
        .expect("write growth probe");

    let second =
        report_with(&fx.root, None, false, Some(store.path()), Some("1h")).expect("second report");
    let checkout2 = second
        .projects
        .iter()
        .find(|p| p.name == fx.checkout_name)
        .expect("checkout project");
    let main2 = checkout2
        .worktrees
        .iter()
        .find(|w| w.path == fx.checkout)
        .expect("main worktree");

    let node_modules_row = main2
        .artifacts
        .iter()
        .find(|a| a.path == fx.node_modules)
        .expect("node_modules row");
    assert_eq!(
        node_modules_row.growth_bytes,
        Some(grow_bytes as i64),
        "node_modules should show exactly the added bytes as growth"
    );

    for artifact in &main2.artifacts {
        if artifact.path == fx.node_modules {
            continue;
        }
        assert_eq!(
            artifact.growth_bytes,
            Some(0),
            "unrelated artifact must show zero growth: {artifact:?}"
        );
    }
}

#[test]
fn deleting_and_recreating_target_counts_one_regrowth() {
    let tmp = tempfile::tempdir().expect("tmp root");
    let fx = fixture::build(tmp.path());
    let store = tempfile::tempdir().expect("tmp store");

    report_with(&fx.root, None, false, Some(store.path()), Some("1h")).expect("first report");

    fs::remove_dir_all(&fx.target_dir).expect("delete target/");
    let after_delete =
        report_with(&fx.root, None, false, Some(store.path()), Some("1h")).expect("second report");
    let checkout = after_delete
        .projects
        .iter()
        .find(|p| p.name == fx.checkout_name)
        .expect("checkout project");
    let main = checkout
        .worktrees
        .iter()
        .find(|w| w.path == fx.checkout)
        .expect("main worktree");
    assert!(
        !main.artifacts.iter().any(|a| a.path == fx.target_dir),
        "deleted target/ must not appear as a report row (tombstones are store-internal)"
    );

    fs::create_dir_all(&fx.target_dir).expect("recreate target/");
    fs::write(fx.target_dir.join("rebuilt"), vec![b'x'; 4096]).expect("write rebuilt file");

    let after_recreate =
        report_with(&fx.root, None, false, Some(store.path()), Some("1h")).expect("third report");
    let checkout3 = after_recreate
        .projects
        .iter()
        .find(|p| p.name == fx.checkout_name)
        .expect("checkout project");
    let main3 = checkout3
        .worktrees
        .iter()
        .find(|w| w.path == fx.checkout)
        .expect("main worktree");
    let target_row = main3
        .artifacts
        .iter()
        .find(|a| a.path == fx.target_dir && a.kind == ArtifactKind::BuildOutput)
        .expect("recreated target/ row");
    assert_eq!(
        target_row.regrowth_count, 1,
        "one absent -> present transition must count as exactly one regrowth"
    );
}

#[test]
fn observed_report_covers_every_worktree_and_leaves_unowned_rows_untouched() {
    // A second observation with no filesystem changes must show zero
    // growth everywhere -- including the linked worktree and the nested
    // repo's own build/ artifact -- and must not disturb the unowned
    // rows (shared cache, loose file), which the growth store never
    // tracks.
    let tmp = tempfile::tempdir().expect("tmp root");
    let fx = fixture::build(tmp.path());
    let store = tempfile::tempdir().expect("tmp store");

    report_with(&fx.root, None, false, Some(store.path()), Some("1h")).expect("first report");
    let second =
        report_with(&fx.root, None, false, Some(store.path()), Some("1h")).expect("second report");

    let checkout = second
        .projects
        .iter()
        .find(|p| p.name == fx.checkout_name)
        .expect("checkout project");
    let main = checkout
        .worktrees
        .iter()
        .find(|w| w.path == fx.checkout)
        .expect("main worktree");
    let target_row = main
        .artifacts
        .iter()
        .find(|a| a.path == fx.target_dir)
        .expect("target row");
    assert_eq!(target_row.bytes, fx.target_bytes);
    assert_eq!(target_row.growth_bytes, Some(0));
    let dist_row = main
        .artifacts
        .iter()
        .find(|a| a.path == fx.dist_dir)
        .expect("dist row");
    assert_eq!(dist_row.bytes, fx.dist_bytes);
    assert_eq!(dist_row.growth_bytes, Some(0));
    let node_modules_row = main
        .artifacts
        .iter()
        .find(|a| a.path == fx.node_modules)
        .expect("node_modules row");
    assert_eq!(node_modules_row.bytes, fx.node_modules_bytes);

    assert!(
        checkout
            .worktrees
            .iter()
            .any(|w| w.path == fx.linked_worktree),
        "linked worktree row must still be present"
    );

    let nested_project = second
        .projects
        .iter()
        .find(|p| p.worktrees.iter().any(|w| w.path == fx.nested_repo))
        .expect("nested repo project row");
    let nested_worktree = nested_project
        .worktrees
        .iter()
        .find(|w| w.path == fx.nested_repo)
        .expect("nested repo worktree row");
    let nested_build_row = nested_worktree
        .artifacts
        .iter()
        .find(|a| a.path == fx.nested_repo_build)
        .expect("nested repo build row");
    assert_eq!(nested_build_row.bytes, fx.nested_repo_build_bytes);
    assert_eq!(nested_build_row.growth_bytes, Some(0));

    let shared_cache_row = second
        .unowned
        .iter()
        .find(|u| u.path_or_object == fx.shared_cache.display().to_string())
        .expect("shared cache stays unowned");
    assert_eq!(shared_cache_row.bytes, fx.shared_cache_bytes);

    // #R10 item 1: unowned rows fold per directory, never per file, so
    // the loose file's row is keyed by its containing directory.
    let loose_row = second
        .unowned
        .iter()
        .find(|u| u.path_or_object == fx.loose_dir.display().to_string())
        .expect("loose file's directory stays unowned");
    assert_eq!(loose_row.bytes, fx.loose_file_bytes);
}

#[test]
fn golden_report_without_observe_flag_stays_read_only() {
    // report() (no store dir) must remain a pure read: no files created,
    // growth always None. This is the R1 golden test's contract; pinned
    // again here so a future change to report_with's defaults cannot
    // silently start writing to disk from a plain `report()` call.
    let tmp = tempfile::tempdir().expect("tmp root");
    let fx = fixture::build(tmp.path());
    let r = swamp_core::report::report(&fx.root, Some(&fx.docker_facts))
        .expect("report() should not error");
    assert!(!r.projects.is_empty());
    for project in &r.projects {
        for worktree in &project.worktrees {
            for artifact in &worktree.artifacts {
                assert_eq!(artifact.growth_bytes, None);
                assert_eq!(artifact.regrowth_count, 0);
            }
        }
    }
}

#[test]
fn no_observe_still_reports_growth_from_an_existing_store() {
    // Issue #32 item 5: growth must come from the store regardless of
    // whether *this* call observed. Two real (`observe: true`)
    // observations establish a store with history, then a read-only
    // (`observe: false`, i.e. `--no-observe`) call over the grown state
    // must still show the growth -- not `None`/`-` -- and must not have
    // written a third observation into the store.
    let tmp = tempfile::tempdir().expect("tmp root");
    let fx = fixture::build(tmp.path());
    let store = tempfile::tempdir().expect("tmp store");

    report_with_observe(&fx.root, None, false, Some(store.path()), Some("1h"), true)
        .expect("first observation");

    let grow_bytes = 3 * 1024 * 1024;
    fs::write(
        fx.node_modules.join("no-observe-probe"),
        vec![b'g'; grow_bytes],
    )
    .expect("write growth probe");

    report_with_observe(&fx.root, None, false, Some(store.path()), Some("1h"), true)
        .expect("second observation");

    let volume_dir_count_before = fs::read_dir(store.path())
        .map(|entries| entries.count())
        .unwrap_or(0);

    // Grow again, but this time call read-only.
    let grow_more = 1024 * 1024;
    fs::write(
        fx.node_modules.join("no-observe-probe-2"),
        vec![b'h'; grow_more],
    )
    .expect("write second growth probe");

    let readonly =
        report_with_observe(&fx.root, None, false, Some(store.path()), Some("1h"), false)
            .expect("read-only report");

    let checkout = readonly
        .projects
        .iter()
        .find(|p| p.name == fx.checkout_name)
        .expect("checkout project");
    let main = checkout
        .worktrees
        .iter()
        .find(|w| w.path == fx.checkout)
        .expect("main worktree");
    let node_modules_row = main
        .artifacts
        .iter()
        .find(|a| a.path == fx.node_modules)
        .expect("node_modules row");
    assert!(
        node_modules_row.growth_bytes.is_some(),
        "no-observe report must still show growth from the store's existing history: {node_modules_row:?}"
    );
    assert!(
        node_modules_row.growth_bytes.unwrap() > 0,
        "growth must be positive after two real growth events: {node_modules_row:?}"
    );

    let volume_dir_count_after = fs::read_dir(store.path())
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(
        volume_dir_count_before, volume_dir_count_after,
        "a read-only (--no-observe) call must not persist a new observation"
    );
}
