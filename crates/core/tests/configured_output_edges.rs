//! Adversarial integration coverage for declared build outputs (#226).
//!
//! These declarations are useful only when shared paths have one owner,
//! config changes are seen on the next observation, and output-local events
//! refresh the stored row independently of project walks.

use std::os::unix::fs::MetadataExt;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};
use swamp_core::{
    fs_events::{FsEventsPlan, FsEventsRequest, FsEventsSource, RefreshRefusal},
    locations::{Environment, Platform, Registry, StorageCategory},
    report::{self, ObservationParts},
    scope::{ScanConfig, resolve_effective_scope},
};

fn allocated(path: &Path) -> u64 {
    fs::metadata(path).unwrap().blocks() * 512
}

struct ScriptedSource {
    event_id: u64,
    changes: Vec<PathBuf>,
}

impl ScriptedSource {
    fn quiet(event_id: u64) -> Self {
        Self {
            event_id,
            changes: Vec::new(),
        }
    }

    fn changed(event_id: u64, changes: Vec<PathBuf>) -> Self {
        Self { event_id, changes }
    }
}

impl FsEventsSource for ScriptedSource {
    fn replay(&self, request: &FsEventsRequest) -> FsEventsPlan {
        if request.since.event_id.is_none() {
            return FsEventsPlan {
                incremental: false,
                refusal: Some(RefreshRefusal::NoStoredEventId),
                changed_dirs: Vec::new(),
                current_event_id: self.event_id,
                device: fs::metadata(&request.root).ok().map(|m| m.dev()),
                device_uuid: None,
                live: false,
                consume: None,
            };
        }
        FsEventsPlan {
            incremental: true,
            refusal: None,
            changed_dirs: self
                .changes
                .iter()
                .filter(|p| p.starts_with(&request.root))
                .cloned()
                .collect(),
            current_event_id: self.event_id,
            device: fs::metadata(&request.root).ok().map(|m| m.dev()),
            device_uuid: None,
            live: true,
            consume: None,
        }
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    repo: PathBuf,
    second_repo: PathBuf,
    store: PathBuf,
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(["-c", "commit.gpgsign=false"])
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
        .output()
        .expect("run git fixture command");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn make_repo(repo: &Path) {
    fs::create_dir_all(repo).unwrap();
    git(repo, &["init", "-q", "-b", "main"]);
    fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(repo.join("package.json"), "{}\n").unwrap();
    git(repo, &["add", "Cargo.toml", "package.json"]);
    git(repo, &["commit", "-q", "-m", "fixture"]);
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let repo = home.join("repo");
    make_repo(&repo);
    let second_repo = home.join("second-repo");
    make_repo(&second_repo);
    let store = home.join("store");
    fs::create_dir_all(&store).unwrap();
    Fixture {
        _tmp: tmp,
        home,
        repo,
        second_repo,
        store,
    }
}

fn scope(fx: &Fixture, exclude: Vec<String>) -> swamp_core::scope::EffectiveScope {
    let registry = Registry::with_builtins();
    let config = ScanConfig {
        defaults: false,
        include: vec![
            fx.repo.display().to_string(),
            fx.second_repo.display().to_string(),
        ],
        exclude,
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .collect(),
        ..Default::default()
    };
    let env = Environment::fixture(fx.home.clone(), Default::default(), Platform::MacOS);
    resolve_effective_scope(&env, &config, &[], &registry, 1_000)
}

fn observe(fx: &Fixture, source: &dyn FsEventsSource) -> report::ScopeObservation {
    observe_with(fx, source, false)
}

fn observe_with(fx: &Fixture, source: &dyn FsEventsSource, full: bool) -> report::ScopeObservation {
    let selected = scope(fx, Vec::new());
    report::observe_scope(
        &selected,
        ObservationParts::ALL,
        None,
        None,
        false,
        Some(&fx.store),
        None,
        true,
        false,
        false,
        full,
        source,
        30,
        24 * 3600,
    )
    .expect("observe configured-output fixture")
}

fn build_output<'a>(
    units: &'a [swamp_core::external::ExternalUnit],
    path: &Path,
) -> &'a swamp_core::external::ExternalUnit {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let matches: Vec<_> = units
        .iter()
        .filter(|unit| unit.category == StorageCategory::BuildOutput && unit.path == canonical)
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected one BuildOutput unit for {}, got {:?}",
        canonical.display(),
        units
            .iter()
            .map(|u| (&u.detector_id, &u.category, &u.path))
            .collect::<Vec<_>>()
    );
    matches[0]
}

