//! target: crates/core/src/external.rs
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: sweep slip -- `json!(..).to_string()` is invisible to a `serde_json::to_string` needle
pub fn sweep_json_macro_write(dir: &std::path::Path, n: u64) {
    let body = serde_json::json!({ "bytes": n }).to_string();
    let _ = std::fs::write(dir.join("units.json"), body);
}
