//! Unsupported last-use facts and supported empty probes survive the
//! observe-to-stored-report round trip without mtime inference.

use std::{fs, path::PathBuf};

use swamp_core::{
    locations::{Environment, Platform, Registry},
    report::{self, ObservationParts},
    scope::{ScanConfig, resolve_effective_scope},
};

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let store = home.join("store");
    let cache = home.join(".cache");
    let generic_child = cache.join("generic-child");
    fs::create_dir_all(&generic_child).unwrap();
    fs::write(generic_child.join("cache.bin"), vec![b'c'; 4_096]).unwrap();

    // rustup declares a key-file access-time source. This child has files
    // and a nonzero mtime, but no `bin` key file, so the supported probe
    // has no last-used value.
    let toolchain = home.join("rustup-home/toolchains/empty-toolchain");
    fs::create_dir_all(toolchain.join("lib")).unwrap();
    fs::write(toolchain.join("lib/manifest"), b"fixture").unwrap();
    fs::create_dir_all(&store).unwrap();
    (tmp, home, store, toolchain)
}

fn scope(home: &std::path::Path, rustup: &std::path::Path) -> swamp_core::scope::EffectiveScope {
    let env = Environment::fixture(
        home.to_path_buf(),
        [("RUSTUP_HOME".to_string(), rustup.display().to_string())]
            .into_iter()
            .collect(),
        Platform::MacOS,
    );
    let registry = Registry::with_builtins();
    let config = ScanConfig {
        disabled_detectors: registry
            .detectors()
            .iter()
            .filter(|detector| !["builtin-defaults", "rustup"].contains(&detector.id()))
            .map(|detector| detector.id().to_owned())
            .collect(),
        ..Default::default()
    };
    resolve_effective_scope(&env, &config, &[], &registry, 1_000)
}

#[test]
fn unsupported_cache_child_and_supported_empty_probe_round_trip() {
    let (_tmp, home, store, toolchain) = fixture();
    let rustup = home.join("rustup-home");
    let selected_scope = scope(&home, &rustup);
    let observed = report::observe_scope(
        &selected_scope,
        ObservationParts::ALL,
        None,
        None,
        false,
        Some(&store),
        None,
        true,
        false,
        false,
        true,
        &swamp_core::fs_events::UnsupportedPlatformSource,
        30,
        86_400,
    )
    .expect("observe fixture roots");

    let cache_path = home.join(".cache");
    let cache = observed
        .external_units
        .iter()
        .find(|unit| unit.path == cache_path)
        .unwrap_or_else(|| {
            panic!(
                "generic cache root; roots={:?}; units={:?}",
                selected_scope
                    .roots
                    .iter()
                    .map(|r| (&r.path, &r.status))
                    .collect::<Vec<_>>(),
                observed
                    .external_units
                    .iter()
                    .map(|u| &u.path)
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(
        cache.last_used.fact(observed.merged.observed_at),
        "tracking unsupported"
    );
    let cache_child = cache
        .children
        .iter()
        .find(|child| child.name == "generic-child")
        .expect("generic cache child");
    assert_eq!(
        cache_child.last_used.fact(observed.merged.observed_at),
        "tracking unsupported"
    );

    let rustup_unit = observed
        .external_units
        .iter()
        .find(|unit| unit.path == rustup.join("toolchains"))
        .unwrap_or_else(|| {
            panic!(
                "rustup toolchains unit; roots={:?}; units={:?}",
                selected_scope
                    .roots
                    .iter()
                    .map(|r| (&r.path, &r.status))
                    .collect::<Vec<_>>(),
                observed
                    .external_units
                    .iter()
                    .map(|u| (&u.path, u.category))
                    .collect::<Vec<_>>()
            )
        });
    let supported_child = rustup_unit
        .children
        .iter()
        .find(|child| child.name == toolchain.file_name().unwrap().to_string_lossy())
        .expect("supported child with no key file");
    assert_eq!(
        supported_child.last_used.fact(observed.merged.observed_at),
        "no record"
    );

    let replayed = report::report_scope_from_store(&selected_scope, &store).unwrap();
    let replayed_cache = replayed
        .external_units
        .iter()
        .find(|unit| unit.path == cache_path)
        .expect("replayed generic cache root");
    assert_eq!(replayed_cache.last_used, cache.last_used);
    let replayed_cache_child = replayed_cache
        .children
        .iter()
        .find(|child| child.name == "generic-child")
        .expect("replayed generic cache child");
    assert_eq!(replayed_cache_child.last_used, cache_child.last_used);
    let replayed_rustup = replayed
        .external_units
        .iter()
        .find(|unit| unit.path == rustup.join("toolchains"))
        .expect("replayed rustup toolchains unit");
    let replayed_supported_child = replayed_rustup
        .children
        .iter()
        .find(|child| child.name == toolchain.file_name().unwrap().to_string_lossy())
        .expect("replayed supported child");
    assert_eq!(
        replayed_supported_child.last_used,
        supported_child.last_used
    );
    assert_eq!(
        replayed_supported_child
            .last_used
            .fact(observed.merged.observed_at),
        "no record"
    );
}
