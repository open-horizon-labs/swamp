//! target: crates/core/src/scan.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M3
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: a `canonical_roots` that uses canonicalize only as an existence filter and returns the raw roots
//! blind-spot (old model): `flows` is `tail.contains("canonicalize")`: the word in the tail statement, not the canonical path in the return value
/// Sweep 4: honoured, in the tail, and never returned.
pub mod sweep4_boundary {
    pub fn canonical_roots(roots: Vec<std::path::PathBuf>) -> Vec<std::path::PathBuf> {
        roots
            .into_iter()
            .filter(|r| std::fs::canonicalize(r).is_ok())
            .collect()
    }
}
