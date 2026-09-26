//! target: crates/core/src/growth.rs
//! mode: append
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: re-review 3 sweep -- the incremental path delegates a full walk to a helper one function away (blind spot: the forbidden tokens are looked for only in `apply_incremental`'s own body; the callee is never followed)
/// Sweep: keeps the required names, does the forbidden thing next door.
pub mod sweep_incremental {
    pub fn apply_incremental(root: &std::path::Path) {
        let _targeted = crate::walk::attribute_one_worktree;
        let _resize = crate::walk::resize_artifact;
        sweep_rebuild_everything(root);
    }

    fn sweep_rebuild_everything(dir: &std::path::Path) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            if e.path().is_dir() {
                sweep_rebuild_everything(&e.path());
            }
        }
    }
}
