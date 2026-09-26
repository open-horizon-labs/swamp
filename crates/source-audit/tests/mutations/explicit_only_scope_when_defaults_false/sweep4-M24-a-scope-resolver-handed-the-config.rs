//! target: crates/core/src/scope.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M24
//! by: compile:E0308
//! why: a scope resolver handed the config's *fields* instead of the config, which runs every detector
//! blind-spot (old model): a resolver is a function whose parameter type names the `*Config`; passing `defaults`/`enabled_detectors` as plain values takes it out of scope
/// Sweep 4: explicit-only scope, resolved from the config's parts.
pub fn sweep4_roots_from_parts(
    env: &Environment,
    registry: &Registry,
    _defaults: bool,
    _enabled_detectors: &[String],
) -> Vec<PathBuf> {
    registry
        .resolve(env, &[])
        .into_iter()
        .flat_map(|(_, locs)| locs.into_iter().filter_map(|l| l.path))
        .collect()
}
