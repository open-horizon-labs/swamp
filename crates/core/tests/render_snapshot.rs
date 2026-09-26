//! Renderer tests against a hand-built, fully deterministic `Report` (no
//! real git checkouts, so no wall-clock-dependent commit ages). Covers:
//! the overview snapshot golden file, a zero-artifact project rendering
//! "0" rather than crashing or sorting as null, and unowned aggregation
//! by top-level directory/reason with shared caches split out.

use std::path::PathBuf;
use swamp_core::entities::Confidence;
use swamp_core::render::{
    render_kinds, render_overview, render_project, render_project_tree, render_view_builds,
    render_view_docker, render_worktrees,
};
use swamp_core::report::{
    ArtifactKind, ArtifactRow, ProjectRow, Reconciliation, Report, Signal, Source, UnownedReason,
    UnownedRow, WorktreeKind, WorktreeRow,
};

fn artifact(kind: ArtifactKind, path: &str, bytes: u64, growth: Option<i64>) -> ArtifactRow {
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
        observed_at: 1_000_000,
        confidence: Confidence::High,
        source: Source::new("test"),
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
        observed_at: 1_000_000,
        root: PathBuf::from("/src"),
        projects: vec![
            ProjectRow {
                project_id: "p-big".to_string(),
                name: "big-grower".to_string(),
                remote: None,
                ecosystems: Vec::new(),
                worktrees: vec![WorktreeRow {
                    worktree_id: "wt-big".to_string(),
                    path: PathBuf::from("/src/big-grower"),
                    kind: WorktreeKind::Main,
                    artifacts: vec![
                        artifact(
                            ArtifactKind::BuildOutput,
                            "/src/big-grower/target",
                            5_000_000_000,
                            Some(2_000_000_000),
                        ),
                        artifact(
                            ArtifactKind::DependencyTree,
                            "/src/big-grower/node_modules",
                            1_000_000_000,
                            Some(0),
                        ),
                    ],
                    signals: vec![
                        Signal {
                            name: "last_commit".to_string(),
                            value: "last commit 3d".to_string(),
                        },
                        Signal {
                            name: "dirty".to_string(),
                            value: "dirty".to_string(),
                        },
                        Signal {
                            name: "unpushed".to_string(),
                            value: "2 unpushed".to_string(),
                        },
                        Signal {
                            name: "locked".to_string(),
                            value: "unlocked".to_string(),
                        },
                    ],
                    branch: None,
                    github: None,
                    merge_complete: None,
                    idle_secs: None,
                }],
            },
            ProjectRow {
                project_id: "p-empty".to_string(),
                name: "no-artifacts".to_string(),
                remote: None,
                ecosystems: Vec::new(),
                worktrees: vec![WorktreeRow {
                    worktree_id: "wt-empty".to_string(),
                    path: PathBuf::from("/src/no-artifacts"),
                    kind: WorktreeKind::Main,
                    artifacts: vec![],
                    signals: vec![],
                    branch: None,
                    github: None,
                    merge_complete: None,
                    idle_secs: None,
                }],
            },
            ProjectRow {
                project_id: "p-small".to_string(),
                name: "small-static".to_string(),
                remote: None,
                ecosystems: Vec::new(),
                worktrees: vec![WorktreeRow {
                    worktree_id: "wt-small".to_string(),
                    path: PathBuf::from("/src/small-static"),
                    kind: WorktreeKind::Main,
                    artifacts: vec![artifact(
                        ArtifactKind::Cache,
                        "/src/small-static/.cache",
                        200_000_000,
                        None,
                    )],
                    signals: vec![],
                    branch: None,
                    github: None,
                    merge_complete: None,
                    idle_secs: None,
                }],
            },
        ],
        notes: vec![],
        series_by_key: Default::default(),
        total_series: Vec::new(),
        series_window_secs: 0,
        summary: Default::default(),
        dirs_by_worktree: None,
        files_by_worktree: None,
        schedule_line: None,
        github_enrichment: None,
        nested_artifacts: Vec::new(),
        unowned: vec![
            UnownedRow {
                measurement: None,
                path_or_object: "old-project/build".to_string(),
                bytes: 300_000_000,
                reason: UnownedReason::NoContainingRepo,
                shared_bytes: None,
                note: None,
                created_at: None,
                containers: Vec::new(),
                shared_with: Vec::new(),
                dangling: false,
                docker_kind: None,
                evidence: Vec::new(),
            },
            UnownedRow {
                measurement: None,
                path_or_object: "old-project/tmp".to_string(),
                bytes: 100_000_000,
                reason: UnownedReason::NoContainingRepo,
                shared_bytes: None,
                note: None,
                created_at: None,
                containers: Vec::new(),
                shared_with: Vec::new(),
                dangling: false,
                docker_kind: None,
                evidence: Vec::new(),
            },
            UnownedRow {
                measurement: None,
                path_or_object: "restricted/vault".to_string(),
                bytes: 0,
                reason: UnownedReason::PermissionDenied,
                shared_bytes: None,
                note: None,
                created_at: None,
                containers: Vec::new(),
                shared_with: Vec::new(),
                dangling: false,
                docker_kind: None,
                evidence: Vec::new(),
            },
            UnownedRow {
                measurement: None,
                path_or_object: "/Users/x/.cache".to_string(),
                bytes: 4_000_000_000,
                reason: UnownedReason::SharedCache,
                shared_bytes: None,
                note: None,
                created_at: None,
                containers: Vec::new(),
                shared_with: Vec::new(),
                dangling: false,
                docker_kind: None,
                evidence: Vec::new(),
            },
        ],
        reconciliation: Reconciliation {
            unique_estimate: None,
            attributed: 6_200_000_000,
            unowned: 4_400_000_000,
            walked_total: 10_600_000_000,
            du_total: None,
            docker_attributed: 0,
            docker_unowned: 0,
        },
    }
}

