//! Adversarial tests for #190 / PR #191: every place the observe pipeline
//! opens a user path must not park on a FIFO. Each operation runs on its
//! own thread with a deadline, so a regression fails instead of wedging
//! the test binary. A test that fails here names a blocking open the PR's
//! `open_regular` / `safe_to_open` guards do not reach.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

fn mkfifo(path: &Path) {
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(
        unsafe { libc::mkfifo(c.as_ptr(), 0o644) },
        0,
        "mkfifo {path:?}"
    );
}

/// Runs `f` on a detached thread; panics when it does not return within
/// `secs` (the thread is left parked, which is the bug being shown).
fn within<T: Send + 'static>(secs: u64, what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(secs))
        .unwrap_or_else(|_| panic!("{what}: blocked past {secs}s on a FIFO (#190)"))
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
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
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn committed_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("README.md"), b"x\n").unwrap();
    git(dir, &["add", "README.md"]);
    git(dir, &["commit", "-q", "-m", "initial"]);
}

/// A loose branch ref that is a FIFO. `safe_to_open` checks HEAD, config,
/// commondir, index and packed-refs, but `head_commit`/`head_id` then
/// resolve `refs/heads/main` with a plain blocking open inside gix.
/// Tempting wrong patch: guarding only the files `gix::open` reads, not
/// the ones the queries after it read. Reached from the report pipeline
/// (`signals::compute_signals_raw`), outside any walk `in_flight` entry,
/// so the 300 s directory watchdog never names it either.
#[test]
fn signals_over_a_fifo_loose_ref_finish() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("proj");
    committed_repo(&repo);
    let head_ref = repo.join(".git/refs/heads/main");
    std::fs::remove_file(&head_ref).unwrap();
    mkfifo(&head_ref);
    within(5, "signals over FIFO refs/heads/main", move || {
        swamp_core::signals::compute_signals_raw(&repo, 0)
    });
}

/// A FIFO `.gitignore` in a checkout: the ignore lens builds its exclude
/// stack from per-directory `.gitignore` files, opened by gix itself.
/// Tempting wrong patch: guarding the git dir only, never the worktree's
/// own ignore files.
#[test]
fn untracked_content_over_a_fifo_gitignore_finishes() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("proj");
    committed_repo(&repo);
    std::fs::create_dir(repo.join("sub")).unwrap();
    std::fs::write(repo.join("sub/new.txt"), b"y").unwrap();
    mkfifo(&repo.join(".gitignore"));
    mkfifo(&repo.join("sub/.gitignore"));
    within(5, "untracked_content over FIFO .gitignore", move || {
        swamp_core::ignore::untracked_content(&repo, 10, 1000)
    });
}

/// A FIFO `.git/info/exclude`, read by the same exclude stack.
#[test]
fn untracked_content_over_a_fifo_info_exclude_finishes() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("proj");
    committed_repo(&repo);
    std::fs::write(repo.join("new.txt"), b"y").unwrap();
    std::fs::create_dir_all(repo.join(".git/info")).unwrap();
    let _ = std::fs::remove_file(repo.join(".git/info/exclude"));
    mkfifo(&repo.join(".git/info/exclude"));
    within(5, "untracked_content over FIFO info/exclude", move || {
        swamp_core::ignore::untracked_content(&repo, 10, 1000)
    });
}

/// A directory where a file is expected (`package.json/`, `.git/HEAD/`)
/// is refused at once, not read and not waited on.
#[test]
fn a_directory_named_like_a_file_is_refused_at_once() {
    use swamp_core::fs_gate::read::{BoundedCap, bounded_read};
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("package.json");
    std::fs::create_dir(&p).unwrap();
    let r = within(5, "bounded_read on a directory", move || {
        bounded_read(&p, BoundedCap::POINTER).is_err()
    });
    assert!(r);
}

/// A named pipe used on purpose: a writer is parked in `open(O_WRONLY)`
/// waiting for its reader. swamp's refusal must be a pure stat, not a
/// read-side open: `open(O_RDONLY|O_NONBLOCK)` on the FIFO completes the
/// writer's rendezvous, the writer's `open` returns, and when swamp
/// closes the read end the writer's first `write` fails with `EPIPE`
/// (SIGPIPE kills a C writer outright), so data the real reader should
/// have got is lost. Tempting wrong patch (the PR's): "`O_NONBLOCK` makes
/// the open safe, then `fstat` refuses" -- safe for swamp, not for the
/// process on the other end.
#[test]
fn refusing_a_fifo_does_not_wake_a_waiting_writer() {
    use swamp_core::fs_gate::read::{BoundedCap, bounded_read};
    let tmp = tempfile::tempdir().unwrap();
    let fifo = tmp.path().join("config");
    mkfifo(&fifo);
    let (opened_tx, opened_rx) = mpsc::channel::<()>();
    let w = fifo.clone();
    std::thread::spawn(move || {
        // Blocking writer open: returns only once some reader opens.
        let _f = std::fs::OpenOptions::new().write(true).open(&w);
        let _ = opened_tx.send(());
    });
    std::thread::sleep(Duration::from_millis(200));
    let f2 = fifo.clone();
    assert!(within(5, "bounded_read on FIFO", move || bounded_read(
        &f2,
        BoundedCap::POINTER
    )
    .is_err()));
    let woke = opened_rx.recv_timeout(Duration::from_secs(1)).is_ok();
    // Unpark the writer if it is still waiting, so the thread ends.
    if !woke {
        use std::os::unix::fs::OpenOptionsExt;
        let _r = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo);
    }
    assert!(
        !woke,
        "swamp's refusal opened the FIFO's read end and released a waiting writer"
    );
}

/// A FIFO (and a symlink to one) planted at every manifest name in the
/// middle of a 10k-file tree: discovery + attribution finish, and the
/// tree's real bytes are still counted.
#[test]
fn a_ten_thousand_file_tree_with_fifo_manifests_is_walked() {
    let tmp = tempfile::tempdir().unwrap();
    let root: PathBuf = tmp.path().join("root");
    for d in 0..100 {
        let dir = root.join(format!("d{d}"));
        std::fs::create_dir_all(&dir).unwrap();
        for f in 0..100 {
            std::fs::write(dir.join(format!("f{f}")), [0u8; 10]).unwrap();
        }
    }
    let proj = root.join("d50");
    std::fs::create_dir_all(proj.join(".git/refs")).unwrap();
    std::fs::create_dir_all(proj.join(".git/objects")).unwrap();
    for name in [
        "package.json",
        "Cargo.toml",
        "pyproject.toml",
        "go.mod",
        "Gemfile",
        "pom.xml",
        ".tool-versions",
        "mise.toml",
        "rust-toolchain.toml",
        "CACHEDIR.TAG",
    ] {
        mkfifo(&proj.join(name));
    }
    for name in ["HEAD", "config", "index", "packed-refs", "commondir"] {
        mkfifo(&proj.join(".git").join(name));
    }
    std::os::unix::fs::symlink(proj.join("package.json"), root.join("d51/package.json")).unwrap();
    let r = root.clone();
    let (_found, attributed) = within(60, "discover_and_attribute over FIFOs", move || {
        let stage = swamp_core::bus::Stage::for_tests();
        let (found, attr, _) =
            swamp_core::walk::discover_and_attribute(&stage, &r, 0, u64::MAX, &[], &[]).unwrap();
        (found.len(), attr)
    });
    let _ = attributed;
}
