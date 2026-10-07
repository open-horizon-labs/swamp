//! A folded exclusion must also survive Maven's shallow packaged-file discovery.
use std::{fs, os::unix::fs::MetadataExt};
use swamp_core::{
    evidence::{FactKind, FactSubtype},
    fs_events::UnsupportedPlatformSource,
    locations::{Environment, Platform, Registry, StorageCategory},
    report::{self, ObservationParts},
    scope::{ScanConfig, resolve_effective_scope},
};

#[test]
fn excluded_maven_jar_is_neither_counted_nor_reintroduced_by_identification() {
    let temp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(temp.path()).unwrap();
    let repo = home.join("repo");
    let output = home.join("maven-output");
    let store = home.join("store");
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&output).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&repo)
            .status()
            .unwrap()
            .success()
    );
    fs::write(repo.join("pom.xml"), "<project><modelVersion>4.0.0</modelVersion><groupId>fixture</groupId><artifactId>example</artifactId><version>1</version><build><directory>../maven-output</directory></build></project>").unwrap();
    let kept = output.join("keep.jar");
    let excluded = output.join("secret.jar");
    fs::write(&kept, vec![b'k'; 4096]).unwrap();
    fs::write(&excluded, vec![b's'; 32768]).unwrap();
    let registry = Registry::with_builtins();
    let config = ScanConfig {
        defaults: false,
        include: vec![repo.display().to_string()],
        exclude: vec![excluded.display().to_string()],
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|detector| detector.id().to_owned())
            .collect(),
        ..Default::default()
    };
    let environment = Environment::fixture(home, Default::default(), Platform::MacOS);
    let scope = resolve_effective_scope(&environment, &config, &[], &registry, 1000);
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
        86400,
    )
    .unwrap();
    let unit = observation
        .external_units
        .iter()
        .find(|unit| unit.path == output && unit.category == StorageCategory::BuildOutput)
        .expect("literal configured Maven output");
    assert_eq!(
        unit.detector_id, "configured-build:maven",
        "configured output identity must not masquerade as the Maven dependency-store detector"
    );
    assert_eq!(
        unit.bytes,
        fs::metadata(&kept).unwrap().blocks() * 512,
        "the excluded packaged file contributes no allocation"
    );
    assert!(
        unit.consumers
            .iter()
            .any(|consumer| consumer.label == repo.display().to_string()
                && consumer
                    .note
                    .as_deref()
                    .is_some_and(|note| note.contains("Maven") && note.contains("directory"))),
        "literal project reference must remain attached"
    );
    assert!(
        observation
            .store_interiors
            .iter()
            .all(|interior| interior.path != excluded),
        "shallow jar identification must not recreate an excluded unit"
    );
    let interior = observation
        .store_interiors
        .iter()
        .find(|interior| interior.path == kept)
        .expect("the retained packaged artifact is identified");
    assert!(
        interior
            .consequence
            .as_deref()
            .is_some_and(|consequence| consequence.contains("mvn package"))
    );
    assert!(
        interior
            .consumer_evidence
            .iter()
            .any(|evidence| evidence.detail.contains(&repo.display().to_string())),
        "the retained interior needs its declared source consumer"
    );
    assert!(
        interior
            .decision_evidence
            .iter()
            .any(|evidence| evidence.kind == FactKind::Recovery
                && evidence.subtype == FactSubtype::Rebuild),
        "project output needs rebuild evidence rather than unknown global-cache recovery"
    );
    let recovery: Vec<_> = interior
        .decision_evidence
        .iter()
        .filter(|evidence| evidence.kind == FactKind::Recovery)
        .collect();
    let recovery_json = serde_json::to_string(&recovery).unwrap();
    assert!(
        recovery_json.contains(&repo.display().to_string()),
        "recovery must identify the declaring source checkout: {recovery_json}"
    );
    assert!(
        !recovery_json.contains(&format!("source present at {}", kept.display())),
        "the generated jar must not be asserted to be its own source checkout"
    );
    let stored = report::report_scope_from_store(&scope, &store).unwrap();
    assert!(
        stored
            .store_interiors
            .iter()
            .all(|interior| interior.path != excluded)
    );
    let persisted = stored
        .store_interiors
        .iter()
        .find(|interior| interior.path == kept)
        .unwrap();
    assert_eq!(persisted.consumer_evidence, interior.consumer_evidence);
    assert_eq!(persisted.decision_evidence, interior.decision_evidence);
}
