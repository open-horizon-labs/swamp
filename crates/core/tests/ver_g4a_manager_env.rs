//! v0.8.0 G4a verification: what the scrubbed manager child sees. One
//! test in this binary: it changes the process environment.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use swamp_core::fs_gate::spawn::{ManagerCommand, run_manager};

/// The mise detector locates the store through `MISE_DATA_DIR` and
/// `MISE_CONFIG_DIR` (locations/mise.rs), and mise itself reads its global
/// config from `MISE_CONFIG_DIR` / `XDG_CONFIG_HOME` (verified with mise
/// 2026.9.15: `ls --global --json` under `env -i` answers from a different
/// file when either is set). Tempting wrong patch: pass only
/// `MISE_GLOBAL_CONFIG_FILE`, so the probe describes a different store
/// than the unit and a person's real global default is not held out.
/// Loader variables must never reach the child.
#[test]
fn ver_mise_child_sees_the_same_store_the_detector_reads_and_no_loader_vars() {
    let programs = tempfile::tempdir().unwrap();
    let mise = programs.path().join("mise");
    std::fs::write(&mise, "#!/bin/sh\n/usr/bin/env\n").unwrap();
    std::fs::set_permissions(&mise, std::fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: the only test in this binary.
    unsafe {
        std::env::set_var("SWAMP_TEST_PROGRAM_DIR", programs.path());
        std::env::set_var("MISE_CONFIG_DIR", "/ver/cfg");
        std::env::set_var("MISE_DATA_DIR", "/ver/data");
        std::env::set_var("DYLD_INSERT_LIBRARIES", "/ver/evil.dylib");
        std::env::set_var("LD_PRELOAD", "/ver/evil.so");
    }
    let out = run_manager(ManagerCommand::MiseListGlobalJson, Duration::from_secs(5)).unwrap();
    let env = out.stdout_lossy();
    for bad in ["DYLD_", "LD_PRELOAD"] {
        assert!(!env.contains(bad), "{bad} reached the child:\n{env}");
    }
    for want in ["MISE_CONFIG_DIR=/ver/cfg", "MISE_DATA_DIR=/ver/data"] {
        assert!(env.contains(want), "{want} did not reach the child:\n{env}");
    }
}
