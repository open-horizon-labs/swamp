//! target: crates/core/src/consumers/docker.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M15
//! by: audit:bus_static_registration
//! why: one consumer calling another consumer's parser through a `const` fn pointer
//! blind-spot (old model): item-level decls are checked for consumer *type* names only, and a call through a const names the const, not the function: no edge, no path, no type
/// Sweep 4: the helper another consumer now depends on.
pub(crate) fn sweep4_docker_parse(s: &str) -> u64 {
    s.len() as u64
}
//! file: crates/core/src/consumers/signals.rs
//! mode: append
/// Sweep 4: coupling through a function pointer.
const SWEEP4_PARSE: fn(&str) -> u64 = super::docker::sweep4_docker_parse;

pub(crate) fn sweep4_reuse(s: &str) -> u64 {
    SWEEP4_PARSE(s)
}
