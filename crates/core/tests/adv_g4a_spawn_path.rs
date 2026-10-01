//! Adversarial (auditor): how the four manager spawns resolve their
//! binary and what environment they inherit. A fake `brew`/`mise` under a
//! temp dir only; the real managers are never run.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use swamp_core::fs_gate::spawn::{ManagerCommand, run_manager};

/// Writes a `brew` and a `mise` into `d` that record they ran (`<tag>`)
/// and the environment they got.
fn fakes(d: &std::path::Path, tag: &str) {
    std::fs::create_dir_all(d).unwrap();
    for name in ["brew", "mise"] {
        let p = d.join(name);
        std::fs::write(
            &p,
            format!(
                "#!/bin/sh\necho {tag} >> \"{}/{name}.ran\"\nenv > \"{}/{name}.env\"\n",
                d.display(),
                d.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// Tempting wrong patch: `Command::new("brew")` resolves through the
/// inherited PATH, so any earlier `brew`/`mise` on PATH is what a
/// scheduled observe runs. The real manager commands run here (through
/// `run_manager`, the shapes the observe pass uses), so a spawn really
/// happens: the fake in the program directory must be what ran, the PATH
/// shim must not have, and a poisoned HOMEBREW_*/MISE_* must not reach
/// the child. (The previous version ran a non-shape and was refused
/// before anything spawned, so it passed without a spawn.)
#[test]
fn adv_manager_spawns_do_not_resolve_through_a_path_shim_or_inherit_poisoned_env() {
    let tmp = tempfile::tempdir().unwrap();
    let shim = tmp.path().join("shim");
    let fake = tmp.path().join("fake");
    fakes(&shim, "shim");
    fakes(&fake, "fake");
    let old = std::env::var_os("PATH").unwrap_or_default();
    let mut path = std::ffi::OsString::from(shim.as_os_str());
    path.push(":");
    path.push(&old);
    // SAFETY: this test binary has a single test; no other thread reads env.
    unsafe {
        std::env::set_var("PATH", &path);
        std::env::set_var("SWAMP_TEST_PROGRAM_DIR", &fake);
        std::env::set_var("HOMEBREW_AUTO_UPDATE_SECS", "1");
        std::env::set_var("HOMEBREW_DEVELOPER", "1");
        std::env::set_var("MISE_YES", "1");
        std::env::set_var("MISE_DATA_DIR", "/tmp/poisoned");
    }
    let _ = run_manager(ManagerCommand::BrewAutoremoveDryRun, Duration::from_secs(5));
    let _ = run_manager(ManagerCommand::MisePruneToolsDryRun, Duration::from_secs(5));
    for name in ["brew", "mise"] {
        assert!(
            fake.join(format!("{name}.ran")).exists(),
            "{name}: nothing ran"
        );
        assert!(
            !shim.join(format!("{name}.ran")).exists(),
            "{name}: the PATH shim ran"
        );
    }
    let brew_env = std::fs::read_to_string(fake.join("brew.env")).unwrap_or_default();
    let mise_env = std::fs::read_to_string(fake.join("mise.env")).unwrap_or_default();
    let leaked: Vec<&str> = brew_env
        .lines()
        .chain(mise_env.lines())
        .filter(|l| {
            l.starts_with("HOMEBREW_AUTO_UPDATE_SECS=")
                || l.starts_with("HOMEBREW_DEVELOPER=")
                || l.starts_with("MISE_YES=")
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "poisoned env reached the child: {leaked:?}"
    );
    // `MISE_DATA_DIR` is one of mise's own directory settings, passed to
    // mise on purpose (`program_paths::MISE_PASSTHROUGH`: the probe must
    // describe the store the mise unit measures) and to nothing else. The
    // old version listed it as a leak but never spawned, so it never saw
    // that it reaches mise by design.
    assert!(
        mise_env.contains("MISE_DATA_DIR=/tmp/poisoned"),
        "{mise_env}"
    );
    assert!(!brew_env.contains("MISE_DATA_DIR="), "{brew_env}");
}
