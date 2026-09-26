//! R4b (#29): FSEvents-driven incremental observation, exercised end to
//! end through `report_full_mode_with_source` with a canned
//! [`FsEventsSource`] so nothing here depends on the live `fseventsd`.
//!
//! Every call in this file goes through `report_full_mode_with_source`
//! with an explicit canned source -- never `report_full_mode` (which
//! resolves the real macOS source) -- and every test disables the
//! [`RefreshRefusal::TooSoon`] floor via
//! `SWAMP_FSEVENTS_MIN_INTERVAL_SECS=0` instead of sleeping past it.
//! Both are load-bearing for CI/shared-machine safety, not style: this
//! suite used to spend real wall-clock seconds per case and, worse, once
//! wired a "first observation" call through the real platform source
//! (harmless in isolation, but on a machine already running many other
//! `fseventsd`-touching processes -- other worktrees' test suites --
//! `source.replay` calls piling up is exactly the kind of shared-resource
//! load this file must never contribute).

#[path = "fixture/mod.rs"]
mod fixture;

use std::fs;
use std::path::PathBuf;
use std::sync::Once;
use swamp_core::actions;
use swamp_core::fs_events::{
    FsEventsPlan, FsEventsRequest, FsEventsSource, RefreshRefusal, testing::CannedSource,
};
use swamp_core::growth::{root_key, volume_store_dir};
use swamp_core::report::{ArtifactKind, report_full_mode_with_source};

/// Disables the `TooSoon` floor for this process. Idempotent and safe to
/// call from every test regardless of thread-parallel execution: every
/// caller wants the same value, so a benign race on the underlying env
/// var write is fine. Must run before any `report_full_mode_with_source`
/// call in this file that expects an incremental (not `too_soon`) verdict
/// from a back-to-back pair of observations with no real time gap.
fn disable_too_soon_floor() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        // SAFETY: set once, before any thread in this test binary reads
        // it via `growth::min_interval_secs`; no other code in this
        // process depends on this variable being absent.
        std::env::set_var("SWAMP_FSEVENTS_MIN_INTERVAL_SECS", "0");
    });
}

/// A source that always refuses with a fixed reason, ignoring the
/// request entirely -- used both to exercise each refusal reason and as
/// the stand-in for "no real FSEvents source" on every call in this file
/// that does not care about the answer (e.g. a first observation, which
/// is always a full walk regardless of what the source says, since there
/// is no stored state yet to replay from).
struct RefusingSource(RefreshRefusal);

impl FsEventsSource for RefusingSource {
    fn replay(&self, _request: &FsEventsRequest) -> FsEventsPlan {
        FsEventsPlan {
            incremental: false,
            refusal: Some(self.0),
            changed_dirs: Vec::new(),
            current_event_id: 999,
            device: Some(1),
            live: false,
            consume: None,
        }
    }
}

/// Standing in for "no source needed" (force_full skips the source
/// entirely) and for a first observation (no stored state, so the
/// source's answer is never consulted either).
fn no_op_source() -> RefusingSource {
    RefusingSource(RefreshRefusal::NoStoredEventId)
}

fn incremental_plan(changed: Vec<PathBuf>, event_id: u64) -> FsEventsPlan {
    FsEventsPlan {
        incremental: true,
        refusal: None,
        changed_dirs: changed,
        current_event_id: event_id,
        device: Some(1),
        live: false,
        consume: None,
    }
}

