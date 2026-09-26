//! `.oh/guardrails/no-dead-public-evidence-api.md`: a capability the
//! docs claim is either wired into the live pipeline or deleted with the
//! claim. The `no_dead_public_evidence_api` audit proves only that
//! *something* calls each evidence function; these tests prove the call
//! is the right one -- that each fact reaches the row kind it is about,
//! and that removing the wiring (or pointing it at the wrong kind of
//! unit) fails here rather than passing quietly.
//!
//! Every test below is written so that deleting the corresponding call
//! site makes it fail. Where a fact must *not* appear (a Docker fact on
//! a filesystem row, a `simctl` query against an Android emulator
//! directory, Maven's recovery limit on a Cargo cache), the negative is
//! asserted too: a fact attached to everything is as useless as a fact
//! attached to nothing.

mod fixture;

use std::fs;
use std::path::{Path, PathBuf};

use swamp_core::evidence::{
    Evidence, EvidenceSource, FactKind, FactStatus, FactSubtype, FactValue, Freshness,
};
use swamp_core::external::ExternalUnit;
use swamp_core::locations::{Provenance, StorageCategory};
use swamp_core::report::{ArtifactKind, Report, report_full_mode};

// ---------------------------------------------------------------------
// Shared fixtures
// ---------------------------------------------------------------------

fn report_for(root: &Path, docker_facts: Option<&Path>, store: &Path) -> Report {
    report_full_mode(
        root,
        docker_facts,
        false,
        Some(store),
        Some("1h"),
        true,
        false,
        false,
        true,
    )
    .expect("report")
}

/// A `docker system df -v`-shaped fixture whose image, build cache and
/// volume all carry the facts #54/#55 are about: a running container
/// against the image and the volume, and a daemon-reported `LastUsedAt`
/// on the build cache. Joined to `project` by compose label, so the rows
/// land inside the report's project tree rather than in `unowned`.
fn docker_facts_with_running_container(dir: &Path, project: &str) -> PathBuf {
    let path = dir.join("docker_system_df_v.json");
    let json = serde_json::json!({
        "Images": [{
            "ID": "sha256:1111000000000000000000000000000000000000000000000000000011",
            "Repository": "fixture/live-image",
            "Tag": "latest",
            "Size": "10485760",
            "SharedSize": "2097152",
            "UniqueSize": "8388608",
        }],
        "ImageInspect": [{
            "Id": "sha256:1111000000000000000000000000000000000000000000000000000011",
            "RepoTags": ["fixture/live-image:latest"],
            "Config": { "Labels": { "com.docker.compose.project": project } },
        }],
        "BuildCache": [{
            "ID": "buildcache-with-last-used-0001",
            "Size": "1048576",
            "LastUsedAt": "2026-09-10T13:00:00Z",
        }],
        "Volumes": [{
            "Name": "fixture-live-volume",
            "Labels": format!("com.docker.compose.project={project}"),
            "Size": "262144",
        }],
        "Containers": [{
            "Name": "/fixture-live-container",
            "Config": { "Image": "fixture/live-image:latest" },
            "State": { "Status": "running" },
            "Mounts": [{ "Type": "volume", "Name": "fixture-live-volume" }],
        }],
    });
    fs::write(&path, serde_json::to_vec_pretty(&json).expect("serialize")).expect("write");
    path
}

fn external_unit(
    detector_id: &str,
    name: &str,
    category: StorageCategory,
    path: &Path,
) -> ExternalUnit {
    ExternalUnit {
        detector_id: detector_id.to_string(),
        detector_name: name.to_string(),
        category,
        provenance: Provenance::BuiltinConvention,
        path: path.to_path_buf(),
        bytes: 0,
        mtime_max: 0,
        hardlinked: false,
        growth_bytes: None,
        regrowth_count: 0,
        observed_at: 0,
        consumers: Vec::new(),
        note: None,
        evidence: Vec::new(),
    }
}

