//! target: crates/core/src/walk.rs
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: sweep slip -- `use std::fs::metadata as stat_path_inner` follows symlinks past the token match
use std::fs::metadata as stat_path_inner;
pub fn sweep_follow_link(p: &std::path::Path) -> u64 {
    stat_path_inner(p).map(|m| m.len()).unwrap_or(0)
}
