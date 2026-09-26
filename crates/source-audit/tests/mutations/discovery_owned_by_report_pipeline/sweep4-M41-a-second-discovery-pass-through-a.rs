//! target: crates/core/src/report.rs
//! mode: append
//! expect: reject
//! by: audit:gate_paths_only_inside_gates
//! ported: 2026-09-22 -- the entry point now takes a `DiscoveryPass`; the reviewer's const fn pointer, with the pass minted in report.rs outside `observe_scope` (the only place that could)
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M41
//! why: a second discovery pass through a `const` fn pointer to `external::discover_and_measure`
//! blind-spot (old model): owners are `callers()` of the entries; a const item is not a function, so naming the entry in one creates no edge, and calling the const names the const
type Sweep4Pass = fn(
    &DiscoveryPass,
    &crate::scope::EffectiveScope,
    Option<&Path>,
    bool,
    u64,
    u64,
    u64,
    &crate::fs_events::EventCoverage,
) -> anyhow::Result<Vec<crate::external::ExternalUnit>>;

const SWEEP4_SECOND_PASS: Sweep4Pass = crate::external::discover_and_measure_in;

/// Sweep 4: a second pass over the shared history table.
fn sweep4_refresh_units(
    scope: &crate::scope::EffectiveScope,
    store_dir: Option<&Path>,
    observed_at: u64,
) -> usize {
    let events = crate::fs_events::EventCoverage::default();
    SWEEP4_SECOND_PASS(&pass::DiscoveryPass::begin(), scope, store_dir, true, observed_at, 0, 0, &events)
        .map(|u| u.len())
        .unwrap_or(0)
}