#[test]
fn overview_matches_snapshot() {
    let report = fixture_report();
    let text = render_overview(&report, false, false, false);
    let golden_path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/snapshots/overview.txt");
    let expected = std::fs::read_to_string(golden_path).unwrap_or_default();
    if std::env::var("UPDATE_SNAPSHOTS").is_ok() {
        std::fs::write(golden_path, &text).unwrap();
        return;
    }
    assert_eq!(
        text, expected,
        "overview output drifted from tests/snapshots/overview.txt; re-run with UPDATE_SNAPSHOTS=1 after hand-checking it reads well"
    );
}

#[test]
fn sorted_by_growth_desc_then_bytes_desc() {
    let report = fixture_report();
    let text = render_overview(&report, false, false, false);
    let big = text.find("big-grower").unwrap();
    let small = text.find("small-static").unwrap();
    let empty = text.find("no-artifacts").unwrap();
    // big-grower has growth > 0, so it must lead; the two zero/None-growth
    // rows follow ordered by bytes desc (small-static has bytes, no-artifacts
    // has none).
    assert!(big < small);
    assert!(small < empty);
}

#[test]
fn zero_artifact_project_renders_zero_never_panics() {
    let report = fixture_report();
    let text = render_overview(&report, false, false, false);
    let line = text
        .lines()
        .find(|l| l.contains("no-artifacts"))
        .expect("no-artifacts row present");
    assert!(
        line.contains(" 0 "),
        "expected a literal 0 byte column, got: {line}"
    );
    assert!(
        line.contains('—'),
        "expected em-dash for absent growth, got: {line}"
    );

    let drill = render_project(&report, "no-artifacts").expect("project found");
    assert!(drill.contains('0'));
}

#[test]
fn unowned_aggregated_by_top_level_dir_and_reason_shared_caches_separate() {
    let report = fixture_report();
    let text = render_overview(&report, false, false, false);
    // Aggregated: one "old-project" row (300MB + 100MB), never two
    // per-file rows.
    let old_project_lines: Vec<&str> = text
        .lines()
        .filter(|l| l.trim_start().starts_with("old-project"))
        .collect();
    assert_eq!(
        old_project_lines.len(),
        1,
        "expected one aggregated row, got: {old_project_lines:?}"
    );
    assert!(old_project_lines[0].contains("400.0MB"));

    // Shared cache reported separately, not folded into by-dir/by-reason.
    assert!(text.contains("shared caches: 4.0GB"));
    assert!(text.contains("no-containing-repo"));
}