#[test]
fn cargo_and_typescript_aliases_share_one_canonical_unit_and_consumer() {
    let fx = fixture();
    let real_output = fx.home.join("shared-output");
    let alias = fx.home.join("output-alias");
    fs::create_dir_all(&real_output).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real_output, &alias).unwrap();
    #[cfg(not(unix))]
    panic!("this symlink identity test requires the Unix CI platforms");
    fs::write(real_output.join("artifact.bin"), vec![b'x'; 4096]).unwrap();
    fs::create_dir_all(fx.repo.join(".cargo")).unwrap();
    fs::write(
        fx.repo.join(".cargo/config.toml"),
        "[build]\ntarget-dir = \"../shared-output\"\n",
    )
    .unwrap();
    fs::write(
        fx.second_repo.join("tsconfig.json"),
        r#"{"compilerOptions":{"outDir":"../output-alias"}}"#,
    )
    .unwrap();

    let first = observe(&fx, &ScriptedSource::quiet(10));
    let unit = build_output(&first.external_units, &real_output);
    assert_eq!(
        unit.bytes,
        allocated(&real_output.join("artifact.bin")),
        "the shared directory contributes its bytes once"
    );
    assert!(
        unit.consumers
            .iter()
            .any(|consumer| consumer.label.contains("repo")
                && !consumer.label.contains("second-repo")),
        "Cargo project's consumer is retained: {:?}",
        unit.consumers
    );
    assert!(
        unit.consumers
            .iter()
            .any(|consumer| consumer.label.contains("second-repo")),
        "TypeScript project's consumer is retained: {:?}",
        unit.consumers
    );
    assert!(
        unit.consumers.iter().all(|consumer| consumer
            .note
            .as_deref()
            .is_some_and(|note| note.contains("target-dir") || note.contains("outDir"))),
        "declaration evidence must stay attached: {:?}",
        unit.consumers
    );
    assert_eq!(
        first.external_units.iter().filter(|unit| unit.category == StorageCategory::BuildOutput && unit.path == real_output).count(),
        1,
        "Cargo's real path and TypeScript's symlink alias must canonicalize to one unit"
    );
}

#[test]
fn excluded_configured_outputs_do_not_become_external_units() {
    let fx = fixture();
    let output = fx.home.join("outside-output");
    fs::create_dir_all(&output).unwrap();
    fs::write(output.join("artifact.bin"), vec![b'x'; 2048]).unwrap();
    fs::write(
        fx.repo.join("tsconfig.json"),
        serde_json::json!({"compilerOptions":{"outDir": output}}).to_string(),
    )
    .unwrap();
    let alias = fx.home.join("excluded-alias");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&output, &alias).unwrap();
    #[cfg(not(unix))]
    panic!("this canonical exclusion test requires the Unix CI platforms");

    let registry = Registry::with_builtins();
    let config = ScanConfig {
        defaults: false,
        include: vec![fx.repo.display().to_string()],
        exclude: vec![alias.display().to_string()],
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .collect(),
        ..Default::default()
    };
    let env = Environment::fixture(fx.home.clone(), Default::default(), Platform::MacOS);
    let selected = resolve_effective_scope(&env, &config, &[], &registry, 1_000);
    let observed = report::observe_scope(
        &selected,
        ObservationParts::ALL,
        None,
        None,
        false,
        Some(&fx.store),
        None,
        true,
        false,
        false,
        false,
        &ScriptedSource::quiet(10),
        30,
        24 * 3600,
    )
    .expect("observe excluded output");
    assert!(
        observed
            .external_units
            .iter()
            .all(|unit| unit.path != output),
        "a canonical alias exclusion must cover the declaration: {:?}",
        observed
            .external_units
            .iter()
            .map(|u| &u.path)
            .collect::<Vec<_>>()
    );
}

