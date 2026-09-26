//! target: crates/core/src/consumer_wiring.rs
//! by: compile:E0425
//! note: 2026-09-22 -- the pass-less `discover_and_measure` exists only under the `testing` feature (dev-dependencies), so a production caller cannot name it; the pass-taking entry point needs a `report::DiscoveryPass` (see 05/06 and compile_fail `discovery_pass_*`)
//! why: the same second pass hidden inside core, one file away from the observation that owns it
pub fn sweep_core_second_pass(
    scope: &crate::scope::EffectiveScope,
    store: &std::path::Path,
) -> usize {
    crate::external::discover_and_measure(scope, Some(store), false, 1_000, 30, 3600)
        .map(|u| u.len())
        .unwrap_or(0)
}
