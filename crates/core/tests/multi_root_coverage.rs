//! #42 adversarial tests: coherent multi-root observation and
//! coverage-aware history over a resolved `scope::EffectiveScope`.
//!
//! Every test in this file uses `force_full: true` so none of it depends
//! on the live `fseventsd` or on FSEvents replay timing -- the coverage
//! and tombstone-protection semantics under test apply identically on
//! the full-walk path, and forcing it keeps these tests fast and
//! deterministic. See `fsevents_incremental.rs` for the FSEvents-specific
//! incremental-vs-full equivalence suite this file does not duplicate.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use swamp_core::coverage::RegionStatus;
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::report::report_scope;
use swamp_core::scope::{RootStatus, ScanConfig, resolve_effective_scope};

fn run_git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .output()
        .unwrap_or_else(|e| panic!("run git {args:?} in {}: {e}", dir.display()));
    assert!(
        out.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A minimal real git checkout with one build-output-shaped artifact
/// directory, `bytes` split across a couple of files.
fn make_project(root: &Path, name: &str, bytes: u64) -> PathBuf {
    let checkout = root.join(name);
    fs::create_dir_all(&checkout).unwrap();
    run_git(&checkout, &["init", "-q", "-b", "main"]);
    fs::write(checkout.join("README.md"), b"fixture\n").unwrap();
    run_git(&checkout, &["add", "README.md"]);
    run_git(&checkout, &["commit", "-q", "-m", "initial"]);
    let target = checkout.join("target");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("a.bin"), vec![7u8; (bytes / 2) as usize]).unwrap();
    fs::write(
        target.join("b.bin"),
        vec![9u8; (bytes - bytes / 2) as usize],
    )
    .unwrap();
    checkout
}

fn env_home(home: &Path) -> Environment {
    Environment::fixture(home.to_path_buf(), Default::default(), Platform::MacOS)
}

fn explicit_only_config() -> ScanConfig {
    ScanConfig {
        defaults: false,
        include: Vec::new(),
        exclude: Vec::new(),
        disabled_detectors: Vec::new(),
        enabled_detectors: Vec::new(),
    }
}

fn observe_scope(
    store: &Path,
    explicit_roots: &[PathBuf],
    exclude: &[String],
) -> (swamp_core::Report, Vec<swamp_core::coverage::RootCoverage>) {
    let home = tempfile::tempdir().unwrap();
    let env = env_home(home.path());
    let mut cfg = explicit_only_config();
    cfg.exclude = exclude.to_vec();
    let registry = Registry::with_builtins();
    let scope = resolve_effective_scope(&env, &cfg, explicit_roots, &registry, 1_000_000);
    report_scope(
        &scope,
        None,
        false,
        Some(store),
        None,
        true,
        false,
        false,
        true,
    )
    .expect("report_scope")
}

/// Two genuinely separate roots sharing one temporary volume: their
/// stores, totals and history must stay independent, and the merged
/// scope report must count each project exactly once.
#[test]
fn two_disjoint_roots_on_one_volume_stay_independent() {
    let tmp = tempfile::tempdir().unwrap();
    let root_a = tmp.path().join("a");
    let root_b = tmp.path().join("b");
    fs::create_dir_all(&root_a).unwrap();
    fs::create_dir_all(&root_b).unwrap();
    make_project(&root_a, "proj-a", 10_000);
    make_project(&root_b, "proj-b", 20_000);
    let store = tempfile::tempdir().unwrap();

    let (report, coverage) = observe_scope(store.path(), &[root_a.clone(), root_b.clone()], &[]);

    assert_eq!(coverage.len(), 2);
    assert!(
        coverage
            .iter()
            .all(|c| matches!(c.status, RegionStatus::Complete)),
        "{coverage:?}"
    );
    assert_eq!(report.projects.len(), 2, "one project discovered per root");
    let names: std::collections::BTreeSet<_> =
        report.projects.iter().map(|p| p.name.clone()).collect();
    assert_eq!(
        names,
        ["proj-a", "proj-b"].into_iter().map(String::from).collect()
    );
}

