//! target: crates/core/src/consumer_wiring.rs
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: a serializer call in a file that is not an allow-listed writer
pub fn sweep_serialize_and_write(dir: &std::path::Path, rows: &[u64]) {
    let body = serde_json::to_vec(rows).unwrap_or_default();
    let _ = std::fs::write(dir.join("rows.json"), body);
}