#[test]
fn touching_one_artifact_resizes_only_that_row_and_matches_a_full_walk() {
    disable_too_soon_floor();
    let tmp = tempfile::tempdir().expect("tmp root");
    let fx = fixture::build(tmp.path());
    let store = tempfile::tempdir().expect("tmp store");

    // First observation: no stored event id yet, so this is a full walk
    // regardless of source (observe_tracked never even reaches the
    // source's answer when there is no prior topology).
    let first = report_full_mode_with_source(
        &fx.root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &no_op_source(),
    )
    .expect("first (full) report");
    assert!(
        first
            .notes
            .iter()
            .any(|n| n.starts_with("fsevents: mode=full")),
        "first observation of a fresh store must be a full walk: {:?}",
        first.notes
    );

    // Touch one file inside node_modules: same directory, different
    // content, so only that artifact's byte total should move.
    fs::write(fx.node_modules.join("touched.bin"), vec![b't'; 4096]).expect("write touch probe");

    // Second observation: canned source reports exactly node_modules as
    // changed.
    let source = CannedSource(incremental_plan(vec![fx.node_modules.clone()], 1));
    let second = report_full_mode_with_source(
        &fx.root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &source,
    )
    .expect("second (incremental) report");

    assert!(
        second
            .notes
            .iter()
            .any(|n| n.starts_with("fsevents: mode=incremental")),
        "second observation must take the incremental path: {:?}",
        second.notes
    );

    // A forced full walk from the same on-disk state, for comparison.
    // force_full never consults the source at all, so any source works.
    let full = report_full_mode_with_source(
        &fx.root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        false, // read-only: don't disturb the store the incremental path just wrote.
        false,
        false,
        true,
        &no_op_source(),
    )
    .expect("forced full report");

    let checkout_worktree = |r: &swamp_core::report::Report| {
        r.projects
            .iter()
            .find(|p| p.name == fx.checkout_name)
            .expect("checkout project")
            .worktrees
            .iter()
            .find(|w| w.path == fx.checkout)
            .expect("main worktree")
            .clone()
    };

    let inc_worktree = checkout_worktree(&second);
    let full_worktree = checkout_worktree(&full);

    let inc_by_path: std::collections::HashMap<_, _> = inc_worktree
        .artifacts
        .iter()
        .map(|a| (a.path.clone(), a.bytes))
        .collect();
    let full_by_path: std::collections::HashMap<_, _> = full_worktree
        .artifacts
        .iter()
        .map(|a| (a.path.clone(), a.bytes))
        .collect();

    assert_eq!(
        inc_by_path, full_by_path,
        "incremental and full walks must agree on every artifact row's bytes"
    );

    let node_modules_bytes = inc_by_path[&fx.node_modules];
    assert!(
        node_modules_bytes > fx.node_modules_bytes,
        "node_modules must have grown from the touched file: {node_modules_bytes} vs original {}",
        fx.node_modules_bytes
    );

    // Every other artifact row in the same worktree is byte-identical to
    // the untouched original fixture value.
    let target_bytes = inc_by_path[&fx.target_dir];
    assert_eq!(
        target_bytes, fx.target_bytes,
        "target/ must carry forward unchanged"
    );
    let dist_bytes = inc_by_path[&fx.dist_dir];
    assert_eq!(
        dist_bytes, fx.dist_bytes,
        "dist/ must carry forward unchanged"
    );

    // Reconciliation still holds exactly on the incremental report.
    assert_eq!(
        second.reconciliation.attributed + second.reconciliation.unowned,
        second.reconciliation.walked_total,
        "attributed + unowned must reconcile to walked_total on an incremental report"
    );
}

