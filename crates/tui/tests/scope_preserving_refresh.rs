//! The scope contract has to survive every update, not just the first
//! render (`.oh/guardrails/tui-refresh-preserves-scope.md`).
//!
//! The 2026-09-21 review found the TUI's startup using the scope-aware
//! report path while its background and live refreshes went through
//! single-root functions that default `pruned_subtrees` to empty -- so a
//! refresh could reintroduce excluded data and re-count external bytes
//! that startup had pruned. Separately, only startup ever set the
//! external/agent unit vectors, and `prune_removed` left their
//! successful action rows in place.
//!
//! Disposable `tempfile` fixtures throughout; nothing reads a real home
//! or the user's own swamp state.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::scope::{EffectiveScope, ScanConfig, resolve_effective_scope};
use swamp_tui::app::App;

/// A directory tree with measurable bytes. Deliberately not a real git
/// checkout: this file is about *scope*, and a tree that produces only
/// unowned rows exercises it just as well while keeping the fixture free
/// of `git` subprocesses.
fn git_project(root: &Path, name: &str) -> PathBuf {
    let p = root.join(name);
    fs::create_dir_all(p.join("target/debug")).unwrap();
    fs::write(p.join("target/debug/blob.bin"), vec![b'x'; 4096]).unwrap();
    fs::write(p.join("Cargo.toml"), b"[package]\nname = \"x\"\n").unwrap();
    // Reports carry canonical paths (macOS `/var` is a symlink to
    // `/private/var`), so the fixture hands back the spelling the report
    // will use.
    fs::canonicalize(&p).unwrap_or(p)
}

/// A scope over one included root, with `exclude` applied. Explicit-only
/// so no detector on the developer's real machine can leak in.
fn scope_for(home: &Path, include: &[&Path], exclude: &[&Path]) -> EffectiveScope {
    let registry = Registry::with_builtins();
    let cfg = ScanConfig {
        defaults: false,
        include: include.iter().map(|p| p.display().to_string()).collect(),
        exclude: exclude.iter().map(|p| p.display().to_string()).collect(),
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .collect(),
        ..Default::default()
    };
    let env = Environment::fixture(home.to_path_buf(), HashMap::new(), Platform::MacOS);
    resolve_effective_scope(&env, &cfg, &[], &registry, 1_000)
}

fn observe(scope: &EffectiveScope, store: &Path) -> swamp_core::report::ScopeObservation {
    swamp_core::report::observe_scope(
        scope,
        swamp_core::report::ObservationParts::ALL,
        None,
        None,
        false,
        Some(store),
        None,
        true,
        true,
        false,
        false,
        swamp_core::fs_events::platform_source().as_ref(),
        30,
        24 * 3600,
    )
    .expect("scope observation")
}

/// Every path this report mentions at all: attributed artifact rows and
/// unowned rows alike. The scope contract is about what swamp *looked
/// at*, so a bare directory tree that produces only unowned rows is as
/// much a violation as a missed project.
fn all_paths(report: &swamp_core::Report) -> Vec<String> {
    report
        .projects
        .iter()
        .flat_map(|p| p.worktrees.iter())
        .flat_map(|wt| wt.artifacts.iter())
        .map(|a| a.path.display().to_string())
        .chain(report.unowned.iter().map(|u| u.path_or_object.clone()))
        .collect()
}

