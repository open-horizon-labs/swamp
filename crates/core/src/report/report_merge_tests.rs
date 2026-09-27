use super::*;

fn empty_report() -> Report {
    merge_reports(&[], &Default::default())
}

fn worktree(path: &str, kind: WorktreeKind, measured: bool) -> WorktreeRow {
    WorktreeRow {
        worktree_id: path.into(),
        path: path.into(),
        kind,
        artifacts: if measured {
            vec![
                serde_json::from_value(serde_json::json!({
                    "kind": "BuildOutput", "path": format!("{path}/target"),
                    "bytes": 123, "growth_bytes": 5, "regrowth_count": 0,
                    "observed_at": 100, "confidence": "High", "source": {"tool": "test"}
                }))
                .unwrap(),
            ]
        } else {
            Vec::new()
        },
        signals: Vec::new(),
        branch: Some("main".into()),
        github: None,
        merge_complete: None,
        idle_secs: measured.then_some(12),
    }
}

fn project(worktrees: Vec<WorktreeRow>, tag: &str) -> ProjectRow {
    ProjectRow {
        project_id: "same-project".into(),
        name: "repo".into(),
        remote: Some("github.com/example/repo".into()),
        ecosystems: vec![tag.into()],
        worktrees,
    }
}

#[test]
fn merge_preserves_measurements_and_normalizes_clones_in_both_orders() {
    let mut a = empty_report();
    a.root = "/a".into();
    a.projects = vec![project(
        vec![
            worktree("/a", WorktreeKind::Main, true),
            worktree("/linked", WorktreeKind::Linked, false),
        ],
        "rs",
    )];
    let mut b = empty_report();
    b.root = "/b".into();
    b.projects = vec![project(
        vec![
            worktree("/b", WorktreeKind::Main, true),
            worktree("/linked", WorktreeKind::Linked, true),
        ],
        "js",
    )];
    for reports in [[a.clone(), b.clone()], [b.clone(), a.clone()]] {
        let mut merged = empty_report();
        for report in reports {
            merge_root_report_into(&mut merged, report);
        }
        assert_eq!(merged.projects.len(), 1);
        let p = &merged.projects[0];
        assert_eq!(p.worktrees.len(), 3);
        for (path, kind) in [
            ("/a", WorktreeKind::Main),
            ("/b", WorktreeKind::Clone),
            ("/linked", WorktreeKind::Linked),
        ] {
            let wt = p
                .worktrees
                .iter()
                .find(|w| w.path == Path::new(path))
                .unwrap();
            assert_eq!(wt.kind, kind);
            assert_eq!(wt.artifacts.len(), 1);
            assert_eq!(wt.artifacts[0].bytes, 123);
            assert_eq!(wt.idle_secs, Some(12));
        }
        assert_eq!(merged.summary.projects, 1);
        assert_eq!(merged.summary.worktrees, 3);
        assert_eq!(merged.summary.artifacts, 3);
        assert!(p.ecosystems.contains(&"rs".into()));
        assert!(p.ecosystems.contains(&"js".into()));
    }
}