#[test]
fn deep_change_inside_a_folded_artifact_resizes_from_interior_rows_and_matches_a_full_walk() {
    disable_too_soon_floor();
    let tmp = tempfile::tempdir().expect("tmp root");
    let fx = fixture::build(tmp.path());
    let store = tempfile::tempdir().expect("tmp store");
    // Deep structure inside node_modules before the first (full) walk.
    let deep = fx.node_modules.join("pkg/lib/sub");
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("old.bin"), vec![b'o'; 8192]).unwrap();
    let doomed = fx.node_modules.join("pkg/doomed");
    fs::create_dir_all(&doomed).unwrap();
    fs::write(doomed.join("x.bin"), vec![b'x'; 16384]).unwrap();

    let first = report_full_mode_with_source(
        &fx.root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &no_op_source(),
    )
    .expect("first (full) report");
    assert!(
        first
            .notes
            .iter()
            .any(|n| n.starts_with("fsevents: mode=full"))
    );
    // Interior rows are in the store, not in the report.
    let with_dirs = report_full_mode_with_source(
        &fx.root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        false,
        true,
        false,
        true,
        &no_op_source(),
    )
    .expect("dirs report");
    let wt_id = with_dirs
        .projects
        .iter()
        .flat_map(|p| p.worktrees.iter())
        .find(|w| w.path == fx.checkout)
        .unwrap()
        .worktree_id
        .clone();
    assert!(
        with_dirs.dirs_by_worktree.as_ref().unwrap()[&wt_id]
            .iter()
            .all(|d| !d.rel_path.starts_with("node_modules")),
        "no interior row of a folded artifact reaches the report"
    );

    // Three kinds of change, three levels down: a write, a new subtree,
    // a deleted subtree.
    fs::write(deep.join("new.bin"), vec![b'n'; 40960]).unwrap();
    let fresh = fx.node_modules.join("pkg/lib/fresh/deeper");
    fs::create_dir_all(&fresh).unwrap();
    fs::write(fresh.join("f.bin"), vec![b'f'; 12288]).unwrap();
    fs::remove_dir_all(&doomed).unwrap();

    // FSEvents names the directories whose listings changed (and, by our
    // rule, their parents).
    let source = CannedSource(incremental_plan(
        vec![
            deep.clone(),
            fx.node_modules.join("pkg/lib"),
            fx.node_modules.join("pkg/lib/fresh"),
            fresh.clone(),
            fx.node_modules.join("pkg"),
        ],
        2,
    ));
    let second = report_full_mode_with_source(
        &fx.root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &source,
    )
    .expect("incremental report");
    assert!(
        second
            .notes
            .iter()
            .any(|n| n.starts_with("fsevents: mode=incremental")),
        "{:?}",
        second.notes
    );

    let full = report_full_mode_with_source(
        &fx.root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        false,
        false,
        false,
        true,
        &no_op_source(),
    )
    .expect("forced full report");
    let bytes_of = |r: &swamp_core::report::Report, path: &std::path::Path| {
        r.projects
            .iter()
            .flat_map(|p| p.worktrees.iter())
            .flat_map(|w| w.artifacts.iter())
            .find(|a| a.path == path)
            .map(|a| a.bytes)
            .expect("row")
    };
    assert_eq!(
        bytes_of(&second, &fx.node_modules),
        bytes_of(&full, &fx.node_modules),
        "interior re-size must agree with a full walk (write + new subtree - deleted subtree)"
    );
    assert_eq!(
        bytes_of(&second, &fx.target_dir),
        fx.target_bytes,
        "untouched artifact carries forward"
    );
    assert_eq!(
        second.reconciliation.attributed + second.reconciliation.unowned,
        second.reconciliation.walked_total
    );
}

