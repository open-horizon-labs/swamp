//! The explicit-root half of `.oh/guardrails/discovery-consumes-effective-scope.md`.
//!
//! The 2026-09-22 re-review's CE1: every exclusion test in the suite
//! passed `&[]` for explicit roots, so nothing exercised
//! `EffectiveScope::authorized_detector_paths_in_explicit_roots` -- the
//! function both discovery families switch to under `--root`. It tested
//! exclusion only against roots whose *status* was `Excluded`, and under
//! `--root <parent>` an excluded home is not a root at all but a
//! `PruneNote` inside the explicit root. So `swamp report --root ~`
//! offered an excluded agent home's contents for removal.
//!
//! This file covers every exclusion case *under an explicit root that
//! contains it*: an excluded tool home, an excluded nested external
//! location, a disabled detector, and a protect entry written in either
//! path spelling (the P2 finding: `external` canonicalizes its
//! candidates and `agents` does not, so one entry could cover one family
//! and not the other).
//!
//! Disposable `tempfile` fixtures only; nothing reads a real home.

use std::{collections::HashMap, fs};
use swamp_core::{
    agents, external,
    locations::{Environment, Platform, Registry},
    scope::{ScanConfig, resolve_effective_scope},
};

/// An explicit-only scope with exactly `ids` enabled, so no detector on
/// the machine running the tests can leak into a fixture.
fn only(ids: &[&str]) -> ScanConfig {
    ScanConfig {
        defaults: false,
        disabled_detectors: Registry::with_builtins()
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .filter(|id| !ids.contains(&id.as_str()))
            .collect(),
        ..Default::default()
    }
}

fn claude_home(tmp: &std::path::Path) -> std::path::PathBuf {
    let home = tmp.join("claude");
    fs::create_dir_all(home.join("debug")).unwrap();
    fs::write(home.join("debug/log.txt"), vec![b'x'; 2048]).unwrap();
    home
}

fn claude_env(tmp: &std::path::Path, home: &std::path::Path) -> Environment {
    Environment::fixture(
        tmp.to_owned(),
        HashMap::from([("CLAUDE_CONFIG_DIR".into(), home.display().to_string())]),
        Platform::MacOS,
    )
}

#[test]
fn an_excluded_tool_home_inside_an_explicit_root_yields_no_units() {
    let tmp = tempfile::tempdir().unwrap();
    let home = claude_home(tmp.path());
    let env = claude_env(tmp.path(), &home);
    let cfg = ScanConfig {
        exclude: vec![home.display().to_string()],
        ..only(&["claude-code"])
    };
    let scope = resolve_effective_scope(
        &env,
        &cfg,
        &[tmp.path().to_path_buf()],
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
    assert!(units.is_empty(), "agent units: {:?}", paths(&units));
    assert!(ext.is_empty(), "external units: {:?}", ext_paths(&ext));
}

/// The same exclusion written with a trailing slash, and written through
/// the *other* path spelling (`/var/...` vs `/private/var/...` on macOS,
/// where `/var` is a symlink). One entry has to cover both families.
#[test]
fn an_exclusion_in_either_path_spelling_covers_both_families() {
    let tmp = tempfile::tempdir().unwrap();
    // The uncanonicalized spelling: what a user's shell tab-completion
    // gives them on macOS under `/var/folders/...`.
    let raw = tmp.path().to_path_buf();
    let canonical = fs::canonicalize(&raw).unwrap();
    let home = claude_home(&canonical);
    let env = claude_env(&canonical, &home);

    for spelling in [
        raw.join("claude").display().to_string(),
        canonical.join("claude").display().to_string(),
        format!("{}/", canonical.join("claude").display()),
    ] {
        let cfg = ScanConfig {
            exclude: vec![spelling.clone()],
            ..only(&["claude-code"])
        };
        for explicit in [vec![], vec![canonical.clone()]] {
            let scope =
                resolve_effective_scope(&env, &cfg, &explicit, &Registry::with_builtins(), 1000);
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
                "exclude `{spelling}` (explicit roots: {explicit:?}) left {} agent and {} \
                 external units: {:?} {:?}",
                units.len(),
                ext.len(),
                paths(&units),
                ext_paths(&ext)
            );
        }
    }
}

