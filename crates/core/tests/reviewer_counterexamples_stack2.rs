//! Independent re-review of the whole stack (#116-#125) at 6a2c563.
//!
//! Every test here asserts **required behavior**, not the bug. All of
//! them FAIL on 6a2c563. They are variants designed to slip past the
//! shape of the repairs the stack already made: the seventeen prior
//! counterexamples all pass, so these attack the *edges* of those fixes
//! rather than repeating them.
//!
//! Safety: every fixture is a disposable `tempfile` tree. Nothing reads
//! a real agent/editor home, a real tool store or the user's own swamp
//! state, and nothing is moved or removed outside the per-test temp dir.
//!
//! Run:
//! ```sh
//! cargo test -p swamp-core --test reviewer_counterexamples_stack2 \
//!   --target-dir <scratch>/target-audit -- --test-threads=1 --nocapture
//! ```
//!
//! The TUI counterexample (CE3) needs `swamp-tui` and so lives in
//! `crates/tui/tests/reviewer_counterexamples_stack2_tui.rs`.
//!
//! stack/27 (2026-09-23, "swamp reports; the human removes"): CE2 and
//! the original CE3 (`reviewed_snapshot_must_see_a_same_second_same_size_rewrite`)
//! tested `recheck::reviewed_snapshot` and the plan/approve/execute
//! pipeline, both deleted with the CLI/agent action path -- there is no
//! execute-time re-derivation left to slip past, so those two are
//! dropped rather than kept as dead code. CE5 is rewritten to the
//! current gate (`agents::discover_and_measure`'s own
//! `protected_paths.conflict` check plus `actions::propose_agents`'s
//! `agent_refusal`) and the current sink (`actions::trash_agent_cache`,
//! what the TUI's Enter now calls), rather than the removed
//! propose/save_plan/approve/execute_with_trash cycle. CE1, CE4 and CE6
//! are unchanged: none of them touched the removed pipeline.

use std::{collections::HashMap, fs};
use swamp_core::{
    actions, agents, external,
    locations::{Environment, Platform, Registry},
    scope::{ScanConfig, resolve_effective_scope},
};

/// An explicit-only scope with exactly `ids` enabled, so no detector on
/// the reviewer's own machine can leak into a fixture.
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
    fs::write(home.join("debug/log.txt"), b"AAAAAAAA").unwrap();
    home
}

fn claude_env(tmp: &std::path::Path, home: &std::path::Path) -> Environment {
    Environment::fixture(
        tmp.to_owned(),
        HashMap::from([("CLAUDE_CONFIG_DIR".into(), home.display().to_string())]),
        Platform::MacOS,
    )
}

