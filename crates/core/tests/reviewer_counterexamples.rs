//! Independent-review counterexamples that are not about the CLI/agent
//! action path (deleted 2026-09-23, "swamp reports; the human removes").
//! The three counterexamples this file used to also carry --
//! `protection_added_after_approval_must_stop_execution`,
//! `replacement_directory_must_not_spend_old_approval`,
//! `open_cache_member_must_stop_parent_removal` -- asserted an
//! execute-time recheck refusal that the product decision explicitly
//! removes (no re-derivation between marking and moving; only OS errors
//! refuse). They are deleted, not adapted, because the behavior they
//! required no longer exists by design.

use std::{collections::HashMap, fs};
use swamp_core::{
    actions, agents, external,
    locations::{Environment, Platform, Registry},
    scope::{ScanConfig, resolve_effective_scope},
};

fn only_claude() -> ScanConfig {
    ScanConfig {
        defaults: false,
        disabled_detectors: Registry::with_builtins()
            .detectors()
            .iter()
            .filter(|d| d.id() != "claude-code")
            .map(|d| d.id().to_string())
            .collect(),
        ..Default::default()
    }
}

fn fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    swamp_core::scope::EffectiveScope,
) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("claude");
    fs::create_dir_all(home.join("debug")).unwrap();
    fs::write(home.join("debug/log.txt"), b"original").unwrap();
    let env = Environment::fixture(
        tmp.path().to_owned(),
        HashMap::from([("CLAUDE_CONFIG_DIR".into(), home.display().to_string())]),
        Platform::MacOS,
    );
    let scope =
        resolve_effective_scope(&env, &only_claude(), &[], &Registry::with_builtins(), 1000);
    (tmp, home, scope)
}

#[test]
fn unchanged_combined_observation_must_not_invent_regrowth() {
    let (_tmp, _home, scope) = fixture();
    let store = tempfile::tempdir().unwrap();
    for t in [1000, 2000] {
        let ext = external::discover_and_measure(
            &scope,
            Some(store.path()),
            true,
            t,
            30,
            3600,
            &swamp_core::fs_events::EventCoverage::untrusted(),
        )
        .unwrap();
        let units = agents::discover_and_measure(
            &scope,
            &[],
            Some(store.path()),
            true,
            t,
            30,
            3600,
            &swamp_core::fs_events::EventCoverage::untrusted(),
        )
        .unwrap();
        assert!(
            ext.iter().all(|u| u.regrowth_count == 0),
            "unchanged external regrowth: {:?}",
            ext.iter()
                .map(|u| (&u.path, u.regrowth_count))
                .collect::<Vec<_>>()
        );
        assert!(
            units.iter().all(|u| u.regrowth_count == 0),
            "unchanged agent regrowth: {:?}",
            units
                .iter()
                .map(|u| (&u.path, u.regrowth_count))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn excluded_agent_home_must_not_be_scanned() {
    let (tmp, home, _) = fixture();
    let env = Environment::fixture(
        tmp.path().to_owned(),
        HashMap::from([("CLAUDE_CONFIG_DIR".into(), home.display().to_string())]),
        Platform::MacOS,
    );
    let scope = resolve_effective_scope(
        &env,
        &ScanConfig {
            exclude: vec![home.display().to_string()],
            ..only_claude()
        },
        &[],
        &Registry::with_builtins(),
        1000,
    );
    let units = agents::discover_and_measure(
        &scope,
        &[],
        None,
        false,
        1000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    let ext = external::discover_and_measure(
        &scope,
        None,
        false,
        1000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    assert!(
        units.is_empty() && ext.is_empty(),
        "excluded home scanned: {} agent units, {} external units",
        units.len(),
        ext.len()
    );
}

#[test]
fn protected_descendant_must_prevent_parent_cache_proposal() {
    let (_tmp, home, scope) = fixture();
    let store = tempfile::tempdir().unwrap();
    agents::protect_add(store.path(), &home.join("debug/log.txt")).unwrap();
    let units = agents::discover_and_measure(
        &scope,
        &[],
        Some(store.path()),
        false,
        1000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    assert!(
        actions::propose_agents(&units, &[home.join("debug")], "review-fixture").is_err(),
        "parent of protected file remained actionable"
    );
}

#[test]
fn defaults_false_must_mean_explicit_only() {
    let (tmp, home, _) = fixture();
    let env = Environment::fixture(
        tmp.path().to_owned(),
        HashMap::from([("CLAUDE_CONFIG_DIR".into(), home.display().to_string())]),
        Platform::MacOS,
    );
    let scope = resolve_effective_scope(
        &env,
        &ScanConfig {
            defaults: false,
            ..Default::default()
        },
        &[],
        &Registry::with_builtins(),
        1000,
    );
    assert!(
        scope.roots.is_empty(),
        "defaults=false still inferred {} roots",
        scope.roots.len()
    );
}
