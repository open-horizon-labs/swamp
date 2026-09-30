//! target: crates/core/src/scope.rs
//! mode: append
//! by: audit:no_root_inference_sources
//! why: the history file name joined from a slice at run time
/// Sweep: history path joined from parts.
pub fn sweep_history_by_join(home: &std::path::Path) -> std::path::PathBuf {
    home.join([".bash", "_history"].concat())
}
