//! Captures ratatui `TestBackend` frames for every view and state at both
//! first-class terminal sizes (80x24, 200x60), and commits them under
//! `tests/frames/*.txt` for hand-checked alignment review.
//!
//! Regenerate with `UPDATE_FRAMES=1 cargo test -p swamp-tui
//! --test frames`; otherwise the test asserts the committed frame is
//! still exactly reproduced.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::path::PathBuf;
use swamp_core::entities::Confidence;
use swamp_core::report::{
    ArtifactKind, ArtifactRow, ProjectRow, Reconciliation, Report, Signal, Source, UnownedReason,
    UnownedRow, WorktreeKind, WorktreeRow,
};
use swamp_tui::app::{App, ViewKind};
use swamp_tui::ui;

#[test]
fn cargo_tree_opens_in_context_and_keeps_exact_group_selection() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let target = root.join("target");
    std::fs::create_dir_all(target.join("debug/incremental/crate-a")).unwrap();
    std::fs::create_dir_all(target.join("debug/deps")).unwrap();
    std::fs::write(target.join("debug/.cargo-lock"), b"").unwrap();
    std::fs::write(target.join("debug/incremental/crate-a/state"), b"data").unwrap();
    for index in 0..40 {
        std::fs::create_dir_all(target.join(format!("debug/incremental/group-{index:02}")))
            .unwrap();
    }
    let mut report = fixture_report();
    report.projects.truncate(1);
    report.root = root.clone();
    let project = &mut report.projects[0];
    project.worktrees.truncate(1);
    project.worktrees[0].path = root.clone();
    project.worktrees[0].artifacts = vec![art(
        ArtifactKind::BuildOutput,
        target.to_str().unwrap(),
        4096,
        Some(0),
    )];
    report.nested_artifacts =
        swamp_core::cargo_artifacts::inspect_target(&target, Some(&target)).units;
    let mut unsupported = report
        .nested_artifacts
        .iter()
        .find(|u| u.path.ends_with("crate-a"))
        .unwrap()
        .clone();
    unsupported.path = target.join("debug/incremental/unsupported");
    unsupported.coverage.complete = false;
    report.nested_artifacts.push(unsupported);
    let mut app = App::new(report, root);
    app.clear_filter();
    app.drill_into_selected();
    assert_eq!(app.view, ViewKind::Tree);
    // Explicit on-demand inspection is a background operation, not marking or
    // deletion. The popup stays separate from stored observation rows.
    let before = app.report.nested_artifacts.len();
    app.selected = app
        .rows()
        .iter()
        .position(|r| r.label == "profile debug")
        .unwrap();
    swamp_tui::handle_key(&mut app, crossterm::event::KeyCode::Char('i'));
    assert!(app.operation.is_some(), "{:?}", app.refusal_active());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.operation.is_some() {
        assert!(std::time::Instant::now() < deadline);
        app.poll_operation();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(app.cargo_inspection.is_some());
    assert!(app.marked.is_empty());
    assert_eq!(app.report.nested_artifacts.len(), before);
    for (width, height) in [(80, 24), (200, 60)] {
        let frame = capture(&app, width, height);
        assert!(frame.contains("Cargo dependency inspection"), "{frame}");
    }
    swamp_tui::handle_key(&mut app, crossterm::event::KeyCode::Esc);
    assert!(app.cargo_inspection.is_none());
    let groups = app.rows();
    let cache = groups
        .iter()
        .find(|r| r.label == "Compiler caches")
        .unwrap()
        .clone();
    assert_eq!(cache.bytes, 4096);
    assert!(
        cache.unit.is_none(),
        "virtual group must never select a directory"
    );
    app.mark_row(&cache);
    assert_eq!(app.marked.len(), 1, "{:?}", app.refusal_active());
    assert!(
        app.marked.contains_key(
            &target
                .join("debug/incremental/crate-a")
                .display()
                .to_string()
        )
    );
    app.mark_row(&cache);
    assert!(
        app.marked.is_empty(),
        "group toggle clears its exact members"
    );
    // Exercise the real asynchronous keyboard route, not only mark_row.
    for label in ["Compiler caches", "profile debug"] {
        app.selected = app.rows().iter().position(|r| r.label == label).unwrap();
        swamp_tui::handle_key(&mut app, crossterm::event::KeyCode::Backspace);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.operation.is_some() {
            assert!(std::time::Instant::now() < deadline);
            app.poll_operation();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(app.confirm_open, "{label}: {:?}", app.refusal_active());
        assert_eq!(app.marked.len(), 1);
        assert!(
            app.marked.contains_key(
                &target
                    .join("debug/incremental/crate-a")
                    .display()
                    .to_string()
            )
        );
        assert!(
            !app.marked
                .contains_key(&target.join("debug").display().to_string())
        );
        app.cancel_confirm();
        app.marked.clear();
    }
    let mut missing = app
        .report
        .nested_artifacts
        .iter()
        .find(|u| u.path.ends_with("crate-a"))
        .unwrap()
        .clone();
    missing.path = target.join("debug/incremental/zzz-missing");
    missing.mtime_max = 0;
    app.report.nested_artifacts.push(missing);
    app.mark_row(&cache);
    assert!(
        app.marked.is_empty(),
        "failed group must roll back earlier successful member checks"
    );
    assert!(app.refusal_active().is_some());
    app.report.nested_artifacts.pop();
    app.refusal = None;
    app.selected = groups
        .iter()
        .position(|r| r.label == "Inspect directories")
        .unwrap();
    app.enter_row();
    let rows = app.rows();
    assert!(rows.iter().any(|r| r.label == "profile debug"));
    let incremental = rows.iter().position(|r| r.label == "incremental").unwrap();
    assert!(rows[incremental].collapsed_children.is_some());
    assert!(
        rows[incremental]
            .cleanup_summary
            .as_ref()
            .unwrap()
            .contains("1 candidate ·"),
        "empty and unsupported groups are not cleanup opportunities"
    );
    assert!(
        rows[incremental].unit.is_none(),
        "category must not become an exact cleanup selection"
    );
    assert!(!rows.iter().any(|r| r.label.contains("crate-a")));
    // Advice must be visible while a different row is selected, not just in
    // selected-row details or the spare-space preview.
    for (width, height) in [(80, 24), (120, 30), (200, 60)] {
        let rendered = capture(&app, width, height);
        assert!(
            rendered.contains("Start here: slower next build"),
            "{rendered}"
        );
        assert!(!rendered.contains("No selectable groups"));
    }
    app.selected = incremental;
    for (width, height) in [(80, 24), (120, 30), (200, 60)] {
        let rendered = capture(&app, width, height);
        assert!(
            rendered.contains("Start here: compiler cache"),
            "{rendered}"
        );
        assert!(
            rendered.contains("slower"),
            "selected consequence should remain visible: {rendered}"
        );
        assert!(rendered.contains("Size*"));
        assert!(rendered.contains("candidate"));
        if width >= 100 {
            assert!(rendered.contains("Change"));
        }
        assert!(rendered.contains("Oldest candidates"));
        assert!(rendered.contains("showing 1 of 1"));
    }
    app.enter_row();
    let rows = app.rows();
    let leaf = rows.iter().find(|r| r.label.contains("crate-a")).unwrap();
    assert_eq!(
        leaf.unit.as_ref().unwrap().0,
        target
            .join("debug/incremental/crate-a")
            .display()
            .to_string()
    );
    let last_group = rows
        .iter()
        .rposition(|r| r.label.contains("group-"))
        .unwrap();
    app.selected = last_group;
    let rendered = capture(&app, 80, 24);
    assert!(
        rendered.contains("group-39"),
        "selection must scroll into view: {rendered}"
    );
    app.selected = incremental;
    app.leave_row();
    assert!(!app.rows().iter().any(|r| r.label.contains("crate-a")));
}

fn art(kind: ArtifactKind, path: &str, bytes: u64, growth: Option<i64>) -> ArtifactRow {
    ArtifactRow {
        kind,
        path: PathBuf::from(path),
        bytes,
        mtime_max: 0,
        ecosystem: None,
        hardlinked: false,
        dedup_stale: false,
        allocated_bytes: None,
        allocated_growth_bytes: None,
        local_bytes: 0,
        track: None,
        growth_bytes: growth,
        regrowth_count: 0,
        observed_at: 1_726_000_000,
        confidence: Confidence::High,
        source: Source::new("fixture"),
        note: None,
        created_at: None,
        containers: Vec::new(),
        shared_with: Vec::new(),
        dangling: false,
        evidence: Vec::new(),
    }
}

fn fixture_report() -> Report {
    Report {
        store_dir: None,
        observed_at: 1_726_000_000,
        root: PathBuf::from("/Users/dev/src"),
        projects: vec![
            ProjectRow {
                project_id: "p-mole".into(),
                name: "mole".into(),
                remote: None,
                ecosystems: Vec::new(),
                worktrees: vec![WorktreeRow {
                    worktree_id: "w-mole".into(),
                    path: PathBuf::from("/Users/dev/src/mole"),
                    kind: WorktreeKind::Main,
                    artifacts: vec![
                        art(
                            ArtifactKind::DependencyTree,
                            "/Users/dev/src/mole/node_modules",
                            420 * 1024 * 1024,
                            Some(180 * 1024 * 1024),
                        ),
                        art(
                            ArtifactKind::BuildOutput,
                            "/Users/dev/src/mole/target",
                            2_147_483_648,
                            Some(1_073_741_824),
                        ),
                        art(
                            ArtifactKind::Source,
                            "/Users/dev/src/mole/src",
                            12 * 1024 * 1024,
                            Some(2048),
                        ),
                    ],
                    signals: vec![Signal {
                        name: "dirty".into(),
                        value: "clean".into(),
                    }],
                    branch: None,
                    github: None,
                    merge_complete: None,
                    idle_secs: None,
                }],
            },
            ProjectRow {
                project_id: "p-slop".into(),
                name: "swamp".into(),
                remote: None,
                ecosystems: Vec::new(),
                worktrees: vec![WorktreeRow {
                    worktree_id: "w-slop".into(),
                    path: PathBuf::from("/Users/dev/src/swamp"),
                    kind: WorktreeKind::Main,
                    artifacts: vec![
                        art(
                            ArtifactKind::BuildOutput,
                            "/Users/dev/src/swamp/target",
                            6_442_450_944,
                            Some(2_684_354_560),
                        ),
                        art(
                            ArtifactKind::DockerImage,
                            "/Users/dev/src/swamp/.docker/img",
                            1_610_612_736,
                            Some(-104_857_600),
                        ),
                    ],
                    signals: vec![],
                    branch: None,
                    github: None,
                    merge_complete: None,
                    idle_secs: None,
                }],
            },
        ],
        unowned: vec![UnownedRow {
            measurement: None,
            path_or_object: "/Users/dev/.cache/leftover".into(),
            bytes: 209_715_200,
            reason: UnownedReason::SharedCache,
            docker_kind: None,
            shared_bytes: None,
            note: None,
            created_at: None,
            containers: Vec::new(),
            shared_with: Vec::new(),
            dangling: false,
            evidence: Vec::new(),
        }],
        dirs_by_worktree: None,
        files_by_worktree: None,
        schedule_line: None,
        github_enrichment: None,
        nested_artifacts: Vec::new(),
        reconciliation: Reconciliation {
            unique_estimate: None,
            attributed: 0,
            unowned: 0,
            walked_total: 0,
            du_total: None,
            docker_attributed: 0,
            docker_unowned: 0,
        },
        notes: vec![],
        series_by_key: Default::default(),
        total_series: Vec::new(),
        series_window_secs: 0,
        summary: Default::default(),
    }
}

fn capture(app: &App, w: u16, h: u16) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::draw(f, app)).unwrap();
    terminal.backend().to_string()
}

