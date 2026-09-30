//! target: crates/core/src/scope.rs
//! mode: append
//! by: audit:no_root_inference_sources
//! why: VS Code keeps its recently opened folders in globalStorage/storage.json; the file path names no listed key
/// Sweep: VS Code recents file.
pub fn sweep_vscode_recents(home: &std::path::Path) -> std::path::PathBuf {
    home.join("Library/Application Support/Code/User/globalStorage/storage.json")
}