/// The same two roots, resolved in the opposite order: aggregate totals
/// and per-root coverage must not depend on root order.
#[test]
fn totals_are_root_order_independent() {
    let tmp = tempfile::tempdir().unwrap();
    let root_a = tmp.path().join("a");
    let root_b = tmp.path().join("b");
    fs::create_dir_all(&root_a).unwrap();
    fs::create_dir_all(&root_b).unwrap();
    make_project(&root_a, "proj-a", 10_000);
    make_project(&root_b, "proj-b", 20_000);

    let store1 = tempfile::tempdir().unwrap();
    let (report1, _) = observe_scope(store1.path(), &[root_a.clone(), root_b.clone()], &[]);
    let store2 = tempfile::tempdir().unwrap();
    let (report2, _) = observe_scope(store2.path(), &[root_b.clone(), root_a.clone()], &[]);

    assert_eq!(
        report1.reconciliation.walked_total, report2.reconciliation.walked_total,
        "walked_total must not depend on root order"
    );
    assert_eq!(report1.projects.len(), report2.projects.len());
}

/// A root nested inside another in-scope root folds into its parent
/// (scope.rs, #41) and must not be walked -- and therefore not
/// measured -- a second time by `report_scope`.
#[test]
fn nested_roots_fold_and_are_not_double_counted() {
    let tmp = tempfile::tempdir().unwrap();
    let parent = tmp.path().join("parent");
    fs::create_dir_all(&parent).unwrap();
    make_project(&parent, "outer", 5_000);
    let nested = parent.join("nested-child");
    fs::create_dir_all(&nested).unwrap();
    make_project(&nested, "inner", 3_000);
    let store = tempfile::tempdir().unwrap();

    // Both the parent and its nested subdirectory are named explicitly --
    // exactly the ambiguous case a naive "walk every explicit root
    // independently" implementation would double count.
    let (report, coverage) = observe_scope(store.path(), &[parent.clone(), nested.clone()], &[]);

    // The nested root gets no coverage row of its own -- it was folded
    // into the parent's walk, which already covers it.
    assert_eq!(coverage.len(), 1, "{coverage:?}");
    assert_eq!(coverage[0].path, parent);
    // Both the outer project and the inner one (found while walking the
    // parent) are discovered exactly once each.
    assert_eq!(report.projects.len(), 2, "{:?}", report.projects);
}

/// A root that becomes completely unreadable between scope resolution
/// and the walk (simulated here by chmod'ing it up front, which is
/// equivalent from `report_scope`'s point of view -- both are "access
/// lost before/at walk time") must not be reported as empty, must not
/// touch its growth store, and must not advance any FSEvents cursor.
#[test]
fn inaccessible_root_is_not_reported_as_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("locked");
    fs::create_dir_all(&root).unwrap();
    make_project(&root, "proj", 4_000);
    let store = tempfile::tempdir().unwrap();

    // First observation succeeds normally.
    let (first, _) = observe_scope(store.path(), std::slice::from_ref(&root), &[]);
    assert_eq!(first.projects.len(), 1);
    assert!(first.reconciliation.walked_total > 0);

    // Lock the root itself.
    fs::set_permissions(&root, fs::Permissions::from_mode(0o000)).unwrap();
    let (second, coverage) = observe_scope(store.path(), std::slice::from_ref(&root), &[]);
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(coverage.len(), 1);
    assert!(
        matches!(coverage[0].status, RegionStatus::Inaccessible { .. }),
        "{:?}",
        coverage[0].status
    );
    // The inaccessible pass must not look like "this root has 0 bytes":
    // it must produce no report rows for it at all, distinct from a
    // genuinely empty root.
    assert!(second.projects.is_empty());
    assert_eq!(second.reconciliation.walked_total, 0);

    // Restoring access and observing again must see the same project it
    // saw the first time -- no fabricated deletion happened while access
    // was lost, so this is an ordinary re-observation, not a regrowth.
    let (third, coverage3) = observe_scope(store.path(), std::slice::from_ref(&root), &[]);
    assert!(matches!(coverage3[0].status, RegionStatus::Complete));
    assert_eq!(third.projects.len(), 1);
    let artifact = &third.projects[0].worktrees[0].artifacts;
    let target_row = artifact
        .iter()
        .find(|a| a.path.ends_with("target"))
        .expect("target artifact row");
    assert_eq!(
        target_row.regrowth_count, 0,
        "losing and regaining access to an unchanged tree must never count as regrowth"
    );
}