#[test]
fn scope_unique_estimate_is_labeled_even_in_a_narrow_header() {
    let mut report = fixture_report();
    report.reconciliation.unique_estimate = Some(swamp_core::report::UniqueEstimate {
        sharing: None,
        bytes: 8192,
        reconciled_at: report.observed_at,
        needs_reconciliation: true,
    });
    let app = App::new(report, "/Users/dev/src".into());
    let narrow = capture(&app, 80, 24);
    assert!(narrow.contains("unique totals not recomputed"), "{narrow}");
    let wide = capture(&app, 180, 24);
    assert!(wide.contains("needs reconciliation"), "{wide}");
}

#[test]
fn selected_worktree_shows_dated_shared_container_evidence() {
    let mut report = fixture_report();
    report.projects.truncate(1);
    let path = report.projects[0].worktrees[0].path.clone();
    report.reconciliation.unique_estimate = Some(swamp_core::report::UniqueEstimate {
        bytes: 8192,
        reconciled_at: 12345,
        needs_reconciliation: true,
        sharing: Some(swamp_core::sharing::SharingSummary {
            groups: vec![swamp_core::sharing::SharingGroup {
                containers: vec![path.join("node_modules"), PathBuf::from("/shared/pnpm")],
                bytes: 4096,
                unresolved_links: false,
            }],
            ..Default::default()
        }),
    });
    let mut app = App::new(report, "/Users/dev/src".into());
    app.clear_filter();
    app.drill_into_selected();
    app.selected = app
        .rows()
        .iter()
        .position(|r| r.worktree.as_ref().is_some_and(|w| w.path == path))
        .unwrap();
    for (width, height) in [(80, 24), (200, 60)] {
        let frame = capture(&app, width, height);
        assert!(frame.contains("shared with /shared/pnpm"), "{frame}");
        assert!(frame.contains("12345"), "{frame}");
        assert!(frame.contains("needs reconciliation"), "{frame}");
    }
}

