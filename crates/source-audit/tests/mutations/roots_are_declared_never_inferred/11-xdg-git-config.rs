//! target: crates/core/src/git.rs
//! mode: append
//! by: audit:no_root_inference_sources
//! why: the XDG git configuration file lists directories in its includeIf sections just like ~/.gitconfig
/// Sweep: the XDG git config path.
pub fn sweep_xdg_git_config() -> &'static str {
    ".config/git/config"
}