#[test]
fn outputs_inside_measured_worktrees_stay_with_the_project() {
    let fx = fixture();
    let output = fx.second_repo.join("generated");
    fs::create_dir_all(&output).unwrap();
    fs::write(output.join("artifact.bin"), vec![b'x'; 1536]).unwrap();
    fs::write(
        fx.repo.join("tsconfig.json"),
        serde_json::json!({"compilerOptions":{"outDir": output}}).to_string(),
    )
    .unwrap();
    let observation = observe(&fx, &ScriptedSource::quiet(10));
    assert!(
        observation
            .external_units
            .iter()
            .all(|unit| unit.path != output),
        "an output inside another measured worktree stays with that project: {:?}",
        observation
            .external_units
            .iter()
            .map(|unit| (&unit.category, &unit.path))
            .collect::<Vec<_>>()
    );
    assert!(
        observation
            .merged
            .configured_outputs
            .iter()
            .any(|reference| reference.path == output && reference.project_root == fx.repo)
    );
    assert!(
        observation
            .merged
            .notes
            .iter()
            .any(|note| note.contains(&output.display().to_string())
                && note.contains(&fx.repo.display().to_string())
                && note.contains("counted under worktree")),
        "the declaring consumer and physical owner must remain visible in a report note"
    );
    let stored = report::report_scope_from_store(&scope(&fx, Vec::new()), &fx.store).unwrap();
    assert_eq!(
        stored.report.configured_outputs,
        observation.merged.configured_outputs
    );
}

#[test]
fn nested_configured_outputs_do_not_double_count_their_shared_tree() {
    let fx = fixture();
    let outer = fx.home.join("shared-build-tree");
    let inner = outer.join("nested-output");
    fs::create_dir_all(&inner).unwrap();
    fs::write(outer.join("outer.bin"), vec![b'x'; 1024]).unwrap();
    fs::write(inner.join("inner.bin"), vec![b'y'; 2048]).unwrap();
    fs::create_dir_all(fx.repo.join(".cargo")).unwrap();
    fs::write(
        fx.repo.join(".cargo/config.toml"),
        "[build]\ntarget-dir = \"../shared-build-tree\"\n",
    )
    .unwrap();
    fs::write(
        fx.second_repo.join("tsconfig.json"),
        r#"{"compilerOptions":{"outDir":"../shared-build-tree/nested-output"}}"#,
    )
    .unwrap();

    let observation = observe(&fx, &ScriptedSource::quiet(10));
    let measured: u64 = observation
        .external_units
        .iter()
        .filter(|unit| {
            unit.category == StorageCategory::BuildOutput && unit.path.starts_with(&outer)
        })
        .map(|unit| unit.bytes)
        .sum();
    assert_eq!(
        measured,
        allocated(&outer.join("outer.bin")) + allocated(&inner.join("inner.bin")),
        "parent and nested declarations must account for the shared bytes exactly once: {:?}",
        observation
            .external_units
            .iter()
            .filter(|unit| unit.path.starts_with(&outer))
            .map(|unit| (&unit.path, unit.bytes))
            .collect::<Vec<_>>()
    );
}

