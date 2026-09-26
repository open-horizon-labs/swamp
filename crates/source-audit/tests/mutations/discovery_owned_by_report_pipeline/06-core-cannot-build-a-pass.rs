//! target: crates/core/src/consumer_wiring.rs
//! by: compile:private-fields-literal, compile:E0451
//! source: corpus, 2026-09-22 (the modern shape of 03)
//! why: a second discovery pass inside core, building the token as a literal
fn sweep_core_pass(
    scope: &crate::scope::EffectiveScope,
    store: &std::path::Path,
) -> usize {
    let pass = crate::report::DiscoveryPass { _minted_by_observe_scope: () };
    let events = crate::fs_events::EventCoverage::default();
    crate::external::observe_external(&pass, scope, Some(store), false, 1_000, 30, 3600, &events)
        .map(|o| o.units.len())
        .unwrap_or(0)
}
