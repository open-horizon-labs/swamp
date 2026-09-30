//! A project worktree that lives inside an external unit (the case that
//! matters: an agent's scratch directory holding worktrees with build
//! output) must be counted once, under its project, and the unit must
//! say how much of what is inside it is counted elsewhere.
//!
//! Tempting wrong patches these fail: (1) leaving the containing unit
//! whole, so the worktree's bytes are in both totals; (2) subtracting
//! the worktree from the unit without saying so, so the row's total
//! silently disagrees with `du`; (3) dropping the worktree from project
//! discovery instead of the unit, which loses the project's own row.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use swamp_core::external::{ExternalUnit, NestedWorktree, discover_and_measure_with_worktrees};
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::scope::{ScanConfig, resolve_effective_scope};

fn du_bytes(p: &Path) -> u64 {
    let out = std::process::Command::new("du")
        .args(["-skx"])
        .arg(p)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .parse::<u64>()
        .unwrap()
        * 1024
}

fn measure(scratch: &Path, home: &Path, wts: &[NestedWorktree]) -> ExternalUnit {
    let mut env_vars: HashMap<String, String> = HashMap::new();
    env_vars.insert("CARGO_HOME".into(), scratch.display().to_string());
    let env = Environment::fixture(home.to_path_buf(), env_vars, Platform::MacOS);
    let registry = Registry::with_builtins();
    let cfg = ScanConfig {
        defaults: false,
        include: Vec::new(),
        exclude: Vec::new(),
        disabled_detectors: Vec::new(),
        enabled_detectors: vec!["cargo-home".into()],
    };
    let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1);
    let store = tempfile::tempdir().unwrap();
    let units = discover_and_measure_with_worktrees(
        &scope,
        wts,
        Some(store.path()),
        true,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    units
        .into_iter()
        .find(|u| u.path == fs::canonicalize(scratch).unwrap())
        .expect("the containing unit is measured")
}

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let scratch = home.path().join("scratch");
    fs::create_dir_all(&scratch).unwrap();
    fs::write(scratch.join("session-note.json"), vec![1u8; 3 << 20]).unwrap();
    let wt = scratch.join("wt-feature");
    fs::create_dir_all(wt.join("target")).unwrap();
    fs::write(wt.join("target/artifact.bin"), vec![2u8; 16 << 20]).unwrap();
    // 1 GiB apparent, almost nothing allocated: allocated accounting.
    let sparse = fs::File::create(wt.join("target/sparse.bin")).unwrap();
    sparse.set_len(1 << 30).unwrap();
    (home, scratch, wt)
}

#[test]
fn a_worktree_inside_a_unit_is_counted_once_and_the_unit_names_the_overlap() {
    let (home, scratch, wt) = fixture();
    let whole = measure(&scratch, home.path(), &[]);
    let wt_bytes = du_bytes(&wt);
    let split = measure(
        &scratch,
        home.path(),
        &[NestedWorktree {
            path: wt.clone(),
            reported_bytes: wt_bytes,
        }],
    );
    // Without the worktree list the unit still holds everything.
    assert!(whole.bytes >= wt_bytes, "{} < {wt_bytes}", whole.bytes);
    assert!(whole.note.is_none());
    // With it, the worktree's bytes are gone from the unit ...
    assert!(
        split.bytes + wt_bytes <= whole.bytes + (1 << 20),
        "unit {} + worktree {wt_bytes} exceeds the whole {}",
        split.bytes,
        whole.bytes
    );
    // ... unit + worktree equals du of the directory, within a directory
    // block or two ...
    let du_total = du_bytes(&scratch);
    let sum = split.bytes + wt_bytes;
    let diff = sum.abs_diff(du_total);
    assert!(diff <= 64 * 1024, "sum {sum} vs du {du_total}: {diff}");
    // ... and the row says so instead of silently disagreeing with du.
    let note = split.note.expect("the overlap is named on the row");
    assert!(
        note.contains("counted under projects") && note.contains("1 worktree"),
        "{note}"
    );
}

#[test]
fn a_worktree_outside_the_unit_changes_nothing() {
    let (home, scratch, _wt) = fixture();
    let elsewhere = tempfile::tempdir().unwrap();
    let whole = measure(&scratch, home.path(), &[]);
    let same = measure(
        &scratch,
        home.path(),
        &[NestedWorktree {
            path: elsewhere.path().to_path_buf(),
            reported_bytes: 123,
        }],
    );
    assert_eq!(whole.bytes, same.bytes);
    assert!(same.note.is_none());
}

#[test]
fn a_worktree_that_is_the_unit_itself_is_not_subtracted_to_zero() {
    // Tempting wrong patch: `starts_with` without the strict-inside
    // test, which makes a unit that IS a worktree vanish.
    let (home, scratch, _wt) = fixture();
    let whole = measure(&scratch, home.path(), &[]);
    let same = measure(
        &scratch,
        home.path(),
        &[NestedWorktree {
            path: scratch.clone(),
            reported_bytes: whole.bytes,
        }],
    );
    assert_eq!(whole.bytes, same.bytes);
}
