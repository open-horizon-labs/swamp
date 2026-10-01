//! Adversarial audit of #208: false "yes" (and false "no") cases.
//! computed locally from git's own refs (offline, no `gh`), reported as
//! its own fact with the branch name. Real temp git repositories.
//!
//! Each test names the tempting wrong patch it fails.

use std::path::{Path, PathBuf};
use std::process::Command;

use swamp_core::github::{MergedStatus, merge_complete};
use swamp_core::signals::TipReach;

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

fn out(dir: &Path, args: &[&str]) -> String {
    swamp_core::work_counters::record_spawn();
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

/// Tempting wrong patch: trust any ref that peels. `git replace` rewrites
/// origin/main's parents locally so an unpushed commit looks contained;
/// the remote's real history does not have it. Must not be "yes".
#[test]
fn adv_replace_ref_does_not_fake_reachability() {
    let tmp = tempfile::tempdir().unwrap();
    let w = with_remote(tmp.path());
    git(&w, &["checkout", "-q", "-b", "feat"]);
    commit(&w, "secret");
    let x = out(&w, &["rev-parse", "HEAD"]);
    let a = out(&w, &["rev-parse", "origin/main"]);
    // a fake replacement for `a` whose parent is the unpushed commit x
    let tree = out(&w, &["rev-parse", "origin/main^{tree}"]);
    swamp_core::work_counters::record_spawn();
    let fake = Command::new("git")
        .arg("-C")
        .arg(&w)
        .args(["commit-tree", &tree, "-p", &x, "-m", "fake"])
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@e")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@e")
        .output()
        .unwrap();
    let fake = String::from_utf8_lossy(&fake.stdout).trim().to_string();
    git(&w, &["replace", &a, &fake]);
    let r = reach(&w);
    assert!(
        !matches!(r, TipReach::Reachable(_)),
        "unpushed commit reported reachable via a replace ref: {r:?}"
    );
}

/// Tempting wrong patch: a missing parent in a shallow repository is "no
/// common history", so the answer is a confident "no". Must never be no.
#[test]
fn adv_shallow_boundary_is_unknown_not_no() {
    let tmp = tempfile::tempdir().unwrap();
    let w = with_remote(tmp.path());
    commit(&w, "c");
    git(&w, &["push", "-q", "origin", "main"]);
    git(&w, &["checkout", "-q", "-b", "feat"]);
    commit(&w, "d");
    git(&w, &["push", "-q", "origin", "feat"]);
    let c = out(&w, &["rev-parse", "main"]);
    let origin = tmp.path().join("origin.git");
    let s = tmp.path().join("shallow");
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            "--depth",
            "1",
            "-b",
            "main",
            &format!("file://{}", origin.display()),
            s.to_str().unwrap(),
        ],
    );
    git(
        &s,
        &[
            "fetch",
            "-q",
            "--depth",
            "1",
            "origin",
            "feat:refs/remotes/origin/feat",
        ],
    );
    git(&s, &["checkout", "-q", "--detach", &c]);
    git(&s, &["update-ref", "-d", "refs/remotes/origin/main"]);
    git(&s, &["update-ref", "-d", "refs/remotes/origin/HEAD"]);
    git(&s, &["branch", "-q", "-D", "main"]);
    // c IS the parent of origin/feat upstream; the shallow graft hides it.
    let r = reach(&s);
    assert_ne!(
        r,
        TipReach::NotReachable,
        "shallow repo gave a confident no"
    );
}

/// Tempting wrong patch: a remote-tracking ref is the remote's state. The
/// branch was deleted upstream; the local origin/x still holds HEAD. The
/// term must say the evidence is as of the last fetch.
#[test]
fn adv_stale_tracking_ref_term_says_as_of_last_fetch() {
    let tmp = tempfile::tempdir().unwrap();
    let w = with_remote(tmp.path());
    git(&w, &["checkout", "-q", "-b", "x"]);
    commit(&w, "b");
    git(&w, &["push", "-q", "-u", "origin", "x"]);
    let origin = tmp.path().join("origin.git");
    git(&origin, &["branch", "-q", "-D", "x"]);
    let tip = reach(&w);
    assert!(matches!(tip, TipReach::Reachable(_)), "{tip:?}");
    let mc = merge_complete(Some(false), Some(0), &MergedStatus::Unknown, &tip);
    let term = mc
        .terms
        .iter()
        .find(|t| t.starts_with("tip_reachable"))
        .unwrap();
    assert!(term.contains("fetch"), "term has no evidence age: {term:?}");
}

/// Tempting wrong patch: local main counts as "elsewhere". In a primary
/// checkout (not a linked worktree) on a feature branch, a never-pushed
/// local master lives in the same .git: deleting this directory deletes it.
#[test]
fn adv_unpushed_local_master_in_primary_checkout_is_not_elsewhere() {
    let tmp = tempfile::tempdir().unwrap();
    let w = tmp.path().join("solo");
    std::fs::create_dir_all(&w).unwrap();
    git(&w, &["init", "-q", "-b", "master"]);
    commit(&w, "a");
    git(&w, &["checkout", "-q", "-b", "feat"]);
    let r = reach(&w);
    assert!(
        !matches!(r, TipReach::Reachable(_)),
        "primary checkout reported safe via its own unpushed local branch: {r:?}"
    );
}

/// Must hold: a fork's remote counts and is named by remote.
#[test]
fn adv_other_remote_is_named_by_its_remote() {
    let tmp = tempfile::tempdir().unwrap();
    let w = with_remote(tmp.path());
    let fork = tmp.path().join("fork.git");
    std::fs::create_dir_all(&fork).unwrap();
    git(&fork, &["init", "-q", "--bare"]);
    git(&w, &["remote", "add", "fork", fork.to_str().unwrap()]);
    git(&w, &["checkout", "-q", "-b", "f"]);
    commit(&w, "b");
    git(&w, &["push", "-q", "fork", "f"]);
    assert_eq!(reach(&w), TipReach::Reachable("fork/f".into()));
}

/// Must hold: packed refs and odd names.
#[test]
fn adv_packed_and_odd_ref_names() {
    let tmp = tempfile::tempdir().unwrap();
    let w = with_remote(tmp.path());
    git(&w, &["checkout", "-q", "-b", "we/ird-name.v1_x"]);
    commit(&w, "b");
    git(&w, &["push", "-q", "-u", "origin", "we/ird-name.v1_x"]);
    git(&w, &["pack-refs", "--all"]);
    assert_eq!(
        reach(&w),
        TipReach::Reachable("origin/we/ird-name.v1_x".into())
    );
}

/// Must hold: >400 remote branches, none containing HEAD: Unknown, never no.
#[test]
fn adv_ref_cap_is_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let w = with_remote(tmp.path());
    let a = out(&w, &["rev-parse", "HEAD"]);
    git(&w, &["checkout", "-q", "-b", "f"]);
    commit(&w, "b");
    let mut s = String::new();
    for i in 0..450 {
        s.push_str(&format!("{a} refs/remotes/origin/b{i:03}\n"));
    }
    std::fs::write(
        w.join(".git/packed-refs"),
        format!("# pack-refs with: peeled fully-peeled sorted\n{s}"),
    )
    .unwrap();
    git(&w, &["update-ref", "-d", "refs/remotes/origin/main"]);
    assert_eq!(reach(&w), TipReach::Unknown);
}
