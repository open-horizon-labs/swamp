//! #190 end to end on the built binary (scratch SWAMP_DIR, HOME and
//! SWAMP_LOG_DIR): a pass parked in one directory is stopped, releases
//! the writer lock, names phase and path in the log, and the next pass
//! skips that path as not measured instead of parking again.
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
    root: &Path,
    park: &Path,
) -> (std::process::Output, Duration) {
    let t = Instant::now();
    swamp_core::work_counters::record_spawn();
    let out = Command::new(env!("CARGO_BIN_EXE_swamp"))
        .arg("observe")
        .arg(root)
        .env("SWAMP_DIR", store)
        .env("HOME", home)
        .env("SWAMP_LOG_DIR", logs)
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
    std::fs::write(store.join("config.toml"), "observe_timeout_sec = 5\n").unwrap();
    let root = tmp.join("root");
    let stuck = root.join("a/stuck");
    std::fs::create_dir_all(&stuck).unwrap();
    std::fs::write(root.join("a/f"), b"x").unwrap();

    let (out, took) = observe(&store, &home, &logs, &root, &stuck);
    assert!(took < Duration::from_secs(30), "{took:?}");
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let log = std::fs::read_to_string(logs.join("observe.log")).unwrap();
    assert!(
        log.contains(&format!("in walk at {}", stuck.display())),
        "log must name phase and path:\n{log}"
    );

    let (out2, took2) = observe(&store, &home, &logs, &root, &stuck);
    let so = String::from_utf8_lossy(&out2.stdout);
    assert!(
        out2.status.success(),
        "{so}\n{}",
        String::from_utf8_lossy(&out2.stderr)
    );
    assert!(took2 < Duration::from_secs(5), "{took2:?}");
    assert!(so.contains("not measured (stalled on"), "{so}");
    assert!(!so.contains("another observation is running"), "{so}");
}
