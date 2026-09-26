//! target: crates/core/src/report.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M30
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: JSON persisted as `serde_json::to_value(..)` then `Value::to_string()`
//! blind-spot (old model): `to_value` is excluded as "a value, not bytes" and `.to_string()` counts only on a `json!` receiver; `Value`'s `Display` is the serializer
/// Sweep 4: JSON bytes through `Display`.
pub fn sweep4_persist_rows<T: serde::Serialize>(p: &Path, rows: &T) -> anyhow::Result<()> {
    let v = serde_json::to_value(rows)?;
    std::fs::write(p, v.to_string())?;
    Ok(())
}
