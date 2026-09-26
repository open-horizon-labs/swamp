//! target: crates/core/src/agents/cline.rs
//! by: compile:E0277
//! why: lifting the protected-by-default flag outside the builder's explicit reason path
pub fn sweep_unprotect(b: super::AgentUnitBuilder) -> super::AgentUnitBuilder {
    b.unprotect_with_reason("")
}
