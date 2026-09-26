//! target: crates/core/src/agents/mod.rs
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: the same store as .jsonl rather than .json
pub fn sweep_jsonl_sidecar(swamp_dir: &std::path::Path, bytes: &[u8]) {
    let _ = std::fs::write(swamp_dir.join("session_index.jsonl"), bytes);
}
