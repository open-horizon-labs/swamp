//! target: crates/core/src/actions.rs
//! by: compile:E0277, compile:E0308
//! why: protection state turned back into "nothing is protected"
pub fn sweep_protect_discard(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    crate::agents::load_protect(dir).unwrap_or_default()
}
