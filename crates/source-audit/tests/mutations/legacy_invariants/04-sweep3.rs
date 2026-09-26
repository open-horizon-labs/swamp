//! target: crates/core/src/scan.rs
//! mode: append
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: re-review 3 sweep -- `canonical_roots` calls canonicalize and throws the answer away, returning the raw roots (blind spot: the rule is `body.contains("canonicalize")` -- presence of the call, not use of its result (slip class 2, never closed for this audit))
/// Sweep: keeps the name and the call, drops the invariant.
pub mod sweep_boundary {
    pub fn canonical_roots(roots: Vec<std::path::PathBuf>) -> Vec<std::path::PathBuf> {
        for r in &roots {
            let _ = std::fs::canonicalize(r);
        }
        roots
    }
}
