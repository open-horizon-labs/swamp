//! target: crates/core/src/report.rs
//! mode: append
//! by: audit:gate_paths_only_inside_gates
//! ported: 2026-09-22 -- a second pass over the shared history table now needs a `DiscoveryPass`; inside `report`, the one module that can reach `DiscoveryPass::begin`, the shortcut is to mint one outside `observe_scope`
//! why: re-review 3 sweep -- a second pass over the shared history table, from report.rs outside observe_scope
/// Sweep: a second pass over the shared history table.
fn sweep_refresh_units(
    scope: &crate::scope::EffectiveScope,
    store_dir: Option<&std::path::Path>,
    observed_at: u64,
) {
    let pass = pass::DiscoveryPass::begin();
    let events = crate::fs_events::EventCoverage::default();
    let _ = crate::external::discover_and_measure_in(
        &pass, scope, store_dir, false, observed_at, 0, 0, &events,
    );
    let _ = crate::agents::discover_and_measure_in(
        &pass, scope, &[], store_dir, false, observed_at, 0, 0, &events,
    );
}
