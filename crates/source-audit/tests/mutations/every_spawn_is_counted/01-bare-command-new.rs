//! target: crates/core/src/git.rs
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods, compile:clippy::disallowed_types
//! why: a subprocess built directly, so `subprocess_spawns == 0` is true and meaningless (re-review 3 F2)
pub fn sweep_git_version() -> Option<String> {
    let out = std::process::Command::new("git").arg("--version").output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}
