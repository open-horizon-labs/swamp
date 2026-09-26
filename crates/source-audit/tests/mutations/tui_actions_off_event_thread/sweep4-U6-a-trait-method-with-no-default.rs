//! target: crates/tui/src/app.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (U group, reviewer_counterexamples_stack4_sweep.rs), U6
//! by: audit:gate_paths_only_inside_gates, compile:E0603
//! why: U6: a trait method with no default body, called in UFCS form from a key handler
//! blind-spot (old model): `Trait::method(..)` resolves through `methods_by_key["Trait::method"]`, which holds only default bodies; implementors are consulted for method-call syntax only
pub trait Sweep4Probe6 {
    fn sweep4_probe6(&self, p: &std::path::Path) -> swamp_core::occupancy::OccupancyState;
}

pub struct Sweep4Lsof6;

impl Sweep4Probe6 for Sweep4Lsof6 {
    fn sweep4_probe6(&self, p: &std::path::Path) -> swamp_core::occupancy::OccupancyState {
        swamp_core::occupancy::probe_path(p)
    }
}

impl App {
    /// Sweep 4 U6.
    pub fn handle_key_sweep4_u6(&mut self, p: &std::path::Path) {
        let _ = Sweep4Probe6::sweep4_probe6(&Sweep4Lsof6, p);
    }
}
