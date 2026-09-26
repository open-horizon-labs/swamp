//! target: crates/core/src/agents/windsurf.rs
//! mode: append
//! by: audit:adapters_do_not_reach_gates
//! why: re-review 3 sweep -- an adapter naming a detector id constant at item level (blind spot: the scan is over `sig + body` of *functions*; a `const`, a `type` alias or a struct field naming `locations::` sits inside no function)
/// Sweep: detector identity, at item level, where the audit never looks.
pub const SWEEP_DETECTOR_ID: &str = crate::locations::windsurf::WINDSURF_DETECTOR_ID;
