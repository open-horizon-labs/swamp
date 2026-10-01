//! #208: whether a worktree's HEAD is contained in another branch is
//! computed locally from git's own refs (offline, no `gh`), reported as
//! its own fact with the branch name. Real temp git repositories.
//!
//! Each test names the tempting wrong patch it fails.

use std::path::{Path, PathBuf};
use std::process::Command;

use swamp_core::fs_gate::git::TipReach;
use swamp_core::github::{MergedStatus, TriState, merge_complete};

fn git(dir: &Path, args: &[&str]) {
    swamp_core::work_counters::record_spawn();
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn commit(dir: &Path, file: &str) {
    std::fs::write(dir.join(file), file).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", file]);
}

/// A bare "origin" and a clone with one commit on main, pushed.
fn with_remote(tmp: &Path) -> PathBuf {
    let origin = tmp.join("origin.git");
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "-q", "--bare", "-b", "main"]);
    let work = tmp.join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    git(
        &work,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    commit(&work, "a");
    git(&work, &["push", "-q", "-u", "origin", "main"]);
    work
}

fn reach(p: &Path) -> TipReach {
    swamp_core::signals::tip_reach_parallel(&[p.to_path_buf()])
        .pop()
        .unwrap()
}

/// Tempting wrong patch: copy `merged` (no PR means unknown). A pushed
/// audit branch with no PR is reachable from its own remote branch, and
/// the answer names it.
#[test]
fn a_pushed_branch_with_no_pr_is_reachable_and_names_the_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let w = with_remote(tmp.path());
    git(&w, &["checkout", "-q", "-b", "audit/x"]);
    commit(&w, "b");
    assert_eq!(reach(&w), TipReach::NotReachable, "not pushed yet");
    git(&w, &["push", "-q", "-u", "origin", "audit/x"]);
    assert_eq!(reach(&w), TipReach::Reachable("origin/audit/x".into()));
}

/// Tempting wrong patch: only the default branch counts. Work merged into
/// an integration branch that is pushed is reachable from that branch.
#[test]
fn merged_into_a_pushed_integration_branch_is_reachable_from_it() {
    let tmp = tempfile::tempdir().unwrap();
    let w = with_remote(tmp.path());
    git(&w, &["checkout", "-q", "-b", "release/v1"]);
    git(&w, &["push", "-q", "-u", "origin", "release/v1"]);
    git(&w, &["checkout", "-q", "-b", "feat"]);
    commit(&w, "f");
    // The feature commit lands in release/v1 only (fast-forward) and is
    // pushed there; main does not have it.
    git(&w, &["checkout", "-q", "release/v1"]);
    git(&w, &["merge", "-q", "--ff-only", "feat"]);
    git(&w, &["push", "-q", "origin", "release/v1"]);
    git(&w, &["checkout", "-q", "feat"]);
    git(&w, &["branch", "-q", "-D", "release/v1"]);
    assert_eq!(reach(&w), TipReach::Reachable("origin/release/v1".into()));
}

/// Tempting wrong patch: a detached HEAD has no branch, so it is unknown.
/// A detached worktree at a pushed commit is reachable; one on an
/// unpushed commit is not.
#[test]
fn detached_head_is_judged_by_its_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let w = with_remote(tmp.path());
    git(&w, &["checkout", "-q", "--detach"]);
    assert_eq!(reach(&w), TipReach::Reachable("origin/main".into()));
    commit(&w, "d");
    assert_eq!(reach(&w), TipReach::NotReachable);
}

/// Tempting wrong patch: no remote means reachable-or-unknown. With no
/// remote-tracking branch and the commit only on a feature branch the
/// answer is a plain no; the commit on the local default branch is yes.
#[test]
fn no_remote_is_no_unless_the_local_default_branch_has_it() {
    let tmp = tempfile::tempdir().unwrap();
    let w = tmp.path().join("solo");
    std::fs::create_dir_all(&w).unwrap();
    git(&w, &["init", "-q", "-b", "main"]);
    commit(&w, "a");
    git(&w, &["checkout", "-q", "-b", "feat"]);
    commit(&w, "b");
    assert_eq!(reach(&w), TipReach::NotReachable);
    git(&w, &["checkout", "-q", "main"]);
    git(&w, &["merge", "-q", "--ff-only", "feat"]);
    git(&w, &["checkout", "-q", "feat"]);
    assert_eq!(reach(&w), TipReach::Reachable("main".into()));
}

/// Tempting wrong patch: the worktree's own branch counts, so the main
/// checkout is always "reachable from main". It is not a candidate.
#[test]
fn the_checked_out_local_default_branch_is_not_its_own_proof() {
    let tmp = tempfile::tempdir().unwrap();
    let w = tmp.path().join("solo");
    std::fs::create_dir_all(&w).unwrap();
    git(&w, &["init", "-q", "-b", "main"]);
    commit(&w, "a");
    assert_eq!(reach(&w), TipReach::NotReachable);
}

/// Tempting wrong patch: an unopenable directory is "no". It is not
/// established.
#[test]
fn a_directory_that_is_not_a_repository_is_unknown_not_no() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(reach(tmp.path()), TipReach::Unknown);
}

/// Tempting wrong patch: the verdict follows the local fact alone, so a
/// squash-merged PR (tip in no branch) turns `No`; or `merged` is
/// overwritten by the local fact. `merged` stays the PR fact and the term
/// names the branch.
#[test]
fn merge_complete_keeps_merged_as_the_pr_fact_and_names_the_branch() {
    let none = MergedStatus::Unknown;
    let mc = merge_complete(
        Some(false),
        Some(0),
        &none,
        &TipReach::Reachable("origin/audit/x".into()),
    );
    assert!(mc.terms.contains(&"merged=unknown".to_string()), "{mc:?}");
    assert!(
        mc.terms
            .contains(&"tip_reachable=yes (origin/audit/x)".to_string()),
        "{mc:?}"
    );
    assert_eq!(mc.verdict, TriState::Unknown);
    let merged = MergedStatus::Yes {
        merged_at: None,
        pr_number: Some(3),
    };
    let squash = merge_complete(Some(false), Some(0), &merged, &TipReach::NotReachable);
    assert!(squash.terms.contains(&"tip_reachable=no".to_string()));
    assert_eq!(squash.verdict, TriState::Yes);
    let unk = merge_complete(Some(false), Some(0), &merged, &TipReach::Unknown);
    assert!(unk.terms.contains(&"tip_reachable=unknown".to_string()));
}