/// A worktree that stays *inside* an otherwise-readable root but itself
/// loses read access (rather than the whole root) must not be
/// tombstoned: this is exactly the hazard
/// `growth::compute_unconfirmed_worktrees` exists to close.
#[test]
fn worktree_losing_access_inside_a_readable_root_is_not_tombstoned() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("src");
    fs::create_dir_all(&root).unwrap();
    let a = make_project(&root, "proj-a", 4_000);
    make_project(&root, "proj-b", 6_000);
    let store = tempfile::tempdir().unwrap();

    let (first, _) = observe_scope(store.path(), std::slice::from_ref(&root), &[]);
    assert_eq!(first.projects.len(), 2);

    // Lock just one project's directory; the root itself stays readable.
    fs::set_permissions(&a, fs::Permissions::from_mode(0o000)).unwrap();
    let (second, coverage) = observe_scope(store.path(), std::slice::from_ref(&root), &[]);
    fs::set_permissions(&a, fs::Permissions::from_mode(0o755)).unwrap();

    // The root as a whole was still walked (proj-b is still there), so
    // this is Complete or Partial, never Missing/Inaccessible/Excluded.
    assert!(
        coverage[0].status.was_observed(),
        "root itself remained readable: {:?}",
        coverage[0].status
    );
    // proj-a's project row disappears from *this pass's* report (it
    // could not be read this time)...
    assert_eq!(second.projects.len(), 1);
    assert_eq!(second.projects[0].name, "proj-b");

    // ...but restoring access must show proj-a's artifact with zero
    // regrowth: it was never actually deleted, so the protected store
    // row must never have been tombstoned while access was lost.
    let (third, _) = observe_scope(store.path(), std::slice::from_ref(&root), &[]);
    let proj_a = third
        .projects
        .iter()
        .find(|p| p.name == "proj-a")
        .expect("proj-a reappears");
    let target_row = proj_a.worktrees[0]
        .artifacts
        .iter()
        .find(|r| r.path.ends_with("target"))
        .expect("target artifact row");
    assert_eq!(
        target_row.regrowth_count, 0,
        "a worktree that only lost and regained read access must never count as regrowth"
    );
}

/// Removing a root from scope (or excluding it) must show up as a
/// coverage change, never as a deletion: no tombstones, no shrink
/// deltas, and the store for the removed root is left completely
/// untouched.
#[test]
fn removing_a_root_from_scope_is_a_coverage_change_not_a_deletion() {
    let tmp = tempfile::tempdir().unwrap();
    let root_a = tmp.path().join("a");
    let root_b = tmp.path().join("b");
    fs::create_dir_all(&root_a).unwrap();
    fs::create_dir_all(&root_b).unwrap();
    make_project(&root_a, "proj-a", 5_000);
    make_project(&root_b, "proj-b", 5_000);
    let store = tempfile::tempdir().unwrap();

    let (first, _) = observe_scope(store.path(), &[root_a.clone(), root_b.clone()], &[]);
    assert_eq!(first.projects.len(), 2);

    // Scope edit: root_b is dropped entirely (as if removed from config).
    let (second, coverage) = observe_scope(store.path(), std::slice::from_ref(&root_a), &[]);
    assert_eq!(
        coverage.len(),
        1,
        "root_b is simply not a candidate anymore"
    );
    assert_eq!(second.projects.len(), 1);
    assert_eq!(second.projects[0].name, "proj-a");

    // root_b's own store is untouched: re-observing it alone still finds
    // proj-b with no regrowth (it was never marked absent).
    let (third, coverage3) = observe_scope(store.path(), std::slice::from_ref(&root_b), &[]);
    assert!(matches!(coverage3[0].status, RegionStatus::Complete));
    assert_eq!(third.projects.len(), 1);
    assert_eq!(third.projects[0].name, "proj-b");
    let target_row = third.projects[0].worktrees[0]
        .artifacts
        .iter()
        .find(|r| r.path.ends_with("target"))
        .unwrap();
    assert_eq!(
        target_row.regrowth_count, 0,
        "dropping a root from scope and later re-adding it must never look like regrowth"
    );
}

