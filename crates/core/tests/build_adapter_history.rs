//! #65 through the real report pipeline, for adapters other than Cargo.
//!
//! `cargo_delivery` proves full/incremental agreement and container reuse
//! for Cargo. The reconciliation table on #65 asks for the same
//! equivalence "for non-Rust adapters and partial/unreadable/shared
//! cases", and for adversarial reclassification/new-nesting history
//! tests across adapters. These drive `report_full_mode_with_source`
//! against disposable Node and Gradle checkouts with a scripted event
//! source, so every pass goes through the folded walk, the build
//! consumer, the `EventCoverage` gate and the current + reverse-delta
//! store exactly as a user's refresh does.

use std::{
    fs,
    path::{Path, PathBuf},
};
use swamp_core::fs_events::{FsEventsPlan, FsEventsRequest, FsEventsSource};

/// An event source that reports the given paths as changed.
struct Live(Vec<PathBuf>);
impl FsEventsSource for Live {
    fn replay(&self, _: &FsEventsRequest) -> FsEventsPlan {
        FsEventsPlan::from_live(self.0.clone(), 1000, None)
    }
}

fn git_init(root: &Path) {
    fs::create_dir_all(root).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(root)
            .status()
            .unwrap()
            .success()
    );
}

fn write(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![7u8; bytes]).unwrap();
}

/// A Node checkout: an installed dependency, a build output and a
/// coverage report, with the ignore rules a real project has.
fn node_fixture() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap().join("web");
    git_init(&root);
    fs::write(
        root.join("package.json"),
        br#"{"name":"web","version":"1.0.0"}"#,
    )
    .unwrap();
    fs::write(root.join(".gitignore"), "node_modules/\ndist/\ncoverage/\n").unwrap();
    fs::create_dir_all(root.join("node_modules/left-pad")).unwrap();
    fs::write(
        root.join("node_modules/left-pad/package.json"),
        br#"{"name":"left-pad","version":"1.3.0"}"#,
    )
    .unwrap();
    write(&root.join("node_modules/left-pad/index.js"), 20_000);
    write(&root.join("dist/app.js"), 40_000);
    write(&root.join("coverage/lcov.info"), 12_000);
    (tmp, root)
}

fn observe(
    root: &Path,
    store: &Path,
    changed: Vec<PathBuf>,
    force_full: bool,
) -> swamp_core::Report {
    observe_excluding(root, store, changed, force_full, &[])
}

fn observe_excluding(
    root: &Path,
    store: &Path,
    changed: Vec<PathBuf>,
    force_full: bool,
    excluded: &[PathBuf],
) -> swamp_core::Report {
    // `docker_in_scope: false`: these fixtures assert exact
    // `containers_identified`/`containers_reused` counts for adapters
    // that have nothing to do with Docker. `report_full_mode_with_source`
    // hardcodes `docker_in_scope: true` (the documented pre-scope
    // behavior for entry points with no scope to consult), which makes
    // `consumers::docker` fetch live facts from whatever real Docker
    // daemon happens to be running on the machine executing the test --
    // its BuildKit builders (`daemon-store://<name>`, one per builder)
    // are never inside any trusted `EventCoverage` window (they are not
    // filesystem paths under the walked root), so they are
    // unconditionally re-"identified" every pass. On a machine with N
    // active builders that adds exactly N to every
    // `containers_identified` count, independent of the fixture -- the
    // root cause of the `an_unchanged_node_checkout_...`/
    // `cost_report_real_pipeline_...`/`python_go_swift_and_android_...`
    // failures previously misattributed to a Linux-only "container
    // re-identification flapping" defect (see the removed TODO(linux-on-
    // gates) note below). Using the scope-aware entry point with
    // `docker_in_scope: false` makes these fixtures' results depend only
    // on the fixture, not on the ambient environment.
    swamp_core::report::report_full_mode_scoped(
        root,
        None,
        false,
        Some(store),
        Some("1h"),
        true,
        false,
        false,
        force_full,
        &Live(changed),
        excluded,
        false,
    )
    .unwrap()
}

/// What two passes must agree on: identity, role, bytes, presence.
fn facts(r: &swamp_core::Report) -> Vec<(String, String, u64, bool)> {
    let mut v: Vec<_> = r
        .nested_artifacts
        .iter()
        .filter(|u| u.adapter.as_deref() != Some("cargo"))
        .map(|u| (u.id.clone(), u.role.label().to_string(), u.bytes, u.present))
        .collect();
    v.sort();
    v
}

/// A pass over a fresh store: the full answer for the tree as it is now.
fn fresh_full(root: &Path) -> swamp_core::Report {
    let store = tempfile::tempdir().unwrap();
    observe(root, store.path(), vec![], true)
}

