//! target: crates/core/src/agents/continue_dev.rs
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods, compile:clippy::disallowed_types
//! why: serde_json::from_reader reads the whole file, unbounded
pub fn sweep_from_reader(p: &std::path::Path) -> Option<serde_json::Value> {
    let f = std::fs::File::open(p).ok()?;
    serde_json::from_reader(f).ok()
}
