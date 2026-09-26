//! every-spawn-is-counted: the gate spawns only a named, counted
//! `Program`; an arbitrary binary name does not compile.
use std::time::Duration;

fn main() {
    let _ = swamp_core::fs_gate::spawn::run("rm", ["-rf", "/"], Duration::from_secs(1));
}
