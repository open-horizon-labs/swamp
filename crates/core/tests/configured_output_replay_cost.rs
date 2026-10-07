//! Independent output cursors must remove work, not merely preserve totals.
use std::os::unix::fs::MetadataExt;
use std::{
    fs,
    path::Path,
    process::Command,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use swamp_core::{
    fs_events::{FsEventsPlan, FsEventsRequest, FsEventsSource, RefreshRefusal},
    locations::{Environment, Platform, Registry, StorageCategory},
    report::{self, ObservationParts},
    scope::{ScanConfig, resolve_effective_scope},
    work_counters,
};

// Work counters cover the process, including walker threads. Serialize these
// fixtures so each measured observation excludes work from the other test.
static COUNTER_FIXTURE: Mutex<()> = Mutex::new(());

#[derive(Default)]
struct CapturingSource {
    requests: Mutex<Vec<FsEventsRequest>>,
    delayed_project: Option<std::path::PathBuf>,
    did_delay: AtomicBool,
}

impl FsEventsSource for CapturingSource {
    fn replay(&self, request: &FsEventsRequest) -> FsEventsPlan {
        self.requests.lock().unwrap().push(request.clone());
        if self.delayed_project.as_ref() == Some(&request.root)
            && !self.did_delay.swap(true, Ordering::SeqCst)
        {
            // The project observation has already chosen its epoch. Force
            // its configured output anchor into a later second, so cached
            // output rows cannot accidentally match an earlier project time.
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        let seeded = request.since.event_id.is_some();
        FsEventsPlan {
            incremental: seeded,
            refusal: (!seeded).then_some(RefreshRefusal::NoStoredEventId),
            changed_dirs: Vec::new(),
            current_event_id: request.since.event_id.unwrap_or(0) + 1,
            device: Some(fs::metadata(&request.root).unwrap().dev()),
            device_uuid: None,
            // Model a fully drained live stream. Persisted FSEvents logs
            // correctly refuse immediate replay through the TooSoon floor.
            live: seeded,
            consume: None,
        }
    }
}

fn output_bytes(observation: &report::ScopeObservation, output: &Path) -> u64 {
    observation
        .external_units
        .iter()
        .find(|unit| unit.path == output && unit.category == StorageCategory::BuildOutput)
        .expect("configured output must be observed")
        .bytes
}

#[test]
fn configured_output_has_its_own_cursor_and_quiet_replay_avoids_member_work() {
    replay_cost(false);
}

#[test]
fn cargo_and_typescript_aliases_keep_consumers_and_replay_one_physical_output() {
    replay_cost(true);
}

fn replay_cost(shared_across_adapters: bool) {
    let _counter_fixture = COUNTER_FIXTURE.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(temp.path()).unwrap();
    let repo = home.join("repo");
    let output = home.join("external-output");
    let store = home.join("store");
    fs::create_dir_all(&repo).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(&repo)
            .status()
            .unwrap()
            .success()
    );
    fs::write(repo.join("package.json"), "{}").unwrap();
    let declared_output = if shared_across_adapters {
        let alias = home.join("typescript-output-alias");
        std::os::unix::fs::symlink(&output, &alias).unwrap();
        alias
    } else {
        output.clone()
    };
    fs::write(
        repo.join("tsconfig.json"),
        serde_json::json!({
            "compilerOptions": {"outDir": declared_output}
        })
        .to_string(),
    )
    .unwrap();
    // A recursive remeasure has a measurable cost even when totals agree.
    for index in 0..200 {
        let directory = output.join(format!("part-{index:03}"));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("generated.js"), vec![b'x'; 4096]).unwrap();
    }
    let mut source_roots = vec![repo.display().to_string()];
    let rust_repo = home.join("rust-consumer");
    if shared_across_adapters {
        fs::create_dir_all(rust_repo.join(".cargo")).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(&rust_repo)
                .status()
                .unwrap()
                .success()
        );
        fs::write(rust_repo.join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(
            rust_repo.join(".cargo/config.toml"),
            format!("[build]\ntarget-dir = \"{}\"\n", output.display()),
        )
        .unwrap();
        source_roots.push(rust_repo.display().to_string());
    }
    let registry = Registry::with_builtins();
    let config = ScanConfig {
        defaults: false,
        include: source_roots,
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_owned())
            .collect(),
        ..Default::default()
    };
    let environment = Environment::fixture(home, Default::default(), Platform::MacOS);
    let scope = resolve_effective_scope(&environment, &config, &[], &registry, 1000);
    let source = CapturingSource {
        delayed_project: shared_across_adapters.then(|| repo.clone()),
        ..Default::default()
    };
    let observe = |force_full| {
        report::observe_scope(
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
            force_full,
            &source,
            30,
            86400,
        )
        .unwrap()
    };
    let (first, cold) = work_counters::measured(|| observe(false));
    assert!(
        cold.dirs_listed >= 201 && cold.files_statted >= 200,
        "cold counters must see the output traversal: {cold:?}"
    );
    source.requests.lock().unwrap().clear();
    let (second, warm) = work_counters::measured(|| observe(false));
    let requests = source.requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .any(|request| request.root == output && request.since.event_id.is_some()),
        "the external output needs an independently seeded cursor; requested roots: {:?}",
        requests
            .iter()
            .map(|request| &request.root)
            .collect::<Vec<_>>()
    );
    if shared_across_adapters {
        let output_request = requests
            .iter()
            .find(|request| request.root == output)
            .unwrap();
        assert!(
            output_request.since.last_observed_at.unwrap() > first.merged.observed_at,
            "the counterexample must cross the project/output observation boundary"
        );
    }
    drop(requests);
    assert_eq!(
        output_bytes(&first, &output),
        output_bytes(&second, &output)
    );
    if shared_across_adapters {
        for observation in [&first, &second] {
            let units: Vec<_> = observation
                .external_units
                .iter()
                .filter(|unit| unit.path == output)
                .collect();
            assert_eq!(
                units.len(),
                1,
                "both adapters and alias retain one physical owner"
            );
            let expected: u64 = (0..200)
                .map(|index| {
                    fs::metadata(output.join(format!("part-{index:03}/generated.js")))
                        .unwrap()
                        .blocks()
                        * 512
                })
                .sum();
            assert_eq!(
                units[0].bytes, expected,
                "physical allocation is charged once"
            );
            for consumer_root in [&repo, &rust_repo] {
                assert!(
                    units[0]
                        .consumers
                        .iter()
                        .any(|consumer| consumer.label == consumer_root.display().to_string()),
                    "every cold and replayed output retains both consumers: {:?}",
                    units[0].consumers
                );
            }
            let interior = observation
                .store_interiors
                .iter()
                .find(|interior| interior.path == output)
                .expect("the chosen adapter identifies the shared output root");
            for consumer_root in [&repo, &rust_repo] {
                assert!(
                    interior.consumer_evidence.iter().any(|evidence| evidence
                        .detail
                        .contains(&consumer_root.display().to_string())),
                    "the shared interior retains every declared consumer on cold and warm reports: {:?}",
                    interior.consumer_evidence
                );
            }
            assert!(
                interior
                    .consumer_evidence
                    .iter()
                    .any(
                        |evidence| evidence.source == "configured-output-interpretation"
                            && evidence.detail.contains("one build command")
                    ),
                "mixed adapter references need an explicit recovery limit"
            );
            assert!(
                interior
                    .consequence
                    .as_deref()
                    .is_some_and(|consequence| consequence
                        .contains("does not establish recovery for every consumer")),
                "the selected adapter's repair command must carry the shared-output limit"
            );
            assert!(
                interior
                    .decision_evidence
                    .iter()
                    .all(
                        |evidence| evidence.kind != swamp_core::evidence::FactKind::Recovery
                            || evidence.subtype != swamp_core::evidence::FactSubtype::Rebuild
                    ),
                "mixed declarations do not establish one recovery build for the shared content"
            );
        }
        let stored = report::report_scope_from_store(&scope, &store).unwrap();
        let warm_interior = second
            .store_interiors
            .iter()
            .find(|interior| interior.path == output)
            .unwrap();
        let cold_interior = first
            .store_interiors
            .iter()
            .find(|interior| interior.path == output)
            .unwrap();
        assert_eq!(
            cold_interior.decision_evidence.len(),
            warm_interior.decision_evidence.len(),
            "quiet replay must refresh derived evidence without accumulating old facts"
        );
        let stored_interior = stored
            .store_interiors
            .iter()
            .find(|interior| interior.path == output)
            .expect("the shared interior survives stored report reconstruction");
        assert_eq!(
            stored_interior.consumer_evidence,
            warm_interior.consumer_evidence
        );
        assert_eq!(
            stored_interior.decision_evidence,
            warm_interior.decision_evidence
        );
        assert_eq!(stored_interior.consequence, warm_interior.consequence);
    }
    let root_work_limit = if shared_across_adapters { 20 } else { 10 };
    assert!(
        warm.dirs_listed <= root_work_limit && warm.files_statted <= root_work_limit,
        "quiet output replay must cost roots, not its 200 member directories/files: {warm:?}"
    );
    assert!(
        warm.identification_cache_hits > 0 || warm.containers_reused > 0,
        "low counters must be explained by actual reuse: {warm:?}"
    );
    // The replayed fold and interior cache must advance together, rather
    // than alternating between one cheap observation and one full walk.
    let (third, warm_again) = work_counters::measured(|| observe(false));
    assert_eq!(
        output_bytes(&second, &output),
        output_bytes(&third, &output)
    );
    assert!(
        warm_again.dirs_listed <= root_work_limit && warm_again.files_statted <= root_work_limit,
        "a second quiet observation must retain replay eligibility: {warm_again:?}"
    );
    assert!(warm_again.identification_cache_hits > 0 || warm_again.containers_reused > 0);
    let (full, forced) = work_counters::measured(|| observe(true));
    assert_eq!(output_bytes(&second, &output), output_bytes(&full, &output));
    assert!(
        forced.dirs_listed >= 201 && forced.files_statted >= 200,
        "force_full must pay the output traversal even with a warm cache: {forced:?}"
    );
}
