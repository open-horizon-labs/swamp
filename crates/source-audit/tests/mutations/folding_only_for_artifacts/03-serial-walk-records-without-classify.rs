//! target: crates/core/src/attribution.rs
//! by: compile:E0425
//! note: 2026-09-22 -- the serial walker and its `record_artifact` are test-only code (`#[cfg(test)]`); production folds only through `AttrJob::Size`, which needs a `Classified`
//! why: the serial walk's half of the same rule -- an artifact row recorded with no classification behind it
pub fn sweep_record_any_dir(path: &std::path::Path, shared: &mut Vec<String>) {
    record_artifact(path, shared);
}
