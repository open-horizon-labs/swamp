//! target: crates/core/src/spawn.rs
//! mode: replace
//! by: audit:gate_paths_only_inside_gates
//! why: the wrapper kept and the count dropped -- every caller still goes through `spawn::command`, and nothing is counted
use std::ffi::OsStr;
use std::process::Command;
pub fn command(program: impl AsRef<OsStr>) -> Command {
    Command::new(program)
}