#[test]
fn new_nested_repo_is_discovered_incrementally() {
    disable_too_soon_floor();
    let tmp = tempfile::tempdir().expect("tmp root");
    let fx = fixture::build(tmp.path());
    let store = tempfile::tempdir().expect("tmp store");

    report_full_mode_with_source(
        &fx.root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &no_op_source(),
    )
    .expect("first (full) report");

    // A brand-new nested checkout appears under the existing checkout,
    // in a location the first walk never saw.
    let new_repo = fx.checkout.join("second-nested-repo");
    fs::create_dir_all(&new_repo).expect("mkdir new nested repo");
    let run_git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&new_repo)
            .args(args)
            .env("GIT_AUTHOR_NAME", "fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
            .env("GIT_COMMITTER_NAME", "fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "{:?}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run_git(&["init", "-q", "-b", "main"]);
    run_git(&["config", "commit.gpgsign", "false"]);
    fs::write(new_repo.join("README.md"), b"new nested repo\n").expect("write README");
    run_git(&["add", "README.md"]);
    run_git(&["commit", "-q", "-m", "initial commit"]);

    let source = CannedSource(incremental_plan(vec![new_repo.clone()], 1));
    let second = report_full_mode_with_source(
        &fx.root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &source,
    )
    .expect("second (incremental) report");

    assert!(
        second
            .projects
            .iter()
            .any(|p| p.worktrees.iter().any(|w| w.path == new_repo)),
        "the newly created nested repo must appear as a discovered worktree after an \
         incremental observation implicates its directory"
    );
}

/// One fixture, one full-observation baseline, reused across every
/// refusal reason: each reason only needs a *fresh store* (so its own
/// "first observation" is a real full walk with a real topology to carry
/// forward the source path into oblivion... actually a refusal always
/// falls back to a full walk anyway, but a fresh store per reason keeps
/// each case's assertions independent). Building the git fixture once
/// instead of seven times is what keeps this suite fast: git subprocess
/// spawns, not the report logic itself, dominated this test's wall time.
#[test]
fn every_refusal_reason_falls_back_to_a_full_walk() {
    disable_too_soon_floor();
    let tmp = tempfile::tempdir().expect("tmp root");
    let fx = fixture::build(tmp.path());

    for reason in [
        RefreshRefusal::NoStoredEventId,
        RefreshRefusal::EventIdFromFuture,
        RefreshRefusal::RootMismatch,
        RefreshRefusal::FseventsdUnavailable,
        RefreshRefusal::HelperInconclusive,
        RefreshRefusal::TooManyChanges,
        RefreshRefusal::UnsupportedPlatform,
        RefreshRefusal::NoPersistedChangeHistory,
    ] {
        let store = tempfile::tempdir().expect("tmp store");

        report_full_mode_with_source(
            &fx.root,
            None,
            false,
            Some(store.path()),
            Some("1h"),
            true,
            false,
            false,
            false,
            &no_op_source(),
        )
        .expect("first (full) report");

        let source = RefusingSource(reason);
        let r = report_full_mode_with_source(
            &fx.root,
            None,
            false,
            Some(store.path()),
            Some("1h"),
            true,
            false,
            false,
            false,
            &source,
        )
        .expect("refused report still succeeds as a full walk");

        let expected = format!(
            "fsevents: mode=full reason={} changed_dirs=0",
            reason.as_str()
        );
        assert!(
            r.notes.iter().any(|n| n == &expected),
            "reason {:?} must produce note {expected:?}, got {:?}",
            reason,
            r.notes
        );
        // A refused observation is still a correct one: every artifact
        // kind classified by the fixture is present.
        let checkout = r
            .projects
            .iter()
            .find(|p| p.name == fx.checkout_name)
            .expect("checkout project");
        let main = checkout
            .worktrees
            .iter()
            .find(|w| w.path == fx.checkout)
            .expect("main worktree");
        assert!(
            main.artifacts
                .iter()
                .any(|a| a.kind == ArtifactKind::DependencyTree),
            "a full walk must still classify node_modules"
        );
    }
}

#[test]
fn stored_event_id_is_recorded_after_an_observation() {
    disable_too_soon_floor();
    let tmp = tempfile::tempdir().expect("tmp root");
    let fx = fixture::build(tmp.path());
    let store = tempfile::tempdir().expect("tmp store");

    report_full_mode_with_source(
        &fx.root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &no_op_source(),
    )
    .expect("report");

    let anchor =
        swamp_core::growth::read_fsevents_anchor(&volume_store_dir(store.path(), &fx.root));
    assert!(
        anchor.event_id.is_some(),
        "cursors.parquet must record the observed event id: {anchor:?}"
    );
}

#[test]
fn report_cache_failure_does_not_advance_the_replay_checkpoint() {
    disable_too_soon_floor();
    let tmp = tempfile::tempdir().expect("tmp root");
    let fx = fixture::build(tmp.path());
    let store = tempfile::tempdir().expect("tmp store");
    report_full_mode_with_source(
        &fx.root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &no_op_source(),
    )
    .expect("baseline");
    let volume = volume_store_dir(store.path(), &fx.root);
    let before = swamp_core::growth::read_fsevents_anchor(&volume);
    // `git_signals.parquet` is `consumers::signals`'s per-root replay
    // cache (R18a-4's replacement for `last_report-<key>.json.zst`):
    // written, like every table, every pass this call observes.
    let cache = store.path().join("git_signals.parquet");
    assert!(cache.exists(), "baseline observe must write {cache:?}");
    // The cache write publishes by renaming a sibling temp file over the
    // cache path; a non-empty directory sitting at that path makes the
    // rename fail, which is the failure this test needs.
    fs::remove_file(&cache).unwrap();
    let blocker = cache.clone();
    fs::create_dir(&blocker).unwrap();
    fs::write(blocker.join("occupied"), b"x").unwrap();
    let source = CannedSource(incremental_plan(vec![], 77));
    let result = report_full_mode_with_source(
        &fx.root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &source,
    );
    assert!(
        result.is_err(),
        "cache failure must propagate rather than advance replay past uncached evidence"
    );
    assert_eq!(swamp_core::growth::read_fsevents_anchor(&volume), before);
    fs::remove_dir_all(blocker).unwrap();
    report_full_mode_with_source(
        &fx.root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &source,
    )
    .expect("retry");
    let after = swamp_core::growth::read_fsevents_anchor(&volume);
    assert_eq!(after.event_id, Some(77));
}

#[test]
fn switching_roots_preserves_history_and_alias_replay_namespace() {
    disable_too_soon_floor();
    let tmp = tempfile::tempdir().expect("tmp root");
    let fx = fixture::build(tmp.path());
    let store = tempfile::tempdir().expect("one shared store");
    let canonical_checkout = std::fs::canonicalize(&fx.checkout).unwrap();
    let canonical_linked = std::fs::canonicalize(&fx.linked_worktree).unwrap();
    let canonical_node_modules = std::fs::canonicalize(&fx.node_modules).unwrap();
    let alias = tmp.path().join("root-alias");
    std::os::unix::fs::symlink(&fx.root, &alias).expect("root alias");

    // Observe the parent through an alias. The public report boundary
    // canonicalizes the root, so topology and report paths share one
    // namespace with later canonical-root calls.
    let parent = report_full_mode_with_source(
        &alias,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &no_op_source(),
    )
    .expect("parent baseline");
    assert_eq!(parent.root, std::fs::canonicalize(&fx.root).unwrap());
    // R18a-4: there is no single whole-`Report` cache left to load: the
    // per-root replay tables are keyed by `growth::root_key`, which
    // canonicalizes before hashing, so the alias and the canonical root
    // must resolve to the same key and the same stored replay cache.
    assert_eq!(root_key(&alias), root_key(&fx.root));
    assert!(
        swamp_core::growth::read_cargo_replay_cache(store.path(), &root_key(&alias)).is_some(),
        "alias and canonical root must load the same cached replay state"
    );

    // A child/root switch uses the same physical store but must select a
    // different root scope. Its full observation must not replace the
    // parent's topology or current/history files.
    report_full_mode_with_source(
        &fx.checkout,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &no_op_source(),
    )
    .expect("child observation");

    fs::write(
        fx.node_modules.join("alias-replay-probe"),
        vec![b'x'; 1024 * 1024],
    )
    .expect("parent growth");
    let changed = std::fs::canonicalize(&fx.node_modules).unwrap();
    let parent_readonly = report_full_mode_with_source(
        &fx.root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        false,
        false,
        false,
        false,
        &CannedSource(incremental_plan(vec![changed], 123)),
    )
    .expect("canonical parent no-observe replay");

    assert!(
        parent_readonly
            .notes
            .iter()
            .any(|note| note.contains("mode=incremental")),
        "alias baseline must support canonical incremental replay: {:?}",
        parent_readonly.notes
    );
    let parent_checkout = parent_readonly
        .projects
        .iter()
        .find(|p| p.name == fx.checkout_name)
        .expect("parent checkout");
    assert!(
        parent_checkout
            .worktrees
            .iter()
            .any(|w| w.path == canonical_linked),
        "parent report/propose scope must still expose the visible linked worktree: {:?}",
        parent_checkout
            .worktrees
            .iter()
            .map(|w| &w.path)
            .collect::<Vec<_>>()
    );
    let proposal = actions::propose(
        &parent_readonly,
        None,
        std::slice::from_ref(&canonical_linked),
        "root-switch-test",
    )
    .expect("visible linked worktree should produce a proposal");
    assert_eq!(
        proposal.len(),
        1,
        "proposal must contain exactly the selected worktree"
    );
    assert_eq!(proposal[0].path(), canonical_linked);
    assert_eq!(proposal[0].worktree_path(), canonical_linked);
    assert_eq!(proposal[0].verb(), "remove-worktree");
    let main = parent_checkout
        .worktrees
        .iter()
        .find(|w| w.path == canonical_checkout)
        .expect("parent main worktree");
    let node_modules = main
        .artifacts
        .iter()
        .find(|a| a.path == canonical_node_modules)
        .expect("parent node_modules");
    assert!(
        node_modules.growth_bytes.is_some_and(|growth| growth > 0),
        "parent history must survive the child observation: {node_modules:?}"
    );

    assert_ne!(
        volume_store_dir(store.path(), &fx.root),
        volume_store_dir(store.path(), &fx.checkout),
        "parent and child must retain independent current/history stores"
    );
}

/// Regression for a live-run bug against `~/src`: a project whose linked
/// worktrees live *inside* the main checkout's own directory tree (e.g.
/// `.worktrees/<name>`, the real shape `swamp` itself uses), each
/// with its own large `target/` artifact. Touching one file in the main
/// checkout's `target/` and re-observing incrementally must not fold any
/// linked worktree's bytes into the main checkout's row: every row in
/// every worktree must equal a forced full walk, not just the touched
/// one, and `walked_total`/`attributed`/`unowned` must reconcile exactly.
///
/// Before the fix, `attribute_one_worktree` re-walked the rewalked
/// worktree's directory with *only that one worktree* in the known list,
/// so `nearest_worktree` matched every path under it -- including a
/// nested linked worktree's own `target/` -- to the outer worktree,
/// double-counting it on top of that linked worktree's still-correct
/// carried-forward row.
#[test]
fn nested_linked_worktree_artifacts_are_not_double_counted_on_incremental_rewalk() {
    disable_too_soon_floor();
    fn run_git(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
            .env("GIT_COMMITTER_NAME", "fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn write_pattern(path: &std::path::Path, size: usize) {
        fs::create_dir_all(path.parent().unwrap()).expect("mkdir parent");
        fs::write(path, vec![b'x'; size]).expect("write file");
    }

    let tmp = tempfile::tempdir().expect("tmp root");
    let root = fs::canonicalize(tmp.path()).unwrap().join("project");
    fs::create_dir_all(&root).expect("mkdir root");
    run_git(&root, &["init", "-q", "-b", "main"]);
    run_git(&root, &["config", "commit.gpgsign", "false"]);
    fs::write(root.join("README.md"), b"main\n").expect("write README");
    run_git(&root, &["add", "README.md"]);
    run_git(&root, &["commit", "-q", "-m", "initial commit"]);

    // Main checkout's own large artifact.
    write_pattern(&root.join("target").join("main.bin"), 2 * 1024 * 1024);

    // Three linked worktrees living *inside* the main checkout's tree,
    // each with its own large artifact -- the exact shape that tripped
    // the bug on the real `swamp` repo (13 nested worktrees).
    let worktrees_dir = root.join(".worktrees");
    let mut linked_paths = Vec::new();
    for name in ["a", "b", "c"] {
        let wt = worktrees_dir.join(name);
        run_git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                wt.to_str().expect("utf8 path"),
                "-b",
                name,
            ],
        );
        write_pattern(
            &wt.join("target").join(format!("{name}.bin")),
            3 * 1024 * 1024,
        );
        linked_paths.push(wt);
    }

    let store = tempfile::tempdir().expect("tmp store");

    let first = report_full_mode_with_source(
        &root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &no_op_source(),
    )
    .expect("first (full) report");
    assert_eq!(
        first.reconciliation.attributed + first.reconciliation.unowned,
        first.reconciliation.walked_total,
        "baseline reconciliation must hold"
    );

    // Touch a Source-tree file directly at the MAIN checkout's root --
    // deliberately *not* inside `target/` (an existing classified
    // artifact would resolve through `resize_artifact` instead, which
    // never exercises the buggy code path). A change outside any known
    // artifact root forces the "rewalk this whole worktree" branch
    // (`attribute_one_worktree`), which is exactly what folded a nested
    // linked worktree's bytes into the outer worktree before the fix.
    fs::write(root.join("touched.txt"), vec![b't'; 4096]).expect("write touch probe");

    let source = CannedSource(incremental_plan(vec![root.clone()], 1));
    let incremental = report_full_mode_with_source(
        &root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &source,
    )
    .expect("incremental report");
    assert!(
        incremental
            .notes
            .iter()
            .any(|n| n.starts_with("fsevents: mode=incremental")),
        "must take the incremental path: {:?}",
        incremental.notes
    );

    let full = report_full_mode_with_source(
        &root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        false, // read-only: don't disturb what the incremental pass just wrote.
        false,
        false,
        true,
        &no_op_source(),
    )
    .expect("forced full report");

    // Reconciliation must hold exactly on the incremental report -- this
    // is what catches the double-count directly, without needing to
    // compare against the full walk at all.
    assert_eq!(
        incremental.reconciliation.attributed + incremental.reconciliation.unowned,
        incremental.reconciliation.walked_total,
        "incremental reconciliation must still hold exactly: {:?}",
        incremental.reconciliation
    );

    // And every row in every worktree (not just the touched one) must be
    // byte-for-byte identical to a forced full walk.
    let rows_by_path = |r: &swamp_core::report::Report| -> std::collections::HashMap<PathBuf, u64> {
        r.projects
            .iter()
            .flat_map(|p| &p.worktrees)
            .flat_map(|w| &w.artifacts)
            .map(|a| (a.path.clone(), a.bytes))
            .collect()
    };
    let inc_rows = rows_by_path(&incremental);
    let full_rows = rows_by_path(&full);
    assert_eq!(
        inc_rows, full_rows,
        "every artifact row (including every linked worktree's target/) must match a full walk exactly"
    );
    assert_eq!(
        incremental.reconciliation.walked_total, full.reconciliation.walked_total,
        "walked_total must match a full walk exactly"
    );
    assert_eq!(
        incremental.reconciliation.attributed,
        full.reconciliation.attributed
    );
    assert_eq!(
        incremental.reconciliation.unowned,
        full.reconciliation.unowned
    );

    // Sanity: every linked worktree's own target/ row is present exactly
    // once and unions to the fixture's real sizes -- if the bug were
    // still present this would be roughly double.
    for wt in &linked_paths {
        let key = wt.join("target");
        assert!(
            inc_rows.contains_key(&key),
            "linked worktree {} must still have its own target/ row",
            wt.display()
        );
    }
}

