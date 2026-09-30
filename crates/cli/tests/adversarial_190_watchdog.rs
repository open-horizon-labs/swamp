//! Adversarial end-to-end checks of the #190 watchdog on the built
//! binary, with a scratch SWAMP_DIR, HOME and SWAMP_LOG_DIR. A blocked
//! worker is produced by a FIFO the PR's refusal does not reach (a loose
//! `refs/heads/main`), so the pass really parks in `open(2)`.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_swamp"))
}

fn git(dir: &Path, args: &[&str]) {
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
    assert!(ok);
}

fn mkfifo(path: &Path) {
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
}

fn observe(
    store: &Path,
    home: &Path,
    logs: &Path,
    root: &Path,
) -> (std::process::Output, Duration) {
    let t = Instant::now();
    let out = Command::new(bin())
        .arg("observe")
        .arg(root)
        .env("SWAMP_DIR", store)
        .env("HOME", home)
        .env("SWAMP_LOG_DIR", logs)
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .unwrap();
    (out, t.elapsed())
}

/// A pass parked on a FIFO stops at `observe_timeout_sec`, exits 1,
/// releases the writer lock (a second observe of a clean root runs to
/// `ok`), and the log line names where it was stuck. Tempting wrong
/// patch: tracking only walk directories in `in_flight`; a pass parked
/// in the report pipeline (git signals, ignore lens) then logs a bare
/// `timeout` with no path, which is what #190 was missing.
#[test]
fn a_parked_pass_stops_releases_the_lock_and_names_the_path() {
    let tmp = tempfile::tempdir().unwrap();
    let (store, home, logs) = (
        tmp.path().join("store"),
        tmp.path().join("home"),
        tmp.path().join("logs"),
    );
    for d in [&store, &home, &logs] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(store.join("config.toml"), "observe_timeout_sec = 5\n").unwrap();
    let root = tmp.path().join("root");
    let repo = root.join("proj");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a"), b"x").unwrap();
    git(&repo, &["add", "a"]);
    git(&repo, &["commit", "-q", "-m", "i"]);
    let r = repo.join(".git/refs/heads/main");
    std::fs::remove_file(&r).unwrap();
    mkfifo(&r);

    let (out, took) = observe(&store, &home, &logs, &root);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        took < Duration::from_secs(30),
        "observe did not stop: {took:?}"
    );
    assert_eq!(out.status.code(), Some(1), "stderr: {stderr}");
    assert!(!store.join("observe.lock").exists(), "lock left behind");

    let clean = tmp.path().join("clean");
    std::fs::create_dir_all(&clean).unwrap();
    std::fs::write(clean.join("f"), b"x").unwrap();
    let (out2, _) = observe(&store, &home, &logs, &clean);
    let so = String::from_utf8_lossy(&out2.stdout);
    assert!(!so.contains("another observation is running"), "{so}");
    assert!(
        out2.status.success(),
        "{}",
        String::from_utf8_lossy(&out2.stderr)
    );

    let log = std::fs::read_to_string(logs.join("observe.log")).unwrap();
    eprintln!("observe.log:\n{log}\nstderr: {stderr}");
    assert!(
        log.contains("timeout(stuck"),
        "the stopped pass must name where it was stuck; log:\n{log}"
    );
}
