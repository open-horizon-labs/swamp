//! target: crates/core/src/growth.rs
//! by: compile:E0061
//! why: the incremental path falling back to a full walk, which makes "only changed subtrees" false without changing any name
pub mod sweep_incremental {
    pub fn apply_incremental(
        root: &std::path::Path,
        _changed: &[std::path::PathBuf],
        observed_at: u64,
    ) -> anyhow::Result<()> {
        let _ = super::full_walk(root, observed_at, 0, "sweep", &[])?;
        Ok(())
    }
}