#[test]
fn kinds_summary_bytes_and_count() {
    let report = fixture_report();
    let text = render_kinds(&report);
    assert!(text.contains("build"));
    assert!(text.contains("deps"));
    assert!(text.contains("cache"));
}

#[test]
fn drill_shows_worktree_kind_path_bytes_growth_regrowth_signals() {
    let report = fixture_report();
    let text = render_project(&report, "big-grower").expect("project found");
    assert!(text.contains("worktree:"));
    assert!(text.contains("build"));
    assert!(text.contains("target"));
    assert!(text.contains("dirty"));
    assert!(text.contains("2 unpushed"));
}

fn tree_fixture_report() -> Report {
    let mut report = fixture_report();
    // Main checkout with a nested, small, deep artifact that must fold,
    // plus a linked worktree, so the tree exercises checkouts + worktrees
    // + folding + relative paths + un-doubled signals in one fixture.
    report.projects[0].worktrees[0].artifacts.push(artifact(
        ArtifactKind::Cache,
        "/src/big-grower/managed_components/foo/test/target",
        44_000,
        None,
    ));
    report.projects[0].worktrees[0].artifacts.push(artifact(
        ArtifactKind::Cache,
        "/src/big-grower/managed_components/foo/test/build",
        12_000,
        None,
    ));
    report.projects[0].worktrees.push(WorktreeRow {
        worktree_id: "wt-linked".to_string(),
        path: PathBuf::from("/src/big-grower/.worktrees/store"),
        kind: WorktreeKind::Linked,
        artifacts: vec![artifact(
            ArtifactKind::Source,
            "/src/big-grower/.worktrees/store",
            54_000_000,
            None,
        )],
        signals: vec![Signal {
            name: "last_commit".to_string(),
            value: "last commit 10h".to_string(),
        }],
        branch: None,
        github: None,
        merge_complete: None,
        idle_secs: None,
    });
    report
}

#[test]
fn tree_paths_are_relative_never_absolute() {
    let report = tree_fixture_report();
    let text = render_project_tree(&report, "big-grower").expect("project found");
    assert!(
        !text.contains("/src/"),
        "tree drill leaked an absolute path: {text}"
    );
    assert!(text.contains("target"));
    assert!(text.contains(".worktrees/store"));
}

#[test]
fn tree_signals_print_value_only_never_doubled() {
    let report = tree_fixture_report();
    let text = render_project_tree(&report, "big-grower").expect("project found");
    // "dirty" must appear as the bare value, never "dirty: dirty".
    assert!(!text.contains("dirty: dirty"));
    assert!(!text.contains("last_commit: last commit"));
    assert!(text.contains("last commit 10h"));
}

#[test]
fn tree_folds_deep_small_artifacts_with_a_count() {
    let report = tree_fixture_report();
    let text = render_project_tree(&report, "big-grower").expect("project found");
    // The 44 KB nested managed_components/foo/test/target row folds into
    // its parent (managed_components/foo) rather than getting its own
    // line, with a count so the human still sees it happened.
    assert!(
        text.contains("managed_components/foo (x2)"),
        "expected a folded row with a count of 2, got: {text}"
    );
    assert!(!text.contains("managed_components/foo/test/target"));
}

#[test]
fn tree_lists_checkouts_before_linked_worktrees() {
    let report = tree_fixture_report();
    let text = render_project_tree(&report, "big-grower").expect("project found");
    let main = text.find("main").unwrap();
    let linked = text.find("linked").unwrap();
    assert!(main < linked);
}

#[test]
fn view_builds_lists_build_and_cache_rows_sorted_by_bytes() {
    let report = fixture_report();
    let text = render_view_builds(&report, None);
    let big = text.find("target").unwrap();
    let small = text.find(".cache").unwrap();
    assert!(big < small);
}

