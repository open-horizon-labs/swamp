//! target: crates/tui/src/app.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (W group, reviewer_counterexamples_stack4_sweep.rs), W11
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods, compile:clippy::disallowed_types
//! why: W11: `Command::new` bound to a local and run from a key handler
//! blind-spot (old model): `blocks()` recognises the builder call (`Command::new`/`spawn::command`), not `.output()`; with the builder bound to a local, nothing on the key path blocks
impl App {
    /// Sweep 4 W11: a subprocess per keystroke.
    pub fn handle_key_sweep4_w11(&mut self) {
        let mk = std::process::Command::new::<&str>;
        let _ = mk("lsof").arg("-t").output();
    }
}
