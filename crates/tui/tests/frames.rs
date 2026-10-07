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

/// A fixture directory on a filesystem Cargo cleanup supports, and
/// whether one was found. Cleanup refuses (correctly) any filesystem the
/// gate does not recognise as local; the fleet's container temp dirs are
/// overlayfs on ZFS, which it does not, but `/dev/shm` (tmpfs) is. The
/// default temp dir wins where it is supported (macOS, ext4, tmpfs), so
/// the assertions are the same everywhere a supported location exists.
fn cleanup_fixture_dir() -> (tempfile::TempDir, bool) {
    let supported =
        |p: &std::path::Path| swamp_core::fs_gate::sys::volume_info(p).is_ok_and(|v| v.is_local());
    let default = tempfile::tempdir().unwrap();
    if supported(default.path()) {
        return (default, true);
    }
    if let Ok(shm) = tempfile::tempdir_in("/dev/shm")
        && supported(shm.path())
    {
        return (shm, true);
    }
    (default, false)
}

#[test]
fn cargo_tree_opens_in_context_and_keeps_exact_group_selection() {
    let (tmp, supported) = cleanup_fixture_dir();
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
    // #197: the inspection below reads exact allocated bytes.
    swamp_core::fs_gate::settle::settle();
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
    // What the filesystem reports for the one incremental file (4096 on
    // APFS, tmpfs and ext4; a 512-byte sector multiple on ZFS).
    let on_disk = {
        use std::os::unix::fs::MetadataExt;
        std::fs::symlink_metadata(target.join("debug/incremental/crate-a/state"))
            .unwrap()
            .blocks()
            * 512
    };
    assert_eq!(cache.bytes, on_disk);
    assert!(
        cache.unit.is_none(),
        "virtual group must never select a directory"
    );
    app.mark_row(&cache);
    if !supported {
        // No supported filesystem on this machine: the exact-member marks
        // below cannot succeed, and must be refused for that reason alone.
        assert!(app.marked.is_empty());
        let why = format!("{:?}", app.refusal_active());
        assert!(why.contains("supported local filesystem"), "{why}");
        return;
    }
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
    // Tempting wrong patch: the category's own folder is swept into an
    // exact cleanup selection (bulk mark, a group mark, a project mark).
    // It is a real folder, so Space on its own row may move it, but it is
    // marked one at a time and never by `A` or a group.
    assert!(
        rows[incremental].unit.is_some() && rows[incremental].individual_only,
        "the folder is markable on its own row only"
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
    // 30 rows at the narrow size: the headline block and the view strip
    // are more rows of chrome than this screen had.
    for (width, height) in [(80, 30), (120, 30), (200, 60)] {
        let rendered = capture(&app, width, height);
        assert!(
            rendered.contains("Start here: compiler cache"),
            "{rendered}"
        );
        assert!(
            rendered.contains("slower"),
            "selected consequence should remain visible: {rendered}"
        );
        assert!(rendered.contains("Size"));
        assert!(rendered.contains("* shared files may be counted again"));
        assert!(
            app.rows()[app.selected]
                .cleanup_summary
                .as_deref()
                .is_none_or(|summary| !summary.contains("oldest"))
        );
        if width >= 100 {
            assert!(rendered.contains("Change"));
        }
        assert!(rendered.contains("Selected group"));
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
        configured_outputs: Vec::new(),
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
        series_window_secs: 7 * 86_400,
        summary: Default::default(),
    }
}

/// The instant every frame is drawn at: the fixtures' `observed_at`
/// (1_726_000_000) plus 749.5 days. Frozen, so a frame's "observed N d
/// ago" is the same on every day the suite runs (it used to roll from
/// 749 d to 750 d and fail every frame).
const FRAME_NOW: u64 = 1_790_756_800;

fn capture(app: &App, w: u16, h: u16) -> String {
    swamp_core::entities::test_clock::freeze(FRAME_NOW);
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::draw(f, app)).unwrap();
    terminal.backend().to_string()
}

/// A store written by an older swamp cannot be read, so the list is empty
/// until the background rebuild lands. It says why, instead of claiming
/// nothing was ever scanned.
#[test]
fn an_older_generation_store_says_it_is_being_rebuilt_not_that_nothing_was_scanned() {
    let mut app = App::new(
        swamp_core::report::Report::empty("/Users/dev/src".into()),
        "/Users/dev/src".into(),
    );
    app.has_index = false;
    app.filter_text = "0".into();
    let plain = capture(&app, 120, 24);
    assert!(plain.contains("No saved observations yet"), "{plain}");
    app.store_rebuild = true;
    let rebuild = capture(&app, 120, 24);
    assert!(rebuild.contains("older"), "{rebuild}");
    assert!(rebuild.contains("in the background"), "{rebuild}");
    assert!(!rebuild.contains("No saved observations yet"), "{rebuild}");
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
        assert!(frame.contains("1970-01-01 03:25 UTC"), "{frame}");
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
fn physical_layout_expand_and_collapse_keep_the_anchor_on_its_screen_line() {
    let mut report = fixture_report();
    let root = "/Users/dev/src/mole/target";
    for (rel, role) in std::iter::once(("debug".to_string(), "Profile"))
        .chain((0..90).map(|n| (format!("debug/folder-{n:02}"), "Dependency")))
    {
        report.nested_artifacts.push(serde_json::from_value(serde_json::json!({
            "id":rel,"path":format!("{root}/{rel}"),"relative_path":rel,
            "parent_id":null,"container_id":"target","role":role,"membership":"Unknown",
            "is_dir":true,"logical_bytes":0,"bytes":64000000,"physical_bytes":0,
            "mtime_max":report.observed_at,"variant":{},"coverage":{"supported":true,"complete":true,"limits":[]},
            "action_group":null,"present":true
        })).unwrap());
    }
    for (width, height) in [(80, 24), (200, 60)] {
        let mut app = App::new(report.clone(), "/Users/dev/src".into());
        app.clear_filter();
        app.selected_project = Some("mole".into());
        app.set_view(ViewKind::Tree);
        let key = format!("layout:{root}/debug");
        app.collapsed.insert(key.clone());
        app.selected = app
            .rows()
            .iter()
            .position(|r| r.expansion_key.as_ref() == Some(&key))
            .unwrap();
        assert!(app.rows()[app.selected].collapsed_children.is_some());
        app.enter_row();
        assert!(app.rows()[app.selected].expansion_key.as_ref() == Some(&key));
        assert!(app.rows()[app.selected].collapsed_children.is_none());
        // The human has scrolled into the layout and returned to its parent.
        // Keep that parent at the top; collapsing must not refill the list
        // from above and move the highlight to the bottom of the screen.
        app.scroll_offset.set(app.selected);
        let open = capture(&app, width, height);
        let line = open
            .lines()
            .position(|s| s.contains("Inspect directories"))
            .unwrap();
        app.leave_row();
        let closed = capture(&app, width, height);
        assert_eq!(
            closed
                .lines()
                .position(|s| s.contains("Inspect directories")),
            Some(line)
        );
        assert_eq!(app.rows()[app.selected].expansion_key.as_ref(), Some(&key));
        assert!(app.rows()[app.selected].collapsed_children.is_some());
        app.enter_row();
        let reopened = capture(&app, width, height);
        assert_eq!(
            reopened
                .lines()
                .position(|s| s.contains("Inspect directories")),
            Some(line)
        );
        assert!(app.rows()[app.selected].expansion_key.as_ref() == Some(&key));
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
        current: "mole · crate-a incremental build (target/debug)".into(),
        bytes_done: 3_100_000_000,
        bytes_total: 17_200_000_000,
        started: std::time::Instant::now(),
        cancel: cancel.clone(),
        checking_open_files: None,
    });
    for (w, h) in [(80, 24), (200, 60)] {
        let frame = capture(&app, w, h);
        assert!(frame.contains("Moving to Trash  12 of 617"), "{frame}");
        assert!(frame.contains("3.1GB of 17.2GB"), "{frame}");
        assert!(frame.contains("crate-a incremental build"), "{frame}");
        assert!(frame.contains("moved items stay in Trash"), "{frame}");
        check(&format!("deleting_{w}x{h}"), &frame);
        cancel.store(true, Ordering::SeqCst);
        let frame = capture(&app, w, h);
        assert!(frame.contains("Stopping after this item"), "{frame}");
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
        assert!(
            swamp_tui::detail::lines(&app.rows()[app.selected], &[])
                .iter()
                .any(|line| line.contains("Last changed 1h ago")),
            "modification evidence must remain available"
        );
        assert!(
            frame.contains("Projects: mole, swamp"),
            "multiple consumers must both be visible: {frame}"
        );
        assert!(
            frame.contains("Unknown: how to get it back"),
            "missing evidence must render explicitly, not be silently dropped: {frame}"
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
            bytes_counted_elsewhere: 0,
            overlap_count: 0,
            last_used: Default::default(),
            children: Vec::new(),
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

/// The mirror case: a row swamp keeps by default (a config file, no swamp
/// rule for its category) marks, drawn `✗` and labelled `[kept by
/// default]`; its confirm says what the tool loses (asserted in the lib
/// tests). Tempting wrong patch: it is refused as "protected" with no way
/// to act on what the person plainly sees. The fixture name is the old one.
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
                reached_by_registry: Vec::new(),
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
    assert!(before.contains("1 Projects"), "{before}");
    assert!(before.contains("Projects · w change period"), "{before}");
    assert!(before.contains("filter: none"), "{before}");
    swamp_tui::handle_key(&mut app, crossterm::event::KeyCode::Enter);
    assert_eq!(app.view, ViewKind::Tree);
    let tree = capture(&app, 200, 60);
    assert!(
        tree.contains("Project folders › "),
        "the view line must name the scope:\n{tree}"
    );
    assert!(tree.contains("Esc: projects"), "{tree}");
    swamp_tui::handle_key(&mut app, crossterm::event::KeyCode::Esc);
    assert_eq!(app.view, ViewKind::Projects);
    let back = capture(&app, 200, 60);
    assert!(back.contains("Projects · w change period"), "{back}");
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
    assert!(f.contains("Enter move to Trash"), "{f}");
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
        table_note: None,
        allocated: false,
        project: None,
        evidence: Vec::new(),
        tool: None,
        last_used: None,
        size_text: None,
        detail_lines: Vec::new(),
        individual_only: false,
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
        session_members: None,
        reclaim: None,
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
                .filter(|r| r.cleanup_summary.as_deref().is_some_and(|s| !s.is_empty()))
                .count()
                >= 5
        );
        assert!(all.iter().all(|row| {
            !row.cleanup_summary
                .as_deref()
                .is_some_and(|summary| summary.contains("oldest"))
        }));
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
        // Marking a group header no cleanup rule covers is a category of
        // paths, not a path: it says so and points at the items inside;
        // nothing is marked.
        app.mark_row(outputs);
        assert!(app.marked.is_empty());
        assert!(
            app.refusal
                .as_ref()
                .is_some_and(|(m, _)| m.contains("Category total: pick one of the items")),
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
        assert!(classes.unit.is_some(), "individual paths remain selectable");
        assert!(
            classes
                .signals
                .iter()
                .all(|s| !s.contains("no cleanup rule"))
        );
        let pnpm = tree
            .iter()
            .find(|r| r.label == ".pnpm")
            .expect("the pnpm virtual store member");
        assert!(
            pnpm.detail_lines
                .iter()
                .any(|s| s.contains("hardlinks shared with pnpm's store")),
            "supporting details retain the shared-store limitation: {:?}",
            pnpm.detail_lines
        );
        check(
            &format!("build_families_tree_open_{w}x{h}"),
            &capture(&app, w, h),
        );
    }
}

// ---- confirm copy, project mark state, Esc semantics (v0.7.5 audit C1/C2/H5) ----

fn plain_unit(path: &str, bytes: u64) -> swamp_tui::actions::MarkedUnit {
    swamp_tui::actions::MarkedUnit {
        cargo_unit: None,
        agent_unit: None,
        session_members: None,
        reclaim: None,
        path: PathBuf::from(path),
        docker: None,
        worktree_path: PathBuf::from(path),
        bytes,
        observed_at: 0,
        worktree: None,
        label: path.to_string(),
        warnings: Vec::new(),
    }
}

#[test]
fn change_column_names_its_actual_period_with_no_filter_and_in_the_picker() {
    for (w, h) in [(80, 24), (200, 60)] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.clear_filter();
        app.width = w;
        app.height = h;
        app.history_secs = Some(14 * 86_400);
        let frame = capture(&app, w, h);
        assert!(frame.contains("Change 7d"), "{frame}");
        assert!(frame.contains("w change period"), "{frame}");
        assert!(frame.contains("filter: none"), "{frame}");
        check(&format!("change_period_{w}x{h}"), &frame);
        app.change_period = Some(swamp_tui::app::ChangePeriod {
            choices: vec!["24h".into(), "7d".into()],
            selected: 0,
        });
        let frame = capture(&app, w, h);
        assert!(frame.contains("Period: 24h"), "{frame}");
        assert!(frame.contains("Enter apply"), "{frame}");
        assert!(!frame.contains("0 clear"), "{frame}");
        check(&format!("change_period_picker_{w}x{h}"), &frame);
        app.change_period = None;
        app.report.series_window_secs = 430_472;
        let frame = capture(&app, w, h);
        assert!(frame.contains("Change ~5d"), "{frame}");
        assert!(!frame.contains("Change 7d"), "{frame}");
    }
}