#[test]
fn an_excluded_subtree_stays_absent_after_a_background_refresh() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();
    // Canonical throughout: an exclusion resolved in one spelling and a
    // root in another would not overlap, and the test would pass for the
    // wrong reason.
    let src = fs::canonicalize(&src).unwrap();
    let kept = git_project(&src, "kept");
    let excluded = git_project(&src, "scratch");
    let store = tempfile::tempdir().unwrap();

    let scope = scope_for(tmp.path(), &[&src], &[&excluded]);
    let first = observe(&scope, store.path());
    assert!(
        all_paths(&first.merged)
            .iter()
            .any(|p| p.starts_with(&kept.display().to_string())),
        "precondition: the included project must be observed: {:?}",
        all_paths(&first.merged)
    );
    assert!(
        !all_paths(&first.merged)
            .iter()
            .any(|p| p.starts_with(&excluded.display().to_string())),
        "precondition: the excluded subtree must be absent at startup"
    );

    let mut app = App::new_multi_root(first.merged, scope.scan_paths());
    app.reports_by_root = first.per_root;
    app.store_dir = Some(store.path().to_path_buf());
    app.scope = Some(scope.clone());

    // The refresh path the review found dropping the scope.
    app.observe_in_background();
    let rx = app.pending.take().expect("a refresh was started");
    let fresh = rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the refresh worker answered")
        .expect("the refresh succeeded");
    // Exactly what the event loop does with a finished observation.
    app.install_refreshed(fresh);

    let after = all_paths(&app.report);
    assert!(
        !after
            .iter()
            .any(|p| p.starts_with(&excluded.display().to_string())),
        "an excluded subtree reappeared on refresh: {after:?}"
    );
    assert!(
        after
            .iter()
            .any(|p| p.starts_with(&kept.display().to_string())),
        "the refresh must still show what is in scope: {after:?}"
    );
}

#[test]
fn a_refresh_without_a_scope_is_refused_rather_than_widened() {
    // The failure mode that made the original bug possible: a refresh
    // path that had no scope simply observed everything. Now it declines
    // and says so.
    let tmp = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();
    // Canonical throughout: an exclusion resolved in one spelling and a
    // root in another would not overlap, and the test would pass for the
    // wrong reason.
    let src = fs::canonicalize(&src).unwrap();
    git_project(&src, "p");
    let scope = scope_for(tmp.path(), &[&src], &[]);
    let first = observe(&scope, store.path());

    let mut app = App::new_multi_root(first.merged, scope.scan_paths());
    app.store_dir = Some(store.path().to_path_buf());
    app.scope = None;
    app.observe_in_background();
    assert!(
        app.pending.is_none(),
        "no scope must mean no observation, not an unscoped one"
    );
    assert!(
        app.status.as_deref().unwrap_or_default().contains("scope"),
        "and it must say why: {:?}",
        app.status
    );
}

#[test]
fn external_and_agent_units_are_replaced_by_every_refresh() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();
    // Canonical throughout: an exclusion resolved in one spelling and a
    // root in another would not overlap, and the test would pass for the
    // wrong reason.
    let src = fs::canonicalize(&src).unwrap();
    git_project(&src, "p");
    let store = tempfile::tempdir().unwrap();
    let scope = scope_for(tmp.path(), &[&src], &[]);
    let first = observe(&scope, store.path());

    let mut app = App::new_multi_root(first.merged, scope.scan_paths());
    app.reports_by_root = first.per_root;
    app.store_dir = Some(store.path().to_path_buf());
    app.scope = Some(scope.clone());
    // Seed a stale unit that no observation would ever produce; a
    // refresh must replace the vector, not leave it.
    app.set_agent_units(vec![]);
    app.set_external_units(vec![]);

    app.observe_in_background();
    let rx = app.pending.take().expect("a refresh was started");
    let fresh = rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the refresh worker answered")
        .expect("the refresh succeeded");
    assert!(
        fresh.external_units.is_some() && fresh.agent_units.is_some(),
        "a refresh carries the external and agent units from its own pass, so the agents view \
         cannot be older than the header"
    );
}

#[test]
fn prune_removed_drops_exactly_the_successful_agent_and_external_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();
    // Canonical throughout: an exclusion resolved in one spelling and a
    // root in another would not overlap, and the test would pass for the
    // wrong reason.
    let src = fs::canonicalize(&src).unwrap();
    git_project(&src, "p");
    let store = tempfile::tempdir().unwrap();
    let scope = scope_for(tmp.path(), &[&src], &[]);
    let first = observe(&scope, store.path());
    let mut app = App::new_multi_root(first.merged, scope.scan_paths());

    let removed = src.join("gone-cache");
    let kept = src.join("kept-cache");
    app.set_external_units(vec![external_unit(&removed), external_unit(&kept)]);

    app.prune_removed(&[
        swamp_tui::actions::UnitResult {
            path: removed.clone(),
            outcome: Ok(swamp_core::execution::Outcome {
                unit_id: "fixture-removed".into(),
                status: "completed".into(),
                reason: None,
                intended_bytes: 10,
                observed_free_space_delta: None,
            }),
        },
        swamp_tui::actions::UnitResult {
            path: kept.clone(),
            // A refused unit keeps its row: the storage is still there.
            outcome: Err("refused for review".to_string()),
        },
    ]);

    let left: Vec<String> = app
        .external_units
        .iter()
        .map(|u| u.path.display().to_string())
        .collect();
    assert!(
        !left.contains(&removed.display().to_string()),
        "a successfully removed external row must leave the screen: {left:?}"
    );
    assert!(
        left.contains(&kept.display().to_string()),
        "a refused row must stay: {left:?}"
    );
}

