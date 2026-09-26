//! target: crates/core/src/consumer_wiring.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M40
//! by: audit:ids_only_in_their_module
//! why: a wiring table of detector id *values* held in an item-level `const`
//! blind-spot (old model): at item level only detector id constant *names* are looked for; inside functions, values only as match-arm patterns or `== "v"` -- a table of literals is neither
/// Sweep 4: the wiring table is back, as data.
pub const SWEEP4_WIRING: &[(&str, &str)] = &[
    ("cargo-home", "cargo fetch"),
    ("npm", "npm ci"),
    ("aider", "pip install aider-chat"),
];

pub fn sweep4_recovery_hint(id: &str) -> &'static str {
    SWEEP4_WIRING
        .iter()
        .find(|(d, _)| *d == id)
        .map(|(_, h)| *h)
        .unwrap_or("")
}