#[test]
fn changing_a_declaration_retargets_the_external_unit() {
    let fx = fixture();
    let old_output = fx.home.join("old-output");
    let new_output = fx.home.join("new-output");
    fs::create_dir_all(&old_output).unwrap();
    fs::create_dir_all(&new_output).unwrap();
    fs::write(old_output.join("old.bin"), vec![b'x'; 1024]).unwrap();
    fs::write(new_output.join("new.bin"), vec![b'y'; 2048]).unwrap();
    let config = fx.repo.join("tsconfig.json");
    fs::write(
        &config,
        serde_json::json!({"compilerOptions":{"outDir": old_output}}).to_string(),
    )
    .unwrap();
    let initial = observe(&fx, &ScriptedSource::quiet(10));
    assert_eq!(
        build_output(&initial.external_units, &old_output).bytes,
        allocated(&old_output.join("old.bin"))
    );

    fs::write(
        &config,
        serde_json::json!({"compilerOptions":{"outDir": new_output}}).to_string(),
    )
    .unwrap();
    let retargeted = observe(&fx, &ScriptedSource::quiet(20));
    assert_eq!(
        build_output(&retargeted.external_units, &new_output).bytes,
        allocated(&new_output.join("new.bin"))
    );
    assert!(
        retargeted
            .external_units
            .iter()
            .all(|unit| unit.path != old_output),
        "a retargeted declaration must not keep the old output active: {:?}",
        retargeted
            .external_units
            .iter()
            .map(|unit| &unit.path)
            .collect::<Vec<_>>()
    );
}

#[test]
fn output_event_refreshes_bytes_and_stored_report_preserves_the_unit() {
    let fx = fixture();
    let output = fx.home.join("outside-output");
    fs::create_dir_all(&output).unwrap();
    let victim = output.join("artifact.bin");
    let payloads = swamp_core::fs_gate::settle::noise(1024 + 3 * 32768);
    fs::write(&victim, &payloads[..1024]).unwrap();
    swamp_core::fs_gate::settle::settle();
    fs::write(
        fx.repo.join("tsconfig.json"),
        serde_json::json!({"compilerOptions":{"outDir": output}}).to_string(),
    )
    .unwrap();

    let first = observe(&fx, &ScriptedSource::quiet(10));
    let initial_bytes = allocated(&victim);
    assert_eq!(
        build_output(&first.external_units, &output).bytes,
        initial_bytes
    );
    let quiet = observe(&fx, &ScriptedSource::quiet(20));
    assert_eq!(
        build_output(&quiet.external_units, &output).bytes,
        initial_bytes,
        "a trusted quiet window reuses the independent output row"
    );
    let root_stamp = fs::metadata(&output).unwrap().modified().unwrap();
    let mut append_index = 0;
    let mut append = || {
        let mut file = fs::OpenOptions::new().append(true).open(&victim).unwrap();
        let start = 1024 + append_index * 32768;
        let end = start + 32768;
        file.write_all(&payloads[start..end]).unwrap();
        append_index += 1;
        file.sync_all().unwrap();
        swamp_core::fs_gate::settle::settle();
        assert_eq!(
            fs::metadata(&output).unwrap().modified().unwrap(),
            root_stamp,
            "appending a file must leave the output directory timestamp unchanged"
        );
        allocated(&victim)
    };
    let full_bytes = append();
    assert!(
        full_bytes > initial_bytes,
        "the full-refresh append must change allocated bytes"
    );
    let full = observe_with(&fx, &ScriptedSource::quiet(30), true);
    assert_eq!(
        build_output(&full.external_units, &output).bytes,
        full_bytes,
        "explicit full observation must refresh output bytes even without directory changes"
    );
    let no_window_bytes = append();
    assert!(
        no_window_bytes > full_bytes,
        "the no-window append must change allocated bytes"
    );
    let no_window = observe(&fx, &swamp_core::fs_events::UnsupportedPlatformSource);
    assert_eq!(
        build_output(&no_window.external_units, &output).bytes,
        no_window_bytes,
        "no trusted event window must force measurement rather than timestamp reuse"
    );
    let _ = observe(&fx, &ScriptedSource::quiet(40));
    let seeded = observe(&fx, &ScriptedSource::quiet(50));
    assert!(
        seeded
            .unit_root_coverage
            .iter()
            .any(|coverage| coverage.path == output
                && coverage.event_covered
                && coverage.reason == "incremental")
    );
    let changed_bytes = append();
    assert!(
        changed_bytes > no_window_bytes,
        "the event-local append must change allocated bytes"
    );
    let after = observe(&fx, &ScriptedSource::changed(60, vec![victim.clone()]));
    assert!(
        after
            .unit_root_coverage
            .iter()
            .any(|coverage| coverage.path == output
                && coverage.event_covered
                && coverage.reason == "incremental"),
        "victim-only change must exercise trusted output-local events"
    );
    assert_eq!(
        build_output(&after.external_units, &output).bytes,
        changed_bytes,
        "an output-local event refreshes the independent unit"
    );

    let declarations = after.merged.configured_outputs.clone();
    fs::remove_file(fx.repo.join("tsconfig.json")).unwrap();
    let selected = scope(&fx, Vec::new());
    let stored =
        report::report_scope_from_store(&selected, &fx.store).expect("read stored observation");
    assert_eq!(
        stored.report.configured_outputs, declarations,
        "stored reports retain observation-time declarations after config disappears"
    );
    let persisted = build_output(&stored.external_units, &output);
    assert_eq!(
        persisted.bytes,
        allocated(&victim),
        "a read-only stored report retains configured output bytes"
    );
}

