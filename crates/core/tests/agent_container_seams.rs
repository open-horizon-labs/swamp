//! The container seam, for every adapter whose session storage is a
//! directory tree: Codex (`sessions/<yyyy>/<mm>/<dd>/`), OpenCode
//! (`storage/session/<project-id>/`) and Pi (`sessions/<dir>/`).
//!
//! Two properties per adapter, both adversarial:
//!
//! * a container this pass's event window vouches for is **replayed** --
//!   no listing, no `stat`, no header read -- and reports the same units
//!   it did when it was identified;
//! * a container the window names is **re-identified**, and the change
//!   is reported in the same pass. A reuse that cannot notice a change
//!   is not reuse.
//!
//! Plus the property that made the conversion possible at all: one
//! container's output does not depend on what a sibling container
//! produced. Codex's session walk used to carry a single entry budget
//! shared across `sessions/` and `archived_sessions/`
//! (`already_seen + out.len()`), so a day directory's contents depended
//! on how many files the days before it had yielded -- which means the
//! rows stored for that day could not be replayed into a pass that
//! reached it in a different order.
//!
//! Fixtures are disposable `tempfile` trees with synthetic content: the
//! "transcripts" here are one-line JSON objects this test wrote itself.

use std::fs;
use std::path::{Path, PathBuf};
use swamp_core::agents::{ContainerCache, IdentificationCache, IdentifyCtx};
use swamp_core::fs_events::EventCoverage;
use swamp_core::work_counters::{self, WorkCounters};

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn append(path: &Path, bytes: usize) {
    use std::io::Write;
    let mut f = fs::OpenOptions::new().append(true).open(path).unwrap();
    f.write_all(&vec![b'x'; bytes]).unwrap();
    f.write_all(b"\n").unwrap();
}

/// One identification pass with a fresh pair of caches loaded from
/// `store`, under `coverage`, returning the units' total bytes, their
/// count, and the work the pass did.
fn pass(
    identify: &dyn Fn(&Path, &IdentifyCtx) -> Vec<swamp_core::agents::CandidateAgentUnit>,
    home: &Path,
    store: &Path,
    at: u64,
    coverage: EventCoverage,
) -> (u64, usize, WorkCounters) {
    let cache = IdentificationCache::load(store);
    let containers = ContainerCache::load(store, coverage);
    let (units, counted) = work_counters::measured(|| {
        let ctx = IdentifyCtx::with_containers(at, &cache, &containers);
        identify(home, &ctx)
    });
    cache.save(store, at).unwrap();
    containers.save(store, at).unwrap();
    let bytes = units.iter().map(|u| u.bytes()).sum();
    (bytes, units.len(), counted)
}

fn quiet(root: &Path, since: u64) -> EventCoverage {
    EventCoverage::trusted(root.to_path_buf(), Vec::new(), since)
}

fn touched(root: &Path, changed: &[PathBuf], since: u64) -> EventCoverage {
    EventCoverage::trusted(root.to_path_buf(), changed.to_vec(), since)
}

/// The shared body of all three adapter tests: identify once cold,
/// replay under a quiet window, then append and re-identify under a
/// window that names the appended file.
fn seam_behaves(
    identify: &dyn Fn(&Path, &IdentifyCtx) -> Vec<swamp_core::agents::CandidateAgentUnit>,
    root: &Path,
    home: &Path,
    containers: u64,
    victim: &Path,
    victim_container: &Path,
) {
    let store = tempfile::tempdir().unwrap();
    let (cold_bytes, cold_units, cold) = pass(
        identify,
        home,
        store.path(),
        1_000,
        EventCoverage::untrusted(),
    );
    assert!(
        cold_units > 0,
        "precondition: the fixture must be identified"
    );
    assert_eq!(
        cold.containers_identified, containers,
        "every container must be identified on the cold pass: {} of {containers}",
        cold.containers_identified
    );

    let (quiet_bytes, quiet_units, replayed) =
        pass(identify, home, store.path(), 2_000, quiet(root, 1_000));
    assert_eq!(
        (quiet_bytes, quiet_units),
        (cold_bytes, cold_units),
        "a replayed home must report exactly what it reported when identified"
    );
    assert_eq!(
        replayed.containers_reused, containers,
        "every container the window vouches for must be replayed: {} of {containers}",
        replayed.containers_reused
    );
    assert_eq!(
        replayed.containers_identified, 0,
        "nothing may be re-identified when the window reports no event: {}",
        replayed.containers_identified
    );
    assert_eq!(
        replayed.header_bytes_read, 0,
        "a replayed container reads no headers: {}",
        replayed.header_bytes_read
    );

    // An in-place append: the container's own directory stamp does not
    // move, which is exactly the change the pre-2026-09-22 stamp key
    // could not see.
    let before = fs::metadata(victim).unwrap().len();
    append(victim, 4096);
    assert!(fs::metadata(victim).unwrap().len() >= before + 4096);

    let events = touched(
        root,
        &[victim.to_path_buf(), victim_container.to_path_buf()],
        2_000,
    );
    let (grown_bytes, _, cost) = pass(identify, home, store.path(), 3_000, events);
    assert_eq!(
        cost.containers_identified, 1,
        "exactly the container the window named may be re-identified: {} identified, {} reused",
        cost.containers_identified, cost.containers_reused
    );
    assert_eq!(
        cost.containers_reused,
        containers - 1,
        "every other container must still be replayed: {}",
        cost.containers_reused
    );
    assert!(
        grown_bytes >= cold_bytes + 4096,
        "the appended bytes must be reported in the same pass that sees their event: \
         {grown_bytes} vs {cold_bytes}"
    );
}

