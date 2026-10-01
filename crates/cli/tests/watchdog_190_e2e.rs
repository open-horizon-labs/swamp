//! #190 end to end on the built binary (scratch SWAMP_DIR, HOME and
//! SWAMP_LOG_DIR): a pass parked in one directory is stopped, releases
//! the writer lock, names phase and path in the log, and the next pass
//! skips that path as not measured instead of parking again. It replaces
//! the auditor's `a_parked_pass_stops_releases_the_lock_and_names_the_path`,
//! whose FIFO loose ref no longer parks a pass at all.
//!
//! The park is the `SWAMP_TEST_PARK_DIR` test hook: the real blocking
//! kinds (FIFOs, dataless files) no longer park a pass at all, so the
//! stall has to be simulated to test the watchdog.

#![cfg(unix)]

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn observe(
    store: &Path,
    home: &Path,
    logs: &Path,
    park: &Path,
) -> (std::process::Output, Duration) {
    swamp_core::fs_gate::settle::settle();
    let t = Instant::now();
    swamp_core::work_counters::record_spawn();
    let out = Command::new(env!("CARGO_BIN_EXE_swamp"))
        .arg("observe")
        .env("SWAMP_DIR", store)
        .env("HOME", home)
        .env("SWAMP_LOG_DIR", logs)
        .env("SWAMP_LAUNCH_AGENTS_DIR", logs)
        .env("SWAMP_TEST_MODE", "1")
        .env("SWAMP_TEST_PARK_DIR", park)
        .output()
        .unwrap();
    (out, t.elapsed())
}

/// Tempting wrong patch: releasing the lock but recording nothing, so
/// every scheduled pass parks on the same path again.
#[test]
fn a_parked_pass_is_stopped_named_and_skipped_next_time() {
    let tmp = tempfile::tempdir().unwrap();
    let tmp = std::fs::canonicalize(tmp.path()).unwrap();
    let (store, home, logs) = (tmp.join("store"), tmp.join("home"), tmp.join("logs"));
    for d in [&store, &home, &logs] {
        std::fs::create_dir_all(d).unwrap();
    }
    let root = tmp.join("root");
    config(&store, &root, "observe_timeout_sec = 5\n");
    let stuck = root.join("a/stuck");
    std::fs::create_dir_all(&stuck).unwrap();
    std::fs::write(root.join("a/f"), b"x").unwrap();

    let (out, took) = observe(&store, &home, &logs, &stuck);
    assert!(took < Duration::from_secs(30), "{took:?}");
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !store.join("observe.lock").exists(),
        "writer lock left behind"
    );
    let log = std::fs::read_to_string(logs.join("observe.log")).unwrap();
    assert!(
        log.contains(&format!("in walk at {}", stuck.display())),
        "log must name phase and path:\n{log}"
    );

    let (out2, took2) = observe(&store, &home, &logs, &stuck);
    let so = String::from_utf8_lossy(&out2.stdout);
    assert!(
        out2.status.success(),
        "{so}\n{}",
        String::from_utf8_lossy(&out2.stderr)
    );
    assert!(took2 < Duration::from_secs(5), "{took2:?}");
    assert!(so.contains("not measured (stalled on"), "{so}");
    assert!(!so.contains("another observation is running"), "{so}");

    // Stored coverage says not measured, with the reason; never excluded.
    // Tempting wrong patch: pruning it as a scope exclusion, which stores
    // it as `excluded` (coverage-changes-are-not-storage-changes).
    let (cov, unowned) = stored(&store, &home, &logs);
    let row = cov
        .iter()
        .find(|c| c["path"] == stuck.display().to_string())
        .unwrap_or_else(|| panic!("no coverage row for {stuck:?}: {cov:?}"));
    assert_eq!(row["status"], "not-measured", "{row}");
    assert!(
        row["reason"]
            .as_str()
            .is_some_and(|r| r.starts_with("stalled on ")),
        "{row}"
    );
    assert!(
        unowned
            .iter()
            .any(|u| u.to_string().contains(&stuck.display().to_string())
                && u["reason"] == "NotMeasured"),
        "{unowned:?}"
    );
}

fn stored(
    store: &Path,
    home: &Path,
    logs: &Path,
) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
    swamp_core::work_counters::record_spawn();
    let out = Command::new(env!("CARGO_BIN_EXE_swamp"))
        .args(["report", "--json"])
        .env("SWAMP_TEST_MODE", "1")
        .env("SWAMP_DIR", store)
        .env("HOME", home)
        .env("SWAMP_LOG_DIR", logs)
        .env("SWAMP_LAUNCH_AGENTS_DIR", logs)
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let arr = |k: &str| v[k].as_array().cloned().unwrap_or_default();
    (arr("scope_coverage"), arr("unowned"))
}

