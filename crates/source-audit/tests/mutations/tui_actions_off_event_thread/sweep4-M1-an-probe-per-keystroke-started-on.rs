//! target: crates/tui/src/app.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M1
//! by: audit:gate_paths_only_inside_gates, compile:E0603
//! why: an `lsof` probe per keystroke, started on a thread and joined on the spot
//! blind-spot (old model): a call inside `thread::spawn(|| ..)` is exempt as off-thread; `.join()` blocks only when its receiver's name contains `handle`/`worker`, so `thread::spawn(..).join()` blocks the event loop and is exempt twice
impl App {
    /// Sweep 4: "off the event thread", then waited for on it.
    pub fn handle_key_sweep4_probe(&mut self, p: &std::path::Path) {
        let p = p.to_path_buf();
        let _state = std::thread::spawn(move || swamp_core::occupancy::probe_path(&p)).join();
    }
}
