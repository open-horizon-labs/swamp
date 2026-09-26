//! target: crates/core/src/walk.rs
//! by: compile:E0063
//! why: the same fold under a guard that is not `classify_at` -- a name-shaped test standing in for classification
pub fn sweep_fold_by_name(
    path: std::path::PathBuf,
    group: std::sync::Arc<SizeGroup>,
) -> Option<AttrJob> {
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if name.ends_with("_cache") {
            return Some(AttrJob::Size { path, group });
        }
    }
    None
}
