//! Auditor (#199): a program run as `Plan::Fixed` (git, gh, docker and the
//! system tools) keeps swamp's inherited environment. Resolution never reads
//! `PATH`, but the trusted binary itself still reads the environment: a
//! loader-injection variable or a `PATH`/`GIT_*` set by a checkout's
//! direnv/mise `[env]` makes the trusted program run attacker code. One
//! test in its own process: it changes the process environment.
//!
//! Tempting wrong patch this fails: "resolution is fixed, so the inherited
//! environment is harmless" (keeping `Command::new(exe)` with no env
//! removal for `Plan::Fixed`).

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use swamp_core::fs_gate::spawn::{Program, run};

#[test]
fn a_fixed_program_does_not_inherit_loader_injection_or_a_poisoned_path() {
    let fakes = tempfile::tempdir().unwrap();
    let out = fakes.path().join("env.txt");
    let docker = fakes.path().join("docker");
    std::fs::write(
        &docker,
        format!("#!/bin/sh\n/usr/bin/env > \"{}\"\n", out.display()),
    )
    .unwrap();
    std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: the only test in this binary.
    unsafe {
        std::env::set_var("SWAMP_TEST_PROGRAM_DIR", fakes.path());
        std::env::set_var("DYLD_INSERT_LIBRARIES", "/tmp/evil.dylib");
        std::env::set_var("LD_PRELOAD", "/tmp/evil.so");
        std::env::set_var("GIT_EXEC_PATH", "/tmp/evil-git-core");
        std::env::set_var("GIT_CONFIG_PARAMETERS", "'core.fsmonitor'='/tmp/evil'");
        std::env::set_var("PATH", "/tmp/evil-bin:/usr/bin:/bin");
    }
    let r = run(
        Program::Docker,
        ["version", "--format", "json"],
        Duration::from_secs(10),
    )
    .expect("fake docker ran");
    assert!(r.success());
    let env = std::fs::read_to_string(&out).unwrap();
    let leaked: Vec<&str> = env
        .lines()
        .filter(|l| {
            [
                "DYLD_INSERT_LIBRARIES=",
                "LD_PRELOAD=",
                "GIT_EXEC_PATH=",
                "GIT_CONFIG_PARAMETERS=",
            ]
            .iter()
            .any(|p| l.starts_with(p))
                || (l.starts_with("PATH=") && l.contains("/tmp/evil-bin"))
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "a Plan::Fixed child inherited: {leaked:?}"
    );
}

/// Prints where each program resolves on this machine (no assertion; run
/// with --ignored --nocapture for the audit's resolution table).
#[test]
#[ignore]
fn audit_print_resolution_table() {
    for p in Program::ALL {
        println!(
            "{:>10}: {:?}",
            p.binary(),
            swamp_core::fs_gate::program_paths::plan(*p).map(|pl| match pl {
                swamp_core::fs_gate::program_paths::Plan::Fixed { exe, .. } =>
                    format!("Fixed {}", exe.display()),
                swamp_core::fs_gate::program_paths::Plan::Scrubbed(s) =>
                    format!("Scrubbed {}", s.exe.display()),
            })
        );
    }
}
