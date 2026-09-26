//! agent-units-built-through-builder: lifting protected-by-default takes
//! an `evidence::Reason`, so a blank string cannot be the reason.
use swamp_core::agents::AgentUnitBuilder;

fn lift(b: AgentUnitBuilder) -> AgentUnitBuilder {
    b.unprotect_with_reason("")
}

fn main() {}
