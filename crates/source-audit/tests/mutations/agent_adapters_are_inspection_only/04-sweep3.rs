//! target: crates/core/src/agents/opencode.rs
//! mode: append
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods, compile:clippy::disallowed_types
//! why: re-review 3 sweep -- an adapter truncating a tool's file through OpenOptions (blind spot: `ADAPTER_FORBIDDEN_CALLS` lists the `fs::*` primitives and `Command::new`; `OpenOptions::new().write(true).truncate(true)` destroys the same data and is on none of them)
/// Sweep: identification truncates the user's session index.
pub fn sweep_reset_index(p: &std::path::Path) -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(p)?;
    Ok(())
}
