//! target: crates/core/src/report.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M19
//! by: compile:E0308
//! why: a pipeline stage called from report.rs through a fn item *returned from a helper*
//! blind-spot (old model): `helper()(..)` is an `ExprCall` whose callee is not a path, so it is no call at all; the helper names the stage only as a value, and values are not calls
type Sweep4Stage = fn(
    &Path,
    u64,
    u64,
    &[PathBuf],
) -> anyhow::Result<(
    Vec<crate::git::DiscoveredWorktree>,
    crate::attribution::AttributionResult,
)>;

fn sweep4_stage() -> Sweep4Stage {
    crate::walk::discover_and_attribute
}

/// Sweep 4: the stage, off the bus, returned from a helper.
pub fn sweep4_report_via_helper(root: &Path) -> usize {
    sweep4_stage()(root, 0, 0, &[])
        .map(|(w, _)| w.len())
        .unwrap_or(0)
}
