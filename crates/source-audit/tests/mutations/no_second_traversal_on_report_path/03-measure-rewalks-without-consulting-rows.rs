//! target: crates/core/src/folded_measurement.rs
//! mode: replace
//! expect: retired
//! retired: 2026-09-22 -- a whole-file replacement of folded_measurement.rs breaks unrelated code; reuse-before-walk is asserted by incremental_external_and_agent_measurement.rs and the cost tests.
//! why: the 2026-09-22 finding itself -- `measure` sits on the allow-list and re-walks every pass while the guardrail claims reuse
use crate::report::ArtifactKind;
use std::path::{Path, PathBuf};

pub struct FoldedUnit {
    pub bytes: u64,
    pub hardlinked: bool,
    pub mtime_max: u64,
}

pub fn measure(
    _store: Option<&Path>,
    path: &Path,
    exclusions: &[PathBuf],
    observed_at: u64,
) -> FoldedUnit {
    let row =
        crate::walk::resize_artifact_excluding(path, ArtifactKind::Unknown, observed_at, exclusions);
    crate::work_counters::record_cache_miss();
    FoldedUnit {
        bytes: row.bytes,
        hardlinked: row.hardlinked,
        mtime_max: row.mtime_max,
    }
}

pub const SHALLOW_LIST_CAP_PLACEHOLDER: usize = 0;