#[test]
fn build_folder_and_model_reviews_show_decision_facts_with_optional_evidence() {
    for (name, path, bytes, warnings) in [
        (
            "build_folder_review",
            "/Users/dev/src/open-horizon-labs/swamp/target/debug/examples",
            995_900_000,
            vec![
                "internal file history and subgroup hardlink attribution are not retained",
                "moves only this selected path to Trash; stop its build before removing it; allocation is not guaranteed freed space",
                "swamp identifies this (the cargo adapter) but has no cleanup rule for it: what else uses it is not established",
                "swamp's selection rules for this build folder do not cover it: only this path moves, companions are not included",
                "the next `cargo build --examples` relinks this example",
            ],
        ),
        (
            "ollama_review",
            "/Users/dev/.ollama/models/manifests/registry.ollama.ai/library/qwen3/0.6b",
            522_700_000,
            vec![
                "getting it back: downloaded again with `ollama pull qwen3:0.6b` when needed, if the registry has it (a model made with `ollama create` exists only here) (from the model's own row)",
                "last used: Aug 30 (file access time of its model layer)",
                "declared consumers: none found among 45 projects in 0 declared roots (incomplete)",
                "moving this manifest frees none of its layers: the layers (522.7MB) stay in blobs/; `ollama rm qwen3:0.6b` removes the model and the layers no other model uses",
            ],
        ),
    ] {
        for (w, h) in [(80, 24), (200, 60)] {
            let mut app = App::new(fixture_report(), "/Users/dev/src".into());
            app.width = w;
            app.height = h;
            let mut unit = plain_unit(path, bytes);
            unit.warnings = warnings.iter().map(|warning| (*warning).into()).collect();
            app.marked.insert(path.into(), unit);
            app.confirm_open = true;
            let frame = capture(&app, w, h);
            assert!(frame.contains("Enter move to Trash"), "{frame}");
            assert!(!frame.contains("companions are not included"), "{frame}");
            assert!(!frame.contains("declared consumers"), "{frame}");
            check(&format!("{name}_{w}x{h}"), &frame);
            swamp_tui::handle_key(&mut app, crossterm::event::KeyCode::Char('l'));
            let details = capture(&app, w, h);
            assert!(!details.contains("Enter move to Trash"), "{details}");
            check(&format!("{name}_details_{w}x{h}"), &details);
        }
    }
}

fn wait_idle(app: &mut App) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.operation.is_some() {
        assert!(std::time::Instant::now() < deadline);
        app.poll_operation();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// The summary groups repeated facts and gates Enter on its decision content;
/// the optional detail inventory keeps every exact action path available.
#[test]
fn trash_review_scrolls_all_paths_and_mixed_docker_facts_before_enter() {
    use crossterm::event::KeyCode;
    for (w, h) in [(80u16, 24u16), (200, 24)] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.width = w;
        app.height = h;
        let phrase = "reclaimable space is a bound, not exact: APFS clone/snapshot extent sharing outside this selection is not queried";
        let long = std::iter::repeat_n(phrase, 100)
            .collect::<Vec<_>>()
            .join(" ");
        for i in 0..5 {
            let mut u = plain_unit(&format!("/Users/dev/src/p/dir{i}"), 3_000_000_000);
            u.warnings = vec![long.clone()];
            app.marked.insert(u.path.display().to_string(), u);
        }
        let mut d = plain_unit("/docker/img", 1_200_000_000);
        d.docker = Some(swamp_core::docker::Removal::Image {
            id: "sha256:exact-image-id".into(),
        });
        d.label = "redis:7".into();
        app.marked.insert("/docker/img".into(), d);
        app.confirm_open = true;

        let first = capture(&app, w, h);
        let summary = app.confirm_summary();
        assert!(summary.contains("Review 6 actions"), "{summary}");
        assert!(summary.contains("Trash · 5 actions · 15.0GB"), "{summary}");
        assert!(
            summary.contains("PERMANENT · Docker image · 1.2GB · sha256:exact-image-id"),
            "{summary}"
        );
        assert!(summary.contains("affects 5 actions"), "{summary}");
        assert_eq!(summary.matches(&long).count(), 1, "{summary}");
        swamp_tui::handle_key(&mut app, KeyCode::Char('l'));
        let details = capture(&app, w, h);
        assert!(details.contains("Paths and supporting facts"), "{details}");
        let inventory =
            swamp_tui::actions::confirm_details(&app.marked.values().cloned().collect::<Vec<_>>());
        for i in 0..5 {
            assert!(
                inventory.contains(&format!("/Users/dev/src/p/dir{i}")),
                "{inventory}"
            );
        }
        assert!(inventory.contains("sha256:exact-image-id"), "{inventory}");
        assert!(first.contains("sha256:exact-image-id"), "{first}");
        swamp_tui::handle_key(&mut app, KeyCode::Enter);
        assert!(app.operation.is_none() && app.confirm_open);
        swamp_tui::handle_key(&mut app, KeyCode::Esc);
        assert!(
            first.contains("Review actions") && !first.contains("Enter apply actions"),
            "{w}x{h}:
{first}"
        );
        assert!(!app.confirm_review_is_complete(w, h));

        // End cannot mark unseen lines reviewed, and Enter remains inert.
        swamp_tui::handle_key(&mut app, KeyCode::End);
        let end = capture(&app, w, h);
        assert!(
            end.contains("Docker has no Trash recovery"),
            "{w}x{h}:
{end}"
        );
        assert!(!app.confirm_review_is_complete(w, h));
        swamp_tui::handle_key(&mut app, KeyCode::Enter);
        assert!(app.operation.is_none() && app.confirm_open);

        swamp_tui::handle_key(&mut app, KeyCode::Home);
        let _ = capture(&app, w, h);
        for _ in 0..100 {
            if app.confirm_review_is_complete(w, h) {
                break;
            }
            swamp_tui::handle_key(&mut app, KeyCode::PageDown);
            let _ = capture(&app, w, h);
        }
        assert!(
            app.confirm_review_is_complete(w, h),
            "review did not reach the end at {w}x{h}"
        );
        assert!(capture(&app, w, h).contains("Enter apply actions"));
    }
}

#[test]
fn one_item_confirm_is_armed_after_its_full_plan_is_drawn() {
    let mut app = App::new(fixture_report(), "/Users/dev/src".into());
    let u = plain_unit("/Users/dev/src/p/cache", 440_400_000);
    app.marked.insert(u.path.display().to_string(), u);
    app.confirm_open = true;
    app.width = 80;
    app.height = 24;
    let f = capture(&app, 80, 24);
    assert!(
        f.contains("TRASH · 440.4MB · /Users/dev/src/p/cache"),
        "{f}"
    );
    assert!(f.contains("Enter move to Trash"), "{f}");
    assert!(app.confirm_review_is_complete(80, 24));
}

/// What a check could not include is counted on the plan, listed with a
/// reason and a next step on `d`, and the keys never go away.
#[test]
fn blocked_items_are_counted_on_the_plan_and_listed_with_next_steps() {
    use crossterm::event::KeyCode;
    let mut app = App::new(fixture_report(), "/Users/dev/src".into());
    let u = plain_unit("/Users/dev/src/p/dir", 1_000_000);
    app.marked.insert(u.path.display().to_string(), u);
    app.confirm_open = true;
    app.blocked = vec![
        swamp_tui::app::BlockedItem {
            name: "p".into(),
            reason: "nothing reclaimable in this project".into(),
            next: swamp_tui::app::blocked_next_step("nothing reclaimable").into(),
        },
        swamp_tui::app::BlockedItem {
            name: "q".into(),
            reason: "protected".into(),
            next: swamp_tui::app::blocked_next_step("protected").into(),
        },
    ];
    for w in [50u16, 80, 120] {
        app.width = w;
        app.height = 24;
        let f = capture(&app, w, 24);
        assert!(f.contains("Enter move to Trash"), "w={w}\n{f}");
        assert!(
            f.contains("2 rows skipped; press d to review why"),
            "w={w}\n{f}"
        );
        assert!(
            f.contains("TRASH · 1.0MB · /Users/dev/src/p/dir"),
            "w={w}\n{f}"
        );
    }
    swamp_tui::handle_key(&mut app, KeyCode::Char('d'));
    assert!(app.blocked_open);
    let f = capture(&app, 80, 24);
    assert!(f.contains("Blocked: 2"), "{f}");
    assert!(f.contains("next: open the project"), "{f}");
    assert!(f.contains("Esc back to the plan"), "{f}");
    // The list scrolls by displayed line and never runs off the sheet.
    app.blocked = (0..12)
        .map(|i| swamp_tui::app::BlockedItem {
            name: format!("project-{i}"),
            reason: "nothing reclaimable in this project".into(),
            next: "open the project with Enter".into(),
        })
        .collect();
    let f = capture(&app, 80, 24);
    assert!(
        f.contains("project-0") && f.contains("lines 1-8 of 24"),
        "{f}"
    );
    for _ in 0..7 {
        swamp_tui::handle_key(&mut app, KeyCode::Down);
    }
    let f = capture(&app, 80, 24);
    assert!(
        f.contains("lines 8-15 of 24") && f.contains("project-7"),
        "{f}"
    );
    assert!(!f.contains("project-0"), "{f}");
    // Enter cannot move anything while the list is open.
    swamp_tui::handle_key(&mut app, KeyCode::Enter);
    assert!(app.operation.is_none() && app.confirm_open && app.blocked_open);
    swamp_tui::handle_key(&mut app, KeyCode::Esc);
    assert!(!app.blocked_open && app.confirm_open);
}

