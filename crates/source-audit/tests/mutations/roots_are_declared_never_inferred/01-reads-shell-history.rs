//! target: crates/core/src/scope.rs
//! mode: append
//! by: audit:no_root_inference_sources
//! why: the tempting shortcut for first run -- read the shell history for the directories the user has cd'd into and offer them as source roots
/// Sweep: a candidate list guessed from the user's shell history.
pub fn sweep_guess_roots_from_history(home: &std::path::Path) -> std::path::PathBuf {
    home.join(".zsh_history")
}
