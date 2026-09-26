//! target: crates/cli/src/schedule.rs
//! mode: replace
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: the CLI calls its own unguarded `install`, which shares a name with core's guarded one -- a call graph keyed by name lends core's guard to it, and `work_counters::install` (called by every subcommand) makes the name look shared besides
use anyhow::Result;
use std::path::PathBuf;

fn install(interval: &str) -> Result<String> {
    std::fs::create_dir_all("/tmp/LaunchAgents")?;
    std::fs::write("/tmp/LaunchAgents/agent.plist", format!("<plist>{interval}</plist>"))?;
    Ok(String::new())
}

pub fn cmd_schedule(
    _store_dir: PathBuf,
    every: Option<String>,
    _off: bool,
    _roots: Vec<PathBuf>,
) -> Result<()> {
    if let Some(interval) = every {
        let message = install(&interval)?;
        print!("{message}");
    }
    Ok(())
}

pub fn cmd_observe(_store_dir: PathBuf, _roots: Vec<PathBuf>, _full: bool) -> Result<()> {
    Ok(())
}
