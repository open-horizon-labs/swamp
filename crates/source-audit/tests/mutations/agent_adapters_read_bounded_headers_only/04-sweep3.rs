//! target: crates/core/src/agents/claude_code.rs
//! mode: append
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods, compile:clippy::disallowed_types
//! why: re-review 3 sweep -- a whole transcript read with the free function `std::io::read_to_string` (blind spot: `UNBOUNDED_READS` matches `fs :: read_to_string (` and the *method* `. read_to_string (`; the `std::io` free function is neither)
/// Sweep: the whole file, past the header cap.
pub fn sweep_slurp(p: &std::path::Path) -> std::io::Result<String> {
    let f = std::fs::File::open(p)?;
    std::io::read_to_string(f)
}
