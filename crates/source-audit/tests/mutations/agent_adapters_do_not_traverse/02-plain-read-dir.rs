//! target: crates/core/src/agents/copilot_cli.rs
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: the original forbidden shape must still be rejected
pub fn sweep_traverse_plain(home: &std::path::Path) -> usize {
    std::fs::read_dir(home).map(|r| r.count()).unwrap_or(0)
}
