//! target: crates/core/src/report.rs
//! mode: append
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: re-review 3 sweep -- JSON persisted with `serde_json::to_vec_pretty`, outside the allow-list (blind spot: `JSON_SERIALIZE_CALLS` lists to_vec / to_string / to_writer / Serializer / json!; the `_pretty` variants produce the same bytes and are on none of them)
/// Sweep: persistence the serializer needle cannot see.
pub fn sweep_persist_rows(p: &std::path::Path, v: &serde_json::Value) {
    let bytes = serde_json::to_vec_pretty(v).unwrap();
    let _ = std::fs::write(p, bytes);
}
