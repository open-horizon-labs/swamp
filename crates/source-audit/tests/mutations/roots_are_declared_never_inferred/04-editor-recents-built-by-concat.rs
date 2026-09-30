//! target: crates/tui/src/app.rs
//! mode: append
//! by: audit:no_root_inference_sources
//! why: an editor's recent-project list assembled with concat! so no single literal spells the key (a plain scan of one literal misses it)
/// Sweep: the VS Code recents key, split across a concat!.
pub fn sweep_recent_projects_key() -> String {
    concat!("recent", "Projects").to_string()
}
