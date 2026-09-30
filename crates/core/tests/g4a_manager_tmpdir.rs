//! v0.8.0 G4a: a bad `TMPDIR` must not make every spawn fail. One test in
//! this file: it sets process environment.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use swamp_core::fs_gate::spawn::{ManagerCommand, run_manager};

/// The tempting wrong patch: the capture files come only from `TMPDIR`,
/// so an invalid one turns every spawn into NotFound and every manager
/// into "not installed".
#[test]
fn an_invalid_tmpdir_does_not_make_every_spawn_fail() {
    let programs = tempfile::tempdir().unwrap();
    let brew = programs.path().join("brew");
    std::fs::write(&brew, "#!/bin/sh\nprintf ok\n").unwrap();
    std::fs::set_permissions(&brew, std::fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: the only test in this binary.
    unsafe {
        std::env::set_var("SWAMP_TEST_PROGRAM_DIR", programs.path());
        std::env::set_var("TMPDIR", "/definitely/not/a/directory");
    }
    let out = run_manager(ManagerCommand::BrewAutoremoveDryRun, Duration::from_secs(5)).unwrap();
    assert_eq!(out.stdout_lossy(), "ok");
}
