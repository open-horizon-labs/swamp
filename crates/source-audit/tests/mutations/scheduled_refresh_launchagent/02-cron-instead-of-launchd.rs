//! target: crates/core/src/schedule.rs
//! mode: replace
//! by: audit:gate_paths_only_inside_gates, audit:guardrail_metadata
//! why: scheduling moved off launchd, so the documented LaunchAgent contract (and its user-visible plist) is no longer what runs
use std::process::Command;

pub fn install(interval_secs: u64) -> anyhow::Result<()> {
    let line = format!("*/{} * * * * swamp observe\n", interval_secs / 60);
    Command::new("crontab").arg("-").arg(line).status()?;
    Ok(())
}
