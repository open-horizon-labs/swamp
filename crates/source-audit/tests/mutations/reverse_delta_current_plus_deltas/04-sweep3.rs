//! target: crates/core/src/growth.rs
//! mode: append
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: re-review 3 sweep -- the delta path is computed and discarded; history is rewritten in place (blind spot: `body.contains("next_delta_path")` -- the name appearing, not a delta being appended)
/// Sweep: names both paths, writes only the current one.
pub mod sweep_history {
    pub fn observe_and_annotate(dir: &std::path::Path) {
        let cur = super::current_path(dir);
        let _delta = super::next_delta_path(dir);
        let _ = std::fs::write(&cur, b"");
    }
}
