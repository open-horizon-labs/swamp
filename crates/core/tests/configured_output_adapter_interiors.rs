//! Configured external outputs retain adapter-specific interiors and their
//! storage evidence through a stored-report round trip (#226).

use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::Command,
};
use swamp_core::{
    artifact::ArtifactRole,
    fs_events::{FsEventsPlan, FsEventsRequest, FsEventsSource, RefreshRefusal},
    locations::{Environment, Platform, Registry as LocationRegistry, StorageCategory},
    report::{self, ObservationParts},
    scope::{ScanConfig, resolve_effective_scope},
};

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    repo: PathBuf,
    store: PathBuf,
    cargo_target: PathBuf,
    maven_classes: PathBuf,
    cargo_file: PathBuf,
    maven_file: PathBuf,
}

struct FirstPass;

impl FsEventsSource for FirstPass {
    fn replay(&self, request: &FsEventsRequest) -> FsEventsPlan {
        FsEventsPlan {
            incremental: false,
            refusal: Some(RefreshRefusal::NoStoredEventId),
            changed_dirs: Vec::new(),
            current_event_id: 1,
            device: fs::metadata(&request.root).ok().map(|meta| meta.dev()),
            device_uuid: None,
            live: false,
            consume: None,
        }
    }
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.test",
            "-C",
        ])
        .arg(repo)
        .args(args)
        .output()
        .expect("run git fixture command");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn allocated(path: &Path) -> u64 {
    fs::metadata(path).unwrap().blocks() * 512
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let repo = home.join("repo");
    fs::create_dir_all(repo.join(".cargo")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname='configured-output-fixture'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
    fs::write(
        repo.join("pom.xml"),
        "<project><build><outputDirectory>../maven-classes</outputDirectory></build></project>",
    )
    .unwrap();
    fs::write(
        repo.join(".cargo/config.toml"),
        "[build]\ntarget-dir='../cargo-target'\n",
    )
    .unwrap();
    git(
        &repo,
        &["add", "Cargo.toml", "pom.xml", ".cargo/config.toml"],
    );
    git(&repo, &["commit", "-q", "-m", "fixture"]);

    let cargo_target = home.join("cargo-target");
    let cargo_file = cargo_target.join("debug/deps/libfixture-a1b2c3.rlib");
    fs::create_dir_all(cargo_file.parent().unwrap()).unwrap();
    fs::write(&cargo_file, vec![b'c'; 4_321]).unwrap();

    let maven_classes = home.join("maven-classes");
    let maven_file = maven_classes.join("org/example/Fixture.class");
    fs::create_dir_all(maven_file.parent().unwrap()).unwrap();
    fs::write(&maven_file, vec![b'm'; 2_345]).unwrap();

    let store = home.join("store");
    fs::create_dir_all(&store).unwrap();
    Fixture {
        _tmp: tmp,
        home,
        repo,
        store,
        cargo_target,
        maven_classes,
        cargo_file,
        maven_file,
    }
}

fn scope(fx: &Fixture) -> swamp_core::scope::EffectiveScope {
    let registry = LocationRegistry::with_builtins();
    let config = ScanConfig {
        defaults: false,
        include: vec![fx.repo.display().to_string()],
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|detector| detector.id().to_owned())
            .collect(),
        ..Default::default()
    };
    let environment = Environment::fixture(fx.home.clone(), Default::default(), Platform::MacOS);
    resolve_effective_scope(&environment, &config, &[], &registry, 1_000)
}

fn observe(fx: &Fixture) -> report::ScopeObservation {
    report::observe_scope(
        &scope(fx),
        ObservationParts::ALL,
        None,
        None,
        false,
        Some(&fx.store),
        None,
        true,
        false,
        false,
        true,
        &FirstPass,
        30,
        24 * 3600,
    )
    .expect("observe configured output fixture")
}

fn external<'a>(
    units: &'a [swamp_core::external::ExternalUnit],
    detector_id: &str,
    path: &Path,
) -> &'a swamp_core::external::ExternalUnit {
    let canonical = fs::canonicalize(path).unwrap();
    units
        .iter()
        .find(|unit| {
            unit.detector_id == format!("configured-build:{detector_id}")
                && unit.category == StorageCategory::BuildOutput
                && unit.path == canonical
        })
        .unwrap_or_else(|| panic!("missing {detector_id} output {canonical:?}: {units:?}"))
}

fn interior<'a>(
    units: &'a [swamp_core::artifact::NestedArtifact],
    adapter: &str,
    path: &Path,
) -> &'a swamp_core::artifact::NestedArtifact {
    units
        .iter()
        .find(|unit| unit.adapter.as_deref() == Some(adapter) && unit.path == path)
        .unwrap_or_else(|| panic!("missing {adapter} interior {path:?}: {units:?}"))
}

#[test]
fn external_cargo_and_maven_outputs_keep_measured_interiors_and_evidence() {
    let fx = fixture();
    let observed = observe(&fx);
    let cargo = external(&observed.external_units, "cargo", &fx.cargo_target);
    let maven = external(&observed.external_units, "maven", &fx.maven_classes);

    assert_eq!(cargo.bytes, allocated(&fx.cargo_file));
    assert_eq!(maven.bytes, allocated(&fx.maven_file));
    assert!(matches!(
        &cargo.provenance,
        swamp_core::locations::Provenance::ConfigField(source) if source.contains("target-dir")
    ));
    assert!(matches!(
        &maven.provenance,
        swamp_core::locations::Provenance::ConfigField(source) if source.contains("outputDirectory")
    ));

    let cargo_deps = interior(
        &observed.store_interiors,
        "cargo",
        &fx.cargo_target.join("debug/deps"),
    );
    assert_eq!(cargo_deps.role, ArtifactRole::Dependency);
    assert_eq!(cargo_deps.bytes, allocated(&fx.cargo_file));
    assert!(
        cargo_deps
            .producer_evidence
            .iter()
            .any(|evidence| evidence.source.starts_with("cargo-"))
    );

    let maven_classes = interior(&observed.store_interiors, "maven", &fx.maven_classes);
    assert_eq!(maven_classes.role, ArtifactRole::Output);
    assert_eq!(maven_classes.bytes, allocated(&fx.maven_file));
    assert!(
        maven_classes
            .producer_evidence
            .iter()
            .any(|evidence| evidence.source == "maven-layout")
    );

    let stored = report::report_scope_from_store(&scope(&fx), &fx.store).unwrap();
    assert_eq!(
        external(&stored.external_units, "cargo", &fx.cargo_target).bytes,
        cargo.bytes
    );
    assert_eq!(
        external(&stored.external_units, "maven", &fx.maven_classes).bytes,
        maven.bytes
    );
    assert_eq!(
        interior(
            &stored.store_interiors,
            "cargo",
            &fx.cargo_target.join("debug/deps")
        ),
        cargo_deps
    );
    assert_eq!(
        interior(&stored.store_interiors, "maven", &fx.maven_classes),
        maven_classes
    );
}
