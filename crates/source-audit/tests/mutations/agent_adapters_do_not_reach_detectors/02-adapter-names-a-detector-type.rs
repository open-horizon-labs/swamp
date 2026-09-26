//! target: crates/core/src/agents/aider.rs
//! by: audit:adapters_do_not_reach_gates
//! why: the same reach written through the detector trait, one adapter away
pub fn sweep_detector_hint(d: &dyn crate::locations::Detector) -> Option<String> {
    d.recovery_hint().map(str::to_string)
}
