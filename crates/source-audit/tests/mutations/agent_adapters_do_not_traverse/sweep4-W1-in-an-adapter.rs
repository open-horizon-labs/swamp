//! target: crates/core/src/agents/gemini_cli.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (W group, reviewer_counterexamples_stack4_sweep.rs), W1
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: W1 (owner X1, confirm): `let f = std::fs::read_dir; f(p)` in an adapter
//! blind-spot (old model): value references to std/dependency items are dropped (only local targets become edges); capability predicates read `calls` only
/// Sweep 4 W1.
pub fn sweep4_w1_walk(home: &Path) -> usize {
    let list = std::fs::read_dir;
    let mut n = 0;
    if let Ok(rd) = list(home) {
        for e in rd.flatten() {
            n += 1;
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                n += sweep4_w1_walk(&e.path());
            }
        }
    }
    n
}
