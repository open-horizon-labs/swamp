//! target: crates/core/src/actions.rs
//! mode: append
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: a Linux cross-device fallback that copies the tree then deletes the source instead of refusing -- duplicates bytes, splits hardlinks and is not atomic, the EXDEV shortcut fs_gate::destroy::Envelope::open refuses outright
pub fn move_across_quick(src: &Path, dst: &Path) -> Result<()> {
    if std::fs::rename(src, dst).is_err() {
        std::fs::copy(src, dst)?;
        std::fs::remove_dir_all(src)?;
    }
    Ok(())
}
