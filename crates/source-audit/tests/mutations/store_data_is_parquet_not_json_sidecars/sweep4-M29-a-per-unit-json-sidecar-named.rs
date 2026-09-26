//! target: crates/core/src/store.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M29
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: a per-unit JSON sidecar named with `with_extension("json")`
//! blind-spot (old model): a JSON file is a literal that *ends with* `.json`; the extension given on its own (`"json"`) has no dot
/// Sweep 4: one JSON file per unit, named without `.json`.
pub fn sweep4_write_unit_cache(dir: &std::path::Path, id: &str) {
    let p = dir.join("units").join(id).with_extension("json");
    let _ = std::fs::write(p, b"{}");
}