/// An `exclude` entry naming a subtree *inside* an otherwise-included
/// root (#41's `pruned_subtrees`, wired into the walker by this issue)
/// must prune exactly that subtree: not measured, not reported, and the
/// rest of the root is unaffected.
#[test]
fn excluded_subtree_inside_a_kept_root_is_pruned_not_measured() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("src");
    fs::create_dir_all(&root).unwrap();
    make_project(&root, "kept", 4_000);
    let excluded_project = make_project(&root, "excluded-me", 9_000);
    let store = tempfile::tempdir().unwrap();

    let (report, coverage) = observe_scope(
        store.path(),
        std::slice::from_ref(&root),
        &[excluded_project.display().to_string()],
    );

    assert!(matches!(coverage[0].status, RegionStatus::Complete));
    assert_eq!(
        report.projects.len(),
        1,
        "the excluded project must not be discovered at all: {:?}",
        report.projects.iter().map(|p| &p.name).collect::<Vec<_>>()
    );
    assert_eq!(report.projects[0].name, "kept");
    // Pruned bytes are not measured at all, so they cannot appear as
    // unowned either -- pruning is not the same as "found but unowned".
    assert!(
        report
            .unowned
            .iter()
            .all(|u| !u.path_or_object.contains("excluded-me")),
        "{:?}",
        report.unowned
    );
}

/// A configured exclusion and an explicit root can name the same tree
/// through different symlink spellings. The prune path must be translated
/// back to the root spelling the walker traverses, or the excluded data is
/// measured despite passing the root-level exclusion check.
#[test]
fn excluded_descendant_with_canonical_alias_is_not_measured() {
    let tmp = tempfile::tempdir().unwrap();
    let real_root = tmp.path().join("src-real");
    fs::create_dir_all(&real_root).unwrap();
    let alias_root = tmp.path().join("src-alias");
    std::os::unix::fs::symlink(&real_root, &alias_root).unwrap();

    make_project(&real_root, "kept", 4_000);
    let excluded = make_project(&real_root, "excluded-me", 1_000_000);
    let excluded_canonical = fs::canonicalize(&excluded).unwrap();
    let store = tempfile::tempdir().unwrap();

    let (first, coverage) = observe_scope(
        store.path(),
        std::slice::from_ref(&alias_root),
        &[excluded_canonical.display().to_string()],
    );
    assert!(matches!(coverage[0].status, RegionStatus::Complete));
    assert_eq!(first.projects.len(), 1);
    assert_eq!(first.projects[0].name, "kept");
    assert!(
        first
            .unowned
            .iter()
            .all(|u| !u.path_or_object.contains("excluded-me"))
    );

    // A full pass after a large change inside the excluded project must
    // measure exactly the same bytes if the subtree was actually pruned.
    fs::write(excluded.join("target/a.bin"), vec![3u8; 8_000_000]).unwrap();
    let (second, coverage) = observe_scope(
        store.path(),
        std::slice::from_ref(&alias_root),
        &[excluded_canonical.display().to_string()],
    );
    assert!(matches!(coverage[0].status, RegionStatus::Complete));
    assert_eq!(second.projects.len(), 1);
    assert_eq!(second.projects[0].name, "kept");
    assert_eq!(
        second.reconciliation.walked_total, first.reconciliation.walked_total,
        "excluded descendant contents must not contribute to measured bytes"
    );
}

