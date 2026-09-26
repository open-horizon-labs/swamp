//! `systemd --user` unit files: swamp writes at most three, all named by
//! `crate::systemd_user::{SERVICE,TIMER,COLLECTOR}` inside
//! `crate::systemd_user::unit_dir()`. Every read, write and remove of
//! one of those files is here, inside the capability gate, the same
//! discipline `fs_gate::store` holds for the macOS LaunchAgent plist
//! (`docs/architecture.md`, "Capability gates"). `systemd_user.rs` keeps
//! the marker/ownership decisions (`is_ours`'s caller decides what a
//! `None`/`Some(false)` means); this module only touches disk.

use std::io;
use std::path::Path;

/// Whether `path` is a file swamp wrote (its first line is
/// `crate::systemd_user::MARKER`), does not exist (`Ok(None)`), or
/// exists and is not swamp's (`Ok(Some(false))`).
pub fn is_ours(path: &Path) -> io::Result<Option<bool>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(
            text.lines().next() == Some(crate::systemd_user::MARKER),
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Creates the unit directory (`~/.config/systemd/user` or its test
/// override) if it is missing.
pub fn create_unit_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// Writes `body` to `dir/name`, atomically (sibling temp file + rename,
/// same discipline as `fs_gate::store`'s writers). The caller
/// (`systemd_user::write_unit`) has already refused to overwrite a
/// foreign file; this never checks that itself.
pub fn write_unit(dir: &Path, name: &str, body: &str) -> io::Result<()> {
    let path = dir.join(name);
    let tmp = dir.join(format!(".{name}.swamp-tmp"));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)
}

/// Removes a unit file swamp owns. The caller has already confirmed
/// `is_ours(path) == Ok(Some(true))`.
pub fn remove_unit(path: &Path) -> io::Result<()> {
    std::fs::remove_file(path)
}

/// This process's real uid, for a `loginctl show-user <uid>` argument
/// (`systemd_user::linger`).
pub fn current_uid() -> u32 {
    super::sys::current_uid()
}

/// Runs `systemctl --user`/`loginctl` (`program`/`args` -- the shape
/// allow-list in `fs_gate::spawn` decides what is actually permitted),
/// bounded by `timeout`.
pub fn run(
    program: super::spawn::Program,
    args: Vec<&str>,
    timeout: std::time::Duration,
) -> io::Result<super::spawn::RunOutput> {
    super::spawn::run(program, args, timeout)
}