#[test]
fn view_docker_lists_unowned_name_alike_candidates_as_unattributed() {
    let mut report = fixture_report();
    report.unowned.push(UnownedRow {
        measurement: None,
        path_or_object: "big-grower-staging:latest".to_string(),
        bytes: 500_000_000,
        reason: UnownedReason::DockerNoJoin,
        shared_bytes: Some(100_000_000),
        note: None,
        created_at: None,
        containers: Vec::new(),
        shared_with: Vec::new(),
        dangling: false,
        docker_kind: Some("image".to_string()),
        evidence: Vec::new(),
    });
    let text = render_view_docker(&report, Some("big-grower"));
    assert!(text.contains("unowned, name-alike"));
    assert!(text.contains("big-grower-staging:latest"));
}

#[test]
fn worktrees_view_shows_removal_hint_only_for_linked() {
    let mut report = fixture_report();
    // Add a Linked worktree alongside the existing Main one so both
    // branches of the removal-hint logic run.
    report.projects[0].worktrees.push(WorktreeRow {
        worktree_id: "wt-linked".to_string(),
        path: PathBuf::from("/src/big-grower/.worktrees/store"),
        kind: WorktreeKind::Linked,
        artifacts: vec![],
        signals: vec![],
        branch: None,
        github: None,
        merge_complete: None,
        idle_secs: None,
    });
    let text = render_worktrees(&report, &swamp_core::filter::Filter::default());
    assert!(
        text.contains("git worktree remove /src/big-grower/.worktrees/store"),
        "expected removal command for the linked worktree, got: {text}"
    );
    assert!(
        text.contains("main checkout -- not removable as a worktree"),
        "expected the main-checkout message instead of a removal command, got: {text}"
    );
    // The Main checkout's own row must never carry the literal removal
    // command -- it isn't a worktree `git worktree remove` can act on.
    assert!(!text.contains("git worktree remove /src/big-grower\n"));
}

// ---------------------------------------------------------------------
// `render_view_external`: the CLI text evidence surface DESIGN.md names.
//
// The 2026-09-22 re-review found it referenced by no test in the
// workspace -- so the one place a user reads an external unit's
// decision evidence in the terminal had no rendering test at all, and
// the guardrails about *what may be printed* (facts, never verdicts;
// an unknown always with its reason; a modification age labelled
// correctly) were unenforced on this surface.
// ---------------------------------------------------------------------

fn external_unit(
    path: &str,
    bytes: u64,
    mtime_max: u64,
    evidence: Vec<swamp_core::evidence::Evidence>,
) -> swamp_core::external::ExternalUnit {
    use swamp_core::locations::{Provenance, StorageCategory};
    swamp_core::external::ExternalUnit {
        detector_id: "cargo-home".into(),
        detector_name: "Cargo home".into(),
        category: StorageCategory::Cache,
        provenance: Provenance::BuiltinConvention,
        path: PathBuf::from(path),
        bytes,
        mtime_max,
        hardlinked: false,
        growth_bytes: Some(4_096),
        regrowth_count: 0,
        observed_at: 1_000_000,
        consumers: Vec::new(),
        note: None,
        evidence,
    }
}

#[test]
fn external_view_renders_units_largest_first_with_a_total() {
    let out = swamp_core::render::render_view_external(&[
        external_unit("/fixture/small", 1_024, 0, Vec::new()),
        external_unit("/fixture/large", 1_048_576, 0, Vec::new()),
    ]);
    let large_at = out.find("/fixture/large").expect("large unit rendered");
    let small_at = out.find("/fixture/small").expect("small unit rendered");
    assert!(large_at < small_at, "largest unit first:\n{out}");
    assert!(
        out.contains("external storage total:"),
        "the view must state its own total:\n{out}"
    );
    assert!(
        out.contains("(2 units"),
        "the total must say what it covers:\n{out}"
    );
}

#[test]
fn an_empty_external_view_says_so_rather_than_rendering_nothing() {
    let out = swamp_core::render::render_view_external(&[]);
    assert!(
        out.contains("no external storage units detected"),
        "an empty view must say it is empty, never print a blank screen: {out:?}"
    );
    // "none detected" is a coverage statement, not a verdict about the
    // disk: it must not claim there is nothing there.
    assert!(!out.contains("total:"), "{out:?}");
}

