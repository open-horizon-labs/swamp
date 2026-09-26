//! agent-units-built-through-builder: nor through a `&mut` binding.
use swamp_core::agents::CandidateAgentUnit;

fn unprotect(u: &mut CandidateAgentUnit) {
    let f = &mut u.protected;
    *f = false;
}

fn main() {}