fn rows(r: &Report) -> Vec<&swamp_core::report::ArtifactRow> {
    r.projects
        .iter()
        .flat_map(|p| p.worktrees.iter())
        .flat_map(|wt| wt.artifacts.iter())
        .collect()
}

fn is_docker_row(kind: &ArtifactKind) -> bool {
    matches!(
        kind,
        ArtifactKind::DockerImage | ArtifactKind::DockerVolume | ArtifactKind::DockerBuildCache
    )
}

// ---------------------------------------------------------------------
// #54 activity: access time, Docker's own last_used
// ---------------------------------------------------------------------

/// `activity::access_time_evidence` must run for every filesystem
/// artifact row of an ordinary report -- and for no Docker row, whose
/// "path" is a repo tag or volume name that no `stat` can answer for.
///
/// Fails if the call is removed from `report::attach_decision_evidence`
/// (no `Accessed` fact anywhere) and equally if it is hoisted out of the
/// `is_docker_object` guard (an `Accessed` fact on a daemon object).
#[test]
fn access_time_is_asked_of_every_filesystem_row_and_of_no_docker_row() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = fixture::build(tmp.path());
    let docker = docker_facts_with_running_container(tmp.path(), &fx.checkout_name);
    let store = tempfile::tempdir().unwrap();
    let r = report_for(&fx.root, Some(&docker), store.path());

    let all = rows(&r);
    let filesystem_rows: Vec<_> = all.iter().filter(|a| !is_docker_row(&a.kind)).collect();
    assert!(
        !filesystem_rows.is_empty(),
        "precondition: the fixture must produce filesystem artifact rows"
    );
    for a in &filesystem_rows {
        let accessed: Vec<&Evidence> = a
            .evidence
            .iter()
            .filter(|e| e.kind == FactKind::Activity && e.subtype == FactSubtype::Accessed)
            .collect();
        assert_eq!(
            accessed.len(),
            1,
            "{:?} {} must carry exactly one access-time fact: {:?}",
            a.kind,
            a.path.display(),
            a.evidence
        );
        // Whatever the mount says, the fact states its own basis: on a
        // `noatime`/`relatime` volume the honest answer is
        // `Unavailable` *with the reason*, never a silent omission and
        // never a bare timestamp the mount does not actually maintain.
        match &accessed[0].status {
            FactStatus::Known(FactValue::Timestamp(_)) => {}
            FactStatus::Unavailable { reason } | FactStatus::Unknown { reason } => {
                assert!(!reason.is_empty(), "an unanswerable atime must say why");
            }
            other => panic!("unexpected access-time status: {other:?}"),
        }
    }

    let docker_rows: Vec<_> = all.iter().filter(|a| is_docker_row(&a.kind)).collect();
    assert!(
        !docker_rows.is_empty(),
        "precondition: the fixture must produce joined Docker rows"
    );
    for a in &docker_rows {
        assert!(
            !a.evidence
                .iter()
                .any(|e| e.subtype == FactSubtype::Accessed),
            "a {:?} row has no filesystem path to stat; an access-time fact here would be \
             invented provenance: {:?}",
            a.kind,
            a.evidence
        );
    }
}

