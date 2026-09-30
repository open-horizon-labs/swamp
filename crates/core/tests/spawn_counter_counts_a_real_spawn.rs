//! The instrument `a_disabled_detector_must_not_probe_its_tool`
//! (reviewer_counterexamples_stack2) rests on, checked rather than assumed:
//! a command that really starts is counted, in the scoped sink.
//!
//! It used to call the machine's own `brew --prefix` and assumed "a failure
//! to spawn is still a spawn attempt". Since the manager pass's program
//! resolver (`fs_gate::program_paths`), `brew` is resolved from fixed
//! locations with an ownership check before anything starts: on a Linux
//! runner there is no `brew` (not found), and on a standard Apple Silicon
//! install `/opt/homebrew/bin` is group-writable (refused). Neither starts
//! a process, so neither is counted, which is correct: the counter counts
//! processes, not intentions. So the real spawn here is a fixture `brew`.
//!
//! One test in this binary: it sets the process-wide
//! `SWAMP_TEST_PROGRAM_DIR`.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use swamp_core::locations::{CommandRunner, SystemCommandRunner};

#[test]
fn the_spawn_counter_counts_a_real_spawn_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: the only test in this binary; no other thread touches the
    // environment while it runs.
    unsafe { std::env::set_var("SWAMP_TEST_PROGRAM_DIR", dir.path()) };

    // Not there: nothing starts, nothing is counted. Tempting wrong patch:
    // counting before resolution, so a refused or missing program looks
    // like a spawn and a disabled detector's zero-spawn check can be met
    // by a counter that counts attempts.
    let (missing, counted) = swamp_core::work_counters::measured(|| {
        SystemCommandRunner.run("brew", &["--prefix"], Duration::from_secs(5))
    });
    let why = missing.err().expect("no fixture brew yet");
    assert_eq!(counted.subprocess_spawns, 0, "nothing started: {why}");

    // There: one process starts, and it is counted in the scoped sink.
    // Tempting wrong patch: a counter nothing increments, which would make
    // every `== 0` spawn assertion pass vacuously.
    let brew = dir.path().join("brew");
    std::fs::write(&brew, "#!/bin/sh\necho /fixture/homebrew\n").unwrap();
    std::fs::set_permissions(&brew, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (ran, counted) = swamp_core::work_counters::measured(|| {
        SystemCommandRunner.run("brew", &["--prefix"], Duration::from_secs(5))
    });
    let ran = ran.unwrap_or_else(|e| panic!("the fixture brew runs: {e}"));
    assert_eq!(ran.stdout, "/fixture/homebrew");
    assert_eq!(
        counted.subprocess_spawns, 1,
        "the spawn counter must count an allow-listed command runner spawn"
    );
}
