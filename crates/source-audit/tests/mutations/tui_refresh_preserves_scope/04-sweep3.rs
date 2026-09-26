//! target: crates/core/src/report.rs
//! mode: append
//! by: audit:gate_paths_only_inside_gates
//! why: re-review 3 sweep -- a TUI refresh through a new scopeless core entry point (blind spot: `SCOPELESS_REPORT_ENTRIES` is a hand-written list of eight names; a ninth scopeless entry point is unaudited the day it lands)
/// Sweep: a new scopeless report entry point.
pub fn report_quick(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    vec![root.to_path_buf()]
}
//! file: crates/tui/src/app.rs
//! mode: append
impl App {
    /// Sweep: refreshes through it, dropping every exclusion.
    pub fn sweep_refresh(&mut self, root: &std::path::Path) {
        let _roots = swamp_core::report::report_quick(root);
    }
}
