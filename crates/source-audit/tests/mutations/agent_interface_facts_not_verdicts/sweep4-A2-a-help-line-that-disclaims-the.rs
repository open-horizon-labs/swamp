//! target: crates/core/src/render.rs
//! mode: append
//! expect: accept
//! source: review-4 sweep (A group, reviewer_counterexamples_stack4_sweep.rs), A2
//! ported: 2026-09-22 -- private: a public item nothing names is now dead public API (no_unreferenced_public_items); the literal, the reviewer's point, is unchanged
//! why: A2: a help line that *disclaims* the verdict: "swamp never labels a directory unused"
//! blind-spot (old model): negation is a fixed list of whole phrases (`never unused`, `not proven unused`, ...); any other disclaimer is a verdict
/// Sweep 4 A2: the policy, stated to the reader.
fn sweep4_a2_disclaimer() -> &'static str {
    "swamp never labels a directory unused; it reports sizes and evidence"
}
