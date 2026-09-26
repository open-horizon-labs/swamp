//! target: crates/core/src/agents/roo_code.rs
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: sweep slip -- `use std::fs::read_dir as list_dir` traverses past a token match
use std::fs::read_dir as list_dir;
pub fn sweep_traverse(home: &std::path::Path) -> usize {
    list_dir(home).map(|r| r.count()).unwrap_or(0)
}