// ---------------------------------------------------------------------
// CE1. An `exclude` entry must still exclude under `--root`.
//
// `excluded_agent_home_must_not_be_scanned` is fixed for the
// non-explicit path only. Under an explicit command root,
// `agents::authorized_tool_homes` and `external::authorized_candidates`
// switch to `EffectiveScope::authorized_detector_paths_in_explicit_roots`
// (scope.rs:502), which tests exclusion *only* against
// `self.roots` entries whose status is `RootStatus::Excluded`. Under
// `--root <parent>` the excluded home is not a root at all -- it is a
// `PruneNote` inside the explicit root -- so nothing matches and the
// home the user excluded is discovered, measured and made actionable.
// ---------------------------------------------------------------------
#[test]
fn an_excluded_home_must_stay_excluded_under_an_explicit_root() {
    let tmp = tempfile::tempdir().unwrap();
    let home = claude_home(tmp.path());
    let env = claude_env(tmp.path(), &home);
    let cfg = ScanConfig {
        exclude: vec![home.display().to_string()],
        ..only(&["claude-code"])
    };
    // The user names the *parent* of the excluded home as the root.
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
    assert!(
        units.is_empty() && ext.is_empty(),
        "an excluded home was scanned under --root: {} agent units {:?}, {} external units {:?}",
        units.len(),
        units.iter().map(|u| &u.path).collect::<Vec<_>>(),
        ext.len(),
        ext.iter().map(|u| &u.path).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------
// CE4. A config-only exclusion must not invent growth or regrowth.
//
// `unchanged_combined_observation_must_not_invent_regrowth` is fixed for
// the two-family case via `ObservationOwnership`. The ownership window
// is still *path-prefix* based (growth.rs:3607), and
// `external::discover_and_measure` seeds it with the paths it measured
// this pass (external.rs:342-345). Excluding a nested location therefore
// does two things at once:
//
//   * the parent's `nested_exclusions` are computed from the surviving
//     candidate list (external.rs:280-289), so the parent silently
//     absorbs the child's bytes -- reported as real `growth_bytes`; and
//   * the child's stored row still lies under the parent's covered root,
//     so the owned sweep tombstones it, and un-excluding it later scores
//     a `regrowth_count`.
//
// Zero bytes changed on disk in this test. Both numbers are fiction, and
// `.oh/guardrails/coverage-changes-are-not-storage-changes.md` is the
// guardrail they violate.
// ---------------------------------------------------------------------
#[test]
fn a_config_only_exclusion_must_not_invent_growth_or_regrowth() {
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
    let store = tempfile::tempdir().unwrap();
    let registry = Registry::with_builtins();

    let base = resolve_effective_scope(&env, &only(&["cargo-home"]), &[], &registry, 1000);
    let first = external::discover_and_measure(
        &base,
        Some(store.path()),
        true,
        1000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    assert!(
        first.len() >= 2,
        "precondition: the parent and the nested location are both measured: {:?}",
        first.iter().map(|u| (&u.path, u.bytes)).collect::<Vec<_>>()
    );

    // One line of config changes. Nothing on disk does.
    let excluded = resolve_effective_scope(
        &env,
        &ScanConfig {
            exclude: vec![inner.display().to_string()],
            ..only(&["cargo-home"])
        },
        &[],
        &registry,
        2000,
    );
    let second = external::discover_and_measure(
        &excluded,
        Some(store.path()),
        true,
        2000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    let invented_growth: Vec<_> = second
        .iter()
        .filter(|u| u.growth_bytes.unwrap_or(0) != 0)
        .map(|u| (u.path.clone(), u.bytes, u.growth_bytes))
        .collect();
    assert!(
        invented_growth.is_empty(),
        "excluding a nested location reported the parent as having grown: {invented_growth:?}"
    );

    // And restoring the config must not read as the child coming back.
    let third = external::discover_and_measure(
        &base,
        Some(store.path()),
        true,
        3000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    let invented_regrowth: Vec<_> = third
        .iter()
        .filter(|u| u.regrowth_count > 0)
        .map(|u| (u.path.clone(), u.regrowth_count))
        .collect();
    assert!(
        invented_regrowth.is_empty(),
        "un-excluding a nested location was recorded as regrowth: {invented_regrowth:?}"
    );
}

// ---------------------------------------------------------------------
// CE5. `swamp protect add` must not accept a path it cannot enforce.
//
// The original counterexample: `protect_add` stored a relative argument
// verbatim, and `ProtectList::conflict` compares only absolute unit
// paths in both directions -- so a relative entry protected nothing
// while `protect add`/`protect list` reported it as stored. That is
// fixed now: `protection::protect_add_unchecked` refuses a non-absolute
// path outright (`protection.rs`). This test is kept as the regression
// guard for that fail-closed behavior, and, on the (should-not-happen)
// branch where `protect_add` ever again accepts a relative entry, it
// still proves the full required behavior: nothing downstream may treat
// it as protecting the path.
//
// Rewritten for stack/27: there is no propose/save_plan/approve/
// execute_with_trash cycle any more. The gate under test now is
// `agents::discover_and_measure` (which loads the protect list from
// `swamp_dir` and sets `AgentUnit.protected`/`protect_reason` via
// `ProtectList::conflict`) and `actions::propose_agents` (whose
// `agent_refusal` refuses anything `protected`); the sink is
// `actions::trash_agent_cache`, the same call the TUI's Enter makes.
// ---------------------------------------------------------------------
#[test]
fn protect_add_must_not_accept_a_path_it_cannot_enforce() {
    let tmp = tempfile::tempdir().unwrap();
    let home = claude_home(tmp.path());
    let env = claude_env(tmp.path(), &home);
    let scope = resolve_effective_scope(
        &env,
        &only(&["claude-code"]),
        &[],
        &Registry::with_builtins(),
        1000,
    );
    let store = tempfile::tempdir().unwrap();
    let trash = tempfile::tempdir().unwrap();
    let path = home.join("debug");

    // What a user types from inside the tool home.
    let added = swamp_core::protection::protect_add(store.path(), std::path::Path::new("debug"));
    if added.is_ok() {
        assert_eq!(
            swamp_core::protection::protect_list(store.path()).unwrap(),
            vec![std::path::PathBuf::from("debug")],
            "precondition: the relative entry was stored as given"
        );
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
        if let Ok(plan_units) = actions::propose_agents(&units, &[path.clone()], "reviewer") {
            // Not refused as protected: propose_agents let it through.
            // Fall all the way to the real sink, exactly what the TUI's
            // Enter would do next.
            assert!(
                !plan_units.is_empty(),
                "propose_agents returned an empty, non-error result"
            );
            let _ = actions::trash_agent_cache(&path, trash.path(), 1000);
        }
    }
    assert!(
        added.is_err() || path.exists(),
        "`protect add debug` was accepted and confirmed, and the data it named was still moved"
    );
}

// ---------------------------------------------------------------------
// CE6. An unchanged observation must not spawn subprocesses, and a
// disabled detector must stop its probes.
//
// `consumers::docker::DockerConsumer` calls `docker::load_cached`
// unconditionally on every `ProjectsGrouped` event
// (consumers/docker.rs:32) and consults the effective scope nowhere. So
// `docker-desktop` being disabled does not stop swamp asking the Docker
// daemon about the user's images, volumes and containers -- and because
// `load_cached` only caches a *successful* answer (docker.rs:651-682),
// an unavailable daemon is re-probed on every single pass. Measured on
// the multi-ecosystem fixture: five `docker` spawns per observation,
// forever, with Docker out of scope.
// ---------------------------------------------------------------------
#[test]
fn a_disabled_detector_must_not_probe_its_tool() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let src = root.join("src/p");
    fs::create_dir_all(src.join("target/debug")).unwrap();
    fs::write(src.join("target/debug/blob.bin"), vec![b'x'; 4096]).unwrap();
    fs::write(src.join("Cargo.toml"), b"[package]\nname=\"x\"\n").unwrap();
    let store = tempfile::tempdir().unwrap();

    // No PATH shim. The previous version of this test put a directory of
    // fake `docker`/`lsof`/... scripts at the front of the process-wide
    // `PATH` and counted lines in a shared log file, which made it
    // unrunnable under the default test harness: a sibling test in the
    // same binary that legitimately probes occupancy appended to the
    // same log, and the failure read as a real regression
    // (`["lsof", "lsof"]`). `scripts/check.sh` carried
    // `--test-threads=1` for that reason alone.
    //
    // Every `std::process::Command` in `swamp-core` now records itself
    // through `work_counters::record_spawn`, and `measured` installs a
    // sink scoped to this thread and the pools it starts. Same
    // assertion, no process-wide state, thread-safe.
    let registry = Registry::with_builtins();
    let cfg = ScanConfig {
        defaults: false,
        include: vec![root.join("src").display().to_string()],
        // Every detector disabled, Docker Desktop included.
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .collect(),
        ..Default::default()
    };
    let env = Environment::fixture(root.clone(), HashMap::new(), Platform::MacOS);
    let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1000);
    let (_, counted) = swamp_core::work_counters::measured(|| {
        for _ in 0..2 {
            let _ = swamp_core::report::observe_scope(
                &scope,
                swamp_core::report::ObservationParts::ALL,
                None,
                None,
                false,
                Some(store.path()),
                None,
                true,
                true,
                false,
                false,
                swamp_core::fs_events::platform_source().as_ref(),
                30,
                24 * 3600,
            );
        }
    });
    assert_eq!(
        counted.subprocess_spawns, 0,
        "two observations with every detector disabled spawned {} subprocesses",
        counted.subprocess_spawns
    );
}

/// The instrument the test above rests on, checked rather than assumed.
///
/// A spawn counter that nothing increments would make
/// `a_disabled_detector_must_not_probe_its_tool` pass vacuously -- which
/// is precisely the failure mode the 2026-09-22 re-review found in the
/// work counters themselves ("a 20,000-file traversal reporting 2 dirs
/// listed"). So: a command that really does spawn must be counted, and
/// it must be counted in the scoped sink, not only globally.
#[test]
fn the_spawn_counter_counts_a_real_spawn() {
    use swamp_core::locations::{CommandRunner, SystemCommandRunner};
    let (_, counted) = swamp_core::work_counters::measured(|| {
        // Allow-listed, read-only, and harmless if absent: a failure to
        // spawn is still a spawn attempt for every purpose this counter
        // has, and `brew --prefix` is the query the detector registry
        // itself makes.
        let _ = SystemCommandRunner.run("brew", &["--prefix"], std::time::Duration::from_secs(5));
    });
    assert_eq!(
        counted.subprocess_spawns, 1,
        "the spawn counter must count an allow-listed command runner spawn"
    );
}