fn check(name: &str, got: &str) {
    let path = format!("{}/tests/frames/{name}.txt", env!("CARGO_MANIFEST_DIR"));
    if std::env::var("UPDATE_FRAMES").is_ok() {
        std::fs::write(&path, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing frame fixture {path}; run with UPDATE_FRAMES=1"));
    assert_eq!(got, want, "frame {name} does not match committed fixture");
}

#[test]
fn cargo_cleanup_guidance_frames() {
    let mut report = fixture_report();
    let root = "/Users/dev/src/mole/target";
    for (rel, role, is_dir) in [
        ("debug", "Profile", true),
        ("debug/deps", "Dependency", true),
        ("debug/incremental", "Incremental", true),
        ("debug/incremental/crate-a", "Incremental", true),
        ("debug/deps/test-a", "TestExecutable", false),
        ("debug/examples", "Example", true),
        ("debug/examples/demo", "Example", false),
        ("debug/deps/libkeep.rlib", "Dependency", false),
    ] {
        report.nested_artifacts.push(serde_json::from_value(serde_json::json!({
            "id":rel,"path":format!("{root}/{rel}"),"relative_path":rel,
            "parent_id":null,"container_id":"target","role":role,"membership":"Unknown",
            "is_dir":is_dir,"logical_bytes":0,"bytes":if role == "Profile" {256000000} else if role == "Dependency" && is_dir {128000000} else {64000000},"physical_bytes":0,
            "mtime_max":report.observed_at - 21 * 86400,"variant":{},"coverage":{"supported":true,"complete":true,"limits":[]},
            "action_group":null,"present":true
        })).unwrap());
    }
    for (w, h) in [(80, 24), (200, 60)] {
        let mut app = App::new(report.clone(), "/Users/dev/src".into());
        app.clear_filter();
        app.set_view(ViewKind::Builds);
        let frame = capture(&app, w, h);
        assert!(frame.contains("category"), "{frame}");
        assert!(frame.contains("unchecked"), "{frame}");
        check(&format!("cargo_cleanup_{w}x{h}"), &frame);
        app.selected_project = Some("mole".into());
        app.set_view(ViewKind::Tree);
        for kind in ["cache", "runnable", "tests", "examples", "scripts"] {
            app.collapsed.insert(format!("cleanup:{kind}:{root}/debug"));
        }
        app.collapsed.insert(format!("layout:{root}/debug"));
        app.collapsed
            .insert(format!("cargo:{root}/debug/incremental"));
        app.selected = app
            .rows()
            .iter()
            .position(|r| r.label == "Compiled tests & examples")
            .unwrap();
        let group = app.rows()[app.selected].clone();
        assert_eq!(
            group.bytes, 128000000,
            "only tests and examples, not dependencies"
        );
        assert!(group.unit.is_none());
        check(&format!("cargo_tree_{w}x{h}"), &capture(&app, w, h));
        app.enter_row();
        assert!(app.rows().iter().any(|r| r.label == "Tests"));
        assert!(app.rows().iter().any(|r| r.label == "Examples"));
        assert!(!app.rows().iter().any(|r| r.label.contains("libkeep")));
        // This synthetic report has no real files: failed exact-member checks
        // must not turn the virtual group into a broad-directory fallback.
        app.mark_row(&group);
        assert!(app.marked.is_empty());
        assert!(app.refusal_active().is_some());
    }
}

#[test]
fn deleting_progress_and_cancellation_frames() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let mut app = App::new(fixture_report(), "/Users/dev/src".into());
    let cancel = Arc::new(AtomicBool::new(false));
    app.operation = Some(swamp_tui::app::Operation {
        label: "Deleting",
        completed: 12,
        total: 617,
        succeeded: 11,
        failed: 1,
        current: "/Users/dev/src/mole/target/debug/incremental/crate-a".into(),
        started: std::time::Instant::now(),
        cancel: cancel.clone(),
    });
    for (w, h) in [(80, 24), (200, 60)] {
        let frame = capture(&app, w, h);
        assert!(frame.contains("Deleting · 12/617 groups"), "{frame}");
        assert!(frame.contains("11 successful · 1 refused"));
        assert!(frame.contains("Ctrl-C"));
        check(&format!("deleting_{w}x{h}"), &frame);
        cancel.store(true, Ordering::SeqCst);
        let frame = capture(&app, w, h);
        assert!(frame.contains("Cancelling after current group"));
        cancel.store(false, Ordering::SeqCst);
    }
}

/// #60: the selected-row detail area shows decision-evidence lines
/// (activity, consumers, current-use, recovery, reclaimability) for an
/// artifact row, at both first-class terminal sizes. Covers missing
/// evidence (a `Recovery`/`Consumer` `Unknown` fact, e.g. "no known
/// project"), multiple consumers (a `Consumer` fact whose value is a
/// list), and the recovery assessment's own smallest-useful follow-up
/// check (carried through `Evidence::note`).
#[test]
fn evidence_detail_area_frames() {
    use swamp_core::evidence::{Evidence, EvidenceSource, FactKind, FactSubtype, FactValue};
    let mut report = fixture_report();
    let dep_row = report.projects[0].worktrees[0]
        .artifacts
        .iter_mut()
        .find(|a| a.kind == ArtifactKind::DependencyTree)
        .unwrap();
    dep_row.evidence = vec![
        Evidence::known(
            FactKind::Activity,
            FactSubtype::Modified,
            FactValue::Timestamp(1_726_000_000 - 3600),
            EvidenceSource::FilesystemMetadata {
                detail: "newest recorded modification among measured children".into(),
            },
            1_726_000_000,
        ),
        // Multiple consumers: two projects' lockfiles both declare this
        // exact name+version.
        Evidence::known(
            FactKind::Consumer,
            FactSubtype::DeclaredConsumer,
            FactValue::List(vec!["mole".into(), "swamp".into()]),
            EvidenceSource::Lockfile {
                ecosystem: "npm".into(),
                path: "lodash@4.17.21".into(),
            },
            1_726_000_000,
        )
        .with_note("declared by more than one project; a shared reference, not an error"),
        // Missing evidence / no known project: an explicit Unknown, never
        // silently omitted.
        Evidence::unknown(
            FactKind::Recovery,
            FactSubtype::UnknownPrerequisites,
            EvidenceSource::Inferred {
                basis: "no sourced restoration evidence found".into(),
            },
            1_726_000_000,
            "no lockfile found declaring this dependency tree's origin",
        )
        .with_note("check: inspect this unit directly (its manifest/metadata) to find a restoration source before removing it"),
    ];
    for (w, h) in [(80, 24), (200, 60)] {
        let mut app = App::new(report.clone(), "/Users/dev/src".into());
        app.clear_filter();
        app.set_view(ViewKind::Deps);
        app.selected = 0;
        let frame = capture(&app, w, h);
        assert!(frame.contains("activity"), "{frame}");
        assert!(frame.contains("consumer"), "{frame}");
        assert!(frame.contains("recovery"), "{frame}");
        assert!(
            frame.contains("mole") && frame.contains("swamp"),
            "multiple consumers must both be visible: {frame}"
        );
        assert!(
            frame.contains("unknown"),
            "missing/unknown evidence must render explicitly, not be silently dropped: {frame}"
        );
        check(&format!("evidence_detail_{w}x{h}"), &frame);
    }
}

/// #60/#61: the inline confirmation row shows consumer/recovery
/// warnings sourced from `PlanUnit.evidence` (`render::evidence_warnings`),
/// distinct from the pre-existing git-status warnings line. Reuses the
/// same evidence-bearing row `evidence_detail_area_frames` builds, then
/// actually marks and opens the confirm banner -- the real
/// `mark_row` -> `actions::propose` -> `PlanUnit.evidence` path, not a
/// hand-constructed warning string.
#[test]
fn confirm_row_shows_evidence_warnings() {
    use swamp_core::evidence::{Evidence, EvidenceSource, FactKind, FactSubtype, FactValue};
    let mut report = fixture_report();
    let dep_row = report.projects[0].worktrees[0]
        .artifacts
        .iter_mut()
        .find(|a| a.kind == ArtifactKind::DependencyTree)
        .unwrap();
    dep_row.evidence = vec![
        Evidence::known(
            FactKind::Consumer,
            FactSubtype::DeclaredConsumer,
            FactValue::List(vec!["mole".into(), "swamp".into()]),
            EvidenceSource::Lockfile {
                ecosystem: "npm".into(),
                path: "lodash@4.17.21".into(),
            },
            1_726_000_000,
        ),
        Evidence::unknown(
            FactKind::Recovery,
            FactSubtype::UnknownPrerequisites,
            EvidenceSource::Inferred {
                basis: "no sourced restoration evidence found".into(),
            },
            1_726_000_000,
            "no lockfile found declaring this dependency tree's origin",
        ),
    ];
    for (w, h) in [(80, 24), (200, 60)] {
        let mut app = App::new(report.clone(), "/Users/dev/src".into());
        app.clear_filter();
        app.set_view(ViewKind::Tree);
        app.selected_project = Some("mole".into());
        app.selected = 2; // node_modules (deps); see marked_rows_state.
        app.mark_selected();
        app.open_confirm();
        let summary = app.confirm_summary();
        assert!(
            summary.contains("declared by") && summary.contains("mole"),
            "confirm row must surface the Consumer fact: {summary}"
        );
        assert!(
            summary.contains("recovery path unknown"),
            "confirm row must surface the Recovery fact: {summary}"
        );
        check(&format!("evidence_confirm_{w}x{h}"), &capture(&app, w, h));
    }
}

#[test]
fn projects_view() {
    for (w, h) in [(80, 24), (200, 60)] {
        let app = App::new(fixture_report(), "/Users/dev/src".into());
        check(&format!("projects_{w}x{h}"), &capture(&app, w, h));
    }
}

#[test]
fn worktree_counts_have_room_in_project_rows() {
    for count in [1, 12, 100] {
        let mut report = fixture_report();
        let project = &mut report.projects[0];
        for index in 0..count {
            let mut linked = project.worktrees[0].clone();
            linked.worktree_id = format!("linked-{index}");
            linked.path = format!("/Users/dev/worktrees/mole-{index}").into();
            linked.kind = WorktreeKind::Linked;
            linked.artifacts.clear();
            project.worktrees.push(linked);
        }
        let mut app = App::new(report, "/Users/dev/src".into());
        for selected in [0, 1] {
            app.selected = selected;
            for (w, h) in [(80, 24), (200, 60)] {
                let frame = capture(&app, w, h);
                assert!(frame.contains(&format!("mole  🔨 ⎇ {count}")), "{frame}");
            }
        }
    }
}

#[test]
fn tree_view() {
    for (w, h) in [(80, 24), (200, 60)] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.set_view(ViewKind::Tree);
        app.selected_project = Some("mole".into());
        check(&format!("tree_{w}x{h}"), &capture(&app, w, h));
    }
}