/// The root is declared in config (not passed on the command line) so a
/// plain `swamp report` reads the same scope back.
fn config(store: &Path, root: &Path, extra: &str) {
    std::fs::write(
        store.join("config.toml"),
        format!(
            "{extra}[scan]\ndefaults = false\ninclude = [\"{}\"]\n",
            root.display()
        ),
    )
    .unwrap();
}

fn mkfifo(path: &Path) {
    let _ = std::fs::remove_file(path);
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
}

/// A repository git cannot safely open (a FIFO loose ref) is stored as
/// not measured with the reason, not only printed. Tempting wrong patch:
/// returning `None` from the open and letting the repo read as having no
/// git signals, with no coverage fact at all.
#[test]
fn a_declined_repository_is_stored_as_not_measured() {
    let tmp = tempfile::tempdir().unwrap();
    let tmp = std::fs::canonicalize(tmp.path()).unwrap();
    let (store, home, logs) = (tmp.join("store"), tmp.join("home"), tmp.join("logs"));
    for d in [&store, &home, &logs] {
        std::fs::create_dir_all(d).unwrap();
    }
    let root = tmp.join("root");
    config(&store, &root, "");
    let repo = root.join("proj");
    std::fs::create_dir_all(repo.join(".git/refs/heads")).unwrap();
    std::fs::create_dir_all(repo.join(".git/objects")).unwrap();
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(
        repo.join(".git/config"),
        "[core]\n\trepositoryformatversion = 0\n",
    )
    .unwrap();
    mkfifo(&repo.join(".git/refs/heads/main"));
    let (out, took) = observe(&store, &home, &logs, &tmp.join("none"));
    assert!(took < Duration::from_secs(30), "{took:?}");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (cov, _) = stored(&store, &home, &logs);
    let row = cov
        .iter()
        .find(|c| c["path"] == repo.display().to_string())
        .unwrap_or_else(|| panic!("no coverage row for the repo: {cov:?}"));
    assert_eq!(row["status"], "not-measured", "{row}");
    assert!(
        row["reason"]
            .as_str()
            .is_some_and(|r| r.starts_with("git repository: ")),
        "{row}"
    );
}

/// #190 was reported under `~/Library/Caches`, where tool caches are
/// measured as external units, not by the project walk. A pass parked
/// inside one (the pip cache here) is stopped and named, and the next pass
/// skips that path there too, stores it as not measured, and finishes.
/// Tempting wrong patch: honoring the quarantine only in the project walk,
/// so the external-unit measurement parks on it again.
#[cfg(target_os = "macos")]
#[test]
fn a_pass_parked_inside_an_external_unit_is_skipped_next_time() {
    let tmp = tempfile::tempdir().unwrap();
    let tmp = std::fs::canonicalize(tmp.path()).unwrap();
    let (store, home, logs) = (tmp.join("store"), tmp.join("home"), tmp.join("logs"));
    for d in [&store, &home, &logs] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(
        store.join("config.toml"),
        "observe_timeout_sec = 5\n[scan]\ndefaults = false\nenabled_detectors = [\"pip\"]\n",
    )
    .unwrap();
    let pip = home.join("Library/Caches/pip");
    let stuck = pip.join("http/stuck");
    std::fs::create_dir_all(&stuck).unwrap();
    std::fs::write(pip.join("http/f"), swamp_core::fs_gate::settle::noise(8192)).unwrap();

    let (out, took) = observe(&store, &home, &logs, &stuck);
    assert!(took < Duration::from_secs(30), "{took:?}");
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let log = std::fs::read_to_string(logs.join("observe.log")).unwrap();
    assert!(
        log.contains(&format!("in walk at {}", stuck.display())),
        "{log}"
    );

    let (out2, took2) = observe(&store, &home, &logs, &stuck);
    let so = String::from_utf8_lossy(&out2.stdout);
    assert!(
        out2.status.success(),
        "{so}\n{}",
        String::from_utf8_lossy(&out2.stderr)
    );
    assert!(took2 < Duration::from_secs(30), "{took2:?}");
    assert!(
        so.contains("external_units=1"),
        "measured as an external unit: {so}"
    );
    assert!(so.contains("not measured (stalled on"), "{so}");
    let (cov, _) = stored(&store, &home, &logs);
    assert!(
        cov.iter()
            .any(|c| c["path"] == stuck.display().to_string() && c["status"] == "not-measured"),
        "{cov:?}"
    );
}