/// Regression for the live #29 finding: touching one file inside a Cargo
/// `target/` inflated `walked_total` by ~32 MB because the re-size
/// re-charged hardlinked inodes the full walk had already charged to
/// another row. Two artifact rows share hardlinked files; after an
/// incremental update, allocation totals advance but unique charges remain
/// explicitly stale until a full walk reconciles them.
#[test]
fn hardlinks_shared_across_rows_are_not_recharged_on_incremental_resize() {
    disable_too_soon_floor();
    let tmp = tempfile::tempdir().expect("tmp root");
    let fx = fixture::build(tmp.path());
    let store = tempfile::tempdir().expect("tmp store");

    // Hardlinks: 6 files of 64 KiB living in node_modules, each also
    // linked from target/. Whichever row the full walk charges, the
    // bytes must be counted exactly once.
    let target = fx.checkout.join("target");
    fs::create_dir_all(&target).expect("target dir");
    for i in 0..6 {
        let a = fx.node_modules.join(format!("shared-{i}.bin"));
        fs::write(&a, vec![b'h'; 64 * 1024]).expect("write shared");
        fs::hard_link(&a, target.join(format!("shared-{i}.bin"))).expect("hard link");
    }

    let first = report_full_mode_with_source(
        &fx.root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &no_op_source(),
    )
    .expect("first (full) report");
    let full_before = first.reconciliation.walked_total;

    // Touch inside target/ (the row whose shared inodes may belong to
    // node_modules in the full walk's accounting).
    fs::write(target.join("touched.bin"), vec![b't'; 8192]).expect("touch probe");

    let source = CannedSource(incremental_plan(vec![target.clone()], 1));
    let second = report_full_mode_with_source(
        &fx.root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &source,
    )
    .expect("second (incremental) report");
    assert!(
        second
            .notes
            .iter()
            .any(|n| n.starts_with("fsevents: mode=incremental")),
        "must take the incremental path: {:?}",
        second.notes
    );

    let full = report_full_mode_with_source(
        &fx.root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        false,
        false,
        false,
        true,
        &no_op_source(),
    )
    .expect("forced full report");

    assert_eq!(
        second.reconciliation.walked_total, full_before,
        "unique totals retain the last measurement, not a sum of overlapping allocations"
    );
    assert_eq!(
        second.reconciliation.attributed,
        first.reconciliation.attributed
    );
    assert_eq!(second.reconciliation.unowned, full.reconciliation.unowned);
    // The only change is the 8 KiB probe.
    assert_eq!(full.reconciliation.walked_total, full_before + 8192);

    // Per-row equality, order-independent.
    let rows = |r: &swamp_core::Report| {
        let mut v: Vec<(String, String, u64)> = r
            .projects
            .iter()
            .flat_map(|p| p.worktrees.iter())
            .flat_map(|w| {
                w.artifacts.iter().map(|a| {
                    (
                        format!("{:?}", a.kind),
                        a.path.display().to_string(),
                        a.bytes,
                    )
                })
            })
            .collect();
        v.sort();
        v
    };
    // The two rows that share hardlinked inodes may split them differently
    // between two parallel full walks (whichever worker sees an inode first
    // charges it), so they are compared as a pair; every other row must be
    // identical.
    let is_pair = |kind: &str, path: &str| {
        (kind == "BuildOutput" && path.ends_with("/checkout/target"))
            || (kind == "DependencyTree" && path.ends_with("/checkout/node_modules"))
    };
    let split = |v: Vec<(String, String, u64)>| {
        let pair: u64 = v
            .iter()
            .filter(|(k, p, _)| is_pair(k, p))
            .map(|(_, _, b)| b)
            .sum();
        let rest: Vec<_> = v.into_iter().filter(|(k, p, _)| !is_pair(k, p)).collect();
        (pair, rest)
    };
    let (pair_inc, rest_inc) = split(rows(&second));
    let (pair_full, rest_full) = split(rows(&full));
    assert_eq!(
        pair_inc + 8192,
        pair_full,
        "full reconciliation accounts for the added probe exactly once"
    );
    assert_eq!(
        rest_inc, rest_full,
        "every other row must match a full walk"
    );
    let target_row = |r: &swamp_core::Report| {
        r.projects
            .iter()
            .flat_map(|p| &p.worktrees)
            .flat_map(|w| &w.artifacts)
            .find(|a| a.path == target)
            .unwrap()
            .clone()
    };
    assert!(target_row(&second).dedup_stale);
    assert_eq!(target_row(&second).growth_bytes, None);
    assert_eq!(
        target_row(&second).allocated_bytes.unwrap(),
        target_row(&first).allocated_bytes.unwrap() + 8192
    );
    assert!(!target_row(&full).dedup_stale);
    assert!(second.notes.iter().any(|n| n.contains("reconciliation")));
    let unchanged = report_full_mode_with_source(
        &fx.root,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &CannedSource(incremental_plan(vec![], 2)),
    )
    .unwrap();
    assert!(
        target_row(&unchanged).dedup_stale,
        "stale state survives persisted no-change refresh"
    );
    assert_eq!(
        target_row(&unchanged).allocated_bytes,
        target_row(&second).allocated_bytes
    );
}

