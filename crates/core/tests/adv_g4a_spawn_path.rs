//! Adversarial (auditor): how the four manager spawns resolve their
//! binary and what environment they inherit. A fake `brew`/`mise` under a
//! temp dir only; the real managers are never run.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use swamp_core::fs_gate::spawn::{Program, run};

fn shim_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("swamp-adv-g4a-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    for name in ["brew", "mise"] {
        let p = d.join(name);
        std::fs::write(
            &p,
            format!(
                "#!/bin/sh\necho shim > \"{}/{name}.ran\"\nenv > \"{}/{name}.env\"\n",
                d.display(),
                d.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    d
}

/// Tempting wrong patch (the PR head): `Command::new("brew")` resolves
/// through the inherited PATH, so any earlier `brew`/`mise` on PATH is
/// what a scheduled observe runs. Same gap as the G3 finding
/// adv_tmutil_is_not_resolved_through_a_path_shim. Also: a poisoned
/// parent HOMEBREW_*/MISE_*/RUSTUP_* env must not reach the child.
#[test]
fn adv_manager_spawns_do_not_resolve_through_a_path_shim_or_inherit_poisoned_env() {
    let d = shim_dir();
    let old = std::env::var_os("PATH").unwrap_or_default();
    let mut path = std::ffi::OsString::from(d.as_os_str());
    path.push(":");
    path.push(&old);
    // SAFETY: this test binary has a single test; no other thread reads env.
    unsafe {
        std::env::set_var("PATH", &path);
        std::env::set_var("HOMEBREW_NO_AUTO_UPDATE", "");
        std::env::set_var("HOMEBREW_AUTO_UPDATE_SECS", "1");
        std::env::set_var("HOMEBREW_DEVELOPER", "1");
        std::env::set_var("MISE_YES", "1");
        std::env::set_var("MISE_DATA_DIR", "/tmp/poisoned");
    }
    let _ = run(
        Program::Brew,
        ["autoremove", "--dry-run"],
        Duration::from_secs(5),
    );
    let _ = run(
        Program::Mise,
        ["prune", "--dry-run"],
        Duration::from_secs(5),
    );
    let brew_ran = d.join("brew.ran").exists();
    let mise_ran = d.join("mise.ran").exists();
    let env = std::fs::read_to_string(d.join("brew.env")).unwrap_or_default();
    let menv = std::fs::read_to_string(d.join("mise.env")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&d);
    let leaked: Vec<&str> = env
        .lines()
        .chain(menv.lines())
        .filter(|l| {
            l.starts_with("HOMEBREW_AUTO_UPDATE_SECS=")
                || l.starts_with("HOMEBREW_DEVELOPER=")
                || l.starts_with("MISE_YES=")
                || l.starts_with("MISE_DATA_DIR=")
        })
        .collect();
    assert!(
        !brew_ran && !mise_ran,
        "PATH shim ran: brew={brew_ran} mise={mise_ran}; poisoned env reaching the child: {leaked:?}"
    );
    assert!(
        leaked.is_empty(),
        "poisoned env reached the child: {leaked:?}"
    );
}