#[test]
fn checkoutless_incremental_remeasurement_matches_full_across_file_mutations() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let store = tempfile::tempdir().unwrap();
    let file = root.join("loose/cache/file");
    write(&file, 16_384);
    for _ in 0..3 {
        observe(&root, store.path(), vec![], false);
    }
    let unchanged = observe(&root, store.path(), vec![], false);
    assert!(
        unchanged
            .notes
            .iter()
            .any(|n| n.starts_with("fsevents: mode=incremental")),
        "{:?}",
        unchanged.notes
    );
    for mutation in 0..5 {
        match mutation {
            0 => write(&root.join("new/file"), 32_768),
            1 => write(&file, 65_536),
            2 => write(&file, 4_096),
            3 => fs::rename(&file, root.join("renamed")).unwrap(),
            _ => fs::remove_file(root.join("renamed")).unwrap(),
        }
        let incremental = observe(
            &root,
            store.path(),
            vec![root.clone(), root.join("loose/cache"), root.join("new")],
            false,
        );
        let full = fresh_full(&root);
        assert!(
            incremental
                .notes
                .iter()
                .any(|n| n.starts_with("fsevents: mode=incremental")),
            "changed unowned bytes must be measured locally: {:?}",
            incremental.notes
        );
        assert_eq!(
            incremental.reconciliation.walked_total, full.reconciliation.walked_total,
            "mutation {mutation}"
        );
        assert!(incremental.projects.is_empty());
    }
}

#[test]
fn mixed_root_refreshes_unowned_mutations_without_disabling_unchanged_reuse() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    git_init(&root.join("repo"));
    write(&root.join("repo/source"), 4096);
    let store = tempfile::tempdir().unwrap();
    let loose = root.join("loose/file");
    write(&loose, 16384);
    for _ in 0..3 {
        observe(&root, store.path(), vec![], false);
    }
    for mutation in 0..6 {
        match mutation {
            0 => write(&loose, 65536),
            1 => write(&loose, 4096),
            2 => fs::rename(&loose, root.join("loose/renamed")).unwrap(),
            3 => fs::remove_file(root.join("loose/renamed")).unwrap(),
            4 => write(&loose, 32768),
            _ => git_init(&root.join("loose")),
        }
        let actual = observe(
            &root,
            store.path(),
            vec![root.join(if mutation == 5 { "loose/.git" } else { "loose" })],
            false,
        );
        assert!(
            actual.notes.iter().any(|n| if mutation == 5 {
                n.contains("reason=unowned_changes")
            } else {
                n.starts_with("fsevents: mode=incremental")
            }),
            "{:?}",
            actual.notes
        );
        let expected = fresh_full(&root);
        assert_eq!(
            actual.reconciliation.walked_total,
            expected.reconciliation.walked_total
        );
        assert_eq!(
            actual.reconciliation.unowned,
            expected.reconciliation.unowned
        );
        let unchanged = observe(&root, store.path(), vec![], false);
        assert!(
            unchanged
                .notes
                .iter()
                .any(|n| n.starts_with("fsevents: mode=incremental")),
            "{:?}",
            unchanged.notes
        );
    }
    // The repair must not turn ordinary checkout edits into full-root walks.
    write(&root.join("repo/source"), 8192);
    let owned = observe(&root, store.path(), vec![root.join("repo")], false);
    assert!(
        owned
            .notes
            .iter()
            .any(|n| n.starts_with("fsevents: mode=incremental")),
        "{:?}",
        owned.notes
    );
    assert_eq!(
        owned.reconciliation.walked_total,
        fresh_full(&root).reconciliation.walked_total
    );
}