/// Space on a project row: the row shows a mark state and a result line
/// says how many are marked. Space again clears it.
#[test]
fn space_on_a_project_row_shows_marks_and_a_result() {
    use crossterm::event::KeyCode;
    let mut app = App::new(fixture_report(), std::path::PathBuf::from("/Users/dev/src"));
    app.width = 80;
    swamp_tui::handle_key(&mut app, KeyCode::Char('0'));
    assert_eq!(app.view, ViewKind::Projects);
    swamp_tui::handle_key(&mut app, KeyCode::Char(' '));
    wait_idle(&mut app);
    assert!(!app.confirm_open);
    assert!(!app.marked.is_empty());
    let f = capture(&app, 80, 24);
    assert!(
        f.contains("✗ ") || f.contains('~'),
        "no mark on the project row:\n{f}"
    );
    assert!(f.contains(" marked ("), "no result line:\n{f}");
    let (n, of) = app.project_mark_state("mole");
    assert!(n > 0 && n <= of, "{n}/{of}");
    swamp_tui::handle_key(&mut app, KeyCode::Char(' '));
    wait_idle(&mut app);
    assert!(app.marked.is_empty());
    let f = capture(&app, 80, 24);
    assert!(f.contains("Nothing is marked"), "{f}");
    assert!(!f.contains("refused:"), "unmarking is not a refusal:\n{f}");
}

/// Backspace, Esc, move, Backspace elsewhere: the second confirm is about
/// the second row. Esc must not leave marks the Backspace made.
#[test]
fn esc_on_a_confirm_takes_back_the_marks_it_made() {
    use crossterm::event::KeyCode;
    let mut app = App::new(fixture_report(), std::path::PathBuf::from("/Users/dev/src"));
    app.width = 80;
    swamp_tui::handle_key(&mut app, KeyCode::Char('0'));
    swamp_tui::handle_key(&mut app, KeyCode::Backspace);
    wait_idle(&mut app);
    assert!(app.confirm_open);
    let first: Vec<String> = app.marked.keys().cloned().collect();
    assert!(!first.is_empty());
    swamp_tui::handle_key(&mut app, KeyCode::Esc);
    assert!(!app.confirm_open);
    assert!(
        app.marked.is_empty(),
        "Esc left invisible marks: {:?}",
        app.marked.keys()
    );
    swamp_tui::handle_key(&mut app, KeyCode::Down);
    swamp_tui::handle_key(&mut app, KeyCode::Backspace);
    wait_idle(&mut app);
    for k in app.marked.keys() {
        assert!(!first.contains(k), "the first row's mark came back: {k}");
    }
}

/// Marks made earlier with Space survive an Esc and stay drawn.
#[test]
fn esc_keeps_marks_made_by_space_and_says_so() {
    use crossterm::event::KeyCode;
    let mut app = App::new(fixture_report(), std::path::PathBuf::from("/Users/dev/src"));
    app.width = 80;
    swamp_tui::handle_key(&mut app, KeyCode::Char('0'));
    swamp_tui::handle_key(&mut app, KeyCode::Char(' '));
    wait_idle(&mut app);
    let marked = app.marked.len();
    swamp_tui::handle_key(&mut app, KeyCode::Backspace);
    assert!(
        app.confirm_open,
        "marks exist, so Backspace asks about them"
    );
    swamp_tui::handle_key(&mut app, KeyCode::Esc);
    assert_eq!(app.marked.len(), marked);
    let f = capture(&app, 80, 24);
    assert!(f.contains("still marked"), "{f}");
    assert!(f.contains("✗ ") || f.contains('~'), "{f}");
}

/// A whole checkout is named as one, and the help no longer claims the
/// checkout always stays.
#[test]
fn a_checkout_is_named_as_a_checkout() {
    let mut u = plain_unit("/Users/dev/src/esp32", 5_000_000);
    u.worktree = Some(swamp_tui::actions::WorktreeTerms {
        merge_complete: false,
        pr: None,
        whole_checkout: true,
        remote: None,
    });
    let s = swamp_tui::actions::confirm_summary(std::slice::from_ref(&u));
    assert!(
        s.contains("CHECKOUT → Trash · 5.0MB · /Users/dev/src/esp32"),
        "{s}"
    );
    assert!(s.contains(".git and source"), "{s}");
    let mut app = App::new(fixture_report(), "/Users/dev/src".into());
    app.help_open = true;
    let f = capture(&app, 120, 50);
    assert!(!f.contains("source stay"), "{f}");
    assert!(f.contains("checkout"), "{f}");
}

/// A plan larger than the available screen is scrolled; the footer stays
/// visible and Enter stays unavailable until the intervening lines are read.
#[test]
fn a_long_plan_scrolls_instead_of_hiding_facts_or_enabling_enter() {
    use crossterm::event::KeyCode;
    let mut app = App::new(fixture_report(), "/Users/dev/src".into());
    app.width = 40;
    app.height = 14;
    for i in 0..4 {
        let mut u = plain_unit(&format!("/Users/dev/src/p/dir{i}"), 1_000_000_000);
        u.warnings = vec![format!(
            "warning number {i} which is long enough to wrap across a few rows on a narrow screen ok"
        )];
        app.marked.insert(u.path.display().to_string(), u);
    }
    app.confirm_open = true;
    let f = capture(&app, 40, 14);
    assert!(f.contains("Review actions"), "{f}");
    assert!(f.contains("Esc cancel"), "{f}");
    assert!(!f.contains("Enter move to Trash"), "{f}");
    assert!(app.confirm_fits(40, 14));
    assert!(!app.confirm_review_is_complete(40, 14));
    swamp_tui::handle_key(&mut app, KeyCode::Enter);
    assert!(app.operation.is_none());
}

// ---- steady layout: nothing moves, nothing goes quiet ----------------

fn busy_review(phase_one: bool, completed: usize) -> swamp_tui::app::Operation {
    swamp_tui::app::Operation {
        label: "Reviewing",
        completed,
        total: 40,
        succeeded: completed.saturating_sub(1),
        failed: 1.min(completed),
        current: "mole · node_modules".into(),
        bytes_done: 0,
        bytes_total: 0,
        started: std::time::Instant::now(),
        cancel: Default::default(),
        checking_open_files: phase_one.then(std::time::Instant::now),
    }
}

fn line_of(frame: &str, i: usize) -> String {
    frame.lines().nth(i).unwrap_or("").trim().to_string()
}

/// The table rows stay at their original positions; the confirmation and
/// blocked overlays may cover them, then the full table returns on close.
#[test]
fn table_rows_stay_put_from_idle_through_review_confirm_and_result() {
    for w in [50u16, 80, 120] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.width = w;
        app.clear_filter();
        let idle = capture(&app, w, 24);
        let header = idle
            .lines()
            .position(|l| l.trim_start_matches('"').starts_with("Project "))
            .unwrap();
        // Header, two headline rows, the view strip, the filter line,
        // then the table.
        assert_eq!(header, 5, "w={w}\n{idle}");
        let table = |f: &str| (6..=7).map(|i| line_of(f, i)).collect::<Vec<_>>();
        let want = table(&idle);

        let mut states: Vec<(&str, String)> = Vec::new();
        app.operation = Some(busy_review(true, 0));
        states.push(("checking what is in use", capture(&app, w, 24)));
        app.operation = Some(busy_review(false, 3));
        states.push(("checking items", capture(&app, w, 24)));
        app.operation = None;
        // A mark outside the listed projects: a mark on a listed row is a
        // deliberate change of that row's text, not a shift of position.
        let u = plain_unit("/elsewhere/node_modules", 1_000_000);
        app.marked.insert(u.path.display().to_string(), u);
        app.confirm_open = true;
        app.blocked = vec![swamp_tui::app::BlockedItem {
            name: "swamp".into(),
            reason: "nothing reclaimable in this project".into(),
            next: "open the project".into(),
        }];
        states.push(("confirm", capture(&app, w, 24)));
        app.blocked_open = true;
        states.push(("blocked list", capture(&app, w, 24)));
        app.blocked_open = false;
        app.confirm_open = false;
        app.set_result(
            "Moved 338 items (17.0GB) to Trash. Space is freed when Trash is emptied.".into(),
        );
        states.push(("result", capture(&app, w, 24)));
        app.last_result = None;
        states.push(("result gone", capture(&app, w, 24)));
        for (name, f) in &states {
            if matches!(*name, "confirm" | "blocked list") {
                assert_eq!(table(f)[0], want[0], "w={w} state={name}\n{f}");
            } else {
                assert_eq!(table(f), want, "w={w} state={name}\n{f}");
            }
            assert_eq!(f.lines().count(), 24, "w={w} state={name}");
        }
    }
}

