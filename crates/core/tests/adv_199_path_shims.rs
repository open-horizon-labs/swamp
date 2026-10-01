//! #199: a shim earlier on `PATH` is never what swamp runs, for any
//! allow-listed program, and a fake in the test build's program directory
//! is the only way a test reaches a fake. One test per process step: this
//! binary sets PATH and SWAMP_TEST_PROGRAM_DIR, so it holds one test.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use swamp_core::fs_gate::program_paths::{Plan, plan};
use swamp_core::fs_gate::spawn::Program;

fn exe_of(p: Program) -> Option<std::path::PathBuf> {
    match plan(p) {
        Ok(Plan::Fixed { exe, .. }) => Some(exe),
        Ok(Plan::Scrubbed(s)) => Some(s.exe),
        Err(_) => None,
    }
}

/// Tempting wrong patch: a PATH lookup kept as a fallback when the fixed
/// list has nothing ("not found, try PATH"), which is exactly where a shim
/// wins.
#[test]
fn path_shims_never_run_and_only_the_program_dir_supplies_fakes() {
    let shims = tempfile::tempdir().unwrap();
    let shim_dir = std::fs::canonicalize(shims.path()).unwrap();
    for p in Program::ALL {
        let f = shim_dir.join(p.binary());
        std::fs::write(&f, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // SAFETY: the only test in this binary.
    unsafe { std::env::set_var("PATH", format!("{}:/usr/bin:/bin", shim_dir.display())) };
    for p in Program::ALL {
        if let Some(exe) = exe_of(*p) {
            assert!(
                !exe.starts_with(&shim_dir),
                "{p:?} resolved to the PATH shim {}",
                exe.display()
            );
        }
    }

    // A fake in the program directory is used; one absent from it is not
    // available (never the real program, never PATH).
    let fakes = tempfile::tempdir().unwrap();
    let git = fakes.path().join("git");
    std::fs::write(&git, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
    unsafe { std::env::set_var("SWAMP_TEST_PROGRAM_DIR", fakes.path()) };
    assert_eq!(exe_of(Program::Git), Some(git));
    for p in Program::ALL.iter().filter(|p| **p != Program::Git) {
        assert_eq!(exe_of(*p), None, "{p:?} reached outside the fake directory");
    }
    unsafe { std::env::remove_var("SWAMP_TEST_PROGRAM_DIR") };
}
