//! target: crates/core/src/agents/codex_desktop.rs
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: a recursive walker in an adapter is the unbounded scan the epic forbids
pub fn sweep_walk(home: &std::path::Path) -> usize {
    let mut stack = vec![home.to_path_buf()];
    let mut n = 0;
    while let Some(d) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                n += 1;
                stack.push(e.path());
            }
        }
    }
    n
}