/// One Down moves the selection exactly one screen row (or none, when the
/// window scrolls), whatever the rows above and below carry as evidence.
#[test]
fn one_down_moves_the_selection_one_row_at_80x24() {
    use ratatui::style::Modifier;
    use swamp_core::evidence::{Evidence, EvidenceSource, FactKind, FactSubtype, FactValue};
    let mut report = fixture_report();
    let evidence = vec![
        Evidence::known(
            FactKind::Activity,
            FactSubtype::Modified,
            FactValue::Timestamp(1_726_000_000 - 3600),
            EvidenceSource::FilesystemMetadata {
                detail: "newest recorded modification among measured children".into(),
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
    let wt = &mut report.projects[0].worktrees[0];
    let base = wt.path.clone();
    for i in 0..40 {
        let mut a = art(
            ArtifactKind::DependencyTree,
            base.join(format!("pkg{i:02}/node_modules"))
                .to_str()
                .unwrap(),
            1_000_000 + i as u64,
            None,
        );
        // Detail heights differ row to row: none, two facts.
        if i % 3 == 0 {
            a.evidence = evidence.clone();
        }
        wt.artifacts.push(a);
    }
    let mut app = App::new(report, "/Users/dev/src".into());
    app.clear_filter();
    app.drill_into_selected();
    assert_eq!(app.view, ViewKind::Tree);
    let selected_row = |app: &App| -> usize {
        let backend = TestBackend::new(80, 24);
        let mut t = Terminal::new(backend).unwrap();
        t.draw(|f| ui::draw(f, app)).unwrap();
        let buf = t.backend().buffer().clone();
        // The section strip's current section is reversed too.
        let strip_y = 1 + ui::headline_rows(24);
        (0..24u16)
            .filter(|y| *y != strip_y)
            .find(|y| buf[(0, *y)].modifier.contains(Modifier::REVERSED))
            .expect("a selected row is drawn") as usize
    };
    let mut prev = selected_row(&app);
    let mut moves = 0;
    for _ in 0..30 {
        swamp_tui::handle_key(&mut app, crossterm::event::KeyCode::Down);
        let now = selected_row(&app);
        assert!(
            now == prev || now == prev + 1,
            "one Down moved the selection from screen row {prev} to {now}"
        );
        moves += usize::from(now != prev);
        prev = now;
    }
    assert!(moves >= 5, "the selection never walked down the screen");
    for _ in 0..30 {
        swamp_tui::handle_key(&mut app, crossterm::event::KeyCode::Up);
        let now = selected_row(&app);
        assert!(
            now + 1 == prev || now == prev,
            "one Up moved the selection from screen row {prev} to {now}"
        );
        prev = now;
    }
}

/// The activity chip is on the header at 50, 80 and 120 columns while our
/// own walk runs, and while another process holds the lock.
#[test]
fn the_activity_chip_is_present_at_every_width_when_busy() {
    for w in [50u16, 80, 120] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.width = w;
        app.observing = Some((0, 0));
        app.observing_started = Some(std::time::Instant::now());
        let head = line_of(&capture(&app, w, 24), 0);
        assert!(head.contains("observing 0s"), "w={w}: {head}");
        app.observing = None;
        app.external_observer = Some(swamp_core::schedule::LockHolder {
            pid: 4242,
            since: swamp_core::entities::now() - 72,
        });
        let head = line_of(&capture(&app, w, 24), 0);
        assert!(head.contains("1m 12s"), "w={w}: {head}");
        assert!(head.contains("observ"), "w={w}: {head}");
    }
}

/// While a check runs the status rows show elapsed time, the count of
/// items looked at, and ready and blocked, at every width. A check
/// changes nothing, so nothing on screen may call it successful or
/// refused.
#[test]
fn the_review_status_has_elapsed_progress_and_no_verdict_counters() {
    for w in [50u16, 80, 120] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        app.width = w;
        for op in [busy_review(true, 0), busy_review(false, 12)] {
            let phase_one = op.checking_open_files.is_some();
            app.operation = Some(op);
            let f = capture(&app, w, 24);
            let lines: Vec<&str> = f.lines().collect();
            let status = lines[lines.len() - 3].trim().to_string();
            assert!(status.contains("0s"), "w={w}: {status}");
            if phase_one {
                assert!(
                    status.contains("Checking what is in use"),
                    "w={w}: {status}"
                );
            } else {
                assert!(status.contains("Checked 12 of 40"), "w={w}: {status}");
                assert!(status.contains("ready") || w == 50, "w={w}: {status}");
            }
            let lower = f.to_lowercase();
            assert!(!lower.contains("successful"), "w={w}\n{f}");
            assert!(!lower.contains("refused"), "w={w}\n{f}");
            assert!(f.contains("Nothing has been changed"), "w={w}\n{f}");
        }
    }
}

/// The busy glyph is a different character on the next redraw.
#[test]
fn the_busy_glyph_moves_between_redraws() {
    let mut app = App::new(fixture_report(), "/Users/dev/src".into());
    app.operation = Some(busy_review(false, 1));
    let a = capture(&app, 80, 24);
    app.frame += 1;
    let b = capture(&app, 80, 24);
    assert_ne!(a, b);
}

/// The selected row is marked by reverse video: an attribute, not a color,
/// so it follows any theme (light included) and survives `NO_COLOR`. No
/// cell sets a background color, and nothing else on the screen is
/// reversed.
#[test]
fn the_selected_row_is_reverse_video_and_sets_no_background_color() {
    use ratatui::style::{Color, Modifier};
    let mut app = App::new(fixture_report(), "/Users/dev/src".into());
    app.clear_filter();
    for (w, h) in [(50u16, 20u16), (80, 24), (120, 40)] {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| ui::draw(f, &app)).unwrap();
        let buf = t.backend().buffer().clone();
        // The view strip's current tab is reversed too (no color either);
        // the selected ROW is the one reversed line that is not the strip.
        let strip_y = 1 + ui::headline_rows(h);
        let reversed_rows: Vec<u16> = (0..h)
            .filter(|y| *y != strip_y)
            .filter(|y| buf[(0, *y)].modifier.contains(Modifier::REVERSED))
            .collect();
        assert_eq!(reversed_rows.len(), 1, "{w}x{h}: exactly one selected row");
        let y = reversed_rows[0];
        for x in 0..w {
            let c = &buf[(x, y)];
            // The second cell of a wide glyph (an emoji badge) is not drawn.
            let continuation = x > 0
                && buf[(x - 1, y)]
                    .symbol()
                    .chars()
                    .any(|ch| ch as u32 > 0x2fff);
            assert!(
                continuation || c.modifier.contains(Modifier::REVERSED),
                "{w}x{h} cell {x} {c:?}"
            );
            assert_eq!(c.bg, Color::Reset, "{w}x{h}: no fixed background");
            assert_eq!(c.fg, Color::Reset, "{w}x{h}: no color inside the bar");
        }
    }
}

fn many_rows_app(n: usize) -> App {
    let mut report = fixture_report();
    let wt = &mut report.projects[0].worktrees[0];
    let base = wt.path.clone();
    for i in 0..n {
        wt.artifacts.push(art(
            ArtifactKind::DependencyTree,
            base.join(format!("pkg{i:03}/node_modules"))
                .to_str()
                .unwrap(),
            1_000_000 + i as u64,
            None,
        ));
    }
    let mut app = App::new(report, "/Users/dev/src".into());
    app.clear_filter();
    app.drill_into_selected();
    assert_eq!(app.view, ViewKind::Tree);
    app
}

#[test]
fn summary_tables_reuse_empty_columns_without_hiding_measured_zero_change() {
    let mut report = fixture_report();
    for project in &mut report.projects {
        for wt in &mut project.worktrees {
            for artifact in &mut wt.artifacts {
                artifact.growth_bytes = Some(0);
            }
        }
    }
    let mut app = App::new(report, "/Users/dev/src".into());
    app.clear_filter();
    app.set_view(ViewKind::Kinds);
    for (w, h) in [(80, 24), (200, 60)] {
        let f = capture(&app, w, h);
        let heading = line_of(&f, 5);
        assert!(
            heading.contains("Change"),
            "zero is measured change: {heading}"
        );
        assert!(!heading.contains("Notes"), "{heading}");
        assert!(
            !heading.contains("Change bar"),
            "all-zero bars convey nothing: {heading}"
        );
        assert!(f.contains("0B"));
        check(&format!("summary_columns_{w}x{h}"), &f);
    }
    app.set_view(ViewKind::Unowned);
    let f = capture(&app, 80, 24);
    assert!(
        !line_of(&f, 5).contains("Change"),
        "no recorded changes in this view:\n{f}"
    );
}

#[test]
fn build_tables_pair_relative_identity_with_recovery_and_keep_exact_action_paths() {
    let mut app = App::new(fixture_report(), "/Users/dev/src".into());
    app.clear_filter();
    for view in [ViewKind::Builds, ViewKind::Deps] {
        app.set_view(view);
        let rows = app.rows();
        let first = &rows[0];
        assert!(!first.label.contains("/Users/dev/src"), "{}", first.label);
        assert!(
            first
                .unit
                .as_ref()
                .unwrap()
                .0
                .starts_with("/Users/dev/src/")
        );
        for (w, h) in [(80, 24), (200, 60)] {
            let f = capture(&app, w, h);
            let details = swamp_tui::detail::lines(first, &[]).join("\n");
            assert!(details.contains(&first.unit.as_ref().unwrap().0));
            assert!(
                details.contains(if view == ViewKind::Builds {
                    "build tool makes it again"
                } else {
                    "Installing again"
                }),
                "{details}"
            );
            // Mixed structural/activity facts must not be labelled as a
            // removal consequence just because some rows are cleanup groups.
            assert!(!line_of(&f, 5).contains("If removed"), "{f}");
            assert!(line_of(&f, 5).contains("Notes"), "{f}");
            assert!(f.contains(first.table_note.as_deref().unwrap()), "{f}");
            check(&format!("relative_{}_{w}x{h}", view.label()), &f);
        }
    }
}

/// Marks stay visible after the marking result clears, even when the selected
/// items are outside the current page, filter or view. Position counts the
/// whole current list, not just the viewport.
#[test]
fn pending_selection_keeps_its_place_across_navigation_and_filters() {
    use crossterm::event::KeyCode;
    let mut app = many_rows_app(80);
    for (path, bytes) in [
        ("/tmp/first", 2_000_000_000),
        ("/tmp/second", 1_000_000_000),
    ] {
        app.marked.insert(path.into(), plain_unit(path, bytes));
    }
    app.set_result("Marked 2 items.".into());
    let _ = capture(&app, 80, 24);
    swamp_tui::handle_key(&mut app, KeyCode::PageDown);
    let total = app.rows().len();
    for (w, h) in [(80, 24), (200, 60)] {
        let frame = capture(&app, w, h);
        assert!(frame.contains("2 marked"), "{frame}");
        assert!(frame.contains("3.0GB selected"), "{frame}");
        assert!(frame.contains("Backspace review"), "{frame}");
        assert!(
            frame.contains(&format!("Row {} of {total}", app.selected + 1)),
            "{frame}"
        );
        check(&format!("pending_selection_{w}x{h}"), &frame);
    }
    swamp_tui::handle_key(&mut app, KeyCode::End);
    for w in [80, 200] {
        let frame = capture(&app, w, 24);
        assert!(
            frame.contains(&format!("Row {total} of {total}")),
            "{frame}"
        );
        assert!(frame.contains("2 marked"));
    }
    app.start_filter_edit();
    app.filter_text = "kind:BuildOutput".into();
    app.commit_filter();
    assert!(capture(&app, 80, 24).contains("2 marked"));
    app.set_view(ViewKind::Builds);
    app.start_filter_edit();
    app.filter_text = "project:does-not-exist".into();
    app.commit_filter();
    assert!(app.rows().is_empty());
    let empty = capture(&app, 80, 24);
    assert!(empty.contains("2 marked"));
    assert!(empty.contains("Backspace review"));
    app.set_view(ViewKind::Projects);
    assert!(capture(&app, 80, 24).contains("2 marked"));
    app.marked.clear();
    let cleared = capture(&app, 80, 24);
    assert!(!cleared.contains("2 marked"));
    assert!(!cleared.contains("3.0GB selected"));
    app.clear_filter();
    assert!(capture(&app, 80, 24).contains("Enter open"));
    swamp_tui::handle_key(&mut app, KeyCode::Enter);
    assert_eq!(app.view, ViewKind::Tree);
    assert!(capture(&app, 80, 24).contains("Enter close"));
}