/// `activity::docker_last_used_evidence` must reach the build-cache row
/// it describes, sourced from the daemon -- and must stay distinct from
/// the filesystem `Modified` fact rather than being flattened into it.
///
/// Fails if the call is removed from `report::join_docker_facts`, and
/// fails if the daemon's timestamp is ever recorded with filesystem
/// provenance.
#[test]
fn docker_reported_last_used_reaches_the_build_cache_row_as_its_own_fact() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = fixture::build(tmp.path());
    let docker = docker_facts_with_running_container(tmp.path(), &fx.checkout_name);
    let store = tempfile::tempdir().unwrap();
    let r = report_for(&fx.root, Some(&docker), store.path());

    // The build cache in this fixture joins to no project, so it lands
    // in `unowned` -- where its own activity evidence matters just as
    // much: "no project claims it" is a consumer fact, not a reason to
    // drop the daemon's own last-used timestamp.
    let cache_row = r
        .unowned
        .iter()
        .find(|u| u.docker_kind.as_deref() == Some("build-cache"))
        .expect("the fixture's build-cache entry");
    let fact = cache_row
        .evidence
        .iter()
        .find(|e| e.subtype == FactSubtype::ToolReportedUse)
        .expect("the build-cache row must carry the daemon's own last_used fact");
    assert!(
        matches!(&fact.source, EvidenceSource::ToolReported { tool, .. } if tool == "docker"),
        "last_used is the daemon's fact, not the filesystem's: {fact:?}"
    );
    assert_eq!(
        fact.event_at,
        Some(1_789_045_200),
        "2026-09-10T13:00:00Z must be carried as the event time, distinct from observation time"
    );
    assert_ne!(
        fact.subtype,
        FactSubtype::Modified,
        "a tool-reported use timestamp must never be flattened into filesystem modification age"
    );

    // And no filesystem row ever acquires one.
    for a in rows(&r).iter().filter(|a| !is_docker_row(&a.kind)) {
        assert!(
            !a.evidence
                .iter()
                .any(|e| matches!(&e.source, EvidenceSource::ToolReported { tool, .. } if tool == "docker")),
            "a Docker-sourced fact must never land on the filesystem row {}: {:?}",
            a.path.display(),
            a.evidence
        );
    }
}

// ---------------------------------------------------------------------
// #55 current use: running containers, manager locks, booted simulators
// ---------------------------------------------------------------------

/// `occupancy::docker_running_container_evidence` must reach the image
/// and volume rows a running container references, and no filesystem
/// row. This is the provenance rule
/// `reviewer_counterexamples_123`'s `docker_reclaimability_must_not_
/// claim_filesystem_provenance` pins, in the other direction.
#[test]
fn running_container_occupancy_reaches_docker_rows_and_only_docker_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = fixture::build(tmp.path());
    let docker = docker_facts_with_running_container(tmp.path(), &fx.checkout_name);
    let store = tempfile::tempdir().unwrap();
    let r = report_for(&fx.root, Some(&docker), store.path());

    let all = rows(&r);
    let image_row = all
        .iter()
        .find(|a| a.kind == ArtifactKind::DockerImage)
        .expect("the fixture's joined image row");
    let fact = image_row
        .evidence
        .iter()
        .find(|e| e.subtype == FactSubtype::RunningContainer)
        .expect("an image a running container references must carry that fact");
    assert_eq!(
        fact.status,
        FactStatus::Known(FactValue::Bool(true)),
        "a running container is a live consumer: {fact:?}"
    );
    assert!(
        matches!(fact.source, EvidenceSource::DockerApi { .. }),
        "the daemon is the source of this fact: {fact:?}"
    );

    let volume_row = all
        .iter()
        .find(|a| a.kind == ArtifactKind::DockerVolume)
        .expect("the fixture's joined volume row");
    assert!(
        volume_row
            .evidence
            .iter()
            .any(|e| e.subtype == FactSubtype::RunningContainer),
        "a volume a running container has mounted must carry that fact: {:?}",
        volume_row.evidence
    );

    for a in all.iter().filter(|a| !is_docker_row(&a.kind)) {
        assert!(
            !a.evidence
                .iter()
                .any(|e| e.subtype == FactSubtype::RunningContainer),
            "a container fact must never land on the filesystem row {}: {:?}",
            a.path.display(),
            a.evidence
        );
    }
}

