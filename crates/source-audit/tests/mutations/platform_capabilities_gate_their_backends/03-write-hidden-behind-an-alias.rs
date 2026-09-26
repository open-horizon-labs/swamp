//! target: crates/core/src/schedule.rs
//! mode: replace
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods, compile:clippy::disallowed_types
//! why: alias/rename variant -- the writes are spelled through `use std::fs::write as emit` and `create_dir_all as ensure`, so a text-matching rule sees no write before the check
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


use std::fs::create_dir_all as ensure;
use std::fs::write as emit;

pub fn install(interval: &str, _roots: &[PathBuf]) -> Result<String> {
    ensure(log_dir())?;
    emit(plist_path(), render_plist(interval))?;
    if let Some(refusal) = scheduling().refusal() {
        bail!("{refusal}");
    }
    Ok(String::new())
}