#[test]
fn excluded_descendant_alias_is_pruned_from_configured_output() {
    let fx = fixture();
    let output = fx.home.join("compiled");
    let private = output.join("private");
    fs::create_dir_all(&private).unwrap();
    fs::write(output.join("keep.bin"), vec![b'k'; 4096]).unwrap();
    fs::write(private.join("secret.bin"), vec![b's'; 32768]).unwrap();
    let alias = fx.home.join("private-alias");
    std::os::unix::fs::symlink(&private, &alias).unwrap();
    fs::write(
        fx.repo.join("tsconfig.json"),
        r#"{"compilerOptions":{"outDir":"../compiled"}}"#,
    )
    .unwrap();
    let selected = scope(&fx, vec![alias.display().to_string()]);
    let observation = report::observe_scope(
        &selected,
        ObservationParts::ALL,
        None,
        None,
        false,
        Some(&fx.store),
        None,
        true,
        false,
        false,
        false,
        &ScriptedSource::quiet(10),
        30,
        24 * 3600,
    )
    .unwrap();
    let unit = build_output(&observation.external_units, &output);
    assert_eq!(
        unit.bytes,
        allocated(&output.join("keep.bin")),
        "excluded descendant contributes no allocation"
    );
    assert!(
        observation
            .store_interiors
            .iter()
            .all(|interior| !interior.path.starts_with(&private))
    );
    fs::write(private.join("secret.bin"), vec![b's'; 65536]).unwrap();
    let refreshed = report::observe_scope(
        &selected,
        ObservationParts::ALL,
        None,
        None,
        false,
        Some(&fx.store),
        None,
        true,
        false,
        false,
        false,
        &ScriptedSource::changed(20, vec![private.clone()]),
        30,
        24 * 3600,
    )
    .unwrap();
    assert_eq!(
        build_output(&refreshed.external_units, &output).bytes,
        allocated(&output.join("keep.bin"))
    );
    let stored = report::report_scope_from_store(&selected, &fx.store).unwrap();
    assert_eq!(
        build_output(&stored.external_units, &output).bytes,
        allocated(&output.join("keep.bin"))
    );
    assert!(
        stored
            .store_interiors
            .iter()
            .all(|interior| !interior.path.starts_with(&private))
    );
}
