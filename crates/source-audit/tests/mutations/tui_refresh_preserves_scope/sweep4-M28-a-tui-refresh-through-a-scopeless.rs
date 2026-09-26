//! target: crates/core/src/report.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M28
//! by: audit:gate_paths_only_inside_gates
//! why: a TUI refresh through a scopeless report entry whose root is `impl AsRef<Path>`
//! blind-spot (old model): a bare root is a parameter whose type text *ends with* `Path`/`PathBuf`; `impl AsRef<Path>` ends with `>`
/// Sweep 4: a scopeless entry point, generically typed.
pub fn sweep4_report_quick(root: impl AsRef<Path>) -> Vec<PathBuf> {
    vec![root.as_ref().to_path_buf()]
}
//! file: crates/tui/src/app.rs
//! mode: append
impl App {
    /// Sweep 4: refreshes through it, dropping every exclusion.
    pub fn sweep4_refresh(&mut self, root: &std::path::Path) {
        let _roots = swamp_core::report::sweep4_report_quick(root);
    }
}