#[test]
fn kinds_view() {
    for (w, h) in [(80, 24), (200, 60)] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.set_view(ViewKind::Kinds);
        check(&format!("kinds_{w}x{h}"), &capture(&app, w, h));
    }
}

#[test]
fn docker_view() {
    for (w, h) in [(80, 24), (200, 60)] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.set_view(ViewKind::Docker);
        check(&format!("docker_{w}x{h}"), &capture(&app, w, h));
    }
}

#[test]
fn unowned_view() {
    for (w, h) in [(80, 24), (200, 60)] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.set_view(ViewKind::Unowned);
        check(&format!("unowned_{w}x{h}"), &capture(&app, w, h));
    }
}

/// #43/#51/#60: the minimal read-only External view.
#[test]
fn external_view() {
    for (w, h) in [(80, 24), (200, 60)] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.set_external_units(vec![swamp_core::external::ExternalUnit {
            detector_id: "cargo-home".into(),
            detector_name: "Cargo home".into(),
            category: swamp_core::locations::StorageCategory::Cache,
            provenance: swamp_core::locations::Provenance::BuiltinConvention,
            path: PathBuf::from("/Users/dev/.cargo/registry"),
            bytes: 2_500_000_000,
            mtime_max: 0,
            hardlinked: true,
            growth_bytes: Some(50_000_000),
            regrowth_count: 0,
            observed_at: 1_700_000_000,
            consumers: Vec::new(),
            note: None,
            evidence: Vec::new(),
        }]);
        app.set_view(ViewKind::External);
        check(&format!("external_{w}x{h}"), &capture(&app, w, h));
    }
}

/// #91/#92/#100: the minimal read-only Agents view.
#[test]
fn agents_view() {
    for (w, h) in [(80, 24), (200, 60)] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.set_agent_units(vec![swamp_core::agents::AgentUnit {
            tool_id: "claude-code".into(),
            tool_name: "Claude Code".into(),
            tool_home: PathBuf::from("/Users/dev/.claude"),
            category: swamp_core::agents::AgentCategory::Sessions,
            id: "fixture-session-1".into(),
            relative_path: "projects/-Users-dev-src-mole/fixture-session.jsonl".into(),
            path: PathBuf::from(
                "/Users/dev/.claude/projects/-Users-dev-src-mole/fixture-session.jsonl",
            ),
            members: Vec::new(),
            bytes: 4_200_000,
            hardlinked: true,
            complete: true,
            growth_bytes: Some(100_000),
            regrowth_count: 0,
            observed_at: 1_700_000_000,
            mtime_max: 1_699_990_000,
            protected: false,
            protect_reason: None,
            project_link: swamp_core::agents::ProjectLinkState::Linked {
                project_id: "fixture-project".into(),
                project_name: "mole".into(),
                project_path: PathBuf::from("/Users/dev/src/mole"),
                source: swamp_core::agents::LinkSource::Declared,
                fallback_reason: None,
                worktree_kind: "main".into(),
            },
            action: swamp_core::agents::AgentActionCapability::SessionRemoval,
            note: None,
            evidence: Vec::new(),
        }]);
        app.set_view(ViewKind::Agents);
        check(&format!("agents_{w}x{h}"), &capture(&app, w, h));
    }
}

