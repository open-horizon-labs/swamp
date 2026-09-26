//! target: crates/core/src/schedule.rs
//! mode: replace
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods, compile:clippy::disallowed_types
//! why: a write through OpenOptions::truncate, which a hand-written list of fs::write/create_dir_all does not name -- the std::fs set is inverted so an unlisted mutation fails closed
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
    use std::io::Write as _;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(plist_path())?;
    f.write_all(render_plist(interval).as_bytes())?;
    if let Some(refusal) = scheduling().refusal() {
        bail!("{refusal}");
    }
    Ok(String::new())
}