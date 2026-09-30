//! v0.8.0 G4a: the manager pass through the real spawn layer, against
//! stand-in `brew` and `mise` programs on a private PATH. One test in this
//! file so nothing else races the process-wide PATH; no real brew, mise or
//! rustup is ever run.
//!
//! The tempting wrong patch each stand-in fails is named where it is used.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};
use swamp_core::external::ExternalUnit;
use swamp_core::last_used::LastUsed;
use swamp_core::locations::{Provenance, StorageCategory};
use swamp_core::manager_facts::{FactKind, SystemProbeRunner, collect_within};

fn unit(detector: &str, category: StorageCategory, path: &str) -> ExternalUnit {
    ExternalUnit {
        detector_id: detector.to_string(),
        detector_name: detector.to_string(),
        category,
        provenance: Provenance::BuiltinConvention,
        path: path.into(),
        bytes: 1,
        mtime_max: 0,
        hardlinked: false,
        growth_bytes: None,
        regrowth_count: 0,
        observed_at: 1,
        consumers: Vec::new(),
        note: None,
        evidence: Vec::new(),
        bytes_counted_elsewhere: 0,
        overlap_count: 0,
        last_used: LastUsed::default(),
        children: Vec::new(),
    }
}

fn shim(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn units() -> Vec<ExternalUnit> {
    vec![
        unit("homebrew-other", StorageCategory::Installation, "/opt/homebrew"),
        unit(
            "mise",
            StorageCategory::Installation,
            "/h/.local/share/mise/installs",
        ),
    ]
}

fn run(timeout: Duration) -> (Vec<swamp_core::manager_facts::ManagerFact>, u64, Duration) {
    let started = Instant::now();
    let (rows, work) = swamp_core::work_counters::measured(|| {
        collect_within(&units(), &SystemProbeRunner, 1, Duration::from_secs(120), timeout)
    });
    (rows, work.subprocess_spawns, started.elapsed())
}

fn not_observed(rows: &[swamp_core::manager_facts::ManagerFact]) -> Vec<&str> {
    rows.iter()
        .filter(|f| f.kind == FactKind::NotObserved)
        .map(|f| f.text.as_str())
        .collect()
}

#[test]
fn the_real_spawn_layer_turns_every_failure_into_a_note_and_counts_every_spawn() {
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: this file has exactly one test, so no other thread reads or
    // writes the environment while it runs.
    unsafe { std::env::set_var("PATH", dir.path()) };

    // A binary that is not there: the tempting wrong patch is an error
    // that fails the observation. Each attempt is counted, and each probe
    // says why it has no answer.
    let (rows, spawns, _) = run(Duration::from_secs(2));
    assert_eq!(spawns, 4, "every attempt is a counted spawn");
    let notes = not_observed(&rows);
    assert!(notes.len() >= 4, "{notes:?}");
    assert!(notes.iter().all(|n| n.contains("not installed or not on PATH")), "{notes:?}");

    // A program that never answers: the tempting wrong patch waits for it.
    shim(dir.path(), "brew", "exec /bin/sleep 60");
    shim(dir.path(), "mise", "exec /bin/sleep 60");
    let (rows, spawns, took) = run(Duration::from_millis(600));
    assert!(took < Duration::from_secs(20), "{took:?}");
    assert_eq!(spawns, 4, "every probe is one counted spawn");
    let notes = not_observed(&rows);
    assert!(notes.iter().all(|n| n.contains("did not answer within")), "{notes:?}");

    // A flood: the tempting wrong patch reads it all into memory and
    // parses it. The capture is bounded and the answer is a note.
    let flood = "/usr/bin/yes aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa | /usr/bin/head -c 30000000";
    shim(dir.path(), "brew", flood);
    shim(dir.path(), "mise", flood);
    let (rows, _, _) = run(Duration::from_secs(30));
    let notes = not_observed(&rows);
    assert!(notes.iter().all(|n| n.contains("could not be read")), "{notes:?}");
    assert!(
        !rows.iter().any(|f| f.kind == FactKind::ReportsUnneeded),
        "a flood reports nothing"
    );

    // Binary garbage and a wrong shape.
    let garbage = "printf '\\377\\376\\000\\001garbage'";
    shim(dir.path(), "brew", garbage);
    shim(dir.path(), "mise", garbage);
    let (rows, _, _) = run(Duration::from_secs(10));
    assert!(not_observed(&rows).iter().all(|n| n.contains("could not be read")));

    // The real answers, in the real shape, through the real spawn layer.
    shim(
        dir.path(),
        "brew",
        "case \"$1\" in autoremove) printf 'Would autoremove 2 unneeded formulae:\\nlibevent\\nunbound\\n';; list) printf 'ansible\\natuin\\n';; esac",
    );
    shim(
        dir.path(),
        "mise",
        "case \"$1\" in prune) printf 'mise poetry@2.1.3 is prunable: no tracked config or tool stub requires poetry\\n' >&2;; ls) printf '{\"node\":[{\"requested_version\":\"latest\",\"source\":{\"path\":\"/h/.config/mise/config.toml\"}}]}';; esac",
    );
    let (rows, spawns, _) = run(Duration::from_secs(10));
    assert_eq!(spawns, 4);
    assert!(not_observed(&rows).is_empty(), "{:?}", not_observed(&rows));
    let subjects = |k: FactKind| -> Vec<String> {
        rows.iter()
            .filter(|f| f.kind == k)
            .filter_map(|f| f.subject.clone())
            .collect()
    };
    assert_eq!(subjects(FactKind::ReportsUnneeded), vec!["libevent", "unbound"]);
    assert_eq!(subjects(FactKind::InstalledOnRequest), vec!["ansible", "atuin"]);
    assert_eq!(subjects(FactKind::ReportsPrunable), vec!["poetry@2.1.3"]);
    assert_eq!(subjects(FactKind::ActiveDefault), vec!["node"]);
}