/// #101's TUI action wiring: Space/Backspace mark a supported agent unit
/// (`App::mark_row`'s new agent-storage branch) and the confirm banner
/// shows this unit's real consequences -- the session-removal loss
/// warning and the linked project name, both sourced from
/// `swamp_core::actions::propose_agents`'s own plan, not invented by the
/// TUI layer. Protected/unsupported rows cannot reach this state at all
/// (see `agents_view_refusal_state` below).
#[test]
fn agents_view_confirm_row_shows_session_removal_consequences() {
    for (w, h) in [(80, 24), (200, 60)] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.set_agent_units(vec![swamp_core::agents::AgentUnit {
            tool_id: "claude-code".into(),
            tool_name: "Claude Code".into(),
            tool_home: PathBuf::from("/Users/dev/.claude"),
            category: swamp_core::agents::AgentCategory::Sessions,
            id: "fixture-session-1".into(),
            relative_path: "projects/-Users-dev-src-mole/fixture-session.jsonl".into(),
            path: PathBuf::from(
                "/Users/dev/.claude/projects/-Users-dev-src-mole/fixture-session.jsonl",
            ),
            members: Vec::new(),
            bytes: 4_200_000,
            hardlinked: true,
            complete: true,
            growth_bytes: Some(100_000),
            regrowth_count: 0,
            observed_at: 1_700_000_000,
            mtime_max: 1_699_990_000,
            protected: false,
            protect_reason: None,
            project_link: swamp_core::agents::ProjectLinkState::Linked {
                project_id: "fixture-project".into(),
                project_name: "mole".into(),
                project_path: PathBuf::from("/Users/dev/src/mole"),
                source: swamp_core::agents::LinkSource::Declared,
                fallback_reason: None,
                worktree_kind: "main".into(),
            },
            action: swamp_core::agents::AgentActionCapability::SessionRemoval,
            note: None,
            evidence: Vec::new(),
        }]);
        app.set_view(ViewKind::Agents);
        app.selected = 0;
        app.mark_selected();
        app.open_confirm();
        check(
            &format!("agents_confirm_session_removal_{w}x{h}"),
            &capture(&app, w, h),
        );
    }
}

/// The mirror case: a protected/unsupported row's footer names the exact
/// reason (`propose_agents`'s own refusal text), never a generic
/// "nothing to delete on this row" -- see `model::agent_rows`'s doc
/// comment on why `unit` is set even for a row that cannot be acted on.
#[test]
fn agents_view_refusal_state_names_the_protection_reason() {
    for (w, h) in [(80, 24), (200, 60)] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.set_agent_units(vec![swamp_core::agents::AgentUnit {
            tool_id: "claude-code".into(),
            tool_name: "Claude Code".into(),
            tool_home: PathBuf::from("/Users/dev/.claude"),
            category: swamp_core::agents::AgentCategory::ProtectedConfig,
            id: "fixture-settings".into(),
            relative_path: "settings.json".into(),
            path: PathBuf::from("/Users/dev/.claude/settings.json"),
            members: Vec::new(),
            bytes: 4_096,
            hardlinked: true,
            complete: true,
            growth_bytes: None,
            regrowth_count: 0,
            observed_at: 1_700_000_000,
            mtime_max: 1_699_990_000,
            protected: true,
            protect_reason: Some("global settings".into()),
            project_link: swamp_core::agents::ProjectLinkState::NotApplicable,
            action: swamp_core::agents::AgentActionCapability::None,
            note: None,
            evidence: Vec::new(),
        }]);
        app.set_view(ViewKind::Agents);
        app.selected = 0;
        app.mark_selected();
        check(
            &format!("agents_refusal_protected_{w}x{h}"),
            &capture(&app, w, h),
        );
    }
}

/// Chunk D follow-up: Shift+A over the Agents view marks the one
/// actionable row (a session, `SessionRemoval`) and leaves the protected
/// config row alone, opening one confirm and naming the skip in the
/// footer -- `App::mark_all_in_view`'s new agent-storage branch, reusing
/// `mark_row`'s own per-row refusal rather than a generic "nothing here"
/// or a silent, misleadingly-total selection.
#[test]
fn agents_view_mark_all_marks_actionable_and_skips_protected() {
    for (w, h) in [(80, 24), (200, 60)] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.set_agent_units(vec![
            swamp_core::agents::AgentUnit {
                tool_id: "claude-code".into(),
                tool_name: "Claude Code".into(),
                tool_home: PathBuf::from("/Users/dev/.claude"),
                category: swamp_core::agents::AgentCategory::Sessions,
                id: "fixture-session-1".into(),
                relative_path: "projects/-Users-dev-src-mole/fixture-session.jsonl".into(),
                path: PathBuf::from(
                    "/Users/dev/.claude/projects/-Users-dev-src-mole/fixture-session.jsonl",
                ),
                members: Vec::new(),
                bytes: 4_200_000,
                hardlinked: true,
                complete: true,
                growth_bytes: Some(100_000),
                regrowth_count: 0,
                observed_at: 1_700_000_000,
                mtime_max: 1_699_990_000,
                protected: false,
                protect_reason: None,
                project_link: swamp_core::agents::ProjectLinkState::Linked {
                    project_id: "fixture-project".into(),
                    project_name: "mole".into(),
                    project_path: PathBuf::from("/Users/dev/src/mole"),
                    source: swamp_core::agents::LinkSource::Declared,
                    fallback_reason: None,
                    worktree_kind: "main".into(),
                },
                action: swamp_core::agents::AgentActionCapability::SessionRemoval,
                note: None,
                evidence: Vec::new(),
            },
            swamp_core::agents::AgentUnit {
                tool_id: "claude-code".into(),
                tool_name: "Claude Code".into(),
                tool_home: PathBuf::from("/Users/dev/.claude"),
                category: swamp_core::agents::AgentCategory::ProtectedConfig,
                id: "fixture-settings".into(),
                relative_path: "settings.json".into(),
                path: PathBuf::from("/Users/dev/.claude/settings.json"),
                members: Vec::new(),
                bytes: 4_096,
                hardlinked: true,
                complete: true,
                growth_bytes: None,
                regrowth_count: 0,
                observed_at: 1_700_000_000,
                mtime_max: 1_699_990_000,
                protected: true,
                protect_reason: Some("global settings".into()),
                project_link: swamp_core::agents::ProjectLinkState::NotApplicable,
                action: swamp_core::agents::AgentActionCapability::None,
                note: None,
                evidence: Vec::new(),
            },
        ]);
        app.set_view(ViewKind::Agents);
        app.mark_all_in_view();
        assert_eq!(
            app.marked.len(),
            1,
            "{:?}",
            app.marked.keys().collect::<Vec<_>>()
        );
        assert!(app.confirm_open);
        check(
            &format!("agents_mark_all_skips_protected_{w}x{h}"),
            &capture(&app, w, h),
        );
    }
}

