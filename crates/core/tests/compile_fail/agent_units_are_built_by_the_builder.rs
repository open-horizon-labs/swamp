//! agent-units-built-through-builder: a candidate agent unit is built only
//! by `AgentUnitBuilder`, which sets protected-by-default; its fields are
//! private, so protection cannot be lifted by assignment.
use swamp_core::agents::CandidateAgentUnit;

fn unprotect(u: &mut CandidateAgentUnit) {
    u.protected = false;
}

fn main() {}