#[test]
fn codex_day_directories_are_containers() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("codex");
    let mut victim = PathBuf::new();
    for (month, day) in [("01", "05"), ("02", "11")] {
        for i in 0..4 {
            let p = home
                .join("sessions/2026")
                .join(month)
                .join(day)
                .join(format!("rollout-{month}{day}-{i}.jsonl"));
            write(
                &p,
                &format!("{{\"cwd\":\"{}\",\"type\":\"user\"}}\n", root.display()),
            );
            if month == "01" && i == 0 {
                victim = p;
            }
        }
    }
    seam_behaves(
        &swamp_core::agents::codex::identify,
        &root,
        &home,
        2,
        &victim,
        &home.join("sessions/2026/01/05"),
    );
}

#[test]
fn codex_refreshes_project_link_without_invalidating_cached_session_size() {
    use swamp_core::agents::ProjectLinkState;

    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("codex");
    let rollout = home.join("sessions/2026/09/25/rollout-live.jsonl");
    write(
        &rollout,
        "synthetic transcript; never parsed for attribution\n",
    );
    let project_a = root.join("project-a");
    let project_b = root.join("project-b");
    fs::create_dir_all(&project_a).unwrap();
    fs::create_dir_all(&project_b).unwrap();

    let db = rusqlite::Connection::open(home.join("state_5.sqlite")).unwrap();
    db.execute_batch(
        "CREATE TABLE threads (rollout_path TEXT, cwd TEXT, title TEXT, preview TEXT, first_user_message TEXT);",
    )
    .unwrap();
    db.execute(
        "INSERT INTO threads (rollout_path, cwd, title, preview, first_user_message) VALUES (?1, ?2, 'private title', 'private preview', 'private message')",
        rusqlite::params![rollout.to_string_lossy(), project_a.to_string_lossy()],
    )
    .unwrap();
    drop(db);

    let store = tempfile::tempdir().unwrap();
    let identify_pass = |at: u64, coverage| {
        let cache = IdentificationCache::load(store.path());
        let containers = ContainerCache::load(store.path(), coverage);
        let (units, work) = work_counters::measured(|| {
            let ctx = IdentifyCtx::with_containers(at, &cache, &containers);
            swamp_core::agents::codex::identify(&home, &ctx)
        });
        cache.save(store.path(), at).unwrap();
        containers.save(store.path(), at).unwrap();
        (units, work)
    };

    let (first, _) = identify_pass(1_000, EventCoverage::untrusted());
    let first_link = first
        .iter()
        .find(|unit| unit.path() == rollout)
        .expect("the rollout is identified")
        .project_link()
        .clone();
    assert!(
        matches!(first_link, ProjectLinkState::NotAProject { ref path } if path == &project_a),
        "first link should use first database cwd, got {first_link:?}"
    );

    let db = rusqlite::Connection::open(home.join("state_5.sqlite")).unwrap();
    db.execute(
        "UPDATE threads SET cwd = ?1 WHERE rollout_path = ?2",
        rusqlite::params![project_b.to_string_lossy(), rollout.to_string_lossy()],
    )
    .unwrap();
    drop(db);

    let (second, work) = identify_pass(2_000, quiet(&root, 1_000));
    let session = second
        .iter()
        .find(|unit| unit.path() == rollout)
        .expect("the cached rollout is returned");
    assert!(
        matches!(session.project_link(), ProjectLinkState::NotAProject { path } if path == &project_b),
        "the fresh Codex index row must replace the stale cached project link: {:?}",
        session.project_link()
    );
    assert_eq!(work.containers_reused, 1, "session bytes remain cached");
    assert_eq!(work.containers_identified, 0, "the day is not rewalked");
    assert_eq!(session.bytes(), rollout.metadata().unwrap().len());
}

