//! Waits for every per-project fact (signals, ecosystems, GitHub, Docker)
//! and emits `RowsAssembled`: the project rows with those facts folded in,
//! the combined unowned list, totals, and notes in the order the report
//! always printed them.

use crate::bus::{Consumer, Ctx, Draft, Event, EventKind, WorktreeSignals};
use crate::github::GithubFacts;
use crate::report::Signal;
use crate::report::{ArtifactRow, DirRollup, FileRow, ProjectRow, Reconciliation, UnownedRow};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

type GithubFactsBundle = (
    Arc<HashMap<String, GithubFacts>>,
    Vec<String>,
    Option<crate::report::GithubEnrichmentSummary>,
);
type DockerBundle = (
    Arc<HashMap<String, Vec<ArtifactRow>>>,
    Arc<Vec<UnownedRow>>,
    u64,
    u64,
    Vec<String>,
);

#[derive(Default)]
struct Pending {
    projects: Option<Arc<Vec<ProjectRow>>>,
    unowned: Option<Arc<Vec<UnownedRow>>>,
    dirs: Option<Arc<Vec<DirRollup>>>,
    files: Option<Arc<Vec<FileRow>>>,
    reconciliation: Option<Reconciliation>,
    walk_notes: Vec<String>,
    protected_worktree_ids: Vec<String>,
    signals: Option<Arc<HashMap<String, WorktreeSignals>>>,
    ecosystems: Option<Arc<HashMap<String, Vec<String>>>>,
    github: Option<GithubFactsBundle>,
    docker: Option<DockerBundle>,
    docker_facts: Option<Arc<crate::docker::DockerFacts>>,
    emitted: bool,
}

#[derive(Default)]
pub struct AssemblyGate {
    pending: Mutex<Pending>,
}

#[async_trait::async_trait(?Send)]
impl Consumer for AssemblyGate {
    fn name(&self) -> &str {
        "gate"
    }
    fn subscribes_to(&self) -> &[EventKind] {
        &[
            EventKind::ProjectsGrouped,
            EventKind::SignalsComputed,
            EventKind::EcosystemsDetected,
            EventKind::GithubEnriched,
            EventKind::DockerJoined,
        ]
    }
    async fn on_event(
        &self,
        event: &Event,
        ctx: &Ctx<'_>,
        _stage: &crate::bus::Stage,
    ) -> Result<Vec<Event>> {
        let mut p = self.pending.lock().unwrap();
        match event {
            Event::ProjectsGrouped {
                projects,
                unowned,
                dirs,
                files,
                reconciliation,
                notes,
                unconfirmed_worktree_ids,
                ..
            } => {
                p.projects = Some(projects.clone());
                p.unowned = Some(unowned.clone());
                p.dirs = Some(dirs.clone());
                p.files = Some(files.clone());
                p.reconciliation = Some(reconciliation.clone());
                p.walk_notes = notes.clone();
                p.protected_worktree_ids = (**unconfirmed_worktree_ids).clone();
            }
            Event::SignalsComputed { by_worktree } => p.signals = Some(by_worktree.clone()),
            Event::EcosystemsDetected { by_project } => p.ecosystems = Some(by_project.clone()),
            Event::GithubEnriched {
                facts_by_worktree,
                summary,
                notes,
            } => p.github = Some((facts_by_worktree.clone(), notes.clone(), summary.clone())),
            Event::DockerJoined {
                rows_by_worktree,
                unowned,
                attributed_bytes,
                unowned_bytes,
                notes,
                facts,
            } => {
                p.docker = Some((
                    rows_by_worktree.clone(),
                    unowned.clone(),
                    *attributed_bytes,
                    *unowned_bytes,
                    notes.clone(),
                ));
                p.docker_facts = facts.clone();
            }
            _ => {}
        }
        let ready = p.projects.is_some()
            && p.signals.is_some()
            && p.ecosystems.is_some()
            && p.github.is_some()
            && p.docker.is_some();
        if !ready || p.emitted {
            return Ok(vec![]);
        }
        p.emitted = true;

        let mut projects: Vec<ProjectRow> = (**p.projects.as_ref().unwrap()).clone();
        let signals = p.signals.clone().unwrap();
        let ecosystems = p.ecosystems.clone().unwrap();
        let (gh_facts, gh_notes, gh_summary) = p.github.clone().unwrap();
        let (docker_rows, docker_unowned, docker_attributed, docker_unowned_bytes, docker_notes) =
            p.docker.clone().unwrap();

        for project in &mut projects {
            if let Some(tags) = ecosystems.get(&project.project_id) {
                project.ecosystems = tags.clone();
            }
            for worktree in &mut project.worktrees {
                let raw = signals.get(&worktree.worktree_id);
                if let Some(sig) = raw {
                    worktree.signals = sig.rows.clone();
                    worktree.branch = sig.branch.clone();
                    worktree.idle_secs = sig.raw.idle_for_secs;
                }
                if let Some(facts) = gh_facts.get(&worktree.worktree_id) {
                    let mc = crate::github::merge_complete(
                        raw.and_then(|r| r.raw.dirty),
                        raw.and_then(|r| r.raw.unpushed),
                        &facts.merged,
                    );
                    worktree.signals.push(Signal {
                        name: "merge_complete".to_string(),
                        value: format!(
                            "{} ({})",
                            match mc.verdict {
                                crate::github::TriState::Yes => "yes",
                                crate::github::TriState::No => "no",
                                crate::github::TriState::Unknown => "unknown",
                            },
                            mc.terms.join(", ")
                        ),
                    });
                    worktree.signals.push(Signal {
                        name: "pull_request".to_string(),
                        value: crate::report::render_pr_status(&facts.pull_request),
                    });
                    worktree.merge_complete = Some(mc);
                    worktree.github = Some(facts.clone());
                }
                if let Some(rows) = docker_rows.get(&worktree.worktree_id) {
                    worktree.artifacts.extend(rows.iter().cloned());
                }
            }
        }
        let mut unowned: Vec<UnownedRow> = (**p.unowned.as_ref().unwrap()).clone();
        unowned.extend(docker_unowned.iter().cloned());
        let mut notes = p.walk_notes.clone();
        notes.extend(gh_notes);
        notes.extend(docker_notes);
        let mut reconciliation = p.reconciliation.clone().unwrap();
        reconciliation.docker_attributed = docker_attributed;
        reconciliation.docker_unowned = docker_unowned_bytes;
        reconciliation.du_total = if ctx.verify_du {
            crate::attribution::du_total(&ctx.root)
        } else {
            None
        };
        let dirs: Vec<DirRollup> = (**p.dirs.as_ref().unwrap()).clone();
        let files: Vec<FileRow> = (**p.files.as_ref().unwrap()).clone();
        Ok(vec![Event::RowsAssembled(Arc::new(Draft {
            projects,
            unowned,
            dirs,
            files,
            dirs_by_worktree: None,
            files_by_worktree: None,
            notes,
            reconciliation,
            github_enrichment: gh_summary,
            schedule_line: None,
            nested_artifacts: Arc::new(Vec::new()),
            docker_facts: p.docker_facts.clone(),
            protected_worktree_ids: p.protected_worktree_ids.clone(),
        }))])
    }
}
