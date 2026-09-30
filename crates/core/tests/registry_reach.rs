//! A linked worktree outside every scan root, reached through its main
//! checkout's `.git/worktrees/` registry, driven through the whole report
//! pipeline (walk, growth store, reverse-delta history) exactly as a
//! user's observe does.
//!
//! FSEvents for the root never report on such a path, so every pass here
//! uses a live source that swears nothing changed. That is the tempting
//! shortcut's best case: a patch that carries the worktree's stored rows
//! forward (or never re-reads the registry) passes a full-walk-only test
//! and fails these.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use swamp_core::fs_events::{FsEventsPlan, FsEventsRequest, FsEventsSource};

struct NothingChanged;
impl FsEventsSource for NothingChanged {
    fn replay(&self, _req: &FsEventsRequest) -> FsEventsPlan {
        FsEventsPlan::from_live(Vec::new(), 1000, None)
    }
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_repo(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    fs::write(dir.join("README.md"), b"x").unwrap();
    git(dir, &["add", "README.md"]);
    git(dir, &["commit", "-q", "-m", "init"]);
}

fn observe(root: &Path, store: &Path) -> swamp_core::Report {
    swamp_core::fs_gate::settle::settle();
    swamp_core::report::report_full_mode_scoped(
        root,
        None,
        false,
        Some(store),
        Some("1h"),
        true,
        false,
        false,
        false,
        &NothingChanged,
        &[],
        false,
    )
    .unwrap()
}

fn target_row(r: &swamp_core::Report, worktree: &Path) -> Option<swamp_core::report::ArtifactRow> {
    r.projects
        .iter()
        .flat_map(|p| p.worktrees.iter())
        .find(|w| w.path == worktree)?
        .artifacts
        .iter()
        .find(|a| a.path == worktree.join("target"))
        .cloned()
}

fn has_worktree(r: &swamp_core::Report, worktree: &Path) -> bool {
    r.projects
        .iter()
        .flat_map(|p| p.worktrees.iter())
        .any(|w| w.path == worktree)
}

fn fsevents_mode(r: &swamp_core::Report) -> String {
    r.notes
        .iter()
        .find_map(|n| n.strip_prefix("fsevents: mode="))
        .and_then(|s| s.split(' ').next())
        .unwrap_or("?")
        .to_string()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    store: tempfile::TempDir,
    root: PathBuf,
    main: PathBuf,
    pool: PathBuf,
}

fn fixture() -> Fixture {
    // The canned source has no real FSEvents log lag to protect against.
    unsafe { std::env::set_var("SWAMP_FSEVENTS_MIN_INTERVAL_SECS", "0") };
    let tmp = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let root = base.join("src");
    let main = root.join("proj");
    init_repo(&main);
    Fixture {
        _tmp: tmp,
        store: tempfile::tempdir().unwrap(),
        root,
        main,
        pool: base.join("pool/task/proj"),
    }
}

fn add_pool_worktree(fx: &Fixture) {
    fs::create_dir_all(fx.pool.parent().unwrap()).unwrap();
    git(
        &fx.main,
        &[
            "worktree",
            "add",
            "-q",
            fx.pool.to_str().unwrap(),
            "-b",
            "task",
        ],
    );
    fs::create_dir_all(fx.pool.join("target")).unwrap();
    fs::write(
        fx.pool.join("target/blob"),
        swamp_core::fs_gate::settle::noise(1 << 20),
    )
    .unwrap();
}

#[test]
fn a_worktree_first_reached_on_a_later_pass_is_not_yet_observed_not_growth() {
    let fx = fixture();
    let first = observe(&fx.root, fx.store.path());
    assert!(!has_worktree(&first, &fx.pool));

    add_pool_worktree(&fx);
    let second = observe(&fx.root, fx.store.path());
    assert_eq!(fsevents_mode(&second), "incremental");
    let row = target_row(&second, &fx.pool).expect("reached and measured on an incremental pass");
    assert!(row.bytes >= 1 << 20, "{row:?}");
    assert_eq!(
        row.growth_bytes, None,
        "a coverage change is not a storage change: first seen => not yet observed"
    );
    let main_growth: i64 = second
        .projects
        .iter()
        .flat_map(|p| p.worktrees.iter())
        .filter(|w| w.path == fx.main)
        .flat_map(|w| w.artifacts.iter())
        .filter_map(|a| a.growth_bytes)
        .sum();
    assert!(
        main_growth < 1 << 20,
        "the main checkout must not absorb the new worktree's bytes: +{main_growth}"
    );
}

#[test]
fn an_out_of_root_worktree_is_re_measured_every_pass_and_dropped_when_unregistered() {
    let fx = fixture();
    add_pool_worktree(&fx);
    let first = observe(&fx.root, fx.store.path());
    let before = target_row(&first, &fx.pool)
        .expect("reached on the first pass")
        .bytes;

    // Grows where no FSEvents for `root` will ever say so.
    fs::write(
        fx.pool.join("target/blob2"),
        swamp_core::fs_gate::settle::noise(3 << 20),
    )
    .unwrap();
    let grown = observe(&fx.root, fx.store.path());
    assert_eq!(fsevents_mode(&grown), "incremental");
    let row = target_row(&grown, &fx.pool).expect("still reached");
    assert!(
        row.bytes >= before + (3 << 20),
        "carried-forward rows would leave this at {before}; got {}",
        row.bytes
    );
    assert!(
        row.growth_bytes.is_some_and(|g| g >= 3 << 20),
        "real growth under a reached worktree is growth: {:?}",
        row.growth_bytes
    );

    git(
        &fx.main,
        &["worktree", "remove", "--force", fx.pool.to_str().unwrap()],
    );
    let after = observe(&fx.root, fx.store.path());
    assert_eq!(fsevents_mode(&after), "incremental");
    assert!(!has_worktree(&after, &fx.pool));
}

#[test]
fn the_reach_round_trips_through_its_coverage_note() {
    let reach = swamp_core::coverage::RegistryReach {
        worktree: PathBuf::from("/pool/task slug/proj"),
        via: PathBuf::from("/src/proj"),
    };
    assert_eq!(
        swamp_core::coverage::RegistryReach::from_note(&reach.to_note()).as_ref(),
        Some(&reach)
    );
    assert!(swamp_core::coverage::RegistryReach::from_note("fsevents: mode=full").is_none());
}
