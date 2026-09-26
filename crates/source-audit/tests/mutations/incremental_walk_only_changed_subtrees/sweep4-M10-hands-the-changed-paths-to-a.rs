//! target: crates/core/src/growth.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M10
//! by: audit:gate_paths_only_inside_gates, compile:E0061
//! why: `apply_incremental` hands the changed paths to a helper that ignores them and walks the whole root
//! blind-spot (old model): "handed the changed paths" is an argument-text test (`args` mention a name tainted from the `&[PathBuf]` parameter); nothing follows the argument into the listing
/// Sweep 4: every required call made, the whole tree walked anyway.
pub mod sweep4_incremental {
    use std::path::{Path, PathBuf};

    pub fn apply_incremental(root: &Path, changed_dirs: &[PathBuf], observed_at: u64) -> u64 {
        let _one = crate::walk::attribute_one_worktree(
            root,
            &[],
            observed_at,
            0,
            Default::default(),
        );
        let _art = crate::walk::resize_artifact(
            root,
            crate::report::ArtifactKind::BuildOutput,
            observed_at,
        );
        sweep4_rewalk(root, changed_dirs)
    }

    fn sweep4_rewalk(dir: &Path, _changed: &[PathBuf]) -> u64 {
        let mut n = 0;
        let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
        for e in rd.flatten() {
            n += 1;
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                n += sweep4_rewalk(&e.path(), _changed);
            }
        }
        n
    }
}
