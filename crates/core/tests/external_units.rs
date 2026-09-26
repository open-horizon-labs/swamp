//! #43 adversarial tests: external/shared storage units, modeled and
//! persisted without a fabricated project owner.

use std::collections::HashMap;
use std::fs;

use swamp_core::external::{ExternalUnit, discover_and_measure};
use swamp_core::locations::{Environment, Platform, Registry, StorageCategory};
use swamp_core::scope::{ScanConfig, resolve_effective_scope};

fn fixture_env(home: &std::path::Path, extra: &[(&str, &str)]) -> Environment {
    let mut env: HashMap<String, String> = HashMap::new();
    for (k, v) in extra {
        env.insert((*k).to_string(), (*v).to_string());
    }
    Environment::fixture(home.to_path_buf(), env, Platform::MacOS)
}

/// An **allow-list**, not a deny-list.
///
/// This used to disable `rustup` and `homebrew` and leave every other
/// detector running, on the assumption that a fixture `Environment`'s
/// injected `home` confines them. It does not: `core_simulator` proposes
/// the absolute system path `/Library/Developer/CoreSimulator/Volumes`,
/// independent of `env.home`, so these tests measured the developer's
/// real simulator runtimes -- 42 GB and ninety seconds of it on the
/// machine where this was found, which is both a wrong assertion and a
/// straight violation of "tests use disposable fixtures, never real user
/// data".
///
/// `enabled_detectors` is the strict reading of explicit-only scope (see
/// `docs/usage.md`), and it is what a fixture wants: exactly the
/// detector under test, nothing inferred.
fn only_cargo_home_config() -> ScanConfig {
    ScanConfig {
        defaults: false,
        include: Vec::new(),
        exclude: Vec::new(),
        disabled_detectors: Vec::new(),
        enabled_detectors: vec!["cargo-home".into()],
    }
}

fn write_pattern(path: &std::path::Path, bytes: u64) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![7u8; bytes as usize]).unwrap();
}

fn cargo_home_unit(units: &[ExternalUnit]) -> &ExternalUnit {
    units
        .iter()
        .find(|u| u.detector_id == "cargo-home" && u.category == StorageCategory::Installation)
        .expect("cargo home's own unit (not registry/git) is present")
}