#[test]
fn unowned_refresh_cost_is_local_and_folded_boundaries_survive_storage() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let store = tempfile::tempdir().unwrap();
    write(&root.join("root-file"), 4096);
    write(&root.join("loose/file"), 4096);
    write(&root.join("target/deep/file"), 8192);
    for i in 0..2000 {
        write(&root.join(format!("unchanged/{i}")), 4096);
    }
    for _ in 0..3 {
        observe(&root, store.path(), vec![], false);
    }
    let (_, full_work) = swamp_core::work_counters::measured(|| fresh_full(&root));
    assert!(
        full_work.files_statted >= 2000,
        "instrument must see the reference traversal: {full_work:?}"
    );
    for (file, changed) in [
        ("root-file", ""),
        ("loose/file", "loose"),
        ("target/deep/file", "target/deep"),
    ] {
        write(&root.join(file), 65536);
        let (actual, work) = swamp_core::work_counters::measured(|| {
            observe(
                &root,
                store.path(),
                vec![root.join(file), root.join(changed), root.clone()],
                false,
            )
        });
        assert!(
            actual
                .notes
                .iter()
                .any(|n| n.starts_with("fsevents: mode=incremental")),
            "{:?}",
            actual.notes
        );
        assert_eq!(
            actual.reconciliation.walked_total,
            fresh_full(&root).reconciliation.walked_total
        );
        assert!(
            work.files_statted < 100,
            "unchanged sibling was traversed: {work:?}"
        );
        eprintln!("{changed}: {work:?}; reference: {full_work:?}");
    }
    let (_, unchanged) =
        swamp_core::work_counters::measured(|| observe(&root, store.path(), vec![], false));
    assert_eq!(unchanged.files_statted, 0, "{unchanged:?}");
    assert_eq!(unchanged.dirs_listed, 0, "{unchanged:?}");
    let (storm, storm_work) = swamp_core::work_counters::measured(|| {
        observe(
            &root,
            store.path(),
            (0..20)
                .map(|i| root.join(format!("unchanged/{i}")))
                .collect(),
            false,
        )
    });
    assert!(
        storm
            .notes
            .iter()
            .any(|n| n.contains("reason=too_many_changes")),
        "{:?}",
        storm.notes
    );
    assert!(
        storm_work.files_statted <= full_work.files_statted + 20,
        "event burst must not relist before choosing a full scan: {storm_work:?}"
    );
    write(&root.join("new/target/deep/file"), 32768);
    let actual = observe(
        &root,
        store.path(),
        vec![
            root.join("new/target/deep/file"),
            root.join("new/target/deep"),
        ],
        false,
    );
    let full = fresh_full(&root);
    assert_eq!(
        actual.reconciliation.walked_total,
        full.reconciliation.walked_total
    );
    assert!(actual.unowned.iter().any(|r| r.path_or_object
        == root.join("new/target").to_string_lossy()
        && r.measurement == Some(swamp_core::report::UnownedMeasurement::Subtree)));
}

#[test]
fn unowned_shared_links_refresh_locally_until_reconciliation() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let store = tempfile::tempdir().unwrap();
    write(&root.join("a/file"), 16384);
    fs::create_dir_all(root.join("b")).unwrap();
    fs::hard_link(root.join("a/file"), root.join("b/file")).unwrap();
    for _ in 0..3 {
        observe(&root, store.path(), vec![], false);
    }
    fs::remove_file(root.join("a/file")).unwrap();
    let (actual, work) = swamp_core::work_counters::measured(|| {
        observe(&root, store.path(), vec![root.join("a")], false)
    });
    assert!(
        actual.notes.iter().any(|n| n.contains("mode=incremental")),
        "sharing must not force a root walk: {:?}",
        actual.notes
    );
    assert!(work.dirs_listed <= 1, "{work:?}");
    assert!(actual.unowned.iter().any(|r| {
        r.measurement
            .is_some_and(|m| m.unique_needs_reconciliation())
    }));
    assert!(swamp_core::render::render_view_unowned(&actual).contains("need reconciliation"));
    let reconciled = observe(&root, store.path(), vec![], true);
    assert_eq!(
        reconciled.reconciliation.walked_total,
        fresh_full(&root).reconciliation.walked_total
    );
    assert!(!reconciled.unowned.iter().any(|r| {
        r.measurement
            .is_some_and(|m| m.unique_needs_reconciliation())
    }));

    let outside = tempfile::tempdir().unwrap();
    write(&outside.path().join("deep/file"), 131072);
    write(&root.join("replace/deep/file"), 32768);
    observe(&root, store.path(), vec![], true);
    fs::rename(root.join("replace"), outside.path().join("old")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("replace")).unwrap();
    let actual = observe(&root, store.path(), vec![root.join("replace/deep")], false);
    assert_eq!(
        actual.reconciliation.walked_total,
        fresh_full(&root).reconciliation.walked_total
    );
}

#[test]
fn unowned_refresh_preserves_excluded_subtrees() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let store = tempfile::tempdir().unwrap();
    write(&root.join("loose/file"), 4096);
    write(&root.join("target/cache/file"), 8192);
    let excluded = vec![root.join("loose/excluded"), root.join("target/excluded")];
    for p in &excluded {
        write(&p.join("file"), 131072);
    }
    for _ in 0..3 {
        observe_excluding(&root, store.path(), vec![], false, &excluded);
    }
    write(&root.join("loose/file"), 32768);
    write(&root.join("target/cache/file"), 65536);
    let actual = observe_excluding(
        &root,
        store.path(),
        vec![root.join("loose"), root.join("target/cache")],
        false,
        &excluded,
    );
    let full_store = tempfile::tempdir().unwrap();
    let full = observe_excluding(&root, full_store.path(), vec![], true, &excluded);
    assert!(
        actual
            .notes
            .iter()
            .any(|n| n.starts_with("fsevents: mode=incremental")),
        "{:?}",
        actual.notes
    );
    assert_eq!(
        actual.reconciliation.walked_total,
        full.reconciliation.walked_total
    );
    assert!(!actual.unowned.iter().any(|r| {
        excluded
            .iter()
            .any(|p| Path::new(&r.path_or_object).starts_with(p))
    }));
}

