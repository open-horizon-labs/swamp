//! `ProjectsGrouped` → `DockerJoined`: images, build cache and volumes
//! joined to worktrees on explicit evidence only (compose label, compose
//! file `name:`, `image.source` matching a remote); the rest unowned with
//! the reason.

use crate::bus::{Consumer, Ctx, Event, EventKind};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

pub struct DockerConsumer;

#[async_trait::async_trait(?Send)]
impl Consumer for DockerConsumer {
    fn name(&self) -> &str {
        "docker"
    }
    fn subscribes_to(&self) -> &[EventKind] {
        &[EventKind::ProjectsGrouped]
    }
    async fn on_event(
        &self,
        event: &Event,
        ctx: &Ctx<'_>,
        _stage: &crate::bus::Stage,
    ) -> Result<Vec<Event>> {
        let Event::ProjectsGrouped {
            projects,
            worktree_paths,
            project_remotes,
            ..
        } = event
        else {
            return Ok(vec![]);
        };
        let mut notes = Vec::new();
        // Docker is probed only when the authorized scope includes it.
        // A disabled or excluded `docker-desktop` detector means the
        // user did not authorize asking the daemon about their images,
        // volumes and containers -- and asking cost ~0.9 s per
        // observation, forever, when the daemon is installed and
        // stopped (the 2026-09-22 re-review's CE6). A mocked facts file
        // is a test/CLI input, not a probe, so it is still honoured.
        let asked = ctx.docker_in_scope || ctx.docker_facts.is_some();
        let facts = if asked {
            crate::docker::load_cached(
                ctx.docker_facts.as_deref(),
                ctx.store_dir.as_deref(),
                ctx.enrich,
            )
        } else {
            crate::docker::DockerFacts {
                unavailable: Some(
                    "docker: not in scope this invocation (the docker-desktop detector is \
                     disabled or excluded), so the daemon was not asked"
                        .to_string(),
                ),
                ..Default::default()
            }
        };
        if let Some(reason) = &facts.unavailable {
            notes.push(reason.clone());
        }
        // compose project name -> worktrees that declare it, so a
        // `com.docker.compose.project` label joins by explicit evidence.
        let mut compose_index: HashMap<String, Vec<String>> = HashMap::new();
        for (path, worktree_id) in worktree_paths.iter() {
            for name in crate::compose::discover_candidate_names(path) {
                compose_index
                    .entry(name)
                    .or_default()
                    .push(worktree_id.clone());
            }
        }
        let join = crate::report::join_docker_facts(
            &facts,
            projects,
            worktree_paths,
            project_remotes,
            &compose_index,
        );
        Ok(vec![Event::DockerJoined {
            rows_by_worktree: Arc::new(join.rows_by_worktree),
            unowned: Arc::new(join.unowned),
            attributed_bytes: join.attributed_bytes,
            unowned_bytes: join.unowned_bytes,
            notes,
            facts: asked.then(|| Arc::new(facts)),
        }])
    }
}