/// The header's coverage clause (`App::set_scope_note`, "Coverage line"
/// in DESIGN.md): a missing configured root beside the one Present root
/// this report actually walked shows up as one short clause, never
/// silently dropped.
#[test]
fn header_shows_scope_coverage_clause_for_a_missing_root() {
    for (w, h) in [(80, 24), (200, 60)] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.set_scope_note(&[
            swamp_core::coverage::RootCoverage {
                path: "/Users/dev/src".into(),
                status: swamp_core::coverage::RegionStatus::Complete,
                walked_total: 0,
                projects: 0,
                mode: String::new(),
            },
            swamp_core::coverage::RootCoverage::missing("/Users/dev/other".into()),
        ]);
        check(
            &format!("scope_coverage_header_{w}x{h}"),
            &capture(&app, w, h),
        );
    }
}

#[test]
fn marked_rows_state() {
    for (w, h) in [(80, 24), (200, 60)] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.clear_filter();
        app.set_view(ViewKind::Tree);
        app.selected_project = Some("mole".into());
        // Rows sort by growth desc within the worktree now (tree.rs):
        // build (index 1, +1.0GB) before deps/node_modules (index 2,
        // +180.0MB) before source (index 3).
        app.selected = 2; // node_modules (deps)
        app.mark_selected();
        check(&format!("marked_{w}x{h}"), &capture(&app, w, h));
    }
}

#[test]
fn confirm_summary_state() {
    for (w, h) in [(80, 24), (200, 60)] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.clear_filter();
        app.set_view(ViewKind::Tree);
        app.selected_project = Some("mole".into());
        app.selected = 2; // node_modules (deps); see marked_rows_state.
        app.mark_selected();
        app.open_confirm();
        check(&format!("confirm_{w}x{h}"), &capture(&app, w, h));
    }
}

#[test]
fn refusal_footer_state() {
    for (w, h) in [(80, 24), (200, 60)] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.clear_filter();
        app.set_view(ViewKind::Tree);
        app.selected_project = Some("mole".into());
        app.selected = 3; // source/src, not markable (last after the
        // growth-desc sort: build, deps, then source).
        app.mark_selected();
        check(&format!("refusal_{w}x{h}"), &capture(&app, w, h));
    }
}

#[test]
fn docker_mark_refusal_state() {
    for (w, h) in [(80, 24), (200, 60)] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.clear_filter();
        app.set_view(ViewKind::Tree);
        app.selected_project = Some("swamp".into());
        // swamp's worktree has one build row (higher growth) then
        // one Docker image row; select the Docker row.
        app.selected = 2;
        app.mark_selected();
        check(
            &format!("docker_mark_refusal_{w}x{h}"),
            &capture(&app, w, h),
        );
    }
}

#[test]
fn empty_filter_state() {
    for (w, h) in [(80, 24), (200, 60)] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.filter_text = "growth > 900GB in 7d".into();
        app.commit_filter();
        check(&format!("empty_{w}x{h}"), &capture(&app, w, h));
    }
}

#[test]
fn help_overlay_state() {
    for (w, h) in [(80, 24), (200, 60)] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.toggle_help();
        check(&format!("help_{w}x{h}"), &capture(&app, w, h));
    }
}

#[test]
fn picker_frame() {
    let mut app = App::new(fixture_report(), std::path::PathBuf::from("/Users/dev/src"));
    app.width = 200;
    app.history_secs = Some(30 * 86_400);
    swamp_tui::handle_key(&mut app, crossterm::event::KeyCode::Char('/'));
    swamp_tui::handle_key(&mut app, crossterm::event::KeyCode::Down);
    swamp_tui::handle_key(&mut app, crossterm::event::KeyCode::Down);
    swamp_tui::handle_key(&mut app, crossterm::event::KeyCode::Right);
    let got = capture(&app, 200, 60);
    assert!(got.contains("▸ kind"), "kind field selected:\n{got}");
    assert!(
        got.contains("kind:BuildOutput"),
        "composed filter shown:\n{got}"
    );
    check("picker_200x60", &got);
}

#[test]
fn drill_shows_view_scope_and_esc_returns_to_projects() {
    let mut app = App::new(fixture_report(), std::path::PathBuf::from("/Users/dev/src"));
    app.width = 200;
    swamp_tui::handle_key(&mut app, crossterm::event::KeyCode::Char('0'));
    let before = capture(&app, 200, 60);
    assert!(before.contains("view: projects · filter: none"), "{before}");
    swamp_tui::handle_key(&mut app, crossterm::event::KeyCode::Enter);
    assert_eq!(app.view, ViewKind::Tree);
    let tree = capture(&app, 200, 60);
    assert!(
        tree.contains("view: tree of "),
        "second line must name the scope:\n{tree}"
    );
    assert!(tree.contains("(Esc back)"), "{tree}");
    swamp_tui::handle_key(&mut app, crossterm::event::KeyCode::Esc);
    assert_eq!(app.view, ViewKind::Projects);
    let back = capture(&app, 200, 60);
    assert!(back.contains("view: projects"), "{back}");
}

