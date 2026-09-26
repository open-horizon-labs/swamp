//! target: crates/core/src/report.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M17
//! by: compile:E0061
//! why: a pipeline stage called from report.rs through a fn item bound to a local (the owner's X1, on this audit)
//! blind-spot (old model): the rule iterates `f.calls` targets; a value reference (`let gather = walk::discover_and_attribute;`) is an edge in the model but not a call, and `gather(..)` resolves to nothing
/// Sweep 4: the stage, off the bus, bound to a local first.
pub fn sweep4_report_direct(root: &Path) -> usize {
    let gather = crate::walk::discover_and_attribute;
    gather(root, 0, 0, &[]).map(|(w, _)| w.len()).unwrap_or(0)
}
