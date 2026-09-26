//! target: crates/core/src/agents/cursor.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M36
//! by: compile:E0616
//! why: the builder's protected-by-default lifted through a `&mut` binding
//! blind-spot (old model): an assignment is checked by its left-hand side's last `.field`; `let f = &mut u.protected; *f = false;` assigns to `*f` (the store rule follows this binding; this rule does not)
/// Sweep 4: unprotected through a reference.
pub fn sweep4_unprotect(u: &mut super::CandidateAgentUnit) {
    let flag = &mut u.protected;
    *flag = false;
    let why = &mut u.protect_reason;
    *why = None;
}
