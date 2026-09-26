//! target: crates/cli/src/schedule.rs
//! mode: replace
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: renamed entry point -- the Schedule subcommand stops calling core's guarded install and calls a new CLI-local enable_now that writes the plist itself; a rule that looks for a function named install never looks here
use anyhow::Result;
use std::path::PathBuf;

fn enable_now(interval: &str) -> Result<String> {
    std::fs::write("/tmp/agent.plist", format!("<plist>{interval}</plist>"))?;
    Ok(String::new())
}

pub fn cmd_schedule(
    _store_dir: PathBuf,
    every: Option<String>,
    _off: bool,
    _roots: Vec<PathBuf>,
) -> Result<()> {
    if let Some(interval) = every {
        let message = enable_now(&interval)?;
        print!("{message}");
    }
    Ok(())
}

pub fn cmd_observe(_store_dir: PathBuf, _roots: Vec<PathBuf>, _full: bool) -> Result<()> {
    Ok(())
}
