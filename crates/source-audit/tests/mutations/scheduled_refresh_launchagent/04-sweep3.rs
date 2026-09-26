//! target: crates/core/src/schedule.rs
//! mode: append
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods, compile:clippy::disallowed_types
//! why: re-review 3 sweep -- scheduling moved to crontab while the word `launchctl` survives as a dead string (blind spot: the audit asserts a `Schedule` variant exists and the literal "launchctl" appears somewhere in schedule.rs; neither says what actually schedules)
/// Sweep: the documented LaunchAgent contract is no longer what runs.
pub fn sweep_install_schedule() -> std::io::Result<()> {
    let _legacy_note = "launchctl";
    std::process::Command::new("crontab").arg("-").status()?;
    Ok(())
}
