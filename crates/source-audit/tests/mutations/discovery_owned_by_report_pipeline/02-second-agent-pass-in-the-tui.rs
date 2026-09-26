//! target: crates/tui/src/app.rs
//! by: compile:E0425
//! note: 2026-09-22 -- the pass-less `discover_and_measure` exists only under the `testing` feature (dev-dependencies), so a production caller cannot name it; the pass-taking entry point needs a `report::DiscoveryPass` (see 05/06 and compile_fail `discovery_pass_*`)
//! why: the agent half of the same second pass, run from the TUI instead of taken from the observation
pub fn sweep_tui_agents(
    scope: &swamp_core::scope::EffectiveScope,
    store: &std::path::Path,
) -> usize {
    swamp_core::agents::discover_and_measure(scope, &[], Some(store), true, 1_000, 30, 3600)
        .map(|u| u.len())
        .unwrap_or(0)
}
