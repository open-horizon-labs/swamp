//! target: crates/core/src/scope.rs
//! mode: append
//! by: audit:no_root_inference_sources
//! why: the history file name assembled with format! from two literals, neither of which names it
/// Sweep: history path built with format!.
pub fn sweep_history_by_format(home: &std::path::Path) -> std::path::PathBuf {
    home.join(format!("{}{}", ".zsh", "_history"))
}
