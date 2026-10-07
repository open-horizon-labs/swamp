//! Configured external outputs use the shared external fold, build adapter
//! identification, and typed report tables.
use std::{fs, path::Path};
use swamp_core::{
    fs_events::UnsupportedPlatformSource,
    locations::{Environment, Platform, Registry},
    report::{self, ObservationParts},
    scope::{ScanConfig, resolve_effective_scope},
};

fn checkout(root: &Path) {
    fs::create_dir_all(root).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(root)
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn external_typescript_output_is_measured_identified_and_read_from_store() {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let root = home.join("repo");
    checkout(&root);
    fs::write(root.join("package.json"), "{}").unwrap();
    let output = home.join("compiled-app");
    fs::create_dir_all(&output).unwrap();
    fs::write(output.join("index.js"), vec![b'x'; 8 * 1024]).unwrap();
    fs::write(
        root.join("tsconfig.json"),
        serde_json::json!({"compilerOptions": {"outDir": output}}).to_string(),
    )
    .unwrap();

    let registry = Registry::with_builtins();
    let config = ScanConfig {
        defaults: false,
        include: vec![root.display().to_string()],
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|detector| detector.id().to_owned())
            .collect(),
        ..Default::default()
    };
    let environment = Environment::fixture(home, Default::default(), Platform::MacOS);
    let scope = resolve_effective_scope(&environment, &config, &[], &registry, 1_000);
    let store = tmp.path().join("swamp-store");
    let observation = report::observe_scope(
        &scope,
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
        &UnsupportedPlatformSource,
        30,
        86_400,
    )
    .unwrap();

    let unit = observation
        .external_units
        .iter()
        .find(|unit| unit.path == output)
        .expect("the configured output has one external physical owner");
    assert_eq!(unit.detector_id, "configured-build:node");
    assert_eq!(
        unit.category,
        swamp_core::locations::StorageCategory::BuildOutput
    );
    assert!(unit.bytes >= 8 * 1024);
    assert!(unit.consumers.iter().any(|consumer| {
        consumer.label == root.display().to_string()
            && consumer
                .note
                .as_deref()
                .is_some_and(|note| note.contains("compilerOptions.outDir"))
    }));
    assert!(
        observation
            .store_interiors
            .iter()
            .any(|interior| interior.path == output),
        "the project BuildContainer should identify from the same folded output rows"
    );

    let stored = report::report_scope_from_store(&scope, &store).expect("stored report");
    let stored_unit = stored
        .external_units
        .iter()
        .find(|unit| unit.path == output)
        .expect("read-only report retains the configured output unit");
    assert_eq!(stored_unit.bytes, unit.bytes);
    assert_eq!(stored_unit.consumers, unit.consumers);
    assert!(
        stored
            .report
            .configured_outputs
            .iter()
            .any(|reference| { reference.path == output && reference.project_root == root })
    );
}
