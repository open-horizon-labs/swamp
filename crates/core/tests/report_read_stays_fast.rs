//! R20 -- deriving a `Report`'s views at read time costs less than the
//! Parquet files that used to cache them
//! (`.oh/guardrails/store-is-facts-report-is-views.md`).
//!
//! A synthetic store an order of magnitude larger than a busy
//! developer's (3,000 artifact rows across 300 projects, a walked root,
//! notes, a run row) is read back through `report_scope_from_store`,
//! which now folds the summary, sums the reconciliation, buckets the
//! history and groups the drill-down on every call. The budget is wall
//! time on the second (warm-cache) read: 1 s where the machine is
//! quiet (`SWAMP_PERF_STRICT=1`, the owner's check), 3 s under CI
//! noise. The first read's time is printed for the record.
//!
//! Disposable `tempfile` fixtures only.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use swamp_core::coverage::{RegionStatus, RootCoverage};
use swamp_core::entities::Confidence;
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::report::{
    self, ArtifactKind, ArtifactRow, ProjectRow, Reconciliation, Source, WorktreeKind, WorktreeRow,
};
use swamp_core::scope::{ScanConfig, resolve_effective_scope};

const PROJECTS: usize = 300;
const WORKTREES_PER_PROJECT: usize = 2;
const ARTIFACTS_PER_WORKTREE: usize = 5;

fn artifact(wt: &std::path::Path, i: usize) -> ArtifactRow {
    ArtifactRow {
        kind: if i.is_multiple_of(2) {
            ArtifactKind::BuildOutput
        } else {
            ArtifactKind::DependencyTree
        },
        path: wt.join(format!("out-{i}")),
        bytes: 1 << 20,
        mtime_max: 1_700_000_000,
        ecosystem: Some(if i.is_multiple_of(2) { "rs" } else { "js" }.to_string()),
        hardlinked: false,
        dedup_stale: false,
        allocated_bytes: None,
        allocated_growth_bytes: None,
        local_bytes: 1 << 20,
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
    }
}

#[test]
fn a_large_store_reads_back_within_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let root = swamp_core::fs_gate::canonicalize(tmp.path()).unwrap_or(tmp.path().to_path_buf());
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let store = root.join("store");
    std::fs::create_dir_all(&store).unwrap();

    let env = Environment::fixture(root.clone(), Default::default(), Platform::MacOS);
    let registry = Registry::with_builtins();
    let cfg = ScanConfig {
        defaults: false,
        include: vec![src.display().to_string()],
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .collect(),
        ..Default::default()
    };
    let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1_000);
    let key = report::scope_snapshot_key(&scope);
    let observed_at = 1_700_000_100u64;

    let mut projects = Vec::with_capacity(PROJECTS);
    for p in 0..PROJECTS {
        let mut worktrees = Vec::with_capacity(WORKTREES_PER_PROJECT);
        for w in 0..WORKTREES_PER_PROJECT {
            let path: PathBuf = src.join(format!("p{p}-w{w}"));
            worktrees.push(WorktreeRow {
                worktree_id: format!("wt-{p}-{w}"),
                path: path.clone(),
                kind: if w == 0 {
                    WorktreeKind::Main
                } else {
                    WorktreeKind::Linked
                },
                artifacts: (0..ARTIFACTS_PER_WORKTREE)
                    .map(|i| artifact(&path, i))
                    .collect(),
                signals: Vec::new(),
                branch: Some("main".into()),
                github: None,
                merge_complete: None,
                idle_secs: None,
            });
        }
        projects.push(ProjectRow {
            project_id: format!("project-{p}"),
            name: format!("project-{p}"),
            worktrees,
            ecosystems: vec!["rs".into(), "js".into()],
            remote: None,
        });
    }

    swamp_core::growth::write_project_worktree_tables(&store, &key, &projects, observed_at)
        .unwrap();
    swamp_core::growth::write_artifact_shape_table(&store, &key, &projects, observed_at).unwrap();
    let coverage = vec![RootCoverage {
        path: src.clone(),
        status: RegionStatus::Complete,
        walked_total: 3_000 << 20,
        projects: PROJECTS,
        mode: "full".into(),
    }];
    let reconciliation = HashMap::from([(
        src.clone(),
        Reconciliation {
            unique_estimate: None,
            attributed: 3_000 << 20,
            unowned: 0,
            walked_total: 3_000 << 20,
            du_total: None,
            docker_attributed: 0,
            docker_unowned: 0,
        },
    )]);
    swamp_core::growth::write_coverage_table(
        &store,
        &key,
        &coverage,
        &[],
        &reconciliation,
        observed_at,
    )
    .unwrap();
    swamp_core::growth::write_notes_table(
        &store,
        &key,
        &["fsevents: mode=full reason=no_store changed_dirs=0".to_string()],
        observed_at,
    )
    .unwrap();
    swamp_core::growth::write_run_row(
        &store,
        &key,
        &swamp_core::growth::RunFacts {
            unique_estimate: None,
            observed_at,
            since_secs: 86_400,
            retention_days: 30,
            include_dirs: true,
            github_enrichment: None,
            schedule_line: None,
        },
    )
    .unwrap();

    let first = Instant::now();
    let snapshot = report::report_scope_from_store(&scope, &store).expect("stored");
    let first = first.elapsed();
    assert_eq!(snapshot.report.projects.len(), PROJECTS);
    assert_eq!(
        snapshot.report.summary.artifacts,
        PROJECTS * WORKTREES_PER_PROJECT * ARTIFACTS_PER_WORKTREE
    );
    assert_eq!(snapshot.report.reconciliation.attributed, 3_000 << 20);

    let warm = Instant::now();
    let _ = report::report_scope_from_store(&scope, &store).expect("stored");
    let warm = warm.elapsed();
    eprintln!("report_scope_from_store: first {first:?}, warm {warm:?}");

    let budget = if std::env::var_os("SWAMP_PERF_STRICT").is_some() {
        Duration::from_secs(1)
    } else {
        Duration::from_secs(3)
    };
    assert!(
        warm <= budget,
        "reading a {}-artifact store took {warm:?}, over the {budget:?} budget: a derived view \
         is being recomputed expensively, or something on the read path walks",
        PROJECTS * WORKTREES_PER_PROJECT * ARTIFACTS_PER_WORKTREE
    );
}
