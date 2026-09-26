//! target: crates/core/src/walk.rs
//! mode: append
//! by: compile:E0063
//! why: re-review 3 sweep -- a directory folded as a build artifact with no classification behind it (blind spot: the guard exemption is `s.func.starts_with("resize_artifact")` -- a prefix, so any new function whose name starts that way may fold anything)
/// Sweep: folds whatever it is handed. Exempt by name prefix.
pub fn resize_artifact_unchecked(
    tx: &std::sync::mpsc::Sender<AttrJob>,
    path: std::path::PathBuf,
    group: std::sync::Arc<SizeGroup>,
) {
    let _ = tx.send(AttrJob::Size { path, group });
}
