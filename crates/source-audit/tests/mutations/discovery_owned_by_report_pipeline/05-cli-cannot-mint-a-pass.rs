//! target: crates/cli/src/main.rs
//! by: compile:E0624
//! source: corpus, 2026-09-22 (the modern shape of 01)
//! why: a second discovery pass from the CLI, minting its own token
fn sweep_cli_units_with_a_pass(
    scope: &swamp_core::scope::EffectiveScope,
    store: &std::path::Path,
) -> usize {
    let pass = swamp_core::report::DiscoveryPass::begin();
    let events = swamp_core::fs_events::EventCoverage::default();
    swamp_core::external::discover_and_measure_in(&pass, scope, Some(store), true, 1_000, 30, 3600, &events)
        .map(|u| u.len())
        .unwrap_or(0)
}
