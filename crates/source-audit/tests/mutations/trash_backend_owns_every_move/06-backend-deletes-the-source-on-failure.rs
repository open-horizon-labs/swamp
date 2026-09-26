//! target: crates/core/src/actions.rs
//! mode: append
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: a permanent-deletion fallback -- when the rename fails, the backend removes the source instead of refusing, so a failed Trash move becomes a silent permanent delete
pub fn move_or_drop(src: &Path, dst: &Path) -> Result<()> {
    if std::fs::rename(src, dst).is_err() {
        std::fs::remove_file(src)?;
    }
    Ok(())
}
