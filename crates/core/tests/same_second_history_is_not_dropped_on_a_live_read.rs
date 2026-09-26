//! R19 (Linux CI): a live read-only annotation at `now` must keep a
//! previous pass's history rows that happen to share its second. Only
//! the store-read derivation (`derive_report_views`, where `observed_at`
//! is exactly the pass's own stamp) excludes rows stamped `observed_at`.
//!
//! Fixture-level: two observations of one artifact one byte apart,
//! stamped the same second, then a read-only annotation at that second.

use std::path::PathBuf;
use swamp_core::entities::Confidence;
use swamp_core::report::{
    ArtifactKind, ArtifactRow, ProjectRow, Source, WorktreeKind, WorktreeRow,
};

fn project(root: &std::path::Path, bytes: u64) -> Vec<ProjectRow> {
    vec![ProjectRow {
        project_id: "p".into(),
        name: "p".into(),
        ecosystems: vec!["js".into()],
        remote: None,
        worktrees: vec![WorktreeRow {
            worktree_id: "w".into(),
            path: root.to_path_buf(),
            kind: WorktreeKind::Main,
            artifacts: vec![ArtifactRow {
                kind: ArtifactKind::DependencyTree,
                path: root.join("node_modules"),
                bytes,
                mtime_max: 1,
                ecosystem: Some("js".into()),
                hardlinked: false,
                dedup_stale: false,
                allocated_bytes: None,
                allocated_growth_bytes: None,
                local_bytes: bytes,
                track: None,
                growth_bytes: None,
                regrowth_count: 0,
                observed_at: 0,
                confidence: Confidence::High,
                source: Source::new("fixture"),
                note: None,
                created_at: None,
                containers: Vec::new(),
                shared_with: Vec::new(),
                dangling: false,
                evidence: Vec::new(),
            }],
            signals: Vec::new(),
            branch: None,
            github: None,
            merge_complete: None,
            idle_secs: None,
        }],
    }]
}

#[test]
fn a_live_read_keeps_history_from_the_same_second() {
    let store = tempfile::tempdir().unwrap();
    let root = PathBuf::from("/fixture/root");
    let volume = swamp_core::growth::root_scoped_volume_id(&root);
    let stage = swamp_core::bus::Stage::for_tests();
    let t = 1_700_000_000u64;
    // Two observations, 1 KiB apart, both stamped `t`.
    let mut first = project(&root, 1024);
    swamp_core::growth::observe_and_annotate(
        &stage,
        store.path(),
        volume,
        &mut first,
        t - 100,
        30,
        60,
        &std::collections::HashSet::new(),
    )
    .unwrap();
    let mut second = project(&root, 2048);
    swamp_core::growth::observe_and_annotate(
        &stage,
        store.path(),
        volume,
        &mut second,
        t,
        30,
        60,
        &std::collections::HashSet::new(),
    )
    .unwrap();
    assert_eq!(
        second[0].worktrees[0].artifacts[0].growth_bytes,
        Some(1024),
        "the persisting pass measures against the earlier row"
    );

    // A live read at the same second as the last pass sees the same growth.
    let mut live = project(&root, 2048);
    swamp_core::growth::annotate_readonly(store.path(), volume, &mut live, t, 30, 60).unwrap();
    assert_eq!(
        live[0].worktrees[0].artifacts[0].growth_bytes,
        Some(1024),
        "a read-only annotation at `now` must not drop rows from this second"
    );
}
