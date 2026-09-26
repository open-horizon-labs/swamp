//! target: crates/tui/src/app.rs
//! by: audit:gate_paths_only_inside_gates
//! why: a blocking report call renamed past a token match
use swamp_core::report::report_full_mode as observe_now;
impl App {
    pub fn sweep_on_key_report(&mut self, root: &std::path::Path) {
        let _ = observe_now(root, None, false, None, None, false, false, false, true);
    }
}
