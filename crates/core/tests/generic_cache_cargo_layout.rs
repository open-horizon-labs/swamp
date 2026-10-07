//! Cargo targets inside generic cache roots use their measured parent's
//! physical ownership and event window without project attribution.

use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Mutex,
};

static OBSERVATION_LOCK: Mutex<()> = Mutex::new(());
use swamp_core::{
    fs_events::{FsEventsPlan, FsEventsRequest, FsEventsSource, RefreshRefusal},
    locations::{Environment, Platform, Registry, StorageCategory},
    report::{self, ObservationParts},
    scope::{ScanConfig, resolve_effective_scope},
    work_counters,
};

struct Source {
    quiet: bool,
    changed: Vec<PathBuf>,
}

impl FsEventsSource for Source {
    fn replay(&self, request: &FsEventsRequest) -> FsEventsPlan {
        FsEventsPlan {
            incremental: self.quiet && request.since.event_id.is_some(),
            refusal: if self.quiet && request.since.event_id.is_some() {
                None
            } else {
                Some(RefreshRefusal::NoStoredEventId)
            },
            changed_dirs: self
                .changed
                .iter()
                .filter(|path| path.starts_with(&request.root))
                .cloned()
                .collect(),
            current_event_id: request.since.event_id.unwrap_or(0) + 1,
            device: fs::metadata(&request.root)
                .ok()
                .map(|metadata| metadata.dev()),
            device_uuid: None,
            // Even a cold refusal can be the start of a drained live stream;
            // it supplies the anchor the next observation can replay from.
            live: true,
            consume: None,
        }
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    cache: PathBuf,
    target: PathBuf,
    fake: PathBuf,
    alias: PathBuf,
    store: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let cache = home.join(".cache");
    let target = cache.join("tdongle-with-an-arbitrary-name");
    let payload = target.join("debug/deps/libgenuine.rlib");
    fs::create_dir_all(payload.parent().unwrap()).unwrap();
    fs::write(target.join(".rustc_info.json"), b"{}\n").unwrap();
    fs::write(
        target.join("CACHEDIR.TAG"),
        b"Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
    fs::write(&payload, vec![b'x'; 4097]).unwrap();
    for i in 0..200 {
        let dir = target.join(format!("debug/incremental/work-{i:03}/data"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("state.bin"), [i as u8; 31]).unwrap();
    }

    let fake = cache.join("tdongle-fake-target");
    fs::create_dir_all(fake.join("debug/deps")).unwrap();
    fs::write(fake.join("debug/deps/not-cargo.bin"), vec![b'n'; 513]).unwrap();
    let alias = home.join("excluded-target-alias");
    std::os::unix::fs::symlink(&target, &alias).unwrap();
    let store = home.join("store");
    fs::create_dir_all(&store).unwrap();
    Fixture {
        _tmp: tmp,
        home,
        cache,
        target,
        fake,
        alias,
        store,
    }
}

fn scope_with_include(
    fx: &Fixture,
    exclude: Option<&Path>,
    include: Option<&Path>,
) -> swamp_core::scope::EffectiveScope {
    let registry = Registry::with_builtins();
    let disabled_detectors = registry
        .detectors()
        .iter()
        .filter(|detector| detector.id() != "builtin-defaults")
        .map(|detector| detector.id().to_owned())
        .collect();
    let config = ScanConfig {
        disabled_detectors,
        include: include
            .map(|path| path.display().to_string())
            .into_iter()
            .collect(),
        exclude: exclude
            .map(|path| path.display().to_string())
            .into_iter()
            .collect(),
        ..Default::default()
    };
    let environment = Environment::fixture(fx.home.clone(), Default::default(), Platform::MacOS);
    resolve_effective_scope(&environment, &config, &[], &registry, 1_000)
}

fn scope(fx: &Fixture, exclude: Option<&Path>) -> swamp_core::scope::EffectiveScope {
    scope_with_include(fx, exclude, None)
}

fn observe(
    fx: &Fixture,
    exclude: Option<&Path>,
    source: &dyn FsEventsSource,
) -> report::ScopeObservation {
    observe_with(fx, exclude, None, false, source)
}

fn observe_with(
    fx: &Fixture,
    exclude: Option<&Path>,
    include: Option<&Path>,
    force_full: bool,
    source: &dyn FsEventsSource,
) -> report::ScopeObservation {
    report::observe_scope(
        &scope_with_include(fx, exclude, include),
        ObservationParts::ALL,
        None,
        None,
        false,
        Some(&fx.store),
        None,
        true,
        false,
        false,
        force_full,
        source,
        30,
        24 * 3600,
    )
    .expect("observe generic cache root")
}

fn git(repo: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
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

fn allocated_tree(path: &Path) -> u64 {
    fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                allocated_tree(&entry.path())
            } else {
                allocated(&entry.path())
            }
        })
        .sum()
}

fn cargo_interior<'a>(
    interiors: &'a [swamp_core::artifact::NestedArtifact],
    path: &Path,
) -> &'a swamp_core::artifact::NestedArtifact {
    interiors
        .iter()
        .find(|unit| unit.adapter.as_deref() == Some("cargo") && unit.path == path)
        .unwrap_or_else(|| panic!("missing Cargo interior {path:?}: {:?}", interiors))
}

