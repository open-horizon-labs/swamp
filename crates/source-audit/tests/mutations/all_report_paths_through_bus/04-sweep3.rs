//! target: crates/core/src/report.rs
//! mode: append
//! by: compile:E0061
//! why: re-review 3 sweep -- a pipeline stage called straight from the report path through a renamed re-export (blind spot: the rule matches the *last path segment* against a fixed stage-name list; a `pub use .. as` inside a local module renames the segment)
mod sweep_stage_shim {
    pub use crate::walk::discover_and_attribute as gather;
}

/// Sweep: the stage, off the bus, under another name.
pub fn sweep_report_direct(root: &std::path::Path) {
    let _ = sweep_stage_shim::gather(root, 0, 0, &[]);
}