#[test]
fn missing_unowned_baseline_reconciles_instead_of_dropping_unchanged_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let store = tempfile::tempdir().unwrap();
    write(&root.join("a/file"), 8192);
    write(&root.join("b/file"), 16384);
    for _ in 0..3 {
        observe(&root, store.path(), vec![], false);
    }
    let baseline =
        swamp_core::growth::volume_store_dir(store.path(), &root).join("unowned.parquet");
    assert!(baseline.exists());
    fs::remove_file(baseline).unwrap(); // disposable fixture cache only
    write(&root.join("a/file"), 32768);
    let actual = observe(&root, store.path(), vec![root.join("a")], false);
    assert_eq!(
        actual.reconciliation.walked_total,
        fresh_full(&root).reconciliation.walked_total
    );
    assert!(
        actual
            .notes
            .iter()
            .any(|n| n.contains("reason=checkoutless_changes")),
        "{:?}",
        actual.notes
    );
}

#[test]
fn removed_cache_tag_reconciles_the_unowned_boundary() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let store = tempfile::tempdir().unwrap();
    write(&root.join("tagged/deep/file"), 8192);
    let tag = root.join("tagged/CACHEDIR.TAG");
    fs::write(&tag, b"Signature: 8a477f597d28d172789f06886806bc55\n").unwrap();
    for _ in 0..3 {
        observe(&root, store.path(), vec![], false);
    }
    fs::remove_file(&tag).unwrap();
    let actual = observe(&root, store.path(), vec![tag, root.join("tagged")], false);
    let full = fresh_full(&root);
    let boundaries = |r: &swamp_core::Report| {
        let mut rows: Vec<_> = r
            .unowned
            .iter()
            .map(|r| {
                (
                    r.path_or_object.clone(),
                    r.bytes,
                    format!("{:?}", r.measurement),
                )
            })
            .collect();
        rows.sort();
        rows
    };
    assert_eq!(boundaries(&actual), boundaries(&full));
    assert_eq!(
        actual.reconciliation.walked_total,
        full.reconciliation.walked_total
    );
}

#[test]
fn node_full_and_incremental_agree_as_a_sub_artifact_changes_vanishes_and_reappears() {
    let (_tmp, root) = node_fixture();
    let store = tempfile::tempdir().unwrap();
    // Warm up until the store has a recorded window start (see
    // `cargo_delivery::trusted_unchanged_container_reuses_units_without_reading_fingerprints`).
    for _ in 0..3 {
        observe(&root, store.path(), vec![], false);
    }
    let baseline = observe(&root, store.path(), vec![], false);
    assert!(
        baseline
            .nested_artifacts
            .iter()
            .any(|u| u.adapter.as_deref() == Some("node")),
        "the Node adapter identified nothing, so agreement would prove nothing"
    );
    assert_eq!(facts(&baseline), facts(&fresh_full(&root)));

    // Changes: a sub-artifact grows ...
    write(&root.join("dist/chunk.js"), 64_000);
    let inc = observe(&root, store.path(), vec![root.join("dist")], false);
    assert_eq!(facts(&inc), facts(&fresh_full(&root)), "after growth");

    // ... vanishes ...
    fs::remove_dir_all(root.join("coverage")).unwrap(); // disposable fixture only
    let inc = observe(&root, store.path(), vec![root.join("coverage")], false);
    assert_eq!(facts(&inc), facts(&fresh_full(&root)), "after removal");
    assert!(
        !inc.nested_artifacts
            .iter()
            .any(|u| u.present && u.path.starts_with(root.join("coverage"))),
        "a vanished container leaves no present unit behind"
    );

    // ... and reappears at the same path with different bytes.
    write(&root.join("coverage/lcov.info"), 3_000);
    let inc = observe(&root, store.path(), vec![root.join("coverage")], false);
    assert_eq!(facts(&inc), facts(&fresh_full(&root)), "after reappearance");
}

#[test]
fn an_unchanged_node_checkout_replays_its_units_without_reading_a_manifest() {
    let (_tmp, root) = node_fixture();
    let store = tempfile::tempdir().unwrap();
    for _ in 0..3 {
        observe(&root, store.path(), vec![], false);
    }
    let before = observe(&root, store.path(), vec![], false);
    let (after, counted) =
        swamp_core::work_counters::measured(|| observe(&root, store.path(), vec![], false));
    assert_eq!(facts(&before), facts(&after));
    assert_eq!(
        counted.header_bytes_read, 0,
        "an unchanged pass under a trusted window re-read a package.json"
    );
    assert!(counted.containers_reused > 0, "{counted:?}");
    // Previously flaky on a machine with a live Docker daemon (root
    // cause fixed in `observe`'s `docker_in_scope: false`, above); now
    // exact on every platform.
    assert_eq!(counted.containers_identified, 0, "{counted:?}");
}