/// Every short-lived current-use fact states *both* halves of its
/// freshness: when it stops being current, and what the question could
/// not see in the first place
/// (`evidence::Freshness::expires_after_with_coverage`). An expiry
/// without a coverage limit reads as "this was the whole answer, and it
/// was true a moment ago".
///
/// Fails if either wiring drops back to a bare `expires_after`.
#[test]
fn current_use_facts_carry_both_an_expiry_and_a_stated_coverage_limit() {
    let running = swamp_core::occupancy::docker_running_container_evidence(&[
        swamp_core::docker::ContainerRef {
            name: "web-1".into(),
            state: "running".into(),
            finished_at: None,
        },
    ]);
    assert!(
        running.freshness.expires_after_secs.is_some(),
        "a container state observed now says nothing about later: {running:?}"
    );
    let coverage = running
        .freshness
        .coverage_note
        .as_deref()
        .expect("a container listing cannot see non-container consumers; say so");
    assert!(coverage.contains("non-container"), "{coverage}");

    let json = r#"{"devices":{"iOS-17":[{"udid":"AAAA","state":"Booted"}]}}"#;
    let runner = std::sync::Arc::new(swamp_core::locations::FakeCommandRunner::new().with_answer(
        "xcrun",
        &["simctl", "list", "devices", "-j"],
        json,
    ));
    let env = swamp_core::locations::Environment::fixture(
        PathBuf::from("/home/dev"),
        std::collections::HashMap::new(),
        swamp_core::locations::Platform::MacOS,
    )
    .with_runner(runner);
    let booted = swamp_core::occupancy::simulator_booted_evidence("AAAA", &env);
    assert!(booted.freshness.expires_after_secs.is_some(), "{booted:?}");
    assert!(
        booted
            .freshness
            .coverage_note
            .as_deref()
            .is_some_and(|c| c.contains("simctl list devices")),
        "a device listing answers for this user's store only; say so: {booted:?}"
    );
}

/// `occupancy::manager_lock_evidence` must reach a proposal for a unit
/// whose own directory holds a manager lock, and must report the lock as
/// *held* when another process holds it.
///
/// Fails if `actions::unit_from_external` stops probing for locks.
#[test]
fn a_held_manager_lock_in_a_units_own_directory_is_reported_as_current_use() {
    use std::os::unix::io::AsRawFd;
    let tmp = tempfile::tempdir().unwrap();
    let store_dir = tmp.path().join("cargo-home");
    fs::create_dir_all(&store_dir).unwrap();
    let lock = store_dir.join(".package-cache");
    fs::write(&lock, b"").unwrap();
    let held = fs::File::open(&lock).unwrap();
    let rc = unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(rc, 0, "test setup must acquire the lock first");

    let unit = external_unit(
        "cargo-home",
        "Cargo home",
        StorageCategory::Cache,
        &store_dir,
    );
    let plan_unit = swamp_core::actions::unit_from_external(&unit);
    let fact = plan_unit
        .evidence()
        .iter()
        .find(|e| e.subtype == FactSubtype::Lock)
        .expect("a unit whose directory holds a manager lock must say so");
    assert_eq!(
        fact.status,
        FactStatus::Known(FactValue::Bool(true)),
        "another process holds this lock right now: {fact:?}"
    );
    drop(held);
}

/// The absence of a lock file is `Unknown` with the candidates named,
/// never silence and never "nothing is using it": most managers only
/// create their lock while they are working.
#[test]
fn a_unit_with_no_manager_lock_states_what_was_looked_for() {
    let tmp = tempfile::tempdir().unwrap();
    let unit = external_unit("pnpm", "pnpm store", StorageCategory::Cache, tmp.path());
    let plan_unit = swamp_core::actions::unit_from_external(&unit);
    let fact = plan_unit
        .evidence()
        .iter()
        .find(|e| e.subtype == FactSubtype::Lock)
        .expect("'no lock file' is an answer that must be stated, not omitted");
    assert!(
        matches!(fact.status, FactStatus::Unknown { .. }),
        "a missing lock file is not evidence that nothing is using the store: {fact:?}"
    );
    let note = fact.note.as_deref().unwrap_or_default();
    assert!(
        note.contains(".package-cache") && note.contains("store.lock"),
        "the candidates that were checked must be named: {note}"
    );
}