#[test]
fn pending_selection_keeps_permanent_removal_and_blocked_counts_visible() {
    let mut app = many_rows_app(80);
    app.marked
        .insert("trash".into(), plain_unit("/tmp/build", 2_000_000_000));
    let mut docker = plain_unit("sha256:image", 1_000_000_000);
    docker.docker = Some(swamp_core::docker::Removal::Image {
        id: "sha256:image".into(),
    });
    app.marked.insert("docker".into(), docker);
    app.blocked = ["protected folder", "unreadable folder"]
        .into_iter()
        .map(|name| swamp_tui::app::BlockedItem {
            name: name.into(),
            reason: name.into(),
            next: "check the reason".into(),
        })
        .collect();
    for w in [40, 80, 200] {
        let frame = capture(&app, w, 24);
        for fact in ["2 marked", "1 Docker permanent", "2 blocked", "b reasons"] {
            assert!(frame.contains(fact), "{fact} at {w}:\n{frame}");
        }
        if w >= 80 {
            assert!(frame.contains("3.0GB selected"), "{frame}");
            assert!(frame.contains("Backspace review"), "{frame}");
        }
        check(&format!("pending_mixed_removal_{w}x24"), &frame);
    }
    app.set_result("Cancelled. Nothing was deleted.".into());
    let frame = capture(&app, 80, 24);
    assert!(frame.contains("Cancelled. Nothing was deleted."));
    assert!(
        !frame.contains("1 Docker permanent"),
        "result owns the status rows"
    );
    app.last_result = None;
    app.help_open = true;
    let frame = capture(&app, 80, 24);
    assert!(
        !frame.contains("1 Docker permanent"),
        "idle feedback stays behind help"
    );
    app.help_open = false;
    app.marked.clear();
    let frame = capture(&app, 80, 24);
    assert_eq!(frame.matches("2 blocked · b reasons").count(), 1, "{frame}");
}

#[test]
fn rejected_filter_is_visible_even_when_the_draft_fills_the_line() {
    use crossterm::event::KeyCode;
    let mut app = many_rows_app(80);
    app.selected = 20;
    let before = app.rows().len();
    app.start_filter_edit();
    app.filter_text = format!("{}終🧪e\u{301}", "invalid-expression-".repeat(12));
    swamp_tui::handle_key(&mut app, KeyCode::Enter);
    assert!(app.editing_filter);
    assert_eq!(app.selected, 20);
    assert_eq!(app.rows().len(), before);
    for (w, h) in [(80, 24), (200, 60)] {
        let frame = capture(&app, w, h);
        assert!(frame.contains("Filter not applied:"), "{frame}");
        assert!(!frame.contains("Filter not applied: filter:"));
        assert!(frame.contains("filter › …"), "{frame}");
        assert!(
            frame.contains("終🧪e\u{301}▏"),
            "the draft end and caret stay visible:\n{frame}"
        );
        assert!(frame.contains("Edit filter · Esc cancel"), "{frame}");
        assert!(!frame.contains("Backspace review"));
        check(&format!("filter_recovery_{w}x{h}"), &frame);
    }
    swamp_tui::handle_key(&mut app, KeyCode::Esc);
    assert!(!app.editing_filter);
    assert_eq!(app.selected, 20);
    assert!(!capture(&app, 80, 24).contains("Filter not applied:"));
}

#[test]
fn manager_hint_follows_the_actual_selected_row_and_existing_marks() {
    let mut app = reclaim_app(false);
    let rows = app.rows();
    app.selected = rows.iter().position(|row| row.tool.is_some()).unwrap();
    let row = &rows[app.selected];
    let manager = row.tool.unwrap();
    let frame = capture(&app, 80, 24);
    assert!(
        frame.contains(&format!("Backspace opens {} list", manager.name())),
        "{frame}"
    );
    assert!(
        frame.contains(&format!("⌫ {} list", manager.name())),
        "{frame}"
    );
    check("manager_context_80x24", &frame);
    app.marked
        .insert("elsewhere".into(), plain_unit("/tmp/elsewhere", 1_000_000));
    let frame = capture(&app, 80, 24);
    assert!(frame.contains("Space mark folder for Trash"), "{frame}");
    assert!(!frame.contains("Backspace opens"));
    assert!(!frame.contains("⌫ review"));
    let key = row.unit.as_ref().unwrap().0.clone();
    app.marked
        .insert(key, plain_unit("/tmp/manager", 1_000_000));
    assert!(capture(&app, 80, 24).contains("Backspace review"));
    app.start_filter_edit();
    let frame = capture(&app, 80, 24);
    assert!(
        !frame.contains("Backspace review"),
        "draft keys do not execute commands"
    );
}

/// PgDn, PgUp, Home and End move the list by a screenful and to its ends,
/// and the selected row stays on screen.
#[test]
fn page_and_home_end_keys_move_the_list() {
    use crossterm::event::KeyCode;
    let mut app = many_rows_app(80);
    let total = app.rows().len();
    let _ = capture(&app, 80, 24); // the draw tells the app its page size
    let page = app.page.get();
    assert!((8..24).contains(&page), "one screenful, got {page}");
    swamp_tui::handle_key(&mut app, KeyCode::PageDown);
    assert_eq!(app.selected, page);
    swamp_tui::handle_key(&mut app, KeyCode::PageDown);
    assert_eq!(app.selected, 2 * page);
    swamp_tui::handle_key(&mut app, KeyCode::PageUp);
    assert_eq!(app.selected, page);
    swamp_tui::handle_key(&mut app, KeyCode::End);
    assert_eq!(app.selected, total - 1);
    let f = capture(&app, 80, 24);
    assert!(f.contains("pkg000"), "End shows the last row:\n{f}");
    // A viewport optimization must keep the tail reachable after a resize;
    // retaining only a top-N display cache would lose this selected row.
    for (width, height) in [(80, 16), (120, 30), (200, 60), (80, 24)] {
        let resized = capture(&app, width, height);
        assert!(
            resized.contains("pkg000"),
            "tail after resize {width}x{height}:\n{resized}"
        );
        assert_eq!(
            app.rows().len(),
            total,
            "rendering must not truncate the data"
        );
        assert_eq!(app.selected, total - 1);
    }
    swamp_tui::handle_key(&mut app, KeyCode::PageDown);
    assert_eq!(app.selected, total - 1, "PgDn stops at the end");
    swamp_tui::handle_key(&mut app, KeyCode::Home);
    assert_eq!(app.selected, 0);
    swamp_tui::handle_key(&mut app, KeyCode::PageUp);
    assert_eq!(app.selected, 0, "PgUp stops at the top");
}

#[test]
fn scrolled_growth_bars_keep_the_offscreen_maximum() {
    let mut report = fixture_report();
    report.projects.truncate(1);
    report.projects[0].worktrees.truncate(1);
    report.projects[0].worktrees[0].artifacts = (0..80)
        .map(|i| {
            art(
                ArtifactKind::DependencyTree,
                &format!("/Users/dev/src/project/pkg{i:03}/node_modules"),
                1_000_000 + i,
                Some(if i == 79 {
                    1_000_000_000_000
                } else {
                    2_000_000
                }),
            )
        })
        .collect();
    let template = report.projects[0].clone();
    report.projects = template.worktrees[0]
        .artifacts
        .iter()
        .enumerate()
        .map(|(i, artifact)| {
            let mut project = template.clone();
            project.project_id = format!("project-{i}");
            project.name = format!("pkg{i:03}");
            project.remote = None;
            project.worktrees[0].artifacts = vec![artifact.clone()];
            project.worktrees[0].worktree_id = format!("worktree-{i}");
            project
        })
        .collect();
    let mut app = App::new(report, "/Users/dev/src".into());
    app.clear_filter();
    app.set_view(ViewKind::Projects);
    app.sort = swamp_tui::model::Sort::Size;
    app.selected = app.rows().len() - 1;
    let frame = capture(&app, 200, 24);
    assert!(
        !frame.contains("pkg079"),
        "maximum must be outside the viewport"
    );
    let line = frame.lines().find(|line| line.contains("pkg000")).unwrap();
    let (left, axis, right) =
        swamp_tui::model::diverging_bar(Some(2_000_000), 1_000_000_000_000, 6);
    let expected = format!("{left}{axis}{right}");
    let (left, axis, right) = swamp_tui::model::diverging_bar(Some(2_000_000), 2_000_000, 6);
    let viewport_only = format!("{left}{axis}{right}");
    assert_ne!(
        expected, viewport_only,
        "fixture must distinguish global and viewport scales"
    );
    assert!(
        line.contains(&expected),
        "growth scale changed on scroll: {line}"
    );
    assert!(!line.contains(&viewport_only));
}

/// Help at 80x24: every entry on its own row (the A and Backspace entries
/// no longer run together), nothing cut, and the rest reachable by
/// scrolling to the very last line.
#[test]
fn help_is_readable_at_80x24_and_scrolls_to_its_end() {
    use crossterm::event::KeyCode;
    let mut app = App::new(fixture_report(), "/Users/dev/src".into());
    swamp_tui::handle_key(&mut app, KeyCode::Char('?'));
    let first = capture(&app, 80, 24);
    let row_with = |f: &str, needle: &str| f.lines().position(|l| l.contains(needle));
    let a = row_with(&first, "review all rows here").expect("A entry");
    let bs = row_with(&first, "Backspace  review paths, costs and warnings").expect("Backspace");
    assert_ne!(a, bs, "A and Backspace on separate rows:\n{first}");
    assert!(first.contains("1-2"), "position is shown:\n{first}");
    assert!(!first.contains("mark every row here the tool can act on  Ba"));
    // Scroll a page at a time to the end: the last line of the text shows.
    swamp_tui::handle_key(&mut app, KeyCode::PageDown);
    let second = capture(&app, 80, 24);
    assert_ne!(first, second, "PgDn scrolls the help");
    assert!(
        second.contains("Backspace  move what is under the cursor"),
        "{second}"
    );
    swamp_tui::handle_key(&mut app, KeyCode::End);
    let end = capture(&app, 80, 24);
    assert!(end.contains("Activity evidence"), "{end}");
    let top_after_end = app.help_scroll.get();
    swamp_tui::handle_key(&mut app, KeyCode::Down);
    let _ = capture(&app, 80, 24);
    assert_eq!(app.help_scroll.get(), top_after_end, "End is the real end");
    swamp_tui::handle_key(&mut app, KeyCode::Up);
    let _ = capture(&app, 80, 24);
    assert_eq!(
        app.help_scroll.get(),
        top_after_end - 1,
        "Up moves one line"
    );
    swamp_tui::handle_key(&mut app, KeyCode::Home);
    assert_eq!(app.help_scroll.get(), 0);
    swamp_tui::handle_key(&mut app, KeyCode::Char('q'));
    assert!(!app.help_open && !app.quit, "q closes help before it quits");
    // 50 columns wraps instead of cutting.
    swamp_tui::handle_key(&mut app, KeyCode::Char('?'));
    let narrow = capture(&app, 50, 24);
    assert!(
        narrow.contains("Backspace  review paths, costs and warnings"),
        "{narrow}"
    );
}