#[test]
fn reclassification_and_new_nesting_never_reach_history_as_growth() {
    // A checkout whose `build/` is Node's until a Gradle marker appears:
    // the same bytes are first one unit (Node's output directory), then
    // a container with conventional children (Gradle's build layout).
    // Nothing moved on disk except a zero-byte marker file, so no unit
    // may report growth -- the directory that was already measured keeps
    // its identity and bytes, and the newly identified children start a
    // baseline rather than arriving as "+N bytes".
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap().join("mixed");
    git_init(&root);
    fs::write(root.join("package.json"), br#"{"name":"mixed"}"#).unwrap();
    fs::write(root.join(".gitignore"), "build/\n").unwrap();
    write(&root.join("build/classes/Main.class"), 30_000);
    write(&root.join("build/tmp/scratch"), 5_000);
    let store = tempfile::tempdir().unwrap();
    for _ in 0..3 {
        observe(&root, store.path(), vec![], false);
    }
    let before = observe(&root, store.path(), vec![], false);
    let build = root.join("build");
    let unit = |r: &swamp_core::Report, p: &Path| {
        r.nested_artifacts
            .iter()
            .find(|u| u.path == p)
            .cloned()
            .unwrap_or_else(|| panic!("no unit at {}", p.display()))
    };
    let b0 = unit(&before, &build);
    assert_eq!(b0.adapter.as_deref(), Some("node"), "{b0:?}");

    fs::write(root.join("settings.gradle"), b"").unwrap();
    let after = observe(
        &root,
        store.path(),
        vec![root.join("settings.gradle")],
        false,
    );
    let b1 = unit(&after, &build);
    assert_eq!(b1.adapter.as_deref(), Some("gradle"), "{b1:?}");
    assert_eq!(
        b1.id, b0.id,
        "a role or adapter change is not an identity change"
    );
    assert_eq!(b1.bytes, b0.bytes);
    assert!(
        matches!(b1.growth_bytes, None | Some(0)),
        "reclassification reached history as growth: {:?}",
        b1.growth_bytes
    );
    let classes = unit(&after, &build.join("classes"));
    assert!(
        matches!(classes.growth_bytes, None | Some(0)),
        "a newly identified unit is a new baseline, not growth: {:?}",
        classes.growth_bytes
    );
    assert_eq!(classes.regrowth_count, 0);
    // And the container row's own history did not move either.
    let row_growth = |r: &swamp_core::Report| {
        r.projects
            .iter()
            .flat_map(|p| &p.worktrees)
            .flat_map(|w| &w.artifacts)
            .find(|a| a.path == build)
            .map(|a| (a.bytes, a.growth_bytes))
    };
    let (bytes_before, _) = row_growth(&before).expect("build/ is an artifact row");
    let (bytes_after, growth_after) = row_growth(&after).unwrap();
    assert_eq!(bytes_before, bytes_after);
    assert!(
        matches!(growth_after, None | Some(0)),
        "the container's history moved on a metadata-only change: {growth_after:?}"
    );
}

#[test]
fn an_unreadable_sub_directory_is_incomplete_coverage_not_a_disappearance() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, root) = node_fixture();
    let store = tempfile::tempdir().unwrap();
    for _ in 0..3 {
        observe(&root, store.path(), vec![], false);
    }
    let before = observe(&root, store.path(), vec![], false);
    let pkg = root.join("node_modules/left-pad");
    fs::set_permissions(&pkg, fs::Permissions::from_mode(0o000)).unwrap();
    let during = observe(&root, store.path(), vec![pkg.clone()], false);
    fs::set_permissions(&pkg, fs::Permissions::from_mode(0o755)).unwrap();
    let nm = root.join("node_modules");
    let container = |r: &swamp_core::Report| {
        r.nested_artifacts
            .iter()
            .find(|u| u.path == nm)
            .cloned()
            .expect("node_modules unit")
    };
    assert!(container(&before).coverage.complete);
    let c = container(&during);
    assert!(
        !c.coverage.complete,
        "an unreadable member makes the container's coverage incomplete: {c:?}"
    );
    assert!(
        c.coverage
            .limits
            .iter()
            .any(|l| l.contains("could not read")),
        "{:?}",
        c.coverage.limits
    );
}

