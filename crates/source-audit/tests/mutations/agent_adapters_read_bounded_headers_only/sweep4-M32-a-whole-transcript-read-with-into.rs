//! target: crates/core/src/agents/claude_code.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M32
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods, compile:clippy::disallowed_types
//! why: a whole transcript read with `std::io::copy` into a Vec
//! blind-spot (old model): `unbounded_read` knows `fs::read*`, `read_to_*`, `serde_json::from_reader`, `BufReader::new`; `io::copy` (and `Read::bytes`, `Read::take(u64::MAX)`) read the same bytes
/// Sweep 4: the whole file, past the header cap.
pub fn sweep4_slurp(p: &Path) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    std::io::copy(&mut std::fs::File::open(p)?, &mut out)?;
    Ok(out)
}
