//! target: crates/core/src/agents/gemini_cli.rs
//! by: compile:private-fields-literal
//! why: the original forbidden shape must still be rejected
pub fn sweep_plain_literal(path: std::path::PathBuf) -> super::CandidateAgentUnit {
    super::CandidateAgentUnit {
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
