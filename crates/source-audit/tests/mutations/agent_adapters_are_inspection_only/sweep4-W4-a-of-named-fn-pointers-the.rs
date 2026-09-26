//! target: crates/core/src/agents/codex.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (W group, reviewer_counterexamples_stack4_sweep.rs), W4
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: W4: a `HashMap` of named fn pointers; the adapter writes through the table
//! blind-spot (old model): the map holds values; `table["reset"](p, ..)` is an index expression called, not a path
/// Sweep 4 W4.
pub fn sweep4_w4_reset(p: &Path) -> std::io::Result<()> {
    let mut table: std::collections::HashMap<&str, fn(PathBuf, Vec<u8>) -> std::io::Result<()>> =
        std::collections::HashMap::new();
    table.insert("reset", std::fs::write::<PathBuf, Vec<u8>>);
    table["reset"](p.to_path_buf(), Vec::new())
}