#[test]
fn a_nested_unit_round_trips_through_the_report_json_with_its_contract_fields() {
    // Serialization/contract fixture (#64): shared entries, opaque
    // residuals, explicit unknowns, the accounting basis, the time
    // source, the action capability and the consequence all survive the
    // `--json` report shape.
    let (_tmp, root) = node_fixture();
    fs::create_dir_all(root.join("node_modules/.mystery")).unwrap();
    write(&root.join("node_modules/.mystery/blob"), 4_000);
    let report = fresh_full(&root);
    let json = serde_json::to_value(&report).unwrap();
    let back: swamp_core::Report = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(facts(&report), facts(&back));
    let units = json["nested_artifacts"].as_array().expect("nested units");
    let residual = units
        .iter()
        .find(|u| u["path"].as_str().is_some_and(|p| p.ends_with(".mystery")))
        .expect("an unrecognised entry is a residual unit, not a missing row");
    assert_eq!(residual["coverage"]["supported"], false);
    assert_eq!(residual["role"], "Residual");
    let pkg = units
        .iter()
        .find(|u| u["path"].as_str().is_some_and(|p| p.ends_with("left-pad")))
        .unwrap();
    assert_eq!(pkg["variant"]["package"], "left-pad");
    assert_eq!(pkg["variant"]["version"], "1.3.0");
    assert!(
        pkg["variant"]["unknowns"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u == "build-generation"),
        "no build generation is invented; its absence is explicit"
    );
    assert_eq!(pkg["basis"], "allocated");
    assert_eq!(pkg["time_source"], "folded-directory-modification");
    assert_eq!(pkg["action"]["capability"], "inspection-only");
    assert!(pkg["consequence"].as_str().unwrap().contains("npm ci"));
    assert_eq!(pkg["adapter"], "node");
}

#[test]
fn the_json_views_carry_the_same_family_summary_as_the_text_view() {
    let (_tmp, root) = node_fixture();
    let report = fresh_full(&root);
    let deps = swamp_core::agent_json::view_payload(&report, "deps", None);
    let nm = deps
        .as_array()
        .unwrap()
        .iter()
        .find(|r| {
            r["path"]
                .as_str()
                .is_some_and(|p| p.ends_with("node_modules"))
        })
        .expect("node_modules row");
    let families = nm["interior"]["families"].as_array().expect("families");
    let dependencies = families
        .iter()
        .find(|f| f["family"] == "dependencies")
        .expect("dependencies family");
    assert_eq!(dependencies["count"], 1);
    assert_eq!(
        dependencies["recommendation"],
        "Review: reinstall from registry"
    );
    assert!(
        dependencies["consequence"]
            .as_str()
            .unwrap()
            .contains("npm ci")
    );
    assert_eq!(dependencies["action"], "inspection-only");
    assert!(nm["interior"]["units"].as_array().unwrap().len() >= 2);
    // A row nothing identified the inside of carries no `interior` at all.
    let builds = swamp_core::agent_json::view_payload(&report, "builds", None);
    assert!(
        builds
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r.get("interior").is_none() || r["interior"]["units"].as_array().is_some())
    );
}

fn dir_bytes(p: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let m = e.metadata().unwrap();
            if m.is_dir() {
                stack.push(e.path());
            } else {
                total += m.len();
            }
        }
    }
    total
}

