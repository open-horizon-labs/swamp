//! R20 -- store = facts, report = view
//! (`.oh/guardrails/store-is-facts-report-is-views.md`).
//!
//! Three claims, adversarial rather than happy-path:
//!
//! 1. After `observe_scope`, the store holds no table for any derived
//!    `Report` field: no `summary.parquet`, `series.parquet`,
//!    `unowned_summary.parquet`, `worktree_entries.parquet`,
//!    `github_enrichment.parquet` or `artifact_shape` growth columns.
//!    The tempting shortcut this fails is "compute it at read time but
//!    also keep the old table around as a cache".
//! 2. `report_scope_from_store` still returns every one of those fields
//!    equal to what the pass produced -- `summary`, `reconciliation`,
//!    `series_*`, `unowned`, `dirs_by_worktree`/`files_by_worktree`,
//!    `schedule_line`, `github_enrichment`, and each artifact's
//!    `growth_bytes`/`allocated_*` -- derived from the fact tables and
//!    their history, at the observation's own timestamp.
//! 3. The facts are what a read reflects: tamper a fact (a walked root's
//!    reconciliation columns in `coverage.parquet`, a volume's Docker
//!    unowned rows, a top-level directory's tracking state) and the
//!    derived view moves with it; there is no cached copy to disagree.
//!
//! Disposable `tempfile` fixtures only.

use std::path::{Path, PathBuf};
use std::process::Command;
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::report::{self, ObservationParts};
use swamp_core::scope::{EffectiveScope, ScanConfig, resolve_effective_scope};

fn run_git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .output()
        .unwrap_or_else(|e| panic!("run git {args:?}: {e}"));
    assert!(out.status.success(), "git {args:?} failed");
}

/// Two Rust checkouts: real `by_type` summary rows, more than one
/// project's worth of reconciliation totals, a coverage row per root,
/// and a top-level ignored directory whose tracking state only the walk
/// can read.
fn make_fixture(src: &Path) {
    for name in ["app-a", "app-b"] {
        let dir = src.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        run_git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("Cargo.toml"), b"[package]\nname=\"a\"\n").unwrap();
        std::fs::write(dir.join(".gitignore"), b"target/\nscratch/\n").unwrap();
        run_git(&dir, &["add", "Cargo.toml", ".gitignore"]);
        run_git(&dir, &["commit", "-q", "-m", "init"]);
        std::fs::create_dir_all(dir.join("target/debug")).unwrap();
        std::fs::write(dir.join("target/debug/blob"), vec![b'r'; 8192]).unwrap();
        std::fs::create_dir_all(dir.join("scratch")).unwrap();
        std::fs::write(dir.join("scratch/notes"), vec![b's'; 4096]).unwrap();
    }
}

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    store: PathBuf,
    scope: EffectiveScope,
}

fn observe(fx: &Fx) -> report::ScopeObservation {
    report::observe_scope(
        &fx.scope,
        ObservationParts::ALL,
        None,
        None,
        false,
        Some(&fx.store),
        None,
        true,
        true,
        false,
        false,
        swamp_core::fs_events::platform_source().as_ref(),
        30,
        24 * 3600,
    )
    .expect("observe_scope")
}

fn build() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = swamp_core::fs_gate::canonicalize(tmp.path()).unwrap_or(tmp.path().to_path_buf());
    let src = root.join("src");
    make_fixture(&src);
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
    Fx {
        _tmp: tmp,
        root: src,
        store,
        scope,
    }
}

/// Every table name that ever existed for a derived field. None may
/// come back.
const DERIVED_TABLES: &[&str] = &[
    "summary.parquet",
    "series.parquet",
    "unowned_summary.parquet",
    "unowned_summary_lists.parquet",
    "worktree_entries.parquet",
    "github_enrichment.parquet",
    "report_rows.parquet",
];

#[test]
fn observe_stores_no_derived_view_and_report_derives_every_one_of_them() {
    let fx = build();
    let observation = observe(&fx);
    assert!(
        observation.merged.projects.len() >= 2,
        "fixture must discover both checkouts"
    );
    assert!(
        !observation.merged.summary.by_type.is_empty(),
        "fixture must produce a non-empty by_type summary"
    );
    assert!(
        observation.merged.reconciliation.walked_total > 0,
        "fixture must produce a non-zero walked_total"
    );
    assert!(
        observation.merged.dirs_by_worktree.is_some(),
        "include_dirs=true must populate the drill-down on the live observation"
    );
    let tracked_dirs = observation
        .merged
        .dirs_by_worktree
        .as_ref()
        .unwrap()
        .values()
        .flatten()
        .filter(|d| d.track.is_some())
        .count();
    assert!(
        tracked_dirs > 0,
        "the walk must have read a tracking state for at least one top-level directory"
    );

    // 1. No derived table anywhere under the store.
    let mut found = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    walk(&fx.store, &mut found);
    for p in &found {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            !DERIVED_TABLES.contains(&name.as_str()),
            "{} is a table for a derived Report field; R20 deleted it -- a read computes it",
            p.display()
        );
    }
    for fact in ["coverage.parquet", "notes.parquet", "runs.parquet"] {
        assert!(fx.store.join(fact).is_file(), "{fact} must exist");
    }

    // 2. Every derived field comes back equal, from facts.
    let rebuilt = report::report_scope_from_store(&fx.scope, &fx.store).expect("stored");
    let observed = serde_json::to_value(&observation.merged).unwrap();
    let derived = serde_json::to_value(&rebuilt.report).unwrap();
    for field in [
        "summary",
        "reconciliation",
        "series_by_key",
        "total_series",
        "series_window_secs",
        "unowned",
        "dirs_by_worktree",
        "files_by_worktree",
        "schedule_line",
        "github_enrichment",
        "notes",
        "projects",
    ] {
        assert_eq!(
            observed.get(field),
            derived.get(field),
            "Report.{field} must derive from the fact tables to exactly what the pass produced"
        );
    }
    assert_eq!(observed, derived, "the whole Report, field for field");
}

