//! target: crates/cli/src/main.rs
//! by: compile:E0425
//! note: 2026-09-22 -- the pass-less `discover_and_measure` exists only under the `testing` feature (dev-dependencies), so a production caller cannot name it; the pass-taking entry point needs a `report::DiscoveryPass` (see 05/06 and compile_fail `discovery_pass_*`)
//! why: a second discovery pass over the shared history table, which is how ordering started mattering
fn sweep_cli_units(
    scope: &swamp_core::scope::EffectiveScope,
    store: &std::path::Path,
) -> Vec<swamp_core::external::ExternalUnit> {
    swamp_core::external::discover_and_measure(scope, Some(store), true, 1_000, 30, 3600)
        .unwrap_or_default()
}
