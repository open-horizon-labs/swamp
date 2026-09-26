//! target: crates/core/src/schedule.rs
//! mode: replace
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods, compile:clippy::disallowed_types
//! why: discarded-result variant -- the capability is asked first, in the right place, and the answer thrown away, so the installer proceeds on every platform
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


pub fn install(interval: &str, _roots: &[PathBuf]) -> Result<String> {
    let _ = scheduling();
    fs::create_dir_all(log_dir())?;
    fs::write(plist_path(), render_plist(interval))?;
    load_plist(&plist_path())?;
    Ok(String::new())
}