//! target: crates/tui/src/app.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (W group, reviewer_counterexamples_stack4_sweep.rs), W10
//! by: audit:gate_paths_only_inside_gates, compile:E0603
//! why: W10: a `static` table of probes called from a key handler
//! blind-spot (old model): a static's initializer is an item, not a function; `SWEEP4_PROBES[0](p)` has an index-expression callee
static SWEEP4_PROBES: [fn(&std::path::Path) -> swamp_core::occupancy::OccupancyState; 1] =
    [swamp_core::occupancy::probe_path];

impl App {
    /// Sweep 4 W10: lsof per keystroke, through a static table.
    pub fn handle_key_sweep4_w10(&mut self, p: &std::path::Path) {
        let _state = SWEEP4_PROBES[0](p);
    }
}
