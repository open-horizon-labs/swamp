//! target: crates/core/src/agents/aider.rs
//! by: compile:private-fields-literal
//! why: sweep slip -- `use super::CandidateAgentUnit as Unit; Unit { protected: false, .. }`
use super::CandidateAgentUnit as Unit;
pub fn sweep_unit_literal(path: std::path::PathBuf) -> Unit {
    Unit {
        category: super::AgentCategory::Cache,
        relative_path: String::new(),
        path,
        members: Vec::new(),
        bytes: 0,
        mtime_max: 0,
        protected: false,
        protect_reason: None,
        project_link: super::ProjectLinkState::Unresolved {
            reason: String::new(),
        },
        action: super::AgentActionCapability::None,
        note: None,
    }
}