#[test]
fn arbitrary_cargo_target_is_recognized_once_and_read_only_report_replays_it() {
    let _guard = OBSERVATION_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let fx = fixture();
    let first = observe(
        &fx,
        None,
        &Source {
            quiet: false,
            changed: Vec::new(),
        },
    );
    let cache_unit = first
        .external_units
        .iter()
        .find(|unit| unit.path == fx.cache && unit.category == StorageCategory::Cache)
        .expect("generic cache root is the physical unit");
    assert_eq!(cache_unit.bytes, allocated_tree(&fx.cache));
    assert_eq!(
        cache_unit.consumers.len(),
        0,
        "a path signature must not invent a consumer"
    );

    let deps = cargo_interior(&first.store_interiors, &fx.target.join("debug/deps"));
    assert!(deps.bytes > 0);
    let container_root = first
        .store_interiors
        .iter()
        .find(|unit| {
            unit.path == fx.cache && unit.id == unit.container_id.as_deref().unwrap_or_default()
        })
        .expect("renderable cache container anchor");
    assert!(!container_root.coverage.supported);
    assert!(container_root.consequence.is_none());
    assert_eq!(
        deps.container_id,
        Some(format!(
            "generic-cache-build-outputs:{}",
            swamp_core::artifact::NestedArtifact::storage_id(&fx.cache, "")
        ))
    );
    assert!(deps.consumer_evidence.is_empty());
    assert!(
        !deps
            .decision_evidence
            .iter()
            .any(|evidence| evidence.kind == swamp_core::evidence::FactKind::Consumer)
    );
    assert!(
        deps.consequence
            .as_deref()
            .unwrap()
            .contains("the layout alone does not establish which project")
    );
    let rendered = swamp_core::render::render_view_external_with(
        &first.external_units,
        &first.store_interiors,
        first.merged.observed_at,
    );
    assert!(rendered.contains("inside (identification only"));
    assert!(rendered.contains(&fx.cache.display().to_string()));
    assert!(rendered.contains("cargo"));
    assert!(
        !first
            .store_interiors
            .iter()
            .any(|unit| unit.path.starts_with(&fx.fake)),
        "a target-like name without Cargo's marker and profile layout is not Cargo"
    );

    let stored = report::report_scope_from_store(&scope(&fx, None), &fx.store).unwrap();
    assert_eq!(
        cargo_interior(&stored.store_interiors, &fx.target.join("debug/deps")),
        deps
    );
}

#[test]
fn excluded_target_is_subtracted_from_parent_and_never_becomes_an_interior() {
    let _guard = OBSERVATION_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let fx = fixture();
    let observed = observe(
        &fx,
        Some(&fx.target),
        &Source {
            quiet: false,
            changed: Vec::new(),
        },
    );
    let cache_unit = observed
        .external_units
        .iter()
        .find(|unit| unit.path == fx.cache)
        .expect("generic cache root remains visible");
    assert_eq!(
        cache_unit.bytes,
        allocated(&fx.fake.join("debug/deps/not-cargo.bin"))
    );
    assert!(
        !observed
            .store_interiors
            .iter()
            .any(|unit| unit.path.starts_with(&fx.target)),
        "an excluded Cargo target must not have interior history"
    );
}

#[test]
fn trusted_quiet_replay_reuses_generic_cache_cargo_interiors() {
    let _guard = OBSERVATION_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let fx = fixture();
    observe(
        &fx,
        None,
        &Source {
            quiet: false,
            changed: Vec::new(),
        },
    );
    std::thread::sleep(std::time::Duration::from_secs(2));
    work_counters::reset();
    let quiet = observe(
        &fx,
        None,
        &Source {
            quiet: true,
            changed: Vec::new(),
        },
    );
    let counts = work_counters::snapshot();
    assert!(
        counts.dirs_listed <= 4,
        "trusted cache replay listed directories: {counts:?}"
    );
    assert!(
        counts.files_statted <= 10,
        "trusted cache replay statted files: {counts:?}"
    );
    cargo_interior(&quiet.store_interiors, &fx.target.join("debug/deps"));

    work_counters::reset();
    let quiet_again = observe(
        &fx,
        None,
        &Source {
            quiet: true,
            changed: Vec::new(),
        },
    );
    let second_counts = work_counters::snapshot();
    assert!(
        second_counts.dirs_listed <= 4,
        "second trusted refresh listed directories: {second_counts:?}"
    );
    cargo_interior(&quiet_again.store_interiors, &fx.target.join("debug/deps"));

    work_counters::reset();
    observe_with(
        &fx,
        None,
        None,
        true,
        &Source {
            quiet: true,
            changed: Vec::new(),
        },
    );
    let full_counts = work_counters::snapshot();
    assert!(
        full_counts.dirs_listed > 200,
        "forced full observation must pay the measured traversal cost: {full_counts:?}"
    );
}

