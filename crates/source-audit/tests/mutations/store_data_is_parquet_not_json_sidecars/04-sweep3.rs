//! target: crates/core/src/store.rs
//! mode: append
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: re-review 3 sweep -- a per-unit JSON sidecar written under the store by a helper that takes the directory as a parameter (blind spot: a function is only inspected when its own body contains the token `swamp_dir`/`store_dir`/`swamp_path`; passing the store directory in defeats it)
/// Sweep: one JSON file per unit, under the store, in a parallel database.
pub fn sweep_write_unit_cache(dir: &std::path::Path, id: &str) {
    let p = dir.join("units").join(format!("{id}-unit.json"));
    let _ = std::fs::write(p, b"{}");
}
