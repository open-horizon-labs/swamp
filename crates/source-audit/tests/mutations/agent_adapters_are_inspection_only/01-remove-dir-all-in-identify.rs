//! target: crates/core/src/agents/claude_code.rs
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: sweep slip -- `fs::remove_dir_all(home.join("logs"))` inside identify: identification deletes data
pub fn sweep_identify_mutation(home: &std::path::Path) {
    let _ = std::fs::remove_dir_all(home.join("logs"));
}
