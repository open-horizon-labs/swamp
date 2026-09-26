//! target: crates/core/src/walk.rs
//! by: compile:E0063
//! why: a directory folded as an artifact without ever being classified as one -- source bytes would be reported as build output
pub fn sweep_fold_anything(path: std::path::PathBuf, group: std::sync::Arc<SizeGroup>) -> AttrJob {
    AttrJob::Size { path, group }
}