#[test]
fn opencode_project_directories_are_containers() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("opencode");
    let mut victim = PathBuf::new();
    for project in ["p1", "p2"] {
        for i in 0..3 {
            let p = home
                .join("storage/session")
                .join(project)
                .join(format!("s{i}.json"));
            write(&p, &format!("{{\"id\":\"s{i}\"}}\n"));
            if project == "p1" && i == 0 {
                victim = p;
            }
        }
    }
    seam_behaves(
        &swamp_core::agents::opencode::identify,
        &root,
        &home,
        2,
        &victim,
        &home.join("storage/session/p1"),
    );
}

#[test]
fn pi_session_directories_are_containers() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("pi");
    write(&home.join("settings.json"), "{}\n");
    let mut victim = PathBuf::new();
    for bucket in ["a", "b"] {
        for i in 0..3 {
            let p = home
                .join("sessions")
                .join(bucket)
                .join(format!("s{i}.jsonl"));
            write(
                &p,
                &format!("{{\"cwd\":\"{}\",\"type\":\"user\"}}\n", root.display()),
            );
            if bucket == "a" && i == 0 {
                victim = p;
            }
        }
    }
    seam_behaves(
        &swamp_core::agents::pi::identify,
        &root,
        &home,
        2,
        &victim,
        &home.join("sessions/a"),
    );
}

/// The property the shared entry bound violated, and the reason Codex
/// could not be converted before: what one container identifies must not
/// depend on what a sibling produced first.
///
/// Stated as an equality over live identifications, because that is the
/// form a replay needs: rows stored for `2026/02/11` are replayed into a
/// pass that may reach it in a different order, or after a sibling grew
/// by thousands of files, and they are only sound if the day's own
/// contents are all that decided them.
#[test]
fn a_codex_day_container_does_not_depend_on_its_siblings() {
    let day_units = |busy_sibling: usize| -> Vec<String> {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("codex");
        for i in 0..busy_sibling {
            write(
                &home.join(format!("sessions/2026/01/05/rollout-a-{i}.jsonl")),
                "{\"type\":\"user\"}\n",
            );
        }
        // The same sharing applied across `sessions/` and
        // `archived_sessions/`, so the archived tree is populated too.
        for i in 0..busy_sibling {
            write(
                &home.join(format!("archived_sessions/2026/01/05/rollout-b-{i}.jsonl")),
                "{\"type\":\"user\"}\n",
            );
        }
        for i in 0..3 {
            write(
                &home.join(format!("sessions/2026/02/11/rollout-c-{i}.jsonl")),
                "{\"type\":\"user\"}\n",
            );
        }
        let cache = IdentificationCache::disabled();
        let containers = ContainerCache::disabled();
        let ctx = IdentifyCtx::with_containers(1_000, &cache, &containers);
        let mut names: Vec<String> = swamp_core::agents::codex::identify(&home, &ctx)
            .into_iter()
            .filter(|u| u.path().to_string_lossy().contains("2026/02/11"))
            .map(|u| u.path().file_name().unwrap().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    };
    let alone = day_units(0);
    let crowded = day_units(500);
    assert_eq!(alone.len(), 3, "precondition: the day has three sessions");
    assert_eq!(
        alone, crowded,
        "a day container's units must not depend on how many files its siblings produced"
    );
}

/// Oh My Pi's session directories are containers too, since 2026-09-22.
///
/// Its conversion needed one thing the others did not: its session
/// bodies feed a **home-wide** shared-blob reference count, so a pass
/// that replayed some containers would have counted only the sessions it
/// identified and printed a number that was *wrong* rather than unknown.
/// The route taken is the one the previous chunk named first -- each
/// container stores its own partial reference count with its rows, and
/// the home level sums stored partials and fresh ones.
/// `a_partially_replayed_oh_my_pi_home_sums_the_same_blob_counts` is the
/// test that route exists for; this one is the ordinary seam contract.
#[test]
fn oh_my_pi_session_directories_are_containers() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("omp");
    write(&home.join("config.yml"), "model: x\n");
    let mut victim = PathBuf::new();
    for bucket in ["a", "b"] {
        for i in 0..3 {
            let p = home
                .join("sessions")
                .join(bucket)
                .join(format!("s{i}.jsonl"));
            write(
                &p,
                &format!("{{\"cwd\":\"{}\",\"type\":\"user\"}}\n", root.display()),
            );
            if bucket == "a" && i == 0 {
                victim = p;
            }
        }
    }
    seam_behaves(
        &swamp_core::agents::oh_my_pi::identify,
        &root,
        &home,
        2,
        &victim,
        &home.join("sessions/a"),
    );
}

