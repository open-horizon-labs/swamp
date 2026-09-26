//! target: crates/core/src/growth.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M4
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: a full walk before the replay, under a condition that names `force_full` and is true anyway
//! blind-spot (old model): the guard is `conditions.any(|k| contains_token(k, "force_full"))`: the token in the condition, not the condition being `force_full`
/// Sweep 4: the guard is decoration.
pub mod sweep4_stage {
    pub struct Sweep4Stream;
    impl Sweep4Stream {
        pub fn replay(&self, _since: u64) -> Vec<String> {
            Vec::new()
        }
    }

    fn sweep4_scan_all(dir: &std::path::Path, n: &mut usize) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            *n += 1;
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                sweep4_scan_all(&e.path(), n);
            }
        }
    }

    pub fn stage_tracked_with_source(root: &std::path::Path, force_full: bool) -> Vec<String> {
        let mut n = 0usize;
        if force_full || root.exists() {
            sweep4_scan_all(root, &mut n);
        }
        Sweep4Stream.replay(0)
    }
}