#[test]
fn checkout_without_a_remote_marks_and_the_confirm_line_warns() {
    use crossterm::event::KeyCode;
    let mut app = App::new(fixture_report(), std::path::PathBuf::from("/Users/dev/src"));
    app.width = 200;
    swamp_tui::handle_key(&mut app, KeyCode::Char('0'));
    swamp_tui::handle_key(&mut app, KeyCode::Enter); // drill into first project
    assert_eq!(app.view, ViewKind::Tree);
    // The fixture's project has no remote: Backspace still marks it and
    // asks once, with that fact on the confirm line.
    swamp_tui::handle_key(&mut app, KeyCode::Backspace);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.operation.is_some() {
        assert!(std::time::Instant::now() < deadline);
        app.poll_operation();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let f = capture(&app, 200, 60);
    assert_eq!(app.marked.len(), 1);
    assert!(app.confirm_open);
    assert!(
        f.contains("no remote to restore from"),
        "warning expected:\n{f}"
    );
    assert!(f.contains("Enter yes"), "{f}");
}

#[test]
fn worktree_rows_always_mark_and_carry_their_warnings() {
    use swamp_tui::model::{Row, WorktreeMark};
    let mark = |linked: bool, dirty: Option<bool>, unpushed: Option<u32>, locked: Option<bool>| {
        WorktreeMark {
            path: "/x/wt".into(),
            linked,
            remote: Some("github.com/o/r".to_string()),
            dirty,
            unpushed,
            locked,
            merge_complete: false,
            pr: None,
        }
    };
    let row = |m: WorktreeMark| Row {
        depth: 1,
        rail: String::new(),
        label: "linked /x/wt".into(),
        bytes: 1,
        growth: None,
        signals: vec![],
        unit: Some(swamp_tui::units::UnitId::for_artifact(
            std::path::Path::new("/x/wt"),
        )),
        kind: None,
        worktree: Some(m),
        track: None,
        series: None,
        badges: String::new(),
        ecosystems: Vec::new(),
        mtime_max: 0,
        collapsed_children: None,
        expandable: false,
        expansion_key: None,
        cleanup_summary: None,
        allocated: false,
        project: None,
        evidence: Vec::new(),
    };
    let mut app = App::new(fixture_report(), std::path::PathBuf::from("/Users/dev/src"));
    for (m, expect_warning) in [
        (mark(true, Some(false), Some(0), Some(false)), ""),
        (mark(true, Some(true), Some(0), Some(false)), "dirty"),
        (mark(true, Some(false), Some(3), Some(false)), "3 unpushed"),
        (mark(true, Some(false), Some(0), Some(true)), "locked"),
        (
            mark(true, Some(false), None, Some(false)),
            "unpushed unknown",
        ),
        (
            WorktreeMark {
                remote: None,
                ..mark(false, Some(false), Some(0), Some(false))
            },
            "no remote",
        ),
    ] {
        app.marked.clear();
        app.mark_row(&row(m));
        assert_eq!(
            app.marked.len(),
            1,
            "every worktree row marks; the bar is the human"
        );
        let unit = app.marked.values().next().unwrap();
        if expect_warning.is_empty() {
            assert!(unit.warnings.is_empty(), "{:?}", unit.warnings);
        } else {
            assert!(
                unit.warnings.iter().any(|w| w.contains(expect_warning)),
                "expected {expect_warning:?} in {:?}",
                unit.warnings
            );
        }
    }
}

/// The archive bar, end to end against real git: a clean, fully pushed
/// checkout is archived to Trash; the same checkout with one untracked
/// file is refused, because nothing would bring that file back.
#[test]
fn archiving_a_checkout_trashes_it_and_records_the_warnings_shown() {
    use std::process::Command;
    use swamp_tui::actions::{MarkedUnit, WorktreeTerms, execute_plan};

    fn git(dir: &std::path::Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .unwrap()
                .status
                .success(),
            "git {args:?}"
        );
    }

    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin.git");
    let work = tmp.path().join("work");
    let trash = tmp.path().join("trash");
    std::fs::create_dir_all(&trash).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q", "--bare"])
            .arg(&origin)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        Command::new("git")
            .args(["clone", "-q"])
            .arg(&origin)
            .arg(&work)
            .output()
            .unwrap()
            .status
            .success()
    );
    git(&work, &["config", "user.email", "t@e"]);
    git(&work, &["config", "user.name", "t"]);
    std::fs::write(work.join("README.md"), "hi").unwrap();
    std::fs::write(work.join(".gitignore"), "build/\n").unwrap();
    std::fs::create_dir_all(work.join("build")).unwrap();
    std::fs::write(work.join("build/out.bin"), vec![b'x'; 4096]).unwrap();
    git(&work, &["add", "README.md", ".gitignore"]);
    git(&work, &["commit", "-qm", "init"]);
    git(&work, &["push", "-q", "-u", "origin", "HEAD"]);

    let unit = |path: &std::path::Path| MarkedUnit {
        cargo_unit: None,
        agent_unit: None,
        path: path.to_path_buf(),
        docker: None,
        worktree_path: PathBuf::new(),
        bytes: 4096,
        observed_at: swamp_core::entities::now(),
        label: String::new(),
        warnings: Vec::new(),
        worktree: Some(WorktreeTerms {
            merge_complete: false,
            pr: None,
            whole_checkout: true,
            remote: Some("github.com/o/r".into()),
        }),
    };
    let ledger = swamp_core::ledger::Ledger::open(tmp.path().join("ledger.parquet")).unwrap();

    // Untracked content present: no longer a bar — the human saw it on
    // the confirm line. The sink moves the checkout and the ledger keeps
    // the warnings that were shown.
    std::fs::write(work.join("secrets.env"), vec![b'k'; 2048]).unwrap();
    let mut u = unit(&work);
    u.warnings = vec!["secrets.env untracked 2.0KB".into()];
    let res = execute_plan(std::slice::from_ref(&u), &ledger, &trash, false);
    assert!(res[0].outcome.is_ok(), "{:?}", res[0].outcome);
    assert!(!work.exists(), "checkout moved to Trash");
    let recs = ledger.all().unwrap();
    let last = recs.last().unwrap();
    assert!(matches!(last.verb, swamp_core::ledger::Verb::Archive));
    let fact = |key: &str| -> String {
        last.evidence
            .iter()
            .find(|f| f.key == key)
            .unwrap_or_else(|| panic!("ledger fact {key} in {:?}", last.evidence))
            .value
            .clone()
    };
    assert!(fact("recover").contains("git clone"));
    assert_eq!(
        fact("warnings_shown"),
        "secrets.env untracked 2.0KB",
        "the confirm-line facts travel into the ledger"
    );
}

