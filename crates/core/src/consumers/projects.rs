//! `RootObserved` → `ProjectsGrouped`: checkouts and worktrees grouped
//! into projects by remote (or object-store id), artifact rows attached
//! to their worktrees, per-directory and per-file rows carried along.

use crate::bus::{Consumer, Ctx, Event, EventKind};
use crate::entities::id_for;
use crate::report::{ProjectRow, Reconciliation, WorktreeKind, WorktreeRow};
use anyhow::Result;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

pub struct ProjectsConsumer;

#[async_trait::async_trait(?Send)]
impl Consumer for ProjectsConsumer {
    fn name(&self) -> &str {
        "projects"
    }
    fn subscribes_to(&self) -> &[EventKind] {
        &[EventKind::RootObserved]
    }
    async fn on_event(
        &self,
        event: &Event,
        _ctx: &Ctx<'_>,
        _stage: &crate::bus::Stage,
    ) -> Result<Vec<Event>> {
        let Event::RootObserved {
            discovered,
            attribution,
            notes,
            rewalked,
            unconfirmed_worktree_ids,
            ..
        } = event
        else {
            return Ok(vec![]);
        };
        let mut attribution = (**attribution).clone();
        let dirs = std::mem::take(&mut attribution.dirs);
        let files = std::mem::take(&mut attribution.files);

        // Group discovered checkouts/worktrees by project identity. When a
        // checkout's `origin` remote is known, identity is the normalized
        // remote URL: two separate clones of the same repo are the same
        // project, and every main checkout after the first-seen one (by
        // path order, for determinism across the parallel walk) is demoted
        // from `Main` to `Clone`. A checkout with no remote falls back to
        // its object-store identity.
        let mut groups: BTreeMap<String, Vec<crate::git::DiscoveredWorktree>> = BTreeMap::new();
        for dw in discovered.iter().cloned() {
            let group_key = match dw
                .remote_url
                .as_deref()
                .and_then(crate::report::normalize_remote)
            {
                Some(remote) => format!("remote:{remote}"),
                None => format!("store:{}", dw.project_id),
            };
            groups.entry(group_key).or_default().push(dw);
        }

        let mut projects: Vec<ProjectRow> = Vec::new();
        let mut worktree_paths: Vec<(PathBuf, String)> = Vec::new();
        let mut project_remotes: HashMap<String, String> = HashMap::new();
        let mut worktree_remotes: HashMap<String, String> = HashMap::new();
        for (group_key, mut members) in groups {
            members.sort_by(|a, b| a.path.cmp(&b.path));
            let project_id = id_for(&group_key);
            let mut name: Option<String> = None;
            let mut worktrees: Vec<WorktreeRow> = Vec::new();
            let mut main_assigned = false;
            for dw in members {
                let worktree_id = id_for(&dw.path.display().to_string());
                worktree_paths.push((dw.path.clone(), worktree_id.clone()));
                if let Some(remote) = &dw.remote_url {
                    worktree_remotes.insert(worktree_id.clone(), remote.clone());
                }
                if let Some(remote) = dw
                    .remote_url
                    .as_deref()
                    .and_then(crate::report::normalize_remote)
                {
                    project_remotes.entry(project_id.clone()).or_insert(remote);
                }
                let kind = match dw.kind {
                    WorktreeKind::Linked => WorktreeKind::Linked,
                    WorktreeKind::Main | WorktreeKind::Clone => {
                        if main_assigned {
                            WorktreeKind::Clone
                        } else {
                            main_assigned = true;
                            WorktreeKind::Main
                        }
                    }
                };
                if kind == WorktreeKind::Main || name.is_none() {
                    name = Some(dw.project_name.clone());
                }
                worktrees.push(WorktreeRow {
                    worktree_id,
                    path: dw.path,
                    kind,
                    artifacts: Vec::new(),
                    signals: Vec::new(),
                    branch: None,
                    github: None,
                    merge_complete: None,
                    idle_secs: None,
                });
            }
            let remote = project_remotes.get(&project_id).cloned();
            projects.push(ProjectRow {
                project_id,
                name: name.unwrap_or_default(),
                worktrees,
                remote,
                ecosystems: Vec::new(),
            });
        }
        // Stable output ordering (by name, then id).
        projects.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.project_id.cmp(&b.project_id))
        });
        for project in &mut projects {
            for worktree in &mut project.worktrees {
                crate::attribution::apply_to_worktree(worktree, &mut attribution);
            }
        }
        let reconciliation = Reconciliation {
            unique_estimate: None,
            attributed: attribution.attributed_total,
            unowned: attribution.unowned_total,
            walked_total: attribution.walked_total,
            du_total: None,
            docker_attributed: 0,
            docker_unowned: 0,
        };
        Ok(vec![Event::ProjectsGrouped {
            projects: Arc::new(projects),
            rewalked: rewalked.clone(),
            worktree_paths: Arc::new(worktree_paths),
            project_remotes: Arc::new(project_remotes),
            worktree_remotes: Arc::new(worktree_remotes),
            unowned: Arc::new(attribution.unowned),
            dirs: Arc::new(dirs),
            files: Arc::new(files),
            reconciliation,
            notes: notes.clone(),
            unconfirmed_worktree_ids: unconfirmed_worktree_ids.clone(),
        }])
    }
}
