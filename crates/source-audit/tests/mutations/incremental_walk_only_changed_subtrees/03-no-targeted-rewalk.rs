//! target: crates/core/src/growth.rs
//! expect: retired
//! retired: 2026-09-22 -- only defines a function (in a stand-in module) that nothing calls; which subtrees the real incremental path re-walks is behaviour, asserted against a full walk by fsevents_incremental.rs.
//! why: the targeted re-walk removed altogether -- changed directories are recorded and never re-measured
pub mod sweep_incremental_noop {
    pub fn apply_incremental(
        _root: &std::path::Path,
        changed: &[std::path::PathBuf],
        _observed_at: u64,
    ) -> anyhow::Result<usize> {
        Ok(changed.len())
    }
}
