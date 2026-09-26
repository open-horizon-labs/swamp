//! target: crates/tui/src/app.rs
//! by: compile:E0451
//! why: the store-interior spelling of the external pass run from the TUI -- a second observation of the same stores and their history family, minting its own token outside the owner
pub fn sweep_second_external_pass(
    scope: &swamp_core::scope::EffectiveScope,
    store: &std::path::Path,
) -> usize {
    let pass = swamp_core::report::DiscoveryPass {
        _minted_by_observe_scope: (),
    };
    swamp_core::external::observe_external(
        &pass,
        scope,
        Some(store),
        true,
        0,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .map(|o| o.interiors.len())
    .unwrap_or(0)
}
