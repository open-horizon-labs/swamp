//! The TUI's agents rows say when a project link was inferred
//! (`model::agent_rows`), so the one-line label a human reads before
//! marking a session never presents a folder-name inference as a
//! declared fact.

use std::path::PathBuf;
use swamp_core::agents::{
    AgentActionCapability, AgentCategory, AgentUnit, LinkSource, ProjectLinkState,
};

fn unit(source: LinkSource) -> AgentUnit {
    AgentUnit {
        tool_id: "claude-code".into(),
        tool_name: "Claude Code".into(),
        tool_home: PathBuf::from("/x/.claude"),
        category: AgentCategory::Sessions,
        id: "s1".into(),
        relative_path: "projects/-x-repo/s1.jsonl".into(),
        path: PathBuf::from("/x/.claude/projects/-x-repo/s1.jsonl"),
        members: Vec::new(),
        bytes: 1024,
        hardlinked: true,
        complete: true,
        growth_bytes: None,
        regrowth_count: 0,
        observed_at: 1_000,
        mtime_max: 900,
        protected: false,
        protect_reason: None,
        project_link: ProjectLinkState::Linked {
            project_id: "p".into(),
            project_name: "repo".into(),
            project_path: PathBuf::from("/x/repo"),
            source,
            fallback_reason: None,
            worktree_kind: "main".into(),
        },
        action: AgentActionCapability::SessionRemoval,
        note: None,
        evidence: Vec::new(),
    }
}

#[test]
fn an_inferred_link_row_says_inferred_and_a_declared_one_does_not() {
    let inferred = swamp_tui::model::agent_rows(&[unit(LinkSource::Inferred)]);
    let declared = swamp_tui::model::agent_rows(&[unit(LinkSource::Declared)]);
    let label = |rows: &[swamp_tui::model::Row]| {
        rows.iter()
            .map(|r| r.label.clone())
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(
        label(&inferred).contains("project: repo (inferred)"),
        "{}",
        label(&inferred)
    );
    assert!(
        label(&declared).contains("project: repo"),
        "{}",
        label(&declared)
    );
    assert!(
        !label(&declared).contains("(inferred)"),
        "{}",
        label(&declared)
    );
}
