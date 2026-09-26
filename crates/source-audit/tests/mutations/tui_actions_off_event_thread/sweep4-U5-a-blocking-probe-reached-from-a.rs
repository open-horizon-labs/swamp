//! target: crates/tui/src/app.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (U group, reviewer_counterexamples_stack4_sweep.rs), U5
//! by: audit:gate_paths_only_inside_gates, compile:E0603
//! why: U5: a blocking probe reached from a key handler through a generic trait bound
//! blind-spot (old model): `receiver_type` of a parameter typed `&P` is `P`; `P` is not a local type, so it is treated as a std/dependency type and the method call gets no target at all
pub trait Sweep4Probe {
    fn sweep4_probe(&self, p: &std::path::Path) -> swamp_core::occupancy::OccupancyState;
}

pub struct Sweep4Lsof;

impl Sweep4Probe for Sweep4Lsof {
    fn sweep4_probe(&self, p: &std::path::Path) -> swamp_core::occupancy::OccupancyState {
        swamp_core::occupancy::probe_path(p)
    }
}

fn sweep4_check<P: Sweep4Probe>(probe: &P, p: &std::path::Path) -> bool {
    matches!(
        probe.sweep4_probe(p),
        swamp_core::occupancy::OccupancyState::Free
    )
}

impl App {
    /// Sweep 4 U5: lsof per keystroke, through a generic.
    pub fn handle_key_sweep4_u5(&mut self, p: &std::path::Path) -> bool {
        sweep4_check(&Sweep4Lsof, p)
    }
}