#[test]
fn canonical_alias_exclusion_blocks_the_same_cargo_descendant() {
    let _guard = OBSERVATION_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let fx = fixture();
    let observed = observe(
        &fx,
        Some(&fx.alias),
        &Source {
            quiet: false,
            changed: Vec::new(),
        },
    );
    assert!(
        !observed
            .store_interiors
            .iter()
            .any(|unit| unit.path.starts_with(&fx.target)),
        "an excluded symlink alias must suppress the canonical Cargo descendant"
    );
}

#[test]
fn cached_negative_layout_becomes_cargo_after_a_reported_signature_change() {
    let _guard = OBSERVATION_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let fx = fixture();
    fs::remove_dir_all(&fx.target).unwrap();
    observe(
        &fx,
        None,
        &Source {
            quiet: false,
            changed: Vec::new(),
        },
    );
    std::thread::sleep(std::time::Duration::from_secs(2));
    work_counters::reset();
    let negative_replay = observe(
        &fx,
        None,
        &Source {
            quiet: true,
            changed: Vec::new(),
        },
    );
    assert!(work_counters::snapshot().dirs_listed <= 4);
    assert!(negative_replay.store_interiors.is_empty());

    let payload = fx.target.join("debug/deps/libappeared.rlib");
    fs::create_dir_all(payload.parent().unwrap()).unwrap();
    fs::write(fx.target.join(".rustc_info.json"), b"{}\n").unwrap();
    fs::write(
        fx.target.join("CACHEDIR.TAG"),
        b"Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
    fs::write(&payload, vec![b'p'; 2048]).unwrap();
    work_counters::reset();
    let updated = observe(
        &fx,
        None,
        &Source {
            quiet: true,
            changed: vec![fx.target.clone()],
        },
    );
    cargo_interior(&updated.store_interiors, &fx.target.join("debug/deps"));
    assert!(
        work_counters::snapshot().dirs_listed > 0,
        "a reported signature change must refresh the cached negative layout"
    );
}

#[test]
fn old_generic_root_without_adapter_state_is_refolded_before_cargo_discovery() {
    let _guard = OBSERVATION_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let fx = fixture();
    observe(
        &fx,
        None,
        &Source {
            quiet: false,
            changed: Vec::new(),
        },
    );

    // A pre-feature generic-root observation has the external folded total
    // but no Cargo container result. Remove only the typed build-store table
    // to model that stored state while keeping the trusted root fold.
    fs::remove_file(fx.store.join("associations/build_stores.parquet")).unwrap();
    std::thread::sleep(std::time::Duration::from_secs(2));
    work_counters::reset();
    let migrated = observe(
        &fx,
        None,
        &Source {
            quiet: true,
            changed: Vec::new(),
        },
    );
    cargo_interior(&migrated.store_interiors, &fx.target.join("debug/deps"));
    assert!(
        work_counters::snapshot().dirs_listed > 200,
        "without a stored container result, the trusted old root must be refolded before discovery"
    );
}

#[test]
fn configured_child_output_is_excluded_from_cache_parent_and_counted_once() {
    let _guard = OBSERVATION_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let fx = fixture();
    let repo = fx.home.join("repo");
    fs::create_dir_all(repo.join(".cargo")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname='cache-output-consumer'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
    fs::write(
        repo.join(".cargo/config.toml"),
        "[build]\ntarget-dir='../.cache/tdongle-with-an-arbitrary-name'\n",
    )
    .unwrap();
    git(&repo, &["add", "Cargo.toml", ".cargo/config.toml"]);
    git(&repo, &["commit", "-q", "-m", "fixture"]);

    let observed = observe_with(
        &fx,
        None,
        Some(&repo),
        false,
        &Source {
            quiet: false,
            changed: Vec::new(),
        },
    );
    let parent = observed
        .external_units
        .iter()
        .find(|unit| unit.path == fx.cache)
        .expect("generic cache parent remains the physical unit");
    let output = observed
        .external_units
        .iter()
        .find(|unit| unit.path == fx.target && unit.category == StorageCategory::BuildOutput)
        .expect("configured target output has its own measured unit");
    assert_eq!(parent.bytes, allocated_tree(&fx.fake));
    assert_eq!(output.bytes, allocated_tree(&fx.target));
    assert_eq!(parent.bytes + output.bytes, allocated_tree(&fx.cache));
    assert_eq!(
        observed
            .external_units
            .iter()
            .filter(|unit| unit.path == fx.target)
            .count(),
        1,
        "the configured output is one physical external unit"
    );
    assert_eq!(
        observed
            .store_interiors
            .iter()
            .filter(|unit| {
                unit.adapter.as_deref() == Some("cargo")
                    && unit.path == fx.target.join("debug/deps")
            })
            .count(),
        1,
        "the cache parent fold excluded this path, so the configured adapter owns its interior once"
    );
}