#[test]
fn the_derived_views_follow_the_facts_they_are_computed_from() {
    let fx = build();
    let observation = observe(&fx);
    let key = report::scope_snapshot_key(&fx.scope);

    // A walked root's own totals are facts on its coverage row: move one
    // and `Report.reconciliation` (their sum) moves with it.
    let mut by_root = std::collections::HashMap::new();
    for (root, r) in &observation.per_root {
        let mut rec = r.reconciliation.clone();
        rec.attributed += 1;
        by_root.insert(root.clone(), rec);
    }
    swamp_core::growth::write_coverage_table(
        &fx.store,
        &key,
        &observation.coverage,
        &observation.unit_root_coverage,
        &by_root,
        observation.merged.observed_at,
    )
    .unwrap();
    let rebuilt = report::report_scope_from_store(&fx.scope, &fx.store).expect("stored");
    assert_eq!(
        rebuilt.report.reconciliation.attributed,
        observation.merged.reconciliation.attributed + observation.per_root.len() as u64,
        "reconciliation is the sum of the coverage rows' totals, read live"
    );

    // A volume's Docker unowned rows are facts the gate joined after the
    // walk's checkpoint: write one and it appears in `Report.unowned`,
    // after that root's walk rows.
    let root = observation
        .coverage
        .iter()
        .find(|c| c.status.was_observed())
        .map(|c| c.path.clone())
        .expect("a walked root");
    let vol = swamp_core::growth::volume_store_dir(&fx.store, &root);
    let docker = swamp_core::report::UnownedRow {
        measurement: None,
        path_or_object: "sha256:tampered".into(),
        bytes: 4096,
        reason: swamp_core::report::UnownedReason::DockerNoJoin,
        shared_bytes: Some(1),
        note: None,
        docker_kind: Some("image".into()),
        created_at: None,
        containers: vec!["web".into()],
        shared_with: Vec::new(),
        dangling: false,
        evidence: Vec::new(),
    };
    swamp_core::growth::write_docker_unowned(&vol, std::slice::from_ref(&docker)).unwrap();
    let rebuilt = report::report_scope_from_store(&fx.scope, &fx.store).expect("stored");
    let got = rebuilt
        .report
        .unowned
        .iter()
        .find(|u| u.path_or_object == "sha256:tampered")
        .expect("the Docker row must be in Report.unowned");
    assert_eq!(got.containers, vec!["web".to_string()]);
    assert_eq!(
        rebuilt.report.unowned.len(),
        observation.merged.unowned.len() + 1
    );

    // A top-level directory's tracking state is a fact of the walk
    // (`.gitignore` is read then, never at report time): flip it in
    // `dir_tracks.parquet` and the drill-down row shows the new state.
    let dirs = observation.merged.dirs_by_worktree.as_ref().unwrap();
    let mut flipped = dirs.clone();
    let mut flips = 0;
    for rows in flipped.values_mut() {
        for d in rows.iter_mut() {
            if d.track.is_some() {
                d.track = Some(swamp_core::ignore::TrackState::Untracked);
                flips += 1;
            }
        }
    }
    assert!(flips > 0);
    swamp_core::growth::write_dir_tracks(&vol, &flipped).unwrap();
    let rebuilt = report::report_scope_from_store(&fx.scope, &fx.store).expect("stored");
    let rows = rebuilt.report.dirs_by_worktree.as_ref().unwrap();
    let held: std::collections::HashSet<String> =
        swamp_core::growth::worktree_ids_held_by_volume(&vol);
    let untracked = rows
        .iter()
        .filter(|(wt, _)| held.contains(*wt))
        .flat_map(|(_, rows)| rows)
        .filter(|d| d.track == Some(swamp_core::ignore::TrackState::Untracked))
        .count();
    assert!(
        untracked > 0,
        "the drill-down must read tracking state from dir_tracks.parquet"
    );
    let _ = fx.root;
}
