//! target: crates/core/src/agents/cursor.rs
//! mode: append
//! by: compile:E0616
//! why: re-review 3 sweep -- a unit built correctly by the builder and then silently unprotected field by field (blind spot: the rule inspects *struct-literal sites* and `unprotect_with_reason` arguments; a later `unit.protected = false` is neither)
/// Sweep: the builder's protected-by-default, lifted after the fact.
pub fn sweep_unprotect(u: &mut super::CandidateAgentUnit) {
    u.protected = false;
    u.protect_reason = None;
}