/// The benchmark #65 asks for: unchanged and one-group-change refresh,
/// through the real pipeline, with the store's size. Printed with
/// `--nocapture` and recorded in
/// `.oh/sessions/2026-09-21-build-adapters-node-jvm.md`; the assertions
/// are the parts that must hold on any machine.
#[test]
fn cost_report_real_pipeline_unchanged_and_one_group_change() {
    let (_tmp, root) = node_fixture();
    // A larger installed tree, so the unchanged/changed contrast is not
    // lost in fixed per-pass overhead.
    for i in 0..300 {
        let pkg = root.join(format!("node_modules/pkg-{i:03}"));
        fs::create_dir_all(&pkg).unwrap();
        fs::write(
            pkg.join("package.json"),
            format!(r#"{{"name":"pkg-{i:03}","version":"1.0.{i}"}}"#),
        )
        .unwrap();
        write(&pkg.join("index.js"), 2_000);
    }
    let store = tempfile::tempdir().unwrap();
    let t = std::time::Instant::now();
    let (_, cold) =
        swamp_core::work_counters::measured(|| observe(&root, store.path(), vec![], false));
    let cold_ms = t.elapsed().as_secs_f64() * 1e3;
    for _ in 0..2 {
        observe(&root, store.path(), vec![], false);
    }
    let store_before = dir_bytes(store.path());
    let t = std::time::Instant::now();
    let (_, unchanged) =
        swamp_core::work_counters::measured(|| observe(&root, store.path(), vec![], false));
    let unchanged_ms = t.elapsed().as_secs_f64() * 1e3;
    let store_after_unchanged = dir_bytes(store.path());

    write(&root.join("dist/chunk.js"), 64_000);
    let t = std::time::Instant::now();
    let (_, one_group) = swamp_core::work_counters::measured(|| {
        observe(&root, store.path(), vec![root.join("dist")], false)
    });
    let one_group_ms = t.elapsed().as_secs_f64() * 1e3;
    let store_after_change = dir_bytes(store.path());

    println!("--- BUILD ADAPTER COST (real pipeline) ---");
    println!("fixture: node_modules with 301 packages, dist, coverage; one walked root");
    for (name, ms, c) in [
        ("cold", cold_ms, &cold),
        ("unchanged", unchanged_ms, &unchanged),
        ("one group changed (dist)", one_group_ms, &one_group),
    ] {
        println!(
            "{name:<26} {ms:>8.2}ms dirs_listed={} files_statted={} manifest_bytes={} \
             containers_reused={} containers_identified={}",
            c.dirs_listed,
            c.files_statted,
            c.header_bytes_read,
            c.containers_reused,
            c.containers_identified
        );
    }
    println!(
        "store bytes: before={store_before} after_unchanged={store_after_unchanged} \
         after_one_group={store_after_change}"
    );
    println!("--- END ---");

    assert!(
        cold.header_bytes_read > 0,
        "the cold pass reads the manifests"
    );
    assert_eq!(unchanged.header_bytes_read, 0);
    // Previously flaky on a machine with a live Docker daemon (root
    // cause fixed in `observe`'s `docker_in_scope: false`); exact on
    // every platform now.
    assert_eq!(unchanged.containers_identified, 0);
    assert_eq!(
        one_group.containers_identified, 1,
        "a change inside dist/ re-identifies dist/ and nothing else: {one_group:?}"
    );
    assert_eq!(
        one_group.header_bytes_read, 0,
        "re-identifying dist/ reads no package.json; node_modules was replayed"
    );
}

/// One checkout per new ecosystem: a Python project with an environment,
/// a distribution and bytecode; a Go module with vendored sources and a
/// built binary; a Swift package with a `.build`; an Android app module.
fn polyglot_fixture() -> (tempfile::TempDir, Vec<PathBuf>) {
    let tmp = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let py = base.join("py");
    git_init(&py);
    fs::write(py.join("pyproject.toml"), b"[project]\nname='demo'\n").unwrap();
    fs::write(py.join(".gitignore"), ".venv/\ndist/\n__pycache__/\n").unwrap();
    fs::create_dir_all(py.join(".venv")).unwrap();
    fs::write(py.join(".venv/pyvenv.cfg"), b"version = 3.12.4\n").unwrap();
    write(&py.join(".venv/lib/python3.12/site-packages/six.py"), 9_000);
    write(&py.join("dist/demo-1.0.0-py3-none-any.whl"), 12_000);
    write(&py.join("pkg/__pycache__/mod.cpython-312.pyc"), 4_000);

    let go = base.join("go");
    git_init(&go);
    fs::write(go.join("go.mod"), b"module example.com/app\n").unwrap();
    fs::write(go.join(".gitignore"), "bin/\nvendor/\n").unwrap();
    fs::create_dir_all(go.join("vendor")).unwrap();
    fs::write(go.join("vendor/modules.txt"), b"# example.com/dep v1.2.3\n").unwrap();
    write(&go.join("vendor/example.com/dep/dep.go"), 7_000);
    write(&go.join("bin/app"), 20_000);

    let swift = base.join("swift");
    git_init(&swift);
    fs::write(swift.join("Package.swift"), b"// swift-tools-version:5.9\n").unwrap();
    fs::write(swift.join(".gitignore"), ".build/\n").unwrap();
    write(&swift.join(".build/arm64-apple-macosx/debug/tool"), 15_000);

    let android = base.join("android");
    git_init(&android);
    fs::write(android.join("settings.gradle"), b"include ':app'\n").unwrap();
    fs::write(android.join(".gitignore"), "build/\n").unwrap();
    fs::create_dir_all(android.join("app/src/main")).unwrap();
    fs::write(
        android.join("app/src/main/AndroidManifest.xml"),
        b"<manifest/>",
    )
    .unwrap();
    write(
        &android.join("app/build/outputs/apk/debug/app-debug.apk"),
        30_000,
    );
    write(
        &android.join("app/build/intermediates/dex/debug/classes.dex"),
        8_000,
    );
    (tmp, vec![py, go, swift, android])
}

#[test]
fn project_local_actions_are_plannable_across_non_rust_adapters() {
    use swamp_core::artifact::NestedActionCapability;
    let (_tmp, mut roots) = polyglot_fixture();
    let (node_tmp, node) = node_fixture();
    roots.push(node);
    for (name, marker, contents, output) in [
        (
            "gradle-actions",
            "build.gradle",
            "",
            "build/classes/java/main/App.class",
        ),
        (
            "maven-actions",
            "pom.xml",
            "<project><artifactId>app</artifactId></project>",
            "target/classes/App.class",
        ),
    ] {
        let root = node_tmp.path().join(name);
        git_init(&root);
        fs::write(root.join(marker), contents).unwrap();
        fs::write(root.join(".gitignore"), "build/\ntarget/\n").unwrap();
        write(&root.join(output), 4096);
        roots.push(root);
    }
    let mut seen = std::collections::BTreeSet::new();
    for root in roots {
        let report = fresh_full(&root);
        let actionable: Vec<_> = report
            .nested_artifacts
            .iter()
            .filter(|u| u.action == NestedActionCapability::TrashPath)
            .collect();
        assert!(
            !actionable.is_empty(),
            "{} produced no supported action",
            root.display()
        );
        for unit in actionable {
            let plan = swamp_core::actions::propose_checking_protection(
                &report,
                None,
                std::slice::from_ref(&unit.path),
                "test",
                &[],
            )
            .unwrap_or_else(|e| {
                panic!(
                    "{} claims action but cannot be planned: {e}",
                    unit.path.display()
                )
            });
            assert_eq!(plan.len(), 1);
            assert_eq!(plan[0].path(), unit.path);
            assert!(plan[0].cargo_group().is_none());
            seen.insert(unit.adapter.clone().unwrap());
        }
        for unit in report.nested_artifacts.iter().filter(|u| {
            matches!(
                u.role,
                swamp_core::artifact::ArtifactRole::InstalledDependencies
                    | swamp_core::artifact::ArtifactRole::Installation
                    | swamp_core::artifact::ArtifactRole::DeviceState
                    | swamp_core::artifact::ArtifactRole::Archive
            )
        }) {
            assert_ne!(unit.action, NestedActionCapability::TrashPath);
        }
    }
    assert_eq!(
        seen,
        [
            "node",
            "gradle",
            "maven",
            "python",
            "go",
            "android",
            "xcode-swift"
        ]
        .into_iter()
        .map(String::from)
        .collect()
    );
}

#[test]
fn python_go_swift_and_android_units_reach_the_report_and_full_equals_incremental() {
    let (_tmp, roots) = polyglot_fixture();
    let expect = [
        ("python", "demo-1.0.0-py3-none-any.whl"),
        ("go", "app"),
        ("xcode-swift", "debug"),
        ("android", "debug"),
    ];
    for (root, (adapter, leaf)) in roots.iter().zip(expect) {
        let store = tempfile::tempdir().unwrap();
        for _ in 0..3 {
            observe(root, store.path(), vec![], false);
        }
        let baseline = observe(root, store.path(), vec![], false);
        assert!(
            baseline
                .nested_artifacts
                .iter()
                .any(|u| u.adapter.as_deref() == Some(adapter) && u.path.ends_with(leaf)),
            "{adapter}: no unit ending {leaf} in {:?}",
            baseline
                .nested_artifacts
                .iter()
                .map(|u| (u.adapter.clone(), u.relative_path.clone()))
                .collect::<Vec<_>>()
        );
        let full = |r: &swamp_core::Report| {
            let mut v: Vec<_> = r
                .nested_artifacts
                .iter()
                .filter(|u| u.adapter.as_deref() == Some(adapter))
                .map(|u| (u.id.clone(), u.role.label().to_string(), u.bytes))
                .collect();
            v.sort();
            v
        };
        assert_eq!(
            full(&baseline),
            full(&fresh_full(root)),
            "{adapter}: steady"
        );
        // An unchanged pass under a trusted window replays every
        // container: no identification, no manifest read.
        let (again, work) =
            swamp_core::work_counters::measured(|| observe(root, store.path(), vec![], false));
        assert_eq!(full(&again), full(&baseline), "{adapter}: replayed");
        // Previously flaky on a machine with a live Docker daemon (root
        // cause fixed in `observe`'s `docker_in_scope: false`); exact on
        // every platform now.
        assert_eq!(work.containers_identified, 0, "{adapter}: {work:?}");
        assert_eq!(work.header_bytes_read, 0, "{adapter}: {work:?}");
        eprintln!(
            "{adapter} unchanged pass: dirs_listed={} files_statted={} manifest_bytes={} \
             containers_reused={} containers_identified={}",
            work.dirs_listed,
            work.files_statted,
            work.header_bytes_read,
            work.containers_reused,
            work.containers_identified
        );
        // Every unit re-observed with the same bytes is zero growth.
        for u in baseline
            .nested_artifacts
            .iter()
            .filter(|u| u.adapter.as_deref() == Some(adapter))
        {
            assert!(
                u.growth_bytes.is_none_or(|g| g == 0),
                "{adapter}: {} grew {:?} on an unchanged tree",
                u.relative_path,
                u.growth_bytes
            );
        }
    }
    // A change inside the Go binary directory: incremental agrees with a
    // fresh full pass.
    let go = &roots[1];
    let store = tempfile::tempdir().unwrap();
    for _ in 0..3 {
        observe(go, store.path(), vec![], false);
    }
    write(&go.join("bin/tool"), 5_000);
    let inc = observe(go, store.path(), vec![go.join("bin")], false);
    let ids = |r: &swamp_core::Report| {
        let mut v: Vec<_> = r
            .nested_artifacts
            .iter()
            .filter(|u| u.adapter.as_deref() == Some("go"))
            .map(|u| (u.id.clone(), u.bytes))
            .collect();
        v.sort();
        v
    };
    assert_eq!(ids(&inc), ids(&fresh_full(go)), "after a new binary");
}
