//! target: crates/core/src/external.rs
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: alias/rename variant -- `use std::fs::read_dir as list_entries` in a file that must never traverse
use std::fs::read_dir as list_entries;

pub fn sweep_list_external_members(path: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(rd) = list_entries(path) else {
        return Vec::new();
    };
    rd.flatten().map(|e| e.path()).collect()
}
