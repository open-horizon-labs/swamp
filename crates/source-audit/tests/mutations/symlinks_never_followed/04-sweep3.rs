//! target: crates/core/src/recheck.rs
//! mode: append
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: re-review 3 sweep -- `fs::metadata` (which follows symlinks) sizing a reviewed member in the recheck module (blind spot: the audit reads exactly walk.rs and attribution.rs; recheck.rs -- whose snapshot gates every destructive sink -- is not one of them)
/// Sweep: stats through the link.
pub fn sweep_member_size(p: &std::path::Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}
