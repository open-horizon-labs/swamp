//! target: crates/core/src/growth.rs
//! by: compile:E0061
//! ported: 2026-09-22 -- `walk::attribute_parallel` is gone; the whole-tree pass is `walk::discover_and_attribute`, which takes a `bus::Stage` this helper does not have
//! why: the same regression written through the whole-tree attribution pass instead of full_walk
mod sweep_incremental_parallel {
    pub fn apply_incremental(
        root: &std::path::Path,
        _changed: &[std::path::PathBuf],
        observed_at: u64,
    ) -> anyhow::Result<()> {
        let _ = crate::walk::discover_and_attribute(root, observed_at, 0, &[]);
        Ok(())
    }
}
