//! target: crates/core/src/attribution.rs
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: canonicalizing a child dereferences whatever it points at
pub fn sweep_canonicalize_child(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    std::fs::canonicalize(dir.join(name)).unwrap_or_default()
}
