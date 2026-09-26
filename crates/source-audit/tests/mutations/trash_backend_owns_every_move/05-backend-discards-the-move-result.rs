//! target: crates/core/src/actions.rs
//! mode: append
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: a discarded-result variant: the move's error is dropped and the item is reported as trashed while it is still at its source
pub fn quiet_move(src: &Path, trash: &Path) -> PathBuf {
    let dest = trash.join("quiet");
    let _ = std::fs::rename(src, &dest);
    dest
}
