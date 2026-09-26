//! target: crates/core/src/agents/claude_code.rs
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: sweep slip -- an adapter walking the tool home itself instead of asking the folded seam
pub fn sweep_count_sessions(home: &std::path::Path) -> usize {
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(home) {
        for _ in rd.flatten() {
            n += 1;
        }
    }
    n
}
