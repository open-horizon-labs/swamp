//! target: crates/core/src/agents/registry.rs
//! expect: retired
//! retired: 2026-09-22 -- only defines a function (in a stand-in module) that nothing calls; an unreferenced second list registers nothing. A duplicate in the real registry fails `agents::registry::tests::every_adapter_is_registered_exactly_once`.
//! why: an adapter registered twice, so one tool's units are identified and counted twice
pub fn sweep_extra_registration() -> Vec<Box<dyn crate::agents::AgentAdapter>> {
    vec![Box::new(codex::Adapter)]
}
