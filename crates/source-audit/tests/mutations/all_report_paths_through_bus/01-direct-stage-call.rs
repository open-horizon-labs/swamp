//! target: crates/core/src/report.rs
//! by: compile:E0061
//! ported: 2026-09-22 -- the current walk signature; the one argument missing is the `bus::Stage` only `EventBus::run` mints
//! why: a pipeline stage called directly from report assembly, off the bus
fn sweep_direct_stage(root: &std::path::Path) {
    let _ = crate::walk::discover_and_attribute(root, 1_000, 0, &[]);
}
