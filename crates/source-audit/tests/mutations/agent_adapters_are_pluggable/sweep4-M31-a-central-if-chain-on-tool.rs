//! target: crates/cli/src/main.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M31
//! by: audit:ids_only_in_their_module
//! why: a central if-chain on tool id *values*, in the CLI
//! blind-spot (old model): central dispatch is a `match` arm on an id, or two id *constant names* in one body; `id == "cline"` comparisons are neither (the detector rule checks `== "v"`, this one does not)
/// Sweep 4: adding an adapter now means editing this chain.
pub fn sweep4_dispatch(id: &str) -> u64 {
    if id == "cline" {
        1
    } else if id == "codex" {
        2
    } else if id == "cursor" {
        3
    } else {
        0
    }
}