/// `occupancy::simulator_booted_evidence` must be asked about a
/// CoreSimulator device store's device directories -- and must *not* be
/// asked about an Android emulator store, which is the same
/// `Environments` category but whose directory names are AVD names
/// `simctl` knows nothing about. Routing by category alone would send a
/// simulator query at an emulator; routing by the device-UDID naming
/// convention does not.
#[test]
fn simulator_device_directories_are_probed_and_emulator_directories_are_not() {
    let tmp = tempfile::tempdir().unwrap();
    let devices = tmp.path().join("CoreSimulator/Devices");
    fs::create_dir_all(devices.join("A1B2C3D4-0000-4000-8000-0123456789AB")).unwrap();
    let simulator_unit = external_unit(
        "core-simulator",
        "CoreSimulator",
        StorageCategory::Environments,
        &devices,
    );
    let probed = swamp_core::actions::unit_from_external(&simulator_unit);
    let booted: Vec<&Evidence> = probed
        .evidence()
        .iter()
        .filter(|e| e.subtype == FactSubtype::Booted)
        .collect();
    assert_eq!(
        booted.len(),
        1,
        "each device directory in the store is one booted-state question: {:?}",
        probed.evidence()
    );
    // The answer depends on the machine (a real `simctl`, or none at
    // all). What must hold everywhere is that the question was asked of
    // the right tool and the answer is never a silent "not booted".
    match &booted[0].status {
        FactStatus::Known(FactValue::Bool(_)) => {}
        FactStatus::Unknown { reason } | FactStatus::Unavailable { reason } => {
            assert!(!reason.is_empty(), "an unanswerable probe must say why");
        }
        other => panic!("unexpected booted status: {other:?}"),
    }
    assert!(
        matches!(&booted[0].source, EvidenceSource::ProcessQuery { tool } if tool.contains("simctl")),
        "{:?}",
        booted[0]
    );

    let avds = tmp.path().join("android/avd");
    fs::create_dir_all(avds.join("Pixel_5_API_31.avd")).unwrap();
    let emulator_unit = external_unit(
        "android",
        "Android SDK",
        StorageCategory::Environments,
        &avds,
    );
    let unprobed = swamp_core::actions::unit_from_external(&emulator_unit);
    assert!(
        !unprobed
            .evidence()
            .iter()
            .any(|e| e.subtype == FactSubtype::Booted),
        "an Android AVD is not a CoreSimulator device; asking simctl about it would answer \
         about nothing: {:?}",
        unprobed.evidence()
    );
}

// ---------------------------------------------------------------------
// #58 recovery: Maven's local repository, installed toolchains
// ---------------------------------------------------------------------

