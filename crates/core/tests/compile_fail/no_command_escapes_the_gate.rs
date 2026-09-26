//! every-spawn-is-counted: no `std::process::Command` builder escapes the
//! gate; callers get a finished `RunOutput` only.
fn main() {
    let _ = swamp_core::fs_gate::spawn::command("lsof");
}