#[test]
fn duplicate_stored_rows_keep_shapes_union_tags_and_dedup_facts() {
    use crate::locations::{Environment, Platform, Registry};
    use crate::scope::{ScanConfig, resolve_effective_scope};
    let temp = tempfile::tempdir().unwrap();
    // This fixture writes a fully formed current-generation cache directly,
    // rather than entering the observer that normally publishes the marker.
    crate::fs_gate::StoreDir::at(temp.path())
        .unwrap()
        .mark_current_format()
        .unwrap();
    let env = Environment::fixture(temp.path().into(), Default::default(), Platform::MacOS);
    let scope = resolve_effective_scope(
        &env,
        &ScanConfig {
            defaults: false,
            ..Default::default()
        },
        &[],
        &Registry::with_builtins(),
        100,
    );
    let key = scope_snapshot_key(&scope);
    let mut placeholder = worktree("/a", WorktreeKind::Main, false);
    placeholder.signals = vec![Signal {
        name: "clean".into(),
        value: "yes".into(),
    }];
    let mut measured = worktree("/a", WorktreeKind::Main, true);
    measured.signals = placeholder.signals.clone();
    measured.signals.push(Signal {
        name: "unpushed".into(),
        value: "0".into(),
    });
    measured.merge_complete = Some(crate::github::MergeComplete {
        verdict: crate::github::TriState::Unknown,
        terms: vec![
            "clean=yes".into(),
            "clean=yes".into(),
            "merged=unknown".into(),
        ],
    });
    let rows = vec![
        project(vec![placeholder], "rs"),
        project(
            vec![measured, worktree("/b", WorktreeKind::Main, true)],
            "js",
        ),
    ];
    for projects in [rows.clone(), rows.into_iter().rev().collect()] {
        crate::growth::write_project_worktree_tables(temp.path(), &key, &projects, 100).unwrap();
        crate::growth::write_artifact_shape_table(temp.path(), &key, &projects, 100).unwrap();
        // Exercise the public read path too: limitations must survive the
        // later coverage/notes and derived-view reconstruction stages.
        crate::growth::write_run_row(
            temp.path(),
            &key,
            &crate::growth::RunFacts {
                unique_estimate: None,
                observed_at: 100,
                since_secs: 24 * 3600,
                retention_days: 30,
                include_dirs: false,
                github_enrichment: None,
                schedule_line: None,
            },
        )
        .unwrap();
        let snapshot = report_scope_from_store(&scope, temp.path()).unwrap();
        let p = &snapshot.report.projects[0];
        assert_eq!(snapshot.report.projects.len(), 1);
        assert_eq!(p.ecosystems.len(), 2);
        assert_eq!(p.worktrees.len(), 2);
        let wt = p.worktrees.iter().find(|w| w.worktree_id == "/a").unwrap();
        assert_eq!(wt.artifacts.len(), 1);
        assert_eq!(wt.artifacts[0].path, Path::new("/a/target"));
        assert_eq!(wt.idle_secs, Some(12));
        assert_eq!(wt.signals.len(), 2);
        assert_eq!(
            wt.merge_complete.as_ref().unwrap().terms,
            ["clean=yes", "merged=unknown"]
        );
        assert_eq!(wt.kind, WorktreeKind::Main);
        assert_eq!(
            p.worktrees
                .iter()
                .find(|w| w.worktree_id == "/b")
                .unwrap()
                .kind,
            WorktreeKind::Clone
        );
        assert_eq!(snapshot.report.summary.projects, 1);
        assert_eq!(snapshot.report.summary.worktrees, 2);
        assert_eq!(snapshot.report.summary.artifacts, 2);
        let mut conflicting = projects.clone();
        conflicting[0].worktrees[0].idle_secs = Some(90);
        conflicting[1].worktrees[0].idle_secs = Some(12);
        crate::growth::write_project_worktree_tables(temp.path(), &key, &conflicting, 100).unwrap();
        let snapshot = report_scope_from_store(&scope, temp.path()).unwrap();
        assert!(
            snapshot
                .report
                .notes
                .iter()
                .any(|note| note.contains("measurement provenance is unavailable"))
        );
        assert_eq!(
            snapshot.report.projects[0]
                .worktrees
                .iter()
                .find(|w| w.worktree_id == "/a")
                .unwrap()
                .idle_secs,
            Some(90)
        );
    }
}

#[test]
fn conflicting_stored_scalars_are_not_inferred_from_artifacts_or_idle_time() {
    let mut old = worktree("/a", WorktreeKind::Main, false);
    old.idle_secs = Some(90);
    let mut incoming = worktree("/a", WorktreeKind::Main, true);
    incoming.branch = None; // detached is meaningful, not a missing value
    assert!(merge_stored_worktree_metadata(&mut old, incoming));
    assert_eq!(old.idle_secs, Some(90));
    assert_eq!(old.branch.as_deref(), Some("main"));
}
