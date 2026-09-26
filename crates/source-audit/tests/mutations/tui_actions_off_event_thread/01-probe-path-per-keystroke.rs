//! target: crates/tui/src/app.rs
//! by: audit:no_unreferenced_public_items
//! why: sweep slip -- `probe_path` (an lsof spawn) per keystroke was not in the blocking sink list; used to fail to compile (E0603), now caught as an unreferenced pub fn instead (stack/27 shifted the rejecting mechanism, not the outcome)
impl App {
    pub fn sweep_on_key_blocking(&mut self, path: &std::path::Path) {
        let _ = swamp_core::occupancy::probe_path(path);
    }
}