#[test]
fn external_view_prints_an_unknown_together_with_its_reason() {
    use swamp_core::evidence::{Evidence, FactKind};
    let unit = external_unit(
        "/fixture/cache",
        2_048,
        0,
        vec![Evidence::unavailable(
            FactKind::Activity,
            swamp_core::evidence::FactSubtype::Accessed,
            swamp_core::evidence::EvidenceSource::FilesystemMetadata {
                detail: "atime".into(),
            },
            1_000,
            "atime is not recorded on this volume",
        )],
    );
    let out = swamp_core::render::render_view_external(&[unit]);
    assert!(
        out.contains("atime is not recorded on this volume"),
        "an unknown printed without its reason reads as \"nothing there\":\n{out}"
    );
}

/// The verdict-vocabulary guardrail, enforced on this surface rather
/// than only on the source: a rendered external view never tells the
/// reader what to conclude.
#[test]
fn external_view_never_renders_a_verdict_word() {
    use swamp_core::evidence::{Evidence, FactKind, FactSubtype};
    let unit = external_unit(
        "/fixture/cache",
        2_048,
        1_000,
        vec![Evidence::known(
            FactKind::Activity,
            FactSubtype::Modified,
            swamp_core::evidence::FactValue::Timestamp(1_000),
            swamp_core::evidence::EvidenceSource::FilesystemMetadata {
                detail: "mtime".into(),
            },
            1_000,
        )],
    );
    let out = swamp_core::render::render_view_external(&[unit]).to_lowercase();
    for verdict in ["safe", "unused", "stale", "can be deleted", "junk"] {
        assert!(
            !out.contains(verdict),
            "the external view rendered the verdict word {verdict:?}:\n{out}"
        );
    }
}

/// Item 3 (2026-09-24 aim review repairs): a rendered evidence line must
/// never be a `{:?}` derive dump of `EvidenceSource`/`StorageCategory` --
/// the exact bug the review found, `[source: FilesystemMetadata {
/// detail: "…" }]` and `Inferred { basis: "…" }` on real output. Every
/// `EvidenceSource` variant is exercised so a new variant added without a
/// label arm fails this test (a `{:?}` fallback would pass compilation
/// silently otherwise).
#[test]
fn evidence_lines_never_render_a_debug_struct_literal() {
    use swamp_core::evidence::{Evidence, EvidenceSource, FactKind, FactSubtype, FactValue};
    let sources = vec![
        EvidenceSource::FilesystemMetadata {
            detail: "mtime".into(),
        },
        EvidenceSource::ToolReported {
            tool: "docker".into(),
            detail: "inspect".into(),
        },
        EvidenceSource::ProcessQuery {
            tool: "lsof".into(),
        },
        EvidenceSource::ManagerLock {
            tool: "cargo".into(),
            path: "/tmp/lock".into(),
        },
        EvidenceSource::ConfigDeclaration {
            path: "/tmp/.tool-versions".into(),
        },
        EvidenceSource::Lockfile {
            ecosystem: "npm".into(),
            path: "/tmp/package-lock.json".into(),
        },
        EvidenceSource::BuildMetadata {
            path: "/tmp/info.plist".into(),
        },
        EvidenceSource::DockerApi {
            detail: "image inspect".into(),
        },
        EvidenceSource::Statvfs,
        EvidenceSource::Inferred {
            basis: "encoded project directory name".into(),
        },
    ];
    let evidence: Vec<Evidence> = sources
        .into_iter()
        .map(|source| {
            Evidence::known(
                FactKind::Activity,
                FactSubtype::Modified,
                FactValue::Timestamp(1_000),
                source,
                1_000,
            )
        })
        .collect();
    let lines = swamp_core::render::render_evidence_lines(&evidence);
    assert_eq!(lines.len(), evidence.len(), "one line per fact");
    for line in &lines {
        // The unmistakable shape of a struct's `{:?}` output: a
        // capitalized-then-space-brace variant name, or a `field: "..."`
        // pair, neither of which any hand-written label produces.
        assert!(
            !line.contains("{ ") && !line.contains(": \""),
            "an evidence line looks like a Debug struct literal, not a label: {line:?}"
        );
    }
}
