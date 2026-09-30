//! target: crates/core/src/git.rs
//! mode: append
//! by: audit:no_root_inference_sources
//! why: `[includeIf "gitdir:~/work/"]` and `safe.directory` entries in ~/.gitconfig list directories the user keeps repositories in
/// Sweep: git configuration read for directories.
pub fn sweep_gitconfig_key() -> &'static str {
    "safe.directory"
}
