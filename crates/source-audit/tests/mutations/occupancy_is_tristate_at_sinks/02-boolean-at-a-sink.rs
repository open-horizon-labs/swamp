//! target: crates/core/src/cargo_cleanup.rs
//! by: compile:E0425
//! note: 2026-09-22 -- the boolean `is_active` exists only under the `testing` feature; see 05 for the production shape
//! why: the boolean probe whose own doc comment forbids sink use, consumed at a sink
pub fn sweep_boolean_occupancy(p: &std::path::Path) -> bool {
    crate::agents::is_active(p)
}
