//! v0.8.0 G4a: a manager probe answers the same wherever swamp was
//! started. `mise ls --global --json` resolves local configuration up
//! the tree, so a project directory hides a global tool; the spawn layer
//! runs every manager from one fixed directory. One test in this file:
//! it changes the process's working directory and environment.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use swamp_core::fs_gate::spawn::{ManagerCommand, run_manager};

/// The tempting wrong patch: the child inherits the caller's working
/// directory. This fake `mise` prints a different answer per directory,
/// as the real one does, and the answer must not depend on where swamp
/// runs.
#[test]
fn a_manager_probe_answers_the_same_from_any_working_directory() {
    let programs = tempfile::tempdir().unwrap();
    let mise = programs.path().join("mise");
    std::fs::write(
        &mise,
        "#!/bin/sh\nif [ \"$PWD\" = / ]; then printf '{\"node\":[]}'; else printf '{\"other\":[]} from %s' \"$PWD\"; fi\n",
    )
    .unwrap();
    std::fs::set_permissions(&mise, std::fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: the only test in this binary.
    unsafe { std::env::set_var("SWAMP_TEST_PROGRAM_DIR", programs.path()) };
    let mut seen = Vec::new();
    for dir in [
        std::env::temp_dir(),
        std::path::PathBuf::from("/usr"),
        programs.path().to_path_buf(),
    ] {
        std::env::set_current_dir(&dir).unwrap();
        let out = run_manager(ManagerCommand::MiseListGlobalJson, Duration::from_secs(5)).unwrap();
        seen.push(out.stdout_lossy());
    }
    assert!(seen.iter().all(|s| s == "{\"node\":[]}"), "{seen:?}");
}
