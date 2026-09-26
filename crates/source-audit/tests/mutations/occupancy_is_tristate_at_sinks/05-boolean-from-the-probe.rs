//! target: crates/core/src/cargo_cleanup.rs
//! by: audit:gate_paths_only_inside_gates
//! source: corpus, 2026-09-22 (the production shape of 02)
//! why: a sink collapsing the tri-state probe into "not free" itself
fn sweep_is_busy(p: &std::path::Path) -> bool {
    !matches!(crate::occupancy::probe_path(p), crate::occupancy::OccupancyState::Free)
}
