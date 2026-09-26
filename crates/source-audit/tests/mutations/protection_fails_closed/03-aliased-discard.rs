//! target: crates/core/src/actions.rs
//! by: compile:E0277, compile:E0308
//! why: the same discard behind an alias
use crate::agents::load_protect as read_keep_list;
pub fn sweep_protect_aliased(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    read_keep_list(dir).ok().unwrap_or_default()
}
