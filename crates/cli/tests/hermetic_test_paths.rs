//! Test hermeticity: fixtures once appended thousands of
//! `projects=0 mode=full` lines to the maintainer's real
//! `~/Library/Logs/swamp/observe.log`, because the CLI tests set
//! `SWAMP_DIR` but neither `HOME` nor `SWAMP_LOG_DIR`, so the log fell
//! through to the real per-user default. Three guards keep it from
//! coming back.

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_swamp"))
}

/// Every file under `dir`, relative to it.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(p);
            } else {
                out.push(p.strip_prefix(dir).unwrap().to_path_buf());
            }
        }
    }
    out.sort();
    out
}

/// Tempting wrong patch: set `SWAMP_LOG_DIR` in the one failing test
/// and leave the default resolution alone, so the next test that forgets
/// it writes to the real log again. The binary itself refuses: under
/// `SWAMP_TEST_MODE=1` a log directory that resolves outside the temp dir
/// panics before the observation starts, and nothing is written.
#[test]
fn a_test_mode_observe_that_would_write_the_real_users_log_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("src");
    std::fs::create_dir_all(&root).unwrap();
    let out = Command::new(bin())
        .arg("observe")
        .arg(&root)
        .env("SWAMP_DIR", tmp.path().join("store"))
        .env_remove("SWAMP_LOG_DIR")
        // Not a temp dir: stands in for the real home without touching it.
        .env("HOME", "/nonexistent-swamp-test-home")
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .unwrap();
    assert!(!out.status.success(), "the guard did not fire");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("test hermeticity"), "{stderr}");
    // Setup tables may exist; no observation was measured or recorded.
    let store_files = files_under(&tmp.path().join("store"));
    for table in ["runs.parquet", "scheduled_runs.parquet", "projects.parquet"] {
        assert!(
            !store_files.iter().any(|f| f.ends_with(table)),
            "the observation ran before the guard fired: {store_files:?}"
        );
    }
}

/// Tempting wrong patch: point only the store at the temp dir and trust
/// that nothing else is written. A fixture observe with every location
/// overridden writes its log, store and ledger inside its own temp dir
/// and nowhere under the (temp) home.
#[test]
fn a_fixture_observe_writes_only_inside_its_temp_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("src/app");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(root.join("node_modules/x")).unwrap();
    std::fs::write(root.join("package.json"), "{}").unwrap();
    std::fs::write(root.join("node_modules/x/a.js"), "x").unwrap();
    let logs = tmp.path().join("logs");
    let out = Command::new(bin())
        .arg("observe")
        .arg(tmp.path().join("src"))
        .env("SWAMP_DIR", tmp.path().join("store"))
        .env("SWAMP_LOG_DIR", &logs)
        .env("HOME", &home)
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let log = std::fs::read_to_string(logs.join("observe.log")).unwrap();
    assert_eq!(
        log.lines()
            .filter(|l| l.starts_with("observed_at="))
            .count(),
        1
    );
    assert_eq!(
        files_under(&home),
        Vec::<PathBuf>::new(),
        "an observe wrote under HOME"
    );
}

/// Tempting wrong patch: a new CLI test that spawns the binary without
/// `SWAMP_TEST_MODE=1`, so the hermeticity guard never runs for it. Every
/// spawn of the swamp binary in this crate's tests sets it.
#[test]
fn every_cli_test_spawn_of_swamp_sets_test_mode() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut offenders = Vec::new();
    for f in files_under(&dir) {
        if f.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(dir.join(&f)).unwrap();
        for (i, _) in text.match_indices("Command::new(") {
            let call = &text[i..];
            let is_swamp =
                call.starts_with("Command::new(bin())") || call.starts_with("Command::new(env!(");
            if !is_swamp {
                continue;
            }
            // The builder chain, up to the statement end, or the next
            // few lines for a `let mut cmd` built over several statements.
            let chain: String = call.lines().take(16).collect::<Vec<_>>().join("\n");
            if !chain.contains("SWAMP_TEST_MODE") {
                let line = text[..i].lines().count() + 1;
                offenders.push(format!("{}:{line}", f.display()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "spawns without SWAMP_TEST_MODE: {offenders:?}"
    );
}