/// An external root with no containing project (there is none -- a
/// manager home is not inside any checkout) is measured as one unit with
/// a real category and provenance, not folded into a project or dropped.
#[test]
fn external_only_root_is_measured_as_one_unit() {
    let home = tempfile::tempdir().unwrap();
    let cargo_home = home.path().join("fixture-cargo");
    write_pattern(&cargo_home.join("bin/cargo"), 1_000);
    write_pattern(&cargo_home.join("config.toml"), 200);

    let env = fixture_env(
        home.path(),
        &[("CARGO_HOME", &cargo_home.display().to_string())],
    );
    let registry = Registry::with_builtins();
    let scope = resolve_effective_scope(&env, &only_cargo_home_config(), &[], &registry, 1);

    let store = tempfile::tempdir().unwrap();
    let units = discover_and_measure(
        &scope,
        Some(store.path()),
        true,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    let unit = cargo_home_unit(&units);
    // Allocated bytes (disk blocks), not logical file size -- a real
    // measurement, so only a lower/upper sanity bound on the two small
    // files written above, not an exact byte count.
    assert!(
        unit.bytes >= 1_200 && unit.bytes < 100_000,
        "{}",
        unit.bytes
    );
    assert_eq!(unit.category, StorageCategory::Installation);
    assert!(unit.consumers.is_empty(), "no consumer declared yet");
    // First-ever observation: unknown baseline, never a synthetic zero.
    assert_eq!(unit.growth_bytes, None);
}

/// Zero consumers is the ordinary starting state, distinct from an error
/// or an omitted field.
#[test]
fn empty_consumer_set_is_empty_not_missing() {
    let home = tempfile::tempdir().unwrap();
    let cargo_home = home.path().join("fixture-cargo");
    write_pattern(&cargo_home.join("bin/cargo"), 500);
    let env = fixture_env(
        home.path(),
        &[("CARGO_HOME", &cargo_home.display().to_string())],
    );
    let registry = Registry::with_builtins();
    let scope = resolve_effective_scope(&env, &only_cargo_home_config(), &[], &registry, 1);
    let store = tempfile::tempdir().unwrap();
    let units = discover_and_measure(
        &scope,
        Some(store.path()),
        true,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    assert_eq!(cargo_home_unit(&units).consumers, Vec::new());
}

/// Two declared consumers of the same unit: the unit is still counted
/// once in totals, and both consumers are visible (not overwritten).
/// A tool's storage is measured from its conventional/overridden path
/// alone -- never gated on finding the tool's own executable. Homebrew's
/// detector proposes its conventional prefixes even with no working
/// `brew` query (`Environment::fixture`'s default `NullCommandRunner`
/// simulates exactly "the tool binary is gone"), so a leftover install
/// prefix is still measured after the tool itself was uninstalled.
#[test]
fn tool_executable_removed_but_storage_remains_still_measures_it() {
    let home = tempfile::tempdir().unwrap();
    let prefix = home.path().join("fixture-homebrew-prefix");
    write_pattern(&prefix.join("Cellar/somepkg/1.0/bin/tool"), 6_000);
    let env = fixture_env(
        home.path(),
        &[("HOMEBREW_PREFIX", &prefix.display().to_string())],
    );
    let registry = Registry::with_builtins();
    let cfg = ScanConfig {
        defaults: false,
        include: Vec::new(),
        exclude: Vec::new(),
        disabled_detectors: Vec::new(),
        enabled_detectors: vec!["homebrew".into()],
    };
    let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1);
    let store = tempfile::tempdir().unwrap();

    // No command runner was ever configured on this fixture Environment
    // (`Environment::fixture` defaults to `NullCommandRunner`): every
    // `brew --prefix` query this detector might attempt fails, exactly
    // as it would with `brew` missing from `PATH`.
    let units = discover_and_measure(
        &scope,
        Some(store.path()),
        true,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    // #49 refined Homebrew into Cellar/Caskroom as their own units,
    // separate from the bare prefix -- the fixture's file lives under
    // Cellar, so *that* unit (not the prefix unit, which now correctly
    // excludes it) is what still measures it with no working `brew`.
    let cellar = units
        .iter()
        .find(|u| {
            u.detector_id == "homebrew"
                && u.path == fs::canonicalize(&prefix).unwrap().join("Cellar")
        })
        .expect("Cellar is still measured with no working brew binary");
    assert!(cellar.bytes > 0, "{}", cellar.bytes);
    // The bare prefix is still measured too, just correctly excluding
    // Cellar/Caskroom's separately-counted bytes (the external-location
    // double-measurement fix, same chunk).
    let prefix_unit = units
        .iter()
        .find(|u| u.detector_id == "homebrew" && u.path == fs::canonicalize(&prefix).unwrap())
        .expect("the conventional prefix is still measured with no working brew binary");
    assert_eq!(
        prefix_unit.bytes, 0,
        "the prefix's own measurement must exclude Cellar's separately-counted bytes"
    );
}

/// A unit whose storage is unreadable this pass (simulated the same way
/// as #42's worktree-access-loss protection) is not tombstoned, and
/// reappears with unaffected history once access returns.
#[test]
fn incomplete_coverage_preserves_unknown_not_absent() {
    let home = tempfile::tempdir().unwrap();
    let cargo_home = home.path().join("fixture-cargo");
    write_pattern(&cargo_home.join("bin/cargo"), 2_500);
    let env = fixture_env(
        home.path(),
        &[("CARGO_HOME", &cargo_home.display().to_string())],
    );
    let registry = Registry::with_builtins();
    let scope = resolve_effective_scope(&env, &only_cargo_home_config(), &[], &registry, 1);
    let store = tempfile::tempdir().unwrap();

    let first = discover_and_measure(
        &scope,
        Some(store.path()),
        true,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    let bytes_first = cargo_home_unit(&first).bytes;
    assert!(bytes_first > 0);

    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&cargo_home, fs::Permissions::from_mode(0o000)).unwrap();
    let during = discover_and_measure(
        &scope,
        Some(store.path()),
        true,
        2_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    fs::set_permissions(&cargo_home, fs::Permissions::from_mode(0o755)).unwrap();
    let protected = during
        .iter()
        .find(|u| u.detector_id == "cargo-home" && u.category == StorageCategory::Installation)
        .expect("a coverage row is still emitted while access is lost");
    assert!(protected.note.is_some(), "{:?}", protected.note);

    let after = discover_and_measure(
        &scope,
        Some(store.path()),
        true,
        3_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    let unit_after = cargo_home_unit(&after);
    assert_eq!(unit_after.bytes, bytes_first);
    assert_eq!(
        unit_after.regrowth_count, 0,
        "losing and regaining read access to an external unit must never count as regrowth"
    );
}

/// No fixture may ever measure a path outside its own fixture tree.
///
/// This is the guard for a bug that cost ninety seconds a run and read
/// 42 GB of the developer's real simulator runtimes: `core_simulator`
/// proposes the **absolute** system path
/// `/Library/Developer/CoreSimulator/Volumes`, which no injected
/// `Environment::home` can confine. Every fixture in this file and its
/// siblings used a *deny-list* of two or three detector ids and left the
/// rest of the catalog running, so that path was in scope.
///
/// The test is deliberately not "core_simulator specifically": it walks
/// the whole registry, asks each detector what it would propose from a
/// fixture home, and fails on any resolved path that escapes it. A new
/// detector with a hardcoded system path fails here rather than in
/// whichever unrelated assertion happens to sum bytes.
#[test]
fn a_detector_that_escapes_the_fixture_home_is_named_here_not_discovered_by_a_byte_total() {
    use swamp_core::locations::LocationStatus;

    let home = tempfile::tempdir().unwrap();
    let env = fixture_env(home.path(), &[]);
    let registry = Registry::with_builtins();
    let mut escaping: Vec<(String, String)> = Vec::new();
    let every_detector = swamp_core::locations::permitted::PermittedDetectors::from_config(
        &swamp_core::scope::ScanConfig::default(),
        &registry,
    );
    for (id, proposals) in registry.resolve(&env, &every_detector) {
        for p in proposals {
            if p.status != LocationStatus::Resolved {
                continue;
            }
            let Some(path) = p.path else { continue };
            if !path.starts_with(home.path()) {
                escaping.push((id.clone(), path.display().to_string()));
            }
        }
    }

    // Three detectors genuinely do this, and all three are correct to:
    // Homebrew's prefixes, ruby-install's `/opt/rubies` and
    // CoreSimulator's system-wide runtime volumes are machine-wide
    // conventions, not per-user paths, so no `HOME` can relocate them.
    // They are listed here by name so the *number* of detectors that can
    // reach outside a fixture stays a reviewed decision instead of
    // something a byte total discovers by accident.
    //
    // The consequence for tests: a fixture scope naming any of these --
    // or, worse, using a deny-list that does not -- reads the
    // developer's real storage.
    let known: &[(&str, &str)] = &[
        ("core-simulator", "/Library/Developer/CoreSimulator/Volumes"),
        ("homebrew", "/opt/homebrew"),
        ("homebrew", "/usr/local"),
        ("homebrew", "/opt/homebrew/Cellar"),
        ("homebrew", "/opt/homebrew/Caskroom"),
        ("homebrew", "/usr/local/Cellar"),
        ("homebrew", "/usr/local/Caskroom"),
        ("ruby-install", "/opt/rubies"),
    ];
    let unexpected: Vec<&(String, String)> = escaping
        .iter()
        .filter(|(id, path)| !known.iter().any(|(k, p)| k == id && p == path))
        .collect();
    assert!(
        unexpected.is_empty(),
        "these detectors resolve a path outside the fixture home, so any test that does not \
         name them in `enabled_detectors` measures real user data: {unexpected:?}"
    );

    // And the consequence, stated: a fixture scope must be an
    // allow-list. A deny-list of a few ids leaves the escaping detector
    // in scope.
    let deny_list_scope = resolve_effective_scope(
        &env,
        &ScanConfig {
            defaults: false,
            include: Vec::new(),
            exclude: Vec::new(),
            disabled_detectors: vec!["rustup".into(), "homebrew".into()],
            enabled_detectors: Vec::new(),
        },
        &[],
        &registry,
        1,
    );
    // Asserted against the roots the scope *considered*, not the ones it
    // authorized. `authorized_roots` drops a candidate whose path is not
    // present, so the original form of this assertion held only on a
    // machine that happened to have CoreSimulator installed: it passed on
    // a Mac and failed on a Linux runner, for a reason that has nothing
    // to do with what it is testing. What a deny-list leaves in scope is
    // a property of the scope, not of the machine running the test.
    assert!(
        deny_list_scope.roots.iter().any(|r| r.reasons.iter().any(
            |reason| matches!(reason, swamp_core::scope::RootReason::Detector { detector_id, .. }
                if detector_id == "core-simulator")
        )),
        "if this ever stops being true, the allow-list fixtures in this file can be relaxed -- \
         until then they must stay allow-lists"
    );

    let allow_list_scope =
        resolve_effective_scope(&env, &only_cargo_home_config(), &[], &registry, 1);
    let (authorized, _) = allow_list_scope.authorized_roots();
    assert!(
        authorized
            .iter()
            .all(|r| r.detector_id.as_deref() == Some("cargo-home")),
        "an allow-list scope must authorize nothing but the detector it names: {:?}",
        authorized
            .iter()
            .map(|r| r.detector_id.clone())
            .collect::<Vec<_>>()
    );
}
