//! R19: `report::annotate_artifact_ecosystems` lists an artifact's
//! parent directory at most once per distinct parent, and never for a
//! nested artifact's history shadow row (`source.tool == "cargo.layout"`).
//!
//! Measured on the owner's machine before the fix: an unchanged
//! `swamp observe` spent 138 s of its 147 s in this one function --
//! 4,577 shadow rows, each `read_dir`-ing a `target/debug/deps`-sized
//! parent, every pass. This pins the two properties that make it cheap.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use swamp_core::entities::Confidence;
use swamp_core::report::{
    ArtifactKind, ArtifactRow, ProjectRow, Source, WorktreeKind, WorktreeRow,
};

fn row(path: &Path, tool: &str) -> ArtifactRow {
    ArtifactRow {
        kind: ArtifactKind::BuildOutput,
        path: path.to_path_buf(),
        bytes: 1,
        mtime_max: 0,
        ecosystem: None,
        hardlinked: false,
        dedup_stale: false,
        allocated_bytes: None,
        allocated_growth_bytes: None,
        local_bytes: 1,
        track: None,
        growth_bytes: None,
        regrowth_count: 0,
        observed_at: 0,
        confidence: Confidence::High,
        source: Source::new(tool),
        note: None,
        created_at: None,
        containers: Vec::new(),
        shared_with: Vec::new(),
        dangling: false,
        evidence: Vec::new(),
    }
}

#[test]
fn each_parent_is_listed_once_and_shadow_rows_are_not_listed_at_all() {
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("app");
    std::fs::create_dir_all(wt.join("target/debug/deps")).unwrap();
    std::fs::write(wt.join("Cargo.toml"), b"[package]\nname=\"a\"\n").unwrap();
    let project = ProjectRow {
        project_id: "p".into(),
        name: "app".into(),
        ecosystems: vec!["rs".into()],
        remote: None,
        worktrees: vec![WorktreeRow {
            worktree_id: "w".into(),
            path: wt.clone(),
            kind: WorktreeKind::Main,
            artifacts: vec![
                // Two real artifacts under the same parent: one listing.
                row(&wt.join("target"), "filesystem.walk"),
                row(&wt.join("build"), "filesystem.walk"),
                // Two hundred shadow rows deep inside the build output:
                // no listing, ecosystem left alone.
            ]
            .into_iter()
            .chain((0..200).map(|i| {
                row(
                    &wt.join(format!("target/debug/deps/unit-{i}")),
                    "cargo.layout",
                )
            }))
            .collect(),
            signals: Vec::new(),
            branch: None,
            github: None,
            merge_complete: None,
            idle_secs: None,
        }],
    };
    let mut projects = vec![project];
    let mut listed: HashMap<PathBuf, Vec<String>> = HashMap::new();
    swamp_core::report::annotate_artifact_ecosystems_listing(&mut projects, &mut listed);

    assert_eq!(
        listed.keys().collect::<Vec<_>>(),
        vec![&wt],
        "exactly one parent listed, once"
    );
    let artifacts = &projects[0].worktrees[0].artifacts;
    assert_eq!(artifacts[0].ecosystem.as_deref(), Some("rs"));
    assert!(
        artifacts[2..].iter().all(|a| a.ecosystem.is_none()),
        "shadow rows must not be annotated (their parents are never listed)"
    );
}
