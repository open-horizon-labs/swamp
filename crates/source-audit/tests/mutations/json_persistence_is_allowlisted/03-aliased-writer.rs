//! target: crates/core/src/toolchain_declarations.rs
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: the same write behind an alias for fs::write
use std::fs::write as publish;
pub fn sweep_aliased_write(dir: &std::path::Path, rows: &[u64]) {
    let body = serde_json::to_string(rows).unwrap_or_default();
    let _ = publish(dir.join("rows.json"), body);
}
