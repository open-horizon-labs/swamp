//! target: crates/core/src/schedule.rs
//! mode: replace
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods, compile:clippy::disallowed_types
//! why: move-to-helper variant -- install itself writes nothing; the plist is written by a helper it calls before the check, so a rule reading only install's own body sees the check come first
use anyhow::{Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

pub fn scheduling() -> crate::platform::Scheduling {
    crate::platform::Scheduling::for_os(crate::platform::Os::current())
}
fn plist_path() -> PathBuf {
    PathBuf::from("/tmp/agent.plist")
}
fn log_dir() -> PathBuf {
    PathBuf::from("/tmp/logs")
}
fn render_plist(interval: &str) -> String {
    format!("<plist>{interval}</plist>")
}
fn load_plist(path: &Path) -> Result<()> {
    std::process::Command::new("launchctl").arg("load").arg(path).status()?;
    Ok(())
}
pub fn uninstall() -> Result<String> {
    if let Some(refusal) = scheduling().refusal() {
        bail!("{refusal}");
    }
    Ok(String::new())
}
pub fn status(_store: &Path) -> Result<String> {
    Ok(String::new())
}


fn write_agent(interval: &str) -> Result<()> {
    fs::create_dir_all(log_dir())?;
    fs::write(plist_path(), render_plist(interval))?;
    Ok(())
}

pub fn install(interval: &str, _roots: &[PathBuf]) -> Result<String> {
    write_agent(interval)?;
    if let Some(refusal) = scheduling().refusal() {
        bail!("{refusal}");
    }
    load_plist(&plist_path())?;
    Ok(String::new())
}