//! target: crates/tui/src/app.rs
//! by: audit:gate_paths_only_inside_gates
//! why: the removal spawned directly, skipping `tool_removal::execute`'s re-review at `Y`
pub fn sweep_remove_now(
    bin: &swamp_core::fs_gate::spawn::ToolBin,
    argv: &[std::ffi::OsString],
) -> bool {
    swamp_core::fs_gate::destroy::tool_remove(bin, argv, std::time::Duration::from_secs(1))
        .is_ok()
}
