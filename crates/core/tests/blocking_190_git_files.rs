//! #190: every file gix may open for a repository is covered by the
//! pre-open sweep of the git dir. One test per file kind the review
//! listed, plus a generic one that plants a FIFO at every loose file of
//! a real `.git`. Each runs under a 5 s deadline so a regression fails
//! instead of hanging.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

fn mkfifo(path: &Path) {
    let _ = std::fs::remove_file(path);
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0, "{path:?}");
}

fn within<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|_| panic!("{what}: blocked past 5s on a FIFO (#190)"))
}

fn git(dir: &Path, args: &[&str]) {
    swamp_core::work_counters::record_spawn();
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "f")
        .env("GIT_AUTHOR_EMAIL", "f@example.com")
        .env("GIT_COMMITTER_NAME", "f")
        .env("GIT_COMMITTER_EMAIL", "f@example.com")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn repo(tmp: &Path) -> PathBuf {
    let r = tmp.join("proj");
    std::fs::create_dir_all(&r).unwrap();
    git(&r, &["init", "-q", "-b", "main"]);
    std::fs::write(r.join("a"), b"x").unwrap();
    git(&r, &["add", "a"]);
    git(&r, &["commit", "-q", "-m", "i"]);
    git(&r, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    r
}

fn signals_finish_with_fifo_at(rel: &str) {
    let tmp = tempfile::tempdir().unwrap();
    let r = repo(tmp.path());
    let p = r.join(".git").join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    mkfifo(&p);
    let what = format!("signals with FIFO .git/{rel}");
    within(&what, move || {
        let _ = swamp_core::signals::compute_signals_raw(&r, 0);
        let _ = swamp_core::ignore::untracked_content(&r, 10, 1000);
    });
}

/// Tempting wrong patch: a fixed list of files `gix::open` reads; the
/// ref lookups after it then open these themselves.
#[test]
fn fifo_remote_tracking_ref() {
    signals_finish_with_fifo_at("refs/remotes/origin/main");
}

#[test]
fn fifo_reflog_head() {
    signals_finish_with_fifo_at("logs/HEAD");
}

#[test]
fn fifo_objects_info_alternates() {
    signals_finish_with_fifo_at("objects/info/alternates");
}

#[test]
fn fifo_config_worktree() {
    signals_finish_with_fifo_at("config.worktree");
}

/// A socket in `.git` (git's fsmonitor daemon keeps one) does not block
/// an open, so it must not decline the repository. Tempting wrong patch:
/// declining every non-regular entry, which would stop measuring every
/// checkout that runs `core.fsmonitor`.
#[test]
fn a_socket_in_the_git_dir_does_not_decline_the_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let r = repo(tmp.path());
    // Short path: sun_path is ~104 bytes.
    let sock = r.join(".git/s");
    let _l = std::os::unix::net::UnixListener::bind(&sock)
        .or_else(|_| {
            let short = std::env::temp_dir().join(format!("s190-{}", std::process::id()));
            let l = std::os::unix::net::UnixListener::bind(&short)?;
            std::fs::rename(&short, &sock).map(|_| l)
        })
        .unwrap();
    let (sigs, _) = within("signals with a socket in .git", move || {
        swamp_core::signals::compute_signals_raw(&r, 0)
    });
    assert!(
        sigs.iter()
            .any(|s| s.name == "last_commit" && !s.value.contains("unknown")),
        "{sigs:?}"
    );
}

/// Generic: a FIFO at every loose file of a real `.git` (refs, logs,
/// hooks samples, info, description, HEAD, config, index), one at a
/// time; each pass finishes.
#[test]
fn a_fifo_at_every_loose_git_file_never_parks_the_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let template = repo(tmp.path());
    let mut files = Vec::new();
    let mut stack = vec![template.join(".git")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            let rel = p.strip_prefix(template.join(".git")).unwrap().to_path_buf();
            if rel.starts_with("objects") && !rel.starts_with("objects/info") {
                continue;
            }
            if e.file_type().unwrap().is_dir() {
                stack.push(p);
            } else {
                files.push(rel);
            }
        }
    }
    assert!(files.len() > 10, "{files:?}");
    for rel in files {
        let run = tempfile::tempdir().unwrap();
        let r = run.path().join("proj");
        let ok = Command::new("cp").arg("-R").arg(&template).arg(&r).status();
        assert!(ok.unwrap().success());
        mkfifo(&r.join(".git").join(&rel));
        let root = run.path().to_path_buf();
        within(
            &format!("observe pipeline with FIFO .git/{}", rel.display()),
            move || {
                let stage = swamp_core::bus::Stage::for_tests();
                let _ =
                    swamp_core::walk::discover_and_attribute(&stage, &root, 0, u64::MAX, &[], &[]);
                let _ = swamp_core::signals::compute_signals_raw(&r, 0);
                let _ = swamp_core::ignore::untracked_content(&r, 10, 1000);
            },
        );
    }
}
