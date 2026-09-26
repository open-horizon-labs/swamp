use std::fs;
use swamp_core::artifact::ArtifactRole;
use swamp_core::cargo_artifacts::inspect_target;

#[test]
fn nested_growth_uses_same_key_for_write_and_annotation() {
    let tmp = tempfile::tempdir().unwrap();
    let mut projects: Vec<swamp_core::report::ProjectRow> =
        serde_json::from_value(serde_json::json!([{
            "project_id":"p", "name":"p", "worktrees":[{
                "worktree_id":"w", "path":"/fixture", "kind":"Main", "signals":[],
                "artifacts":[{
                    "kind":"Unknown", "path":"/fixture/target/debug/a", "bytes":4096,
                    "mtime_max":0, "observed_at":1000, "confidence":"High", "regrowth_count":0,
                    "source":{"tool":"cargo.layout"}, "note":"nested-id=unit-a"
                }]
            }]
        }]))
        .unwrap();
    swamp_core::growth::observe_and_annotate(
        &swamp_core::bus::Stage::for_tests(),
        tmp.path(),
        1,
        &mut projects,
        1000,
        30,
        100,
        &Default::default(),
    )
    .unwrap();
    projects[0].worktrees[0].artifacts[0].bytes = 8192;
    swamp_core::growth::observe_and_annotate(
        &swamp_core::bus::Stage::for_tests(),
        tmp.path(),
        1,
        &mut projects,
        1100,
        30,
        100,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(
        projects[0].worktrees[0].artifacts[0].growth_bytes,
        Some(4096)
    );
}

#[test]
fn independent_roots_have_distinct_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a/target");
    let b = tmp.path().join("b/target");
    for root in [&a, &b] {
        fs::create_dir_all(root.join("debug/deps")).unwrap();
        fs::write(root.join("debug/deps/same"), b"abc").unwrap();
    }
    let x = inspect_target(&a, Some(tmp.path()));
    let y = inspect_target(&b, Some(tmp.path()));
    assert_ne!(x.units[0].id, y.units[0].id, "distinct containers collide");
}

#[test]
fn target_triple_preserves_profile_and_dependency_role() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("aarch64-apple-darwin/debug/deps/liba.rlib");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"abc").unwrap();
    let x = inspect_target(tmp.path(), Some(tmp.path()));
    let unit = x.units.iter().find(|u| u.path == path).unwrap();
    assert_eq!(unit.role, ArtifactRole::Dependency);
    assert_eq!(unit.variant.profile.as_deref(), Some("debug"));
}

#[test]
fn unreadable_root_is_not_supported_empty_observation() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("not-a-directory");
    fs::write(&path, b"abc").unwrap();
    let x = inspect_target(&path, Some(tmp.path()));
    assert!(!x.coverage.supported, "read_dir failure reported supported");
}
