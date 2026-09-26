//! target: crates/cli/src/main.rs
//! mode: append
//! expect: reject
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_types, compile:clippy::disallowed_methods
//! ported: 2026-09-22 -- `swamp_core::spawn::command` is gone (spawns are `fs_gate::spawn::run(Program, ..)`, and there is no `Program` for crontab); the uncounted second scheduler is now spelled with `std::process`
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M7
//! why: a crontab installer in the CLI, beside the live LaunchAgent path
//! blind-spot (old model): the scheduler region is the files whose literals mention `LaunchAgents`; a spawn in any other file is not a scheduler, so a second scheduling mechanism written elsewhere is unaudited
/// Sweep 4: a second scheduler, outside the file that names LaunchAgents.
pub fn sweep4_install_cron() -> std::io::Result<()> {
    std::process::Command::new("crontab").arg("-").status()?;
    Ok(())
}