/// PgUp/PgDn/Home/End page the wrapped reasons, keeping the last item reachable.
#[test]
fn the_blocked_list_pages_and_jumps() {
    use crossterm::event::KeyCode;
    let mut app = App::new(fixture_report(), "/Users/dev/src".into());
    app.blocked = (0..20)
        .map(|i| swamp_tui::app::BlockedItem {
            name: format!("project-{i}"),
            reason: "nothing reclaimable in this project".into(),
            next: "open the project with Enter".into(),
        })
        .collect();
    swamp_tui::handle_key(&mut app, KeyCode::Char('b'));
    let _ = capture(&app, 80, 24);
    swamp_tui::handle_key(&mut app, KeyCode::PageDown);
    assert!(
        app.blocked_scroll.get() >= 3,
        "{}",
        app.blocked_scroll.get()
    );
    swamp_tui::handle_key(&mut app, KeyCode::End);
    let f = capture(&app, 80, 24);
    assert!(f.contains("project-19"), "{f}");
    swamp_tui::handle_key(&mut app, KeyCode::Home);
    assert_eq!(app.blocked_scroll.get(), 0);
}

#[test]
fn wrapped_reasons_and_cargo_inspection_remain_reachable_after_paging_and_resize() {
    use crossterm::event::KeyCode;
    let mut app = App::new(fixture_report(), "/Users/dev/src".into());
    app.blocked = (0..4)
        .map(|i| swamp_tui::app::BlockedItem {
            name: format!("item-{i}"),
            reason: format!(
                "{} REASON_END_{i}",
                "A recorded reason needs context. ".repeat(30)
            ),
            next: format!(
                "{} NEXT_END_{i}",
                "A recovery instruction also needs context. ".repeat(20)
            ),
        })
        .collect();
    app.open_blocked();
    let mut seen = String::new();
    for _ in 0..200 {
        seen.push_str(&capture(&app, 50, 24));
        let old = app.blocked_scroll.get();
        swamp_tui::handle_key(&mut app, KeyCode::PageDown);
        seen.push_str(&capture(&app, 50, 24));
        if app.blocked_scroll.get() == old {
            break;
        }
    }
    for i in 0..4 {
        for marker in [format!("REASON_END_{i}"), format!("NEXT_END_{i}")] {
            assert!(seen.contains(&marker), "paging skipped {marker}");
        }
    }
    swamp_tui::handle_key(&mut app, KeyCode::End);
    let end = capture(&app, 50, 24);
    assert!(end.contains("NEXT_END_3"));
    check("blocked_wrapped_end_50x24", &end);
    swamp_tui::handle_key(&mut app, KeyCode::Esc);

    app.cargo_inspection = Some(vec![format!(
        "{} CARGO_TAIL",
        "dependency evidence ".repeat(80)
    )]);
    let _ = capture(&app, 40, 12);
    swamp_tui::handle_key(&mut app, KeyCode::End);
    let tail = capture(&app, 40, 12);
    assert!(
        tail.contains("CARGO_TAIL"),
        "one source line wraps far beyond one screen:\n{tail}"
    );
    assert!(app.cargo_inspection_scroll.get() > 0);
    let wider = capture(&app, 200, 60);
    assert!(wider.contains("CARGO_TAIL"));
    assert_eq!(
        app.cargo_inspection_scroll.get(),
        0,
        "resizing clamps the actual cursor"
    );
    swamp_tui::handle_key(&mut app, KeyCode::Up);
    assert_eq!(app.cargo_inspection_scroll.get(), 0);
    assert!(app.operation.is_none());
    assert!(app.marked.is_empty());
}

/// `k` states the new value and what it means; the legend keeps filter,
/// view, refresh and delete at 80 columns.
#[test]
fn k_says_what_it_changed_and_the_legend_keeps_the_common_keys() {
    use crossterm::event::KeyCode;
    let mut app = App::new(fixture_report(), "/Users/dev/src".into());
    swamp_tui::handle_key(&mut app, KeyCode::Char('k'));
    assert!(app.keep_executables);
    let f = capture(&app, 80, 24);
    assert!(f.contains("Keep executables is now on"), "{f}");
    assert!(f.contains("Remembered for next time"), "{f}");
    swamp_tui::handle_key(&mut app, KeyCode::Char('k'));
    let f = capture(&app, 80, 24);
    assert!(f.contains("Keep executables is now off"), "{f}");
    swamp_tui::handle_key(&mut app, KeyCode::Down);
    let f = capture(&app, 80, 24);
    assert!(
        f.contains("⌫ review  Tab section  v view"),
        "the four keys people reach for stay at 80 columns:\n{f}"
    );
    assert!(f.contains("? help  q quit"), "{f}");
    let f50 = capture(&app, 50, 24);
    assert!(f50.contains("Space mark  ⌫ review  Tab section"), "{f50}");
    assert!(f50.contains("? help  q quit"), "{f50}");
}

/// Leaving a view and coming back lands on the row you left; the view
/// list is named; an empty list says what to do.
#[test]
fn views_keep_their_cursor_are_named_and_empty_states_teach() {
    use crossterm::event::KeyCode;
    let mut app = many_rows_app(30);
    swamp_tui::handle_key(&mut app, KeyCode::PageDown);
    let at = app.selected;
    assert!(at > 5);
    swamp_tui::handle_key(&mut app, KeyCode::Char('v')); // builds
    assert_eq!(app.view, ViewKind::Builds);
    let f = capture(&app, 80, 24);
    assert!(f.contains("Build outputs · change: 7d · w period"), "{f}");
    assert!(f.contains("filter: none"), "{f}");
    app.set_view(ViewKind::Tree);
    assert_eq!(app.selected, at, "the tree cursor came back");
    // Esc to projects and back to the same project row.
    swamp_tui::handle_key(&mut app, KeyCode::Esc);
    assert_eq!(app.view, ViewKind::Projects);
    // No agent storage in the fixture: the empty view teaches.
    app.set_view(ViewKind::Agents);
    let f = capture(&app, 80, 24);
    assert!(f.contains("No agent storage recorded"), "{f}");
    assert!(!f.contains("no rows match"), "{f}");
    assert!(f.contains("Press v for another view"), "{f}");
    // A filter that matches nothing names itself and both ways out.
    swamp_tui::handle_key(&mut app, KeyCode::Char('1'));
    app.filter_text = "growth > 900GB in 7d".into();
    app.commit_filter();
    let f = capture(&app, 80, 24);
    assert!(f.contains("No matches for the active filters"), "{f}");
    assert!(f.contains("Press / to edit, or 0"), "{f}");
}

/// The picker prints its keys once (the footer); the box holds the form.
#[test]
fn the_picker_shows_its_key_hints_once() {
    use crossterm::event::KeyCode;
    let mut app = App::new(fixture_report(), "/Users/dev/src".into());
    swamp_tui::handle_key(&mut app, KeyCode::Char('/'));
    for w in [80u16, 120] {
        let f = capture(&app, w, 24);
        assert_eq!(f.matches("Enter apply").count(), 1, "{f}");
        assert_eq!(f.matches("e edit as text").count(), 1, "{f}");
    }
    swamp_tui::handle_key(&mut app, KeyCode::End);
    assert_eq!(app.picker.as_ref().unwrap().field, 9);
    swamp_tui::handle_key(&mut app, KeyCode::Home);
    assert_eq!(app.picker.as_ref().unwrap().field, 0);
}

/// The header's right side says what its number measures.
#[test]
fn the_header_net_change_names_its_window() {
    let mut report = fixture_report();
    report.total_series = vec![Some(100_000_000), Some(50_000_000)];
    report.series_window_secs = 7 * 86_400;
    let app = App::new(report, "/Users/dev/src".into());
    let f = capture(&app, 120, 24);
    let head = f.lines().next().unwrap();
    assert!(head.contains("-50.0MB in 1w"), "{head}");
}

/// Every help line survives every width: nothing is cut at the right edge,
/// including the badge legend and the long evidence entries.
#[test]
fn no_help_line_is_cut_at_any_width() {
    use crossterm::event::KeyCode;
    for w in [50u16, 80, 120] {
        let mut app = App::new(fixture_report(), "/Users/dev/src".into());
        swamp_tui::handle_key(&mut app, KeyCode::Char('?'));
        let mut seen = String::new();
        for _ in 0..40 {
            seen.push_str(&capture(&app, w, 24));
            swamp_tui::handle_key(&mut app, KeyCode::PageDown);
        }
        // Words that sit at the end of a long line: a wrapped line keeps
        // them on the next row instead of cutting them off.
        for word in ["net", "ue", "worktrees", "atime", "timestamp", "Trash."] {
            assert!(
                seen.split_whitespace().any(|t| t.trim_matches('│') == word),
                "{w} cols lost the word {word:?}"
            );
        }
        // No line ends in the middle of a border: the frame is intact.
        for l in seen.lines().filter(|l| l.starts_with('"')) {
            let l = l.split("\" Hidden by").next().unwrap_or(l);
            let l = l.trim_start_matches('"').trim_end_matches('"');
            if l.starts_with('│') {
                assert!(l.ends_with('│'), "{w}: {l}");
            }
        }
    }
}

/// `d` shows a blocked item's whole reason and next step, wrapped rather
/// than cut, even at 50 columns; `r` is offered to check again.
#[test]
fn the_blocked_list_shows_the_whole_reason_and_offers_r() {
    use crossterm::event::KeyCode;
    let mut app = App::new(fixture_report(), "/Users/dev/src".into());
    let u = plain_unit("/Users/dev/src/p/dir", 1_000_000);
    app.marked.insert(u.path.display().to_string(), u);
    app.confirm_open = true;
    app.blocked = vec![swamp_tui::app::BlockedItem {
        name: "swamp · swamp_tui incremental build (target/debug)".into(),
        reason: "a process has this open: cargo (pid 4021) is running a build in this directory"
            .into(),
        next: "stop the build, then press r to check again".into(),
    }];
    swamp_tui::handle_key(&mut app, KeyCode::Char('d'));
    for w in [50u16, 80] {
        let f = capture(&app, w, 24);
        let flat: String = f
            .lines()
            .map(|l| l.trim_matches('"').trim_matches('│').trim().to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            flat.contains("cargo (pid 4021) is running a build in this directory"),
            "{w}: the whole reason:\n{f}"
        );
        assert!(
            flat.contains("stop the build, then press r to check again"),
            "{w}\n{f}"
        );
        assert!(f.contains("r check again"), "{w}: the key is offered\n{f}");
    }
}