fn external_unit(path: &Path) -> swamp_core::external::ExternalUnit {
    swamp_core::external::ExternalUnit {
        detector_id: "fixture".into(),
        detector_name: "Fixture".into(),
        category: swamp_core::locations::StorageCategory::Cache,
        provenance: swamp_core::locations::Provenance::BuiltinConvention,
        path: path.to_path_buf(),
        bytes: 10,
        mtime_max: 0,
        hardlinked: false,
        growth_bytes: None,
        regrowth_count: 0,
        observed_at: 1_000,
        consumers: Vec::new(),
        note: None,
        evidence: Vec::new(),
    }
}

/// The TUI half of the integration owner's 2026-09-21 mutation-check
/// finding: an ordinary filesystem row that *contains* a human-protected
/// descendant must be refused when the human marks it, with the reason,
/// not silently at execution.
///
/// One-directional protection (`candidate.starts_with(p)` only) survived
/// every test at the time, because no test marked an ordinary row with a
/// protected file inside it. This is that case on the TUI path;
/// `crates/core/tests/evidence_action_recheck.rs::ordinary_unit_containing_protected_descendant_is_refused_at_proposal`
/// is the same case on the CLI/core path.
#[test]
fn marking_an_ordinary_row_that_contains_a_protected_file_is_refused_with_the_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let project = git_project(&home, "proj");
    // The file the human asked to keep, inside a measurable directory.
    let keep = project.join("target/debug/keep-this.txt");
    fs::write(&keep, b"work I asked you to keep").unwrap();

    let store = tempfile::tempdir().unwrap();
    swamp_core::agents::protect_add(store.path(), &keep).unwrap();

    let scope = scope_for(&home, &[&project], &[]);
    let observation = observe(&scope, store.path());
    let mut app = App::new(observation.merged, project.clone());
    app.store_dir = Some(store.path().to_path_buf());
    app.scope = Some(scope);
    // A fixture with no git checkout produces unowned rows, so that is
    // the view holding the markable row here. `mark_row`'s ordinary
    // branch is the same code in every view.
    app.set_view(swamp_tui::app::ViewKind::Unowned);

    // Open every level until a markable row naming a directory that
    // contains the protected file appears.
    let contains_keep = |row: &swamp_tui::model::Row| {
        row.unit.as_ref().is_some_and(|u| {
            let p = PathBuf::from(&u.0);
            keep.starts_with(&p) && p != keep
        })
    };
    for _ in 0..6 {
        if app.rows().iter().any(contains_keep) {
            break;
        }
        for i in 0..app.rows().len() {
            app.selected = i;
            app.toggle_expand();
        }
    }
    let row = app
        .rows()
        .into_iter()
        .find(|r| contains_keep(r))
        .unwrap_or_else(|| {
            panic!(
                "precondition: some markable row must contain {}; rows were {:?}",
                keep.display(),
                app.rows()
                    .iter()
                    .map(|r| (r.label.clone(), r.unit.clone()))
                    .collect::<Vec<_>>()
            )
        });
    app.mark_row(&row);

    assert!(
        app.marked.is_empty(),
        "a row containing a human-protected file must never be marked"
    );
    let refusal = app.refusal_active().unwrap_or_default();
    assert!(
        refusal.contains("human-protected"),
        "the refusal must name the cause, not be a generic message: {refusal:?}"
    );
    assert!(keep.exists(), "nothing may be touched at mark time");
}