/// The Node and Gradle drill-downs at both first-class sizes.
///
/// These are the #72 half of this chunk for the non-Cargo adapters: a
/// build row expands into one row per role family, leading with what the
/// family is and what losing it costs, and only then the count, size and
/// oldest modification. The narrow frame is the one that matters -- it is
/// where a row has to give something up, and what it gives up must be a
/// number, never the consequence.
#[test]
fn node_and_gradle_family_group_frames() {
    let mut report = fixture_report();
    // `mole` already carries a node_modules and a target row; give the
    // node_modules real identified units, and add a Gradle build row.
    let nm = "/Users/dev/src/mole/node_modules";
    let build = "/Users/dev/src/mole/build";
    report.projects[0].worktrees[0].artifacts.push(art(
        ArtifactKind::BuildOutput,
        build,
        900 * 1024 * 1024,
        Some(64 * 1024 * 1024),
    ));
    let month = report.observed_at - 30 * 86_400;
    let week = report.observed_at - 7 * 86_400;
    for (path, role, adapter, bytes, mtime, consequence) in [
        (
            nm.to_string(),
            "InstalledDependencies",
            "node",
            420u64 * 1024 * 1024,
            month,
            "reinstall with `npm ci` (or `pnpm install`) -- needs registry access",
        ),
        (
            format!("{nm}/.pnpm"),
            "SharedStoreEntry",
            "node",
            300 * 1024 * 1024,
            month,
            "reinstall with `pnpm install`; the entries are hardlinks into pnpm's \
             content-addressed store",
        ),
        (
            format!("{nm}/.cache"),
            "Intermediate",
            "node",
            40 * 1024 * 1024,
            week,
            "the owning tool rebuilds its cache on the next run",
        ),
        (
            format!("{nm}/typescript"),
            "InstalledDependencies",
            "node",
            80 * 1024 * 1024,
            month,
            "reinstall with `npm ci` (or `pnpm install`) -- needs registry access",
        ),
        (
            build.to_string(),
            "Output",
            "gradle",
            900 * 1024 * 1024,
            week,
            "the next `gradle build` regenerates everything it holds",
        ),
        (
            format!("{build}/classes"),
            "Output",
            "gradle",
            500 * 1024 * 1024,
            week,
            "recompiled by the next `gradle build`",
        ),
        (
            format!("{build}/test-results"),
            "TestOutput",
            "gradle",
            120 * 1024 * 1024,
            month,
            "regenerated by the next `gradle test`",
        ),
        (
            format!("{build}/tmp"),
            "Intermediate",
            "gradle",
            80 * 1024 * 1024,
            week,
            "recreated by the next task that needs scratch space",
        ),
    ] {
        report.nested_artifacts.push(
            serde_json::from_value(serde_json::json!({
                "id": path, "path": path, "relative_path": path,
                "parent_id": null,
                "container_id": if adapter == "node" { nm } else { build },
                "role": role, "membership": "Unknown", "is_dir": true,
                "logical_bytes": 0, "bytes": bytes, "physical_bytes": 0,
                "mtime_max": mtime, "variant": {},
                "coverage": {"supported": true, "complete": true, "limits": []},
                "action_group": null, "present": true,
                "adapter": adapter, "basis": "allocated",
                "time_source": "folded-directory-modification",
                // A shared store carries the adapter's real answer: an
                // action exists in principle and is unavailable here,
                // which is a different fact from "no action exists".
                "action": if role == "SharedStoreEntry" {
                    serde_json::json!({
                        "capability": "unsupported",
                        "reason": "these files are hardlinks shared with pnpm's store and                                    possibly other projects",
                    })
                } else {
                    serde_json::json!({"capability": "inspection-only"})
                },
                "consequence": consequence,
            }))
            .unwrap(),
        );
    }

    for (w, h) in [(80, 24), (200, 60)] {
        // Builds view: one row per role family below each identified
        // container, leading with review guidance and what losing the
        // family costs, so a narrow terminal truncates the numbers first.
        let mut app = App::new(report.clone(), "/Users/dev/src".into());
        app.clear_filter();
        app.set_view(ViewKind::Builds);
        let build_rows = app.rows();
        app.set_view(ViewKind::Deps);
        let deps_rows = app.rows();
        app.set_view(ViewKind::Builds);
        let all: Vec<_> = build_rows.iter().chain(deps_rows.iter()).collect();
        let group = |title: &str| {
            all.iter()
                .find(|r| r.label == title)
                .unwrap_or_else(|| {
                    panic!(
                        "no `{title}` group: {:?}",
                        all.iter().map(|r| &r.label).collect::<Vec<_>>()
                    )
                })
                .to_owned()
                .clone()
        };
        let deps = group("Installed dependencies");
        assert!(
            deps.cleanup_summary
                .as_deref()
                .is_some_and(|s| s.starts_with("Review: reinstall from registry · 1 item")),
            "guidance first, then the count: {:?}",
            deps.cleanup_summary
        );
        assert!(
            deps.signals.iter().any(|s| s.contains("npm ci")),
            "the adapter's own consequence travels with the group: {:?}",
            deps.signals
        );
        let tests = group("Test & coverage output");
        assert!(
            tests.signals.iter().any(|s| s.contains("gradle test")),
            "{:?}",
            tests.signals
        );
        let residual = group("Not identified");
        assert_eq!(
            residual.bytes,
            200 * 1024 * 1024,
            "900 MiB of build/, 700 MiB in identified groups: the remainder is shown, not dropped"
        );
        // Not selectable: no neutral-vocabulary adapter has an action,
        // so no group row may reach the confirmation path.
        for row in &all {
            if row.signals.iter().any(|s| s == "blocked") {
                assert!(
                    row.unit.is_none(),
                    "a family row offered a selectable unit with no executor behind it: {}",
                    row.label
                );
            }
        }
        assert!(
            all.iter()
                .filter(|r| r
                    .cleanup_summary
                    .as_deref()
                    .is_some_and(|s| s.contains("· oldest")))
                .count()
                >= 5
        );
        check(&format!("build_families_{w}x{h}"), &capture(&app, w, h));

        // Project tree, groups closed: the answer to "what is this made
        // of" before any member is listed.
        app.selected_project = Some("mole".into());
        app.set_view(ViewKind::Tree);
        let tree = app.rows();
        assert!(
            !tree.iter().any(|r| r.label.contains("typescript")),
            "groups start closed; members are the follow-up question"
        );
        let outputs = tree
            .iter()
            .find(|r| r.label == "Build outputs")
            .expect("the Gradle outputs group");
        assert!(outputs.expandable && outputs.collapsed_children == Some(1));
        // Marking a group is refused as inspection-only, and nothing is
        // marked.
        app.mark_row(outputs);
        assert!(app.marked.is_empty());
        assert!(
            app.refusal
                .as_ref()
                .is_some_and(|(m, _)| m.contains("Inspection-only")),
            "{:?}",
            app.refusal
        );
        app.refusal = None;
        check(
            &format!("build_families_tree_{w}x{h}"),
            &capture(&app, w, h),
        );

        // Opened: members answer in their own ecosystem's words, not
        // Cargo's, and say what they cannot do.
        for key in [
            format!("family-open:{build}:outputs"),
            format!("family-open:{nm}:shared-store"),
            format!("family-open:{nm}:dependencies"),
        ] {
            app.collapsed.insert(key);
        }
        let tree = app.rows();
        let classes = tree
            .iter()
            .find(|r| r.label == "classes")
            .expect("the Gradle classes member");
        assert!(
            classes
                .cleanup_summary
                .as_deref()
                .is_some_and(|s| s.starts_with("recompiled by the next `gradle build`")),
            "the consequence comes first: {:?}",
            classes.cleanup_summary
        );
        assert!(
            classes.signals.iter().any(|s| s == "inspection only"),
            "{:?}",
            classes.signals
        );
        let pnpm = tree
            .iter()
            .find(|r| r.label == ".pnpm")
            .expect("the pnpm virtual store member");
        assert!(
            pnpm.signals
                .iter()
                .any(|s| s.starts_with("selective cleanup unsupported")),
            "a shared store says what it cannot do, separately from what it is: {:?}",
            pnpm.signals
        );
        check(
            &format!("build_families_tree_open_{w}x{h}"),
            &capture(&app, w, h),
        );
    }
}