/// A long name gives way, not the tree rail in front of it; the cargo
/// popup wraps a long line instead of cutting its tail.
#[test]
fn long_rows_keep_their_rail_and_long_popup_lines_wrap() {
    let mut report = fixture_report();
    let wt = &mut report.projects[0].worktrees[0];
    let base = wt.path.clone();
    wt.artifacts.push(art(
        ArtifactKind::DependencyTree,
        base.join("a-very-long-package-directory-name-that-cannot-fit/node_modules")
            .to_str()
            .unwrap(),
        3_000_000_000,
        None,
    ));
    let mut app = App::new(report, "/Users/dev/src".into());
    app.clear_filter();
    app.drill_into_selected();
    let f = capture(&app, 50, 24);
    // The view strip also uses `…` where it is cut; the rail is about rows.
    let cut: Vec<&str> = f
        .lines()
        .filter(|l| l.contains('…') && (l.contains("├─") || l.contains("└─")))
        .collect();
    assert!(!cut.is_empty(), "the long name is shortened:\n{f}");
    for l in cut {
        let ell = l.find('…').unwrap();
        let rail = l.find("├─").or_else(|| l.find("└─")).unwrap_or(usize::MAX);
        assert!(rail < ell, "the rail survives the cut: {l}");
    }
    app.cargo_inspection = Some(vec![
        "features [\"alloc\", \"default\", \"perf-inline\", \"perf-literal\", \"std\", \"unicode-word-boundary\", ENDMARK]"
            .to_string(),
    ]);
    let f = capture(&app, 50, 24);
    assert!(
        f.contains("ENDMARK"),
        "the tail of a long line is wrapped in, not cut:\n{f}"
    );
}

/// Adversarial audit (v0.8.0 G2): a drilldown row for a folder that could
/// not be read must not show a size of zero as a fact, and a negative
/// adjustment row must not be drawn as `0B` (the rows would then not add
/// up to the unit on screen). Wrong patch: `Row.bytes` is unsigned, so the
/// builder writes `bytes.unwrap_or(0).max(0)` and the Size column renders
/// that number like any other.
#[test]
fn adv_a_not_measured_or_negative_drilldown_row_never_draws_a_zero_size() {
    use swamp_core::drilldown::{ChildKind, ChildMeasure, UnitChild};
    let child = |kind, name: &str, bytes: Option<i64>, measure| UnitChild {
        kind,
        name: name.into(),
        bytes,
        measure,
        mtime_max: 0,
        entries: 2,
        not_measured: 0,
        last_used: Default::default(),
        access_evidence: None,
    };
    let mut app = App::new(fixture_report(), "/Users/dev/src".into());
    app.filter_text.clear();
    app.set_external_units(vec![swamp_core::external::ExternalUnit {
        detector_id: "rustup".into(),
        detector_name: "rustup".into(),
        category: swamp_core::locations::StorageCategory::Installation,
        provenance: swamp_core::locations::Provenance::BuiltinConvention,
        path: PathBuf::from("/Users/dev/.rustup/toolchains"),
        bytes: 2_950_000_000,
        mtime_max: 0,
        hardlinked: true,
        growth_bytes: None,
        regrowth_count: 0,
        observed_at: 1_700_000_000,
        consumers: Vec::new(),
        note: None,
        evidence: Vec::new(),
        bytes_counted_elsewhere: 0,
        overlap_count: 0,
        last_used: Default::default(),
        children: vec![
            child(
                ChildKind::Entry,
                "stable",
                Some(2_000_000_000),
                ChildMeasure::Complete,
            ),
            child(
                ChildKind::Entry,
                "lockedtc",
                None,
                ChildMeasure::NotMeasured,
            ),
            child(
                ChildKind::Remainder,
                "",
                Some(1_000_000_000),
                ChildMeasure::Complete,
            ),
            child(
                ChildKind::Adjustment,
                "",
                Some(-50_000_000),
                ChildMeasure::Complete,
            ),
        ],
    }]);
    app.set_view(ViewKind::External);
    app.selected = 0;
    app.enter_row();
    let frame = capture(&app, 200, 60);
    let line_of = |needle: &str| {
        frame
            .lines()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no line with {needle}:\n{frame}"))
            .to_string()
    };
    // The Size column: `0B`, with or without the allocated-basis `*`.
    let zero = |l: &str| l.contains(" 0B*") || l.contains(" 0B ") || l.contains(" 0.0B");
    let locked = line_of("lockedtc");
    let adjustment = line_of("adjustment");
    assert!(
        !zero(&locked) && !zero(&adjustment),
        "a zero size drawn as a fact:\n{locked}\n{adjustment}"
    );
}

// ---------------------------------------------------------------------
// v0.8.0 G4a: the Reclaim view (#175).
// ---------------------------------------------------------------------

fn reclaim_unit(
    detector: &str,
    category: swamp_core::locations::StorageCategory,
    path: &str,
    bytes: u64,
    children: Vec<swamp_core::drilldown::UnitChild>,
) -> swamp_core::external::ExternalUnit {
    swamp_core::external::ExternalUnit {
        detector_id: detector.into(),
        detector_name: detector.into(),
        category,
        provenance: swamp_core::locations::Provenance::BuiltinConvention,
        path: PathBuf::from(path),
        bytes,
        mtime_max: 0,
        hardlinked: false,
        growth_bytes: None,
        regrowth_count: 0,
        observed_at: 1_700_000_000,
        consumers: Vec::new(),
        note: None,
        evidence: Vec::new(),
        bytes_counted_elsewhere: 0,
        overlap_count: 0,
        last_used: Default::default(),
        children,
    }
}

fn reclaim_child(
    kind: swamp_core::drilldown::ChildKind,
    name: &str,
    bytes: Option<i64>,
) -> swamp_core::drilldown::UnitChild {
    swamp_core::drilldown::UnitChild {
        kind,
        name: name.into(),
        bytes,
        measure: if bytes.is_some() {
            swamp_core::drilldown::ChildMeasure::Complete
        } else {
            swamp_core::drilldown::ChildMeasure::NotMeasured
        },
        mtime_max: 0,
        entries: 1,
        not_measured: 0,
        last_used: Default::default(),
        access_evidence: None,
    }
}

fn manager_fact(
    manager: &str,
    probe: &str,
    kind: swamp_core::manager_facts::FactKind,
    subject: Option<&str>,
    text: &str,
) -> swamp_core::manager_facts::ManagerFact {
    swamp_core::manager_facts::ManagerFact {
        manager: manager.into(),
        probe: probe.into(),
        kind,
        subject: subject.map(str::to_string),
        text: text.into(),
        observed_at: 1_700_000_000,
    }
}

fn reclaim_app(declared_missing: bool) -> App {
    use swamp_core::drilldown::ChildKind::{Entry, Remainder};
    use swamp_core::locations::StorageCategory as C;
    use swamp_core::manager_facts::FactKind as K;
    let mut app = App::new(fixture_report(), "/Users/dev/src".into());
    app.filter_text.clear();
    app.set_external_units(vec![
        reclaim_unit(
            "rustup",
            C::Installation,
            "/Users/dev/.rustup/toolchains",
            3_950_000_000,
            vec![
                reclaim_child(Entry, "stable-aarch64-apple-darwin", Some(2_000_000_000)),
                reclaim_child(Entry, "nightly-aarch64-apple-darwin", Some(1_500_000_000)),
                reclaim_child(Entry, "locked-toolchain", None),
                reclaim_child(Remainder, "", Some(450_000_000)),
            ],
        ),
        reclaim_unit(
            "mise",
            C::Installation,
            "/Users/dev/.local/share/mise/installs",
            1_200_000_000,
            vec![
                reclaim_child(Entry, "node", Some(700_000_000)),
                reclaim_child(Entry, "java", Some(500_000_000)),
            ],
        ),
        reclaim_unit(
            "codex",
            C::LocalState,
            "/Users/dev/.codex",
            6_800_000_000,
            Vec::new(),
        ),
        reclaim_unit(
            "uv",
            C::Cache,
            "/Users/dev/.cache/uv",
            3_900_000_000,
            Vec::new(),
        ),
    ]);
    let listing_at = app.report.observed_at;
    app.set_manager_facts(swamp_core::manager_facts::ManagerFacts {
        observed: true,
        facts: [
            manager_fact("", "", K::Pass, None, ""),
            manager_fact(
                "rustup",
                "settings-default",
                K::ActiveDefault,
                Some("stable"),
                "default_toolchain \"stable\" in settings.toml",
            ),
            manager_fact("rustup", "settings-default", K::Checked, None, ""),
            manager_fact(
                "mise",
                "global-tools",
                K::ActiveDefault,
                Some("node"),
                "listed in the global configuration /Users/dev/.config/mise/config.toml",
            ),
            manager_fact("mise", "global-tools", K::Checked, None, ""),
            manager_fact(
                "mise",
                "prune-dry-run",
                K::ReportsPrunable,
                Some("java@temurin-17.0.20+101"),
                "mise java@temurin-17.0.20+101 is prunable: java is required at zulu-8.96.0.19 by ~/src/etl/mise.toml",
            ),
            manager_fact("mise", "prune-dry-run", K::Checked, None, ""),
        ]
        .into_iter()
        // Read by the pass that follows the observation the units come from.
        .map(|f| swamp_core::manager_facts::ManagerFact {
            observed_at: listing_at,
            ..f
        })
        .collect(),
    });
    let mut roots = vec![swamp_core::roots::DeclaredRoot {
        path: "/Users/dev/src".into(),
        state: swamp_core::roots::DeclaredState::Present {
            bytes: Some(1),
            complete: true,
        },
    }];
    if declared_missing {
        roots.push(swamp_core::roots::DeclaredRoot {
            path: "/Volumes/work/src".into(),
            state: swamp_core::roots::DeclaredState::Missing,
        });
    }
    app.set_declared_roots(&roots);
    app.set_view(ViewKind::Reclaim);
    app
}

#[test]
fn reclaim_view_frames() {
    use crossterm::event::KeyCode;
    for (w, h) in [(80, 24), (120, 30)] {
        let mut app = reclaim_app(false);
        app.width = w;
        check(&format!("reclaim_{w}x{h}"), &capture(&app, w, h));
        // Open the toolchains row: the folders add up to its size, the
        // default one is marked and the unreadable one is `unmeasured`.
        let at = app
            .rows()
            .iter()
            .position(|r| r.label.contains(".rustup/toolchains"))
            .unwrap();
        app.selected = at;
        swamp_tui::handle_key(&mut app, KeyCode::Enter);
        check(&format!("reclaim_open_{w}x{h}"), &capture(&app, w, h));
    }
}

/// The tempting wrong patch: the scope statement is a header nobody sees
/// at a small width, or is dropped when a declared root is missing.
#[test]
fn reclaim_view_says_what_its_consumer_evidence_was_checked_against_on_every_screen() {
    for w in [80u16, 120, 200] {
        let app = reclaim_app(false);
        let f = capture(&app, w, 24);
        assert!(f.contains("2 projects in 1 declared root"), "w={w}\n{f}");
        let app = reclaim_app(true);
        let f = capture(&app, w, 24);
        assert!(f.contains("incomplete"), "w={w}\n{f}");
        assert!(f.contains("in 2 declared roots"), "w={w}\n{f}");
    }
}