/// Live #29 finding: with Docker facts joined, the incremental path
/// inflated `walked_total` by the Docker rows' unique bytes (they are
/// persisted for growth history but are not filesystem bytes) and listed
/// them twice. With facts present, incremental must equal a full walk and
/// Docker rows must appear exactly once.
#[test]
fn docker_rows_stay_out_of_walked_total_and_are_not_duplicated_on_incremental() {
    disable_too_soon_floor();
    let tmp = tempfile::tempdir().expect("tmp root");
    let fx = fixture::build(tmp.path());
    let store = tempfile::tempdir().expect("tmp store");
    let facts = Some(fx.docker_facts.as_path());

    let first = report_full_mode_with_source(
        &fx.root,
        facts,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &no_op_source(),
    )
    .expect("first (full) report");
    let docker_rows = |r: &swamp_core::Report| -> Vec<String> {
        let mut v: Vec<String> = r
            .projects
            .iter()
            .flat_map(|p| p.worktrees.iter())
            .flat_map(|w| w.artifacts.iter())
            .filter(|a| {
                matches!(
                    a.kind,
                    swamp_core::report::ArtifactKind::DockerImage
                        | swamp_core::report::ArtifactKind::DockerBuildCache
                        | swamp_core::report::ArtifactKind::DockerVolume
                )
            })
            .map(|a| a.path.display().to_string())
            .collect();
        v.sort();
        v
    };
    let first_docker = docker_rows(&first);
    assert!(
        !first_docker.is_empty(),
        "fixture must join at least one Docker object"
    );

    fs::write(fx.node_modules.join("touched.bin"), vec![b't'; 4096]).expect("touch");
    let source = CannedSource(incremental_plan(vec![fx.node_modules.clone()], 1));
    let second = report_full_mode_with_source(
        &fx.root,
        facts,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        false,
        &source,
    )
    .expect("incremental report");
    assert!(
        second
            .notes
            .iter()
            .any(|n| n.starts_with("fsevents: mode=incremental")),
        "{:?}",
        second.notes
    );

    let full = report_full_mode_with_source(
        &fx.root,
        facts,
        false,
        Some(store.path()),
        Some("1h"),
        false,
        false,
        false,
        true,
        &no_op_source(),
    )
    .expect("forced full report");

    assert_eq!(
        second.reconciliation.walked_total,
        full.reconciliation.walked_total
    );
    assert_eq!(
        second.reconciliation.attributed,
        full.reconciliation.attributed
    );
    assert_eq!(
        second.reconciliation.docker_attributed,
        full.reconciliation.docker_attributed
    );
    assert_eq!(
        full.reconciliation.walked_total,
        first.reconciliation.walked_total + 4096
    );
    let second_docker = docker_rows(&second);
    assert_eq!(
        second_docker, first_docker,
        "docker rows must appear exactly once, unchanged"
    );
    let mut dedup = second_docker.clone();
    dedup.dedup();
    assert_eq!(
        dedup.len(),
        second_docker.len(),
        "no duplicated docker rows"
    );
}
