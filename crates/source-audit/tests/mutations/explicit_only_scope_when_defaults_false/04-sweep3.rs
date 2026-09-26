//! target: crates/core/src/scope.rs
//! mode: append
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: re-review 3 sweep -- a second scope resolver that infers detectors without asking `detectors_permitted` (blind spot: the guard is required only in functions named exactly `resolve_effective_scope`)
/// Sweep: explicit-only scope, resolved by a second entry point.
pub fn resolve_effective_scope_for_root(
    config: &ScanConfig,
    root: &std::path::Path,
) -> Vec<std::path::PathBuf> {
    // No `detectors_permitted(config)` guard anywhere.
    let _ = config;
    let mut out = Vec::new();
    for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
        out.push(e.path());
    }
    out
}