#[test]
fn an_excluded_nested_external_location_inside_an_explicit_root_yields_no_unit() {
    let tmp = tempfile::tempdir().unwrap();
    let outer = tmp.path().join("cargo");
    let inner = outer.join("registry/cache");
    fs::create_dir_all(&inner).unwrap();
    fs::write(outer.join("top.bin"), vec![b'x'; 4096]).unwrap();
    fs::write(inner.join("crate.crate"), vec![b'y'; 65536]).unwrap();
    let env = Environment::fixture(
        tmp.path().to_owned(),
        HashMap::from([("CARGO_HOME".into(), outer.display().to_string())]),
        Platform::MacOS,
    );
    let cfg = ScanConfig {
        exclude: vec![inner.display().to_string()],
        ..only(&["cargo-home"])
    };
    let scope = resolve_effective_scope(
        &env,
        &cfg,
        &[tmp.path().to_path_buf()],
        &Registry::with_builtins(),
        1000,
    );
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
    let canonical_inner = fs::canonicalize(&inner).unwrap();
    assert!(
        !ext.iter().any(|u| u.path == canonical_inner),
        "the excluded nested location was measured under --root: {:?}",
        ext_paths(&ext)
    );
    // ... and the parent must not absorb the excluded child's bytes,
    // which would report the exclusion as growth
    // (`.oh/guardrails/coverage-changes-are-not-storage-changes.md`).
    let canonical_outer = fs::canonicalize(&outer).unwrap();
    if let Some(parent) = ext.iter().find(|u| u.path == canonical_outer) {
        assert!(
            parent.bytes < 65536,
            "the parent absorbed the excluded child's bytes ({} bytes)",
            parent.bytes
        );
    }
}

#[test]
fn a_disabled_detector_inside_an_explicit_root_yields_no_units() {
    let tmp = tempfile::tempdir().unwrap();
    let home = claude_home(tmp.path());
    let env = claude_env(tmp.path(), &home);
    let registry = Registry::with_builtins();
    let cfg = ScanConfig {
        defaults: false,
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .collect(),
        ..Default::default()
    };
    let scope = resolve_effective_scope(&env, &cfg, &[tmp.path().to_path_buf()], &registry, 1000);
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
        "every detector disabled still produced {:?} / {:?}",
        paths(&units),
        ext_paths(&ext)
    );
}

/// A `protect` entry written in either spelling has to cover both
/// families, for the same reason an `exclude` entry does: the agent pass
/// reports `/var/folders/.../claude/debug` and the external pass reports
/// `/private/var/folders/.../claude`, and `protection_conflict` is a
/// component comparison.
#[test]
fn a_protect_entry_in_either_path_spelling_protects_both_families() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path().to_path_buf();
    let canonical = fs::canonicalize(&raw).unwrap();
    let home = claude_home(&canonical);
    let env = claude_env(&canonical, &home);
    let scope = resolve_effective_scope(
        &env,
        &only(&["claude-code"]),
        &[canonical.clone()],
        &Registry::with_builtins(),
        1000,
    );

    for spelling in [raw.join("claude"), canonical.join("claude")] {
        let store = tempfile::tempdir().unwrap();
        agents::protect_add(store.path(), &spelling).expect("an absolute path is protectable");
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
            !units.is_empty(),
            "precondition: the tool home is discovered so there is something to protect"
        );
        for u in &units {
            assert!(
                u.protected,
                "protect `{}` left {} unprotected",
                spelling.display(),
                u.path.display()
            );
        }
    }
}

fn paths(units: &[agents::AgentUnit]) -> Vec<String> {
    units.iter().map(|u| u.path.display().to_string()).collect()
}

fn ext_paths(units: &[external::ExternalUnit]) -> Vec<String> {
    units.iter().map(|u| u.path.display().to_string()).collect()
}