/// `recovery::maven_artifact_recovery` must reach a Maven-layout local
/// repository and state Maven's own documented limit (a downloaded
/// artifact and a locally-`mvn install`ed one share one tree, told apart
/// only by a per-artifact `_remote.repositories` marker), and must not
/// reach a Cargo registry cache, whose entries have a single documented
/// origin.
#[test]
fn maven_layout_store_carries_mavens_own_recovery_limit_and_a_cargo_cache_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    let m2 = tmp.path().join(".m2/repository");
    fs::create_dir_all(&m2).unwrap();
    let maven_unit = external_unit(
        "maven",
        "Maven local repository",
        StorageCategory::Unclassified,
        &m2,
    );
    let plan_unit = swamp_core::actions::unit_from_external(&maven_unit);
    let fact = plan_unit
        .evidence()
        .iter()
        .find(|e| e.kind == FactKind::Recovery)
        .expect("a Maven local repository must carry its own recovery assessment");
    let reason = match &fact.status {
        FactStatus::Unknown { reason } => reason.clone(),
        other => panic!("directory category alone never proves recoverability: {other:?}"),
    };
    assert!(
        reason.contains("_remote.repositories"),
        "the limit must name the marker that would resolve it: {reason}"
    );
    assert!(
        fact.note
            .as_deref()
            .is_some_and(|n| n.starts_with("check: ")),
        "the assessment's smallest useful follow-up check must survive onto the fact: {fact:?}"
    );

    let cargo_cache = tmp.path().join(".cargo/registry");
    fs::create_dir_all(&cargo_cache).unwrap();
    let cargo_unit = external_unit(
        "cargo-home",
        "Cargo home",
        StorageCategory::Cache,
        &cargo_cache,
    );
    let cargo_plan_unit = swamp_core::actions::unit_from_external(&cargo_unit);
    assert!(
        !cargo_plan_unit.evidence().iter().any(|e| e
            .note
            .as_deref()
            .is_some_and(|n| n.contains("_remote.repositories"))),
        "Maven's ambiguity is not Cargo's: {:?}",
        cargo_plan_unit.evidence()
    );
}

/// `recovery::toolchain_installation_recovery` must reach an
/// installation store whose detector declares the installed-versions
/// convention, naming each installed version as separately
/// reinstallable -- and must not reach a store whose detector declares
/// no such convention, where no manager can be named to reinstall
/// anything.
#[test]
fn an_installation_store_names_each_installed_version_as_locally_reinstallable() {
    let tmp = tempfile::tempdir().unwrap();
    let versions = tmp.path().join("pyenv/versions");
    fs::create_dir_all(versions.join("3.12.4")).unwrap();
    fs::create_dir_all(versions.join("3.11.9")).unwrap();
    let unit = external_unit("pyenv", "pyenv", StorageCategory::Installation, &versions);
    let plan_unit = swamp_core::actions::unit_from_external(&unit);
    let recovery: Vec<&Evidence> = plan_unit
        .evidence()
        .iter()
        .filter(|e| e.kind == FactKind::Recovery && e.subtype == FactSubtype::LocalReinstall)
        .collect();
    assert_eq!(
        recovery.len(),
        2,
        "each installed version is separately reinstallable: {:?}",
        plan_unit.evidence()
    );
    let notes: Vec<String> = recovery
        .iter()
        .map(|e| e.note.clone().unwrap_or_default())
        .collect();
    assert!(
        notes.iter().any(|n| n.contains("3.12.4")) && notes.iter().any(|n| n.contains("3.11.9")),
        "the exact version a manager would reinstall must be named: {notes:?}"
    );

    // A store whose detector declares no installed-versions convention
    // gets no reinstall claim: there is no manager to name.
    let models = tmp.path().join("ollama/models");
    fs::create_dir_all(models.join("llama3")).unwrap();
    let model_unit = external_unit("ollama", "Ollama", StorageCategory::Installation, &models);
    let model_plan_unit = swamp_core::actions::unit_from_external(&model_unit);
    assert!(
        !model_plan_unit
            .evidence()
            .iter()
            .any(|e| e.subtype == FactSubtype::LocalReinstall),
        "no detector-declared version convention means no reinstall claim: {:?}",
        model_plan_unit.evidence()
    );
}

// ---------------------------------------------------------------------
// #59 reclaimability: clone/snapshot bounds, sparse files, selections,
// observed free space
// ---------------------------------------------------------------------

