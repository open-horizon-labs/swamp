//! target: crates/core/src/agents/copilot_cli.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (W group, reviewer_counterexamples_stack4_sweep.rs), W7
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: W7: `std::fs::read_to_string` handed to an `impl Fn` parameter
//! blind-spot (old model): the reader is a value reference to std (dropped); the adapter only calls its parameter
fn sweep4_with_reader(p: &Path, read: impl Fn(PathBuf) -> std::io::Result<String>) -> usize {
    read(p.to_path_buf()).map(|s| s.len()).unwrap_or(0)
}

/// Sweep 4 W7.
pub fn sweep4_w7_slurp(p: &Path) -> usize {
    sweep4_with_reader(p, std::fs::read_to_string::<PathBuf>)
}
