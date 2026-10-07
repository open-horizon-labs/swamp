//! Both cache conventions remain subject to ordinary scope policy.
use std::os::unix::fs::MetadataExt;
use std::{collections::HashMap, fs, path::Path};
use swamp_core::{
    locations::{Environment, Platform, Registry},
    scope::{ScanConfig, resolve_effective_scope},
};

fn defaults(registry: &Registry) -> ScanConfig {
    ScanConfig {
        disabled_detectors: registry
            .detectors()
            .iter()
            .filter(|d| d.id() != "builtin-defaults")
            .map(|d| d.id().to_owned())
            .collect(),
        ..Default::default()
    }
}

fn fixture(home: &Path) -> Environment {
    for name in ["src", "Library/Caches", ".cache"] {
        fs::create_dir_all(home.join(name)).unwrap();
    }
    Environment::fixture(home.to_path_buf(), HashMap::new(), Platform::MacOS)
}

#[test]
fn macos_cache_defaults_respect_exclusions_and_explicit_only_policy() {
    let tmp = tempfile::tempdir().unwrap();
    let env = fixture(tmp.path());
    let registry = Registry::with_builtins();
    let cfg = defaults(&registry);
    let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1000);
    let roots = scope.authorized_unit_roots();
    assert!(roots.contains(&tmp.path().join(".cache")));
    assert!(roots.contains(&tmp.path().join("Library/Caches")));
    assert!(!scope.roots.iter().any(|r| r.is_project_root()
        && (r.path == tmp.path().join(".cache") || r.path == tmp.path().join("Library/Caches"))));

    let mut excluded = cfg.clone();
    excluded.exclude.push("~/.cache".into());
    let scope = resolve_effective_scope(&env, &excluded, &[], &registry, 1000);
    assert!(
        !scope
            .authorized_unit_roots()
            .contains(&tmp.path().join(".cache"))
    );
    assert!(
        scope
            .authorized_unit_roots()
            .contains(&tmp.path().join("Library/Caches"))
    );

    let disabled = ScanConfig {
        defaults: false,
        ..cfg.clone()
    };
    assert!(
        resolve_effective_scope(&env, &disabled, &[], &registry, 1000)
            .authorized_unit_roots()
            .is_empty()
    );
    let scope = resolve_effective_scope(&env, &cfg, &[tmp.path().join("src")], &registry, 1000);
    assert!(scope.authorized_unit_roots().iter().all(
        |path| path != &tmp.path().join(".cache") && path != &tmp.path().join("Library/Caches")
    ));
}

#[test]
fn xdg_override_equal_to_native_cache_has_one_measurement_root() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let native = tmp.path().join("Library/Caches");
    let env = Environment::fixture(
        tmp.path().to_path_buf(),
        HashMap::from([("XDG_CACHE_HOME".into(), native.display().to_string())]),
        Platform::MacOS,
    );
    let registry = Registry::with_builtins();
    let scope = resolve_effective_scope(&env, &defaults(&registry), &[], &registry, 1000);
    assert_eq!(
        scope
            .authorized_unit_roots()
            .iter()
            .filter(|p| **p == native)
            .count(),
        1
    );
}

#[test]
fn configured_output_inside_xdg_cache_is_counted_once() {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let env = fixture(&home);
    let repo = home.join("src/app");
    fs::create_dir_all(&repo).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&repo)
            .status()
            .unwrap()
            .success()
    );
    fs::write(repo.join("package.json"), "{}").unwrap();
    let output = home.join(".cache/app-output");
    fs::create_dir_all(&output).unwrap();
    fs::write(output.join("generated.js"), vec![b'x'; 4096]).unwrap();
    fs::write(home.join(".cache/other.bin"), vec![b'y'; 2048]).unwrap();
    fs::write(
        repo.join("tsconfig.json"),
        serde_json::json!({"compilerOptions":{"outDir": output}}).to_string(),
    )
    .unwrap();
    let registry = Registry::with_builtins();
    let scope = resolve_effective_scope(&env, &defaults(&registry), &[], &registry, 1000);
    let observed = swamp_core::report::observe_scope(
        &scope,
        swamp_core::report::ObservationParts::ALL,
        None,
        None,
        false,
        None,
        None,
        true,
        false,
        false,
        true,
        &swamp_core::fs_events::UnsupportedPlatformSource,
        30,
        86400,
    )
    .unwrap();
    let cache = home.join(".cache");
    let units: Vec<_> = observed
        .external_units
        .iter()
        .filter(|u| u.path == cache || u.path == output)
        .collect();
    assert_eq!(
        units.len(),
        2,
        "cache remainder and referenced output stay visible"
    );
    assert_eq!(
        units.iter().map(|u| u.bytes).sum::<u64>(),
        fs::metadata(output.join("generated.js")).unwrap().blocks() * 512
            + fs::metadata(cache.join("other.bin")).unwrap().blocks() * 512,
        "cache and build output must not charge the same bytes twice"
    );
    assert!(
        units
            .iter()
            .find(|u| u.path == output)
            .unwrap()
            .consumers
            .iter()
            .any(|c| c.label.contains("app"))
    );
    let mut excluded_config = defaults(&registry);
    excluded_config.exclude.push(output.display().to_string());
    let excluded_scope = resolve_effective_scope(&env, &excluded_config, &[], &registry, 1001);
    let excluded = swamp_core::report::observe_scope(
        &excluded_scope,
        swamp_core::report::ObservationParts::ALL,
        None,
        None,
        false,
        None,
        None,
        true,
        false,
        false,
        true,
        &swamp_core::fs_events::UnsupportedPlatformSource,
        30,
        86400,
    )
    .unwrap();
    assert!(
        excluded
            .external_units
            .iter()
            .all(|unit| unit.path != output)
    );
    assert_eq!(
        excluded
            .external_units
            .iter()
            .find(|unit| unit.path == cache)
            .unwrap()
            .bytes,
        fs::metadata(cache.join("other.bin")).unwrap().blocks() * 512,
        "excluded referenced output must also be pruned from its containing cache"
    );
}
