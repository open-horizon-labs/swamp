//! Ownership of the shared external/agent current table
//! (`.oh/guardrails/history-sweeps-are-owned.md`).
//!
//! External units and agent units deliberately share one key family in
//! one store -- one store, one key scheme, two granularities. The
//! 2026-09-21 review showed what that costs when neither observation
//! declares what it owns: each swept the table for keys it had not seen,
//! so each tombstoned the other's rows, and the next pass recorded the
//! resurrection as regrowth on a filesystem where nothing had moved.
//!
//! These tests pin the repaired contract from the outside: a sweep may
//! only mark absent a row of its own family, inside a region it actually
//! covered this pass.
//!
//! Every fixture is a disposable `tempfile` tree. Nothing reads a real
//! home, a real tool store, or the user's own swamp state.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::scope::{EffectiveScope, ScanConfig, resolve_effective_scope};

/// Explicit-only scope with exactly the named detectors curated in. The
/// deny-list form is what makes `defaults = false` still run something
/// (see `.oh/guardrails/explicit-only-scope-when-defaults-false.md`).
fn only(keep: &[&str]) -> ScanConfig {
    let registry = Registry::with_builtins();
    ScanConfig {
        defaults: false,
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .filter(|id| !keep.contains(&id.as_str()))
            .collect(),
        ..Default::default()
    }
}

fn claude_scope(tmp: &Path, home: &Path, cfg: ScanConfig) -> EffectiveScope {
    let env = Environment::fixture(
        tmp.to_path_buf(),
        HashMap::from([("CLAUDE_CONFIG_DIR".to_string(), home.display().to_string())]),
        Platform::MacOS,
    );
    resolve_effective_scope(&env, &cfg, &[], &Registry::with_builtins(), 1_000)
}

fn claude_home(tmp: &Path) -> std::path::PathBuf {
    let home = tmp.join("claude");
    fs::create_dir_all(home.join("debug")).unwrap();
    fs::write(home.join("debug/log.txt"), b"original").unwrap();
    home
}

/// `(path, regrowth_count, bytes)` per unit: enough to compare two
/// orderings for *identity*, not just for "no regrowth".
type UnitRows = Vec<(String, u32, u64)>;

/// Both families' rows from one pass.
fn observe(
    scope: &EffectiveScope,
    store: &Path,
    at: u64,
    agent_first: bool,
) -> (UnitRows, UnitRows) {
    let (ext, agents) = if agent_first {
        let a = swamp_core::agents::discover_and_measure(
            scope,
            &[],
            Some(store),
            true,
            at,
            30,
            3600,
            &swamp_core::fs_events::EventCoverage::untrusted(),
        )
        .unwrap();
        let e = swamp_core::external::discover_and_measure(
            scope,
            Some(store),
            true,
            at,
            30,
            3600,
            &swamp_core::fs_events::EventCoverage::untrusted(),
        )
        .unwrap();
        (e, a)
    } else {
        let e = swamp_core::external::discover_and_measure(
            scope,
            Some(store),
            true,
            at,
            30,
            3600,
            &swamp_core::fs_events::EventCoverage::untrusted(),
        )
        .unwrap();
        let a = swamp_core::agents::discover_and_measure(
            scope,
            &[],
            Some(store),
            true,
            at,
            30,
            3600,
            &swamp_core::fs_events::EventCoverage::untrusted(),
        )
        .unwrap();
        (e, a)
    };
    let mut e: UnitRows = ext
        .iter()
        .map(|u| (u.path.display().to_string(), u.regrowth_count, u.bytes))
        .collect();
    let mut a: UnitRows = agents
        .iter()
        .map(|u| (u.path.display().to_string(), u.regrowth_count, u.bytes))
        .collect();
    e.sort();
    a.sort();
    (e, a)
}