/// `reclaimability::apfs_clone_or_snapshot_bound`: on a copy-on-write
/// filesystem, a unit's allocated bytes are a *ceiling* on what removing
/// it frees, because a clone or a snapshot outside the unit can retain
/// every extent. Reporting the allocation as an exact reclaimable figure
/// is the promise #59 forbids.
///
/// Fails on macOS if `report::attach_decision_evidence` drops the
/// copy-on-write branch and falls back to `exclusive_allocation`.
#[cfg(target_os = "macos")]
#[test]
fn reclaimable_bytes_on_a_copy_on_write_volume_are_bounded_not_promised() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = fixture::build(tmp.path());
    let store = tempfile::tempdir().unwrap();
    let mut r = report_for(&fx.root, None, store.path());

    // A row with no unresolved hardlink membership: without the
    // copy-on-write branch this is exactly the row that would claim an
    // exact reclaimable figure.
    for project in &mut r.projects {
        for wt in &mut project.worktrees {
            for a in &mut wt.artifacts {
                a.hardlinked = false;
                a.dedup_stale = false;
                a.evidence.clear();
            }
        }
    }
    swamp_core::report::attach_decision_evidence(&mut r);

    let bounded: Vec<&Evidence> = rows(&r)
        .into_iter()
        .flat_map(|a| a.evidence.iter())
        .filter(|e| e.subtype == FactSubtype::EstimatedReclaimable)
        .collect();
    assert!(!bounded.is_empty(), "no reclaimability estimate at all");
    for fact in &bounded {
        match &fact.status {
            FactStatus::Conflicting { reason, .. } => assert!(
                reason.contains("clone") || reason.contains("snapshot"),
                "the bound must name why it is a bound: {reason}"
            ),
            other => panic!(
                "on a copy-on-write volume, removing a unit is not promised to free its \
                 allocated bytes: {other:?}"
            ),
        }
    }
}

