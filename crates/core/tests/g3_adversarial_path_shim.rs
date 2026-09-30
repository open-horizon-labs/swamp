//! Auditor: production spawns of diskutil/tmutil resolve through `PATH`.
//! Tempting wrong patch (the PR's): `Command::new("tmutil")`. A `tmutil`
//! earlier on PATH (a shim, a hostile dir) is run by the volume pass.
//! Own test binary: it changes PATH for the process.
#[test]
fn adv_tmutil_is_not_resolved_through_a_path_shim() {
    let d = tempfile::tempdir().unwrap();
    let marker = d.path().join("ran");
    let shim = d.path().join("tmutil");
    std::fs::write(&shim, format!("#!/bin/sh\ntouch {}\n", marker.display())).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        d.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    // SAFETY: this test binary has a single test.
    unsafe { std::env::set_var("PATH", path) };
    let _ = swamp_core::fs_gate::spawn::run(
        swamp_core::fs_gate::spawn::Program::Tmutil,
        ["listlocalsnapshots", "/"],
        std::time::Duration::from_secs(10),
    );
    assert!(
        !marker.exists(),
        "a PATH shim named tmutil was executed by the allow-listed spawn"
    );
}