/// The tempting wrong patch: entering the view scans, or asks a manager,
/// to fill itself. It is a function of stored facts: no listing, no stat,
/// no process.
#[test]
fn opening_the_reclaim_view_scans_nothing() {
    let mut app = reclaim_app(false);
    app.set_view(ViewKind::External);
    let (frame, work) = swamp_core::work_counters::measured(|| {
        app.set_view(ViewKind::Reclaim);
        capture(&app, 80, 24)
    });
    assert_eq!(work, swamp_core::work_counters::WorkCounters::default());
    assert!(frame.contains("codex"), "{frame}");
}

/// The tempting wrong patch: the new view lays its table out differently
/// (or moves the footer), so switching views shifts what the eye has. The
/// heading, the footer and its key hints sit on the same screen rows in
/// every view; selecting or opening a row moves nothing above it; and the
/// screen is the same height at 80 and 120 columns.
#[test]
fn reclaim_view_keeps_the_layout_hints_and_rows_still() {
    use crossterm::event::KeyCode;
    for (w, h) in [(80u16, 24u16), (120, 30)] {
        let mut app = reclaim_app(false);
        app.width = w;
        let footer_of = |app: &App| {
            let f = capture(app, w, h);
            assert_eq!(f.lines().count(), h as usize, "w={w}");
            line_of(&f, h as usize - 1)
        };
        let reclaim_footer = footer_of(&app);
        assert!(reclaim_footer.contains("v view"), "{reclaim_footer}");
        assert!(reclaim_footer.contains("q quit"), "{reclaim_footer}");
        assert!(reclaim_footer.contains("? help"), "{reclaim_footer}");
        // Reclaim names `Space mark` and `⌫ review` while the row under the
        // cursor is a real path (a unit, or a listed folder), and never
        // the old "delete" word, which Reclaim did not do.
        for want in ["⌫ review", "Space mark"] {
            assert!(reclaim_footer.contains(want), "{reclaim_footer}");
        }
        assert!(!reclaim_footer.contains("⌫ delete"), "{reclaim_footer}");
        // Every view keeps navigation and exit hints on the same row;
        // action/filter hints reflect whether that view can use them.
        for v in ViewKind::ALL {
            app.set_view(v);
            let footer = footer_of(&app);
            for hint in ["Tab section", "v view", "? help", "q quit"] {
                assert!(footer.contains(hint), "w={w} view={v:?}: {footer}");
            }
            if !v.uses_filter() {
                assert!(!footer.contains("/ filter"));
            }
        }
        app.set_view(ViewKind::Reclaim);
        // The heading is on the row every flat view puts it on.
        let heading = |app: &App| {
            capture(app, w, h)
                .lines()
                .position(|l| {
                    l.contains("Size")
                        && (l.contains("Storage item") || l.contains("Tool location"))
                })
                .unwrap()
        };
        let reclaim_heading = heading(&app);
        app.set_view(ViewKind::External);
        assert_eq!(heading(&app), reclaim_heading, "w={w}");
        app.set_view(ViewKind::Reclaim);
        // Selecting another row moves no table row.
        let table = |app: &App, n: usize| -> Vec<String> {
            let f = capture(app, w, h);
            (0..n).map(|i| line_of(&f, i)).collect()
        };
        app.selected = 0;
        let first = table(&app, 8);
        app.selected = 1;
        assert_eq!(table(&app, 8), first, "w={w}: selecting shifted rows");
        // Opening a row moves only the rows below it.
        app.selected = 0;
        let before = capture(&app, w, h);
        let at = app.selected;
        swamp_tui::handle_key(&mut app, KeyCode::Enter);
        let after = capture(&app, w, h);
        for i in 0..(reclaim_heading + 2 + at) {
            assert_eq!(line_of(&before, i), line_of(&after, i), "w={w} row {i}");
        }
        assert_eq!(after.lines().count(), h as usize);
    }
}

/// The tempting wrong patch: an unreadable folder draws `0B`, and the
/// default toolchain looks like any other row.
#[test]
fn reclaim_children_draw_unmeasured_and_mark_the_default() {
    use crossterm::event::KeyCode;
    let mut app = reclaim_app(false);
    let at = app
        .rows()
        .iter()
        .position(|r| r.label.contains(".rustup/toolchains"))
        .unwrap();
    app.selected = at;
    swamp_tui::handle_key(&mut app, KeyCode::Enter);
    let f = capture(&app, 120, 30);
    assert!(f.contains("locked-toolchain"), "{f}");
    let locked = f.lines().find(|l| l.contains("locked-toolchain")).unwrap();
    assert!(locked.contains("unmeasured"), "{locked}");
    assert!(!locked.contains("0B"), "{locked}");
    let stable = f
        .lines()
        .find(|l| l.contains("active default") && l.contains("├─"))
        .unwrap_or_else(|| panic!("the default toolchain's row says so\n{f}"));
    assert!(stable.contains("stable"), "{stable}");
    let nightly = f
        .lines()
        .find(|l| l.contains("nightly-aarch64-apple-darwin"))
        .unwrap();
    assert!(!nightly.contains("active default"), "{nightly}");
}

/// The tempting wrong patch: a verdict word or an em dash reaches the
/// screen through a state the frames did not cover. Checked over every
/// string the view builds (rows, signals, detail lines, the header and
/// scope line) with every unit opened, and over rendered frames for the
/// verdict words (the table's own "no change" glyph is an existing cell
/// mark, not one of this view's strings).
#[test]
fn reclaim_strings_carry_no_verdict_words_and_no_em_dashes() {
    use crossterm::event::KeyCode;
    let mut app = reclaim_app(true);
    for i in 0..app.rows().len() {
        app.selected = i;
        swamp_tui::handle_key(&mut app, KeyCode::Enter);
    }
    let mut strings: Vec<String> = Vec::new();
    for row in app.rows() {
        strings.push(row.label.clone());
        strings.extend(row.signals.iter().cloned());
        strings.extend(row.detail_lines.iter().cloned());
        strings.extend(row.last_used.iter().cloned());
        strings.extend(row.size_text.iter().cloned());
    }
    let view = app.reclaim_view();
    strings.push(view.scope.statement.clone());
    strings.extend(view.coverage_notes.iter().cloned());
    let mut blob = strings.join("\n");
    assert!(!blob.contains('\u{2014}'), "{blob}");
    for w in [50u16, 80, 120, 200] {
        for sel in 0..app.rows().len().min(8) {
            app.selected = sel;
            blob.push_str(&capture(&app, w, 30));
        }
    }
    let lower = blob.to_lowercase();
    for word in [
        "unused", "obsolete", "stale", "orphan", "junk", "garbage", "safe",
    ] {
        assert!(
            !lower
                .split(|c: char| !c.is_alphanumeric())
                .any(|w| w == word),
            "{word}"
        );
    }
}

/// Adversarial (auditor): the Reclaim view, closed and open, draws at
/// tiny and narrow sizes without panicking, and at 40/50 columns its
/// footer keeps a key hint and the heading row matches External's.
#[test]
fn adv_reclaim_view_survives_tiny_and_narrow_screens() {
    use crossterm::event::KeyCode;
    for (w, h) in [(0u16, 0u16), (1, 1), (2, 2), (10, 3), (40, 12), (50, 16)] {
        let mut app = reclaim_app(false);
        app.width = w;
        draw_only(&app, w, h);
        if let Some(at) = app
            .rows()
            .iter()
            .position(|r| r.label.contains(".rustup/toolchains"))
        {
            app.selected = at;
            swamp_tui::handle_key(&mut app, KeyCode::Enter);
            draw_only(&app, w, h);
            swamp_tui::handle_key(&mut app, KeyCode::Right);
            draw_only(&app, w, h);
        }
    }
    for (w, h) in [(40u16, 12u16), (50, 16)] {
        let mut app = reclaim_app(false);
        app.width = w;
        let r = capture(&app, w, h);
        app.set_view(ViewKind::External);
        let e = capture(&app, w, h);
        assert!(
            !line_of(&r, h as usize - 1).is_empty(),
            "w={w} footer empty\n{r}"
        );
        assert_eq!(
            line_of(&r, 0),
            line_of(&e, 0),
            "w={w} heading differs\n{r}\n{e}"
        );
    }
}

fn draw_only(app: &App, w: u16, h: u16) {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| ui::draw(f, app)).unwrap();
}

/// Guard for the class of failure that broke every frame on a day
/// rollover: frames are drawn at [`FRAME_NOW`], never at the wall clock.
/// Draws the same scenes with the wall clock moved by a day and by 400
/// days and requires identical frames. Tempting wrong patch: freezing the
/// clock in some tests only, or regenerating the fixtures on the day they
/// fail, which only moves the failure to the next rollover.
#[test]
fn frames_do_not_depend_on_the_wall_clock() {
    let app = App::new(fixture_report(), "/Users/dev/src".into());
    let base = capture(&app, 200, 60);
    for shift in [86_400_i64, 400 * 86_400] {
        swamp_core::entities::test_clock::shift_wall(shift);
        let shifted = capture(&app, 200, 60);
        swamp_core::entities::test_clock::shift_wall(0);
        assert_eq!(
            base, shifted,
            "a frame changed when the wall clock moved {shift}s"
        );
    }
    // And nothing in the TUI reads the wall clock around `entities::now`.
    let src = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let mut stack = vec![std::path::PathBuf::from(src)];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let text = std::fs::read_to_string(&p).unwrap();
                assert!(
                    !text.contains("SystemTime::now"),
                    "{} reads the wall clock directly; use swamp_core::entities::now",
                    p.display()
                );
            }
        }
    }
}

#[test]
fn projects_show_current_sizes_with_zero_growth_and_sort_largest_first() {
    let mut app = App::new(fixture_report(), "/Users/dev/src".into());
    for project in &mut app.report.projects {
        for wt in &mut project.worktrees {
            for artifact in &mut wt.artifacts {
                artifact.growth_bytes = Some(0);
            }
        }
    }
    app.clear_filter();
    app.sort = swamp_tui::model::Sort::Size;
    let rows = app.rows();
    assert_eq!(rows.len(), app.report.projects.len());
    assert!(rows.windows(2).all(|pair| pair[0].bytes >= pair[1].bytes));
    assert!(rows.iter().all(|r| r.growth == Some(0)));
    let sizes: Vec<_> = rows.iter().map(|r| r.bytes).collect();
    app.report.series_window_secs = 7 * 86400;
    assert_eq!(
        sizes,
        app.rows().iter().map(|r| r.bytes).collect::<Vec<_>>()
    );
    for (w, h) in [(80, 24), (200, 60)] {
        let frame = capture(&app, w, h);
        assert!(frame.contains("mole") && frame.contains("swamp"), "{frame}");
        assert!(!frame.contains("4 Storage"), "{frame}");
    }
}
