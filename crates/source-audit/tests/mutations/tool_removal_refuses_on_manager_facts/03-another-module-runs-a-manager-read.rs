//! target: crates/core/src/docker.rs
//! by: audit:gate_paths_only_inside_gates
//! why: a resolved manager binary run outside tool removal: reads and removals stay behind their own gate groups
pub fn sweep_mise_version(bin: &crate::fs_gate::spawn::ToolBin) -> bool {
    crate::fs_gate::spawn::run_tool_read(
        bin,
        &[std::ffi::OsString::from("--version")],
        std::time::Duration::from_secs(1),
    )
    .is_ok()
}