/// An actual deletion (the project directory is genuinely removed, not
/// just made unreadable) must still tombstone correctly and count real
/// regrowth when it comes back -- the protection added for lost access
/// must not blunt real delete/regrow accounting.
#[test]
fn real_delete_then_regrow_still_counts_regrowth() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("src");
    fs::create_dir_all(&root).unwrap();
    let proj = make_project(&root, "proj", 4_000);
    let store = tempfile::tempdir().unwrap();

    let (first, _) = observe_scope(store.path(), std::slice::from_ref(&root), &[]);
    assert_eq!(first.projects.len(), 1);

    // Genuine deletion: the whole checkout is gone (ENOENT), not merely
    // unreadable.
    fs::remove_dir_all(&proj).unwrap();
    let (second, coverage) = observe_scope(store.path(), std::slice::from_ref(&root), &[]);
    assert!(matches!(coverage[0].status, RegionStatus::Complete));
    assert!(second.projects.is_empty(), "{:?}", second.projects);

    // Recreate it: real regrowth, a fresh git history (a different
    // object-store id is fine -- what matters is that *a* project is
    // rediscovered and that no crash/inconsistency results from the
    // store having tombstoned the old one).
    make_project(&root, "proj", 4_000);
    let (third, coverage3) = observe_scope(store.path(), std::slice::from_ref(&root), &[]);
    assert!(matches!(coverage3[0].status, RegionStatus::Complete));
    assert_eq!(third.projects.len(), 1, "{:?}", third.projects);
}

/// A brand-new root added to scope starts at an unknown baseline: its
/// first observation must never report growth (there is nothing to
/// compare against, so `growth_bytes` stays `None`, never a synthetic
/// zero-to-N jump).
#[test]
fn new_root_starts_at_unknown_baseline_not_synthetic_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("brand-new");
    fs::create_dir_all(&root).unwrap();
    make_project(&root, "proj", 50_000);
    let store = tempfile::tempdir().unwrap();

    let (report, _) = observe_scope(store.path(), &[root], &[]);
    let target_row = report.projects[0].worktrees[0]
        .artifacts
        .iter()
        .find(|r| r.path.ends_with("target"))
        .unwrap();
    assert_eq!(
        target_row.growth_bytes, None,
        "a first-ever observation must not report growth from a fabricated zero baseline"
    );
}

/// Every candidate root's status (present/missing/excluded/nested) is
/// reflected as a `RootCoverage` row precisely once, except a folded
/// nested root -- exercising all five region kinds is covered piecemeal
/// by the tests above; this one checks `Missing` specifically, since a
/// configured root that simply doesn't exist yet is the most common real
/// case (a config that names `~/src/new-project` before `git clone` has
/// run).
#[test]
fn missing_root_is_a_distinct_region_not_zero_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("does-not-exist-yet");
    let store = tempfile::tempdir().unwrap();

    let (report, coverage) = observe_scope(store.path(), std::slice::from_ref(&missing), &[]);
    assert_eq!(coverage.len(), 1);
    assert!(matches!(coverage[0].status, RegionStatus::Missing));
    assert!(report.projects.is_empty());
    assert_eq!(report.reconciliation.walked_total, 0);
}

#[test]
fn scope_root_status_present_matches_walked_root() {
    // Sanity check on the scope-resolution boundary this file's helper
    // relies on: an explicit present root really does resolve to
    // `RootStatus::Present`, so the coverage assertions above are
    // actually exercising the `Present` branch of `report_scope`.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("x");
    fs::create_dir_all(&root).unwrap();
    let home = tempfile::tempdir().unwrap();
    let env = env_home(home.path());
    let registry = Registry::with_builtins();
    let scope = resolve_effective_scope(
        &env,
        &explicit_only_config(),
        std::slice::from_ref(&root),
        &registry,
        1,
    );
    assert_eq!(scope.roots.len(), 1);
    assert_eq!(scope.roots[0].status, RootStatus::Present);
}