#[test]
fn both_observation_orders_yield_zero_regrowth_and_identical_rows() {
    // The review's counterexample ran external-then-agent; the TUI's
    // startup ran exactly that order. If ownership is right, the order
    // cannot be observable at all.
    let mut results = Vec::new();
    for agent_first in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let home = claude_home(tmp.path());
        let scope = claude_scope(tmp.path(), &home, only(&["claude-code"]));
        let store = tempfile::tempdir().unwrap();
        let mut per_pass = Vec::new();
        for at in [1_000u64, 2_000, 3_000] {
            let (ext, agents) = observe(&scope, store.path(), at, agent_first);
            for (path, regrowth, _) in ext.iter().chain(agents.iter()) {
                assert_eq!(
                    *regrowth, 0,
                    "nothing moved on disk, so nothing may have regrown: {path} \
                     (agent_first={agent_first}, pass at {at})"
                );
            }
            // Byte totals are comparable across orders; absolute paths
            // are not (each order gets its own tempdir).
            per_pass.push((
                ext.iter().map(|r| r.2).collect::<Vec<_>>(),
                agents.iter().map(|r| r.2).collect::<Vec<_>>(),
            ));
        }
        results.push(per_pass);
    }
    assert_eq!(
        results[0], results[1],
        "the two observation orders must produce identical stored values"
    );
}

#[test]
fn disabling_a_detector_between_observations_leaves_its_rows_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let home = claude_home(tmp.path());
    let store = tempfile::tempdir().unwrap();

    let enabled = claude_scope(tmp.path(), &home, only(&["claude-code"]));
    let first = observe(&enabled, store.path(), 1_000, false);
    assert!(
        !first.1.is_empty(),
        "precondition: the enabled detector must identify agent units"
    );

    // The detector is turned off; its storage is untouched on disk.
    let disabled = claude_scope(tmp.path(), &home, only(&[]));
    let (ext, agents) = observe(&disabled, store.path(), 2_000, false);
    assert!(
        ext.is_empty() && agents.is_empty(),
        "a disabled detector contributes no units: {ext:?} {agents:?}"
    );

    // Re-enabling must not look like the storage came back. A coverage
    // change is not a storage change.
    let (_, agents) = observe(&enabled, store.path(), 3_000, false);
    for (path, regrowth, _) in &agents {
        assert_eq!(
            *regrowth, 0,
            "turning a detector off and on again is a coverage change, not a deletion and a \
             recreation: {path}"
        );
    }
}

#[test]
fn excluding_a_home_between_observations_leaves_its_rows_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let home = claude_home(tmp.path());
    let store = tempfile::tempdir().unwrap();

    let included = claude_scope(tmp.path(), &home, only(&["claude-code"]));
    let first = observe(&included, store.path(), 1_000, false);
    assert!(!first.1.is_empty(), "precondition: units identified");

    let excluded_cfg = ScanConfig {
        exclude: vec![home.display().to_string()],
        ..only(&["claude-code"])
    };
    let excluded = claude_scope(tmp.path(), &home, excluded_cfg);
    let (ext, agents) = observe(&excluded, store.path(), 2_000, false);
    assert!(ext.is_empty() && agents.is_empty(), "excluded home scanned");

    let (_, agents) = observe(&included, store.path(), 3_000, false);
    for (path, regrowth, _) in &agents {
        assert_eq!(*regrowth, 0, "exclusion is not deletion: {path}");
    }
}

#[test]
fn a_real_delete_and_recreate_yields_exactly_one_regrowth() {
    // The other half of the contract: ownership must not make the store
    // blind. A unit that genuinely disappears and comes back has
    // regrown, exactly once.
    let tmp = tempfile::tempdir().unwrap();
    let home = claude_home(tmp.path());
    let scope = claude_scope(tmp.path(), &home, only(&["claude-code"]));
    let store = tempfile::tempdir().unwrap();

    let (_, agents) = observe(&scope, store.path(), 1_000, false);
    let target = agents
        .iter()
        .find(|(p, _, _)| p.ends_with("debug"))
        .map(|(p, _, _)| p.clone())
        .expect("the fixture's debug cache must be identified");

    fs::remove_dir_all(home.join("debug")).unwrap();
    let (_, agents) = observe(&scope, store.path(), 2_000, false);
    assert!(
        !agents.iter().any(|(p, _, _)| *p == target),
        "a removed unit must not still be reported"
    );

    fs::create_dir_all(home.join("debug")).unwrap();
    fs::write(home.join("debug/log.txt"), b"recreated").unwrap();
    let (_, agents) = observe(&scope, store.path(), 3_000, false);
    let regrowth = agents
        .iter()
        .find(|(p, _, _)| *p == target)
        .map(|(_, r, _)| *r)
        .expect("the recreated unit must be identified again");
    assert_eq!(
        regrowth, 1,
        "a genuine delete-then-recreate is exactly one regrowth, not zero and not two"
    );
}