/// **The conversion's whole point.** A pass that replays one session
/// container and re-identifies another must report exactly the blob
/// reference counts a fully identified pass reports.
///
/// The failure this is written against is the tempting one: sum only
/// what this pass identified. That prints `1` for a blob two sessions
/// reference, which is the number a reference-based GC would act on. The
/// partial stored with each container is what makes the sum whole.
#[test]
fn a_partially_replayed_oh_my_pi_home_sums_the_same_blob_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("omp");
    write(&home.join("config.yml"), "model: x\n");
    let hash = "a".repeat(64);
    // One blob, referenced once from each of two session containers.
    write(&home.join("blobs").join(&hash), "png-bytes");
    for bucket in ["a", "b"] {
        write(
            &home.join("sessions").join(bucket).join("s0.jsonl"),
            &format!(
                "{{\"cwd\":\"{}\",\"type\":\"user\",\"image_url\":\"blob:sha256:{hash}\"}}\n",
                root.display()
            ),
        );
    }

    let blob_note = |units: &[swamp_core::agents::CandidateAgentUnit]| -> String {
        units
            .iter()
            .find(|u| u.path.ends_with(&hash))
            .and_then(|u| u.note.clone())
            .unwrap_or_else(|| "<no blob unit>".to_string())
    };

    let store = tempfile::tempdir().unwrap();
    let identify_with = |at: u64, coverage: EventCoverage| {
        let cache = IdentificationCache::load(store.path());
        let containers = ContainerCache::load(store.path(), coverage);
        let (units, counted) = work_counters::measured(|| {
            let ctx = IdentifyCtx::with_containers(at, &cache, &containers);
            swamp_core::agents::oh_my_pi::identify(&home, &ctx)
        });
        cache.save(store.path(), at).unwrap();
        containers.save(store.path(), at).unwrap();
        (units, counted)
    };

    let (cold, _) = identify_with(1_000, EventCoverage::untrusted());
    let full = blob_note(&cold);
    assert!(
        full.contains("referenced by 2 known session(s)"),
        "precondition: a fully identified pass counts both references: {full}"
    );

    // Touch only container `a`. Container `b` is replayed and must still
    // contribute its stored reference.
    let touched_file = home.join("sessions/a/s0.jsonl");
    append(&touched_file, 16);
    let (mixed, cost) = identify_with(
        2_000,
        touched(
            &root,
            &[touched_file.clone(), home.join("sessions/a")],
            1_000,
        ),
    );
    assert_eq!(
        (cost.containers_identified, cost.containers_reused),
        (1, 1),
        "precondition: exactly one container replayed and one re-identified: {cost:?}"
    );
    assert_eq!(
        blob_note(&mixed),
        full,
        "a partially replayed pass must print the same reference count, not the count of \
         the sessions it happened to identify"
    );
}

/// The fail-closed half: a replayed container whose stored rows carry no
/// reference partial makes every blob count **unknown**, never a smaller
/// number presented as complete.
///
/// Reached by stripping the fact rows out of the stored container, which
/// is what rows written by a shape that predates the partial look like.
#[test]
fn a_replayed_container_without_its_partial_makes_the_count_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("omp");
    write(&home.join("config.yml"), "model: x\n");
    let hash = "b".repeat(64);
    write(&home.join("blobs").join(&hash), "png-bytes");
    write(
        &home.join("sessions/a/s0.jsonl"),
        &format!(
            "{{\"cwd\":\"{}\",\"type\":\"user\",\"image_url\":\"blob:sha256:{hash}\"}}\n",
            root.display()
        ),
    );

    let store = tempfile::tempdir().unwrap();
    let run = |at: u64, coverage: EventCoverage| {
        let cache = IdentificationCache::load(store.path());
        let containers = ContainerCache::load(store.path(), coverage);
        let ctx = IdentifyCtx::with_containers(at, &cache, &containers);
        let units = swamp_core::agents::oh_my_pi::identify(&home, &ctx);
        cache.save(store.path(), at).unwrap();
        containers.save(store.path(), at).unwrap();
        units
    };
    let cold = run(1_000, EventCoverage::untrusted());
    assert!(
        cold.iter().any(|u| u.path().ends_with(&hash)
            && u.note().is_some_and(|n| n.contains("referenced by 1"))),
        "precondition: the cold pass counts the reference"
    );

    // Strip the fact rows from the stored container, leaving rows that
    // look exactly like a shape that never recorded a partial.
    let table = swamp_core::assoc_store::ContainerTable::open(store.path());
    let mut entries = table.load();
    for rows in entries.values_mut() {
        rows.rows.retain(|row| {
            row.first().map(String::as_str) != Some("facts")
                && row.first().map(String::as_str) != Some("fact")
        });
    }
    table.save(&entries, 1_000).unwrap();

    let replayed = run(2_000, quiet(&root, 1_000));
    let note = replayed
        .iter()
        .find(|u| u.path().ends_with(&hash))
        .and_then(|u| u.note())
        .unwrap_or_default();
    assert!(
        note.contains("unknown"),
        "a missing partial must make the count unknown, not short: {note}"
    );
}