/// `reclaimability::sparse_file_accounting`: a sparse file's apparent
/// length is not disk it occupies, so it is never reclaimable space.
/// The unit's evidence must carry the two numbers separately and set
/// reclaimable to the allocated figure.
///
/// Fails if `actions::unit_from_external` stops distinguishing them --
/// and the dense half fails if it starts claiming sparseness everywhere.
#[test]
fn a_sparse_unit_separates_apparent_length_from_allocated_blocks() {
    use std::os::unix::fs::MetadataExt;
    let tmp = tempfile::tempdir().unwrap();
    let vm_data = tmp.path().join("vm/data");
    fs::create_dir_all(&vm_data).unwrap();
    let image = vm_data.join("Docker.raw");
    let f = fs::File::create(&image).unwrap();
    f.set_len(200 * 1024 * 1024).unwrap();
    drop(f);
    let meta = fs::metadata(&image).unwrap();
    let allocated = meta.blocks() * 512;
    if allocated >= meta.len() {
        // This filesystem did not actually create a sparse file; the
        // wiring cannot be validated here (same guard as
        // `docker_desktop_sparse_backing_file.rs`).
        return;
    }

    let unit = external_unit(
        "docker-desktop",
        "Docker Desktop",
        StorageCategory::LocalState,
        &vm_data,
    );
    let plan_unit = swamp_core::actions::unit_from_external(&unit);
    let find = |subtype: FactSubtype| -> u64 {
        plan_unit
            .evidence()
            .iter()
            .find(|e| e.kind == FactKind::Reclaimability && e.subtype == subtype)
            .and_then(|e| match &e.status {
                FactStatus::Known(FactValue::Bytes(b)) => Some(*b),
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing {subtype:?}: {:?}", plan_unit.evidence()))
    };
    let logical = find(FactSubtype::LogicalBytes);
    let on_disk = find(FactSubtype::AllocatedBytes);
    let reclaimable = find(FactSubtype::EstimatedReclaimable);
    assert!(
        logical > on_disk,
        "the fixture's apparent length must exceed its allocated blocks ({logical} vs {on_disk})"
    );
    assert_eq!(
        reclaimable, on_disk,
        "only the blocks actually charged on disk can be freed; the sparse gap never occupied any"
    );

    // A dense directory claims no sparseness.
    let dense = tmp.path().join("dense");
    fs::create_dir_all(&dense).unwrap();
    fs::write(dense.join("blob"), vec![7u8; 64 * 1024]).unwrap();
    let dense_unit = external_unit("synthetic", "Synthetic", StorageCategory::Cache, &dense);
    let dense_plan_unit = swamp_core::actions::unit_from_external(&dense_unit);
    assert!(
        !dense_plan_unit
            .evidence()
            .iter()
            .any(|e| e.subtype == FactSubtype::LogicalBytes),
        "a dense file has no apparent-versus-allocated gap to report: {:?}",
        dense_plan_unit.evidence()
    );
}

// ---------------------------------------------------------------------
// #53 shared contract: of_kind / stale in the presentation path
// ---------------------------------------------------------------------

fn fact(kind: FactKind, subtype: FactSubtype) -> Evidence {
    Evidence::known(
        kind,
        subtype,
        FactValue::Text("x".into()),
        EvidenceSource::Inferred {
            basis: "test".into(),
        },
        1_000,
    )
}

/// `evidence::of_kind` groups a unit's facts by domain for the reader.
/// Facts arrive in whatever order the passes that produced them ran, so
/// without the grouping the same five facts print in a different order
/// depending on which pass touched the row last.
///
/// Fails if the grouping is removed (input order survives), and fails if
/// grouping ever drops a fact.
#[test]
fn evidence_lines_are_grouped_by_domain_and_drop_nothing() {
    let scrambled = vec![
        fact(FactKind::Reclaimability, FactSubtype::AllocatedBytes),
        fact(FactKind::Activity, FactSubtype::Modified),
        fact(FactKind::Recovery, FactSubtype::Rebuild),
        fact(FactKind::Consumer, FactSubtype::DeclaredConsumer),
        fact(FactKind::CurrentUse, FactSubtype::OpenFile),
        fact(FactKind::Activity, FactSubtype::Accessed),
    ];
    let lines = swamp_core::render::render_evidence_lines(&scrambled);
    assert_eq!(lines.len(), scrambled.len(), "no fact may be dropped");
    let domains: Vec<&str> = lines
        .iter()
        .map(|l| l.split_whitespace().next().unwrap())
        .collect();
    assert_eq!(
        domains,
        vec![
            "activity",
            "activity",
            "consumer",
            "current-use",
            "recovery",
            "reclaimability"
        ],
        "facts must read in a fixed domain order, not in whichever order the passes ran: {lines:?}"
    );
}

/// `evidence::stale`: a short-lived reading past its own recheck window
/// must not sit at a confirmation looking current. The line names the
/// window and says the reading is re-taken before acting -- a fact, not
/// a verdict about the unit.
///
/// Fails if `render::evidence_warnings` stops consulting `stale`.
#[test]
fn a_current_use_reading_past_its_recheck_window_is_surfaced_at_the_confirmation() {
    let now = swamp_core::entities::now();
    let expired = Evidence::known(
        FactKind::CurrentUse,
        FactSubtype::OpenFile,
        FactValue::Bool(false),
        EvidenceSource::ProcessQuery {
            tool: "lsof".into(),
        },
        now.saturating_sub(600),
    )
    .with_freshness(Freshness::expires_after(60));
    let warnings = swamp_core::render::evidence_warnings(&[expired]);
    assert!(
        warnings.iter().any(|w| w.contains("recheck window")),
        "a reading older than its own expiry must be visible at the confirmation: {warnings:?}"
    );

    let fresh = Evidence::known(
        FactKind::CurrentUse,
        FactSubtype::OpenFile,
        FactValue::Bool(false),
        EvidenceSource::ProcessQuery {
            tool: "lsof".into(),
        },
        now,
    )
    .with_freshness(Freshness::expires_after(60));
    assert!(
        !swamp_core::render::evidence_warnings(&[fresh])
            .iter()
            .any(|w| w.contains("recheck window")),
        "a current reading must not produce a freshness line; a warning on every row is noise"
    );
}
