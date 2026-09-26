//! target: crates/core/src/agents/opencode.rs
//! by: audit:adapters_do_not_reach_gates
//! why: alias/rename variant -- the detector registry imported under another name
use crate::locations::Registry as SweepDetectors;

pub fn sweep_aliased_detectors() -> SweepDetectors {
    crate::locations::Registry::with_builtins()
}
