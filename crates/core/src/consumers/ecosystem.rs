//! `ProjectsGrouped` → `EcosystemsDetected`: the ecosystem tags at each
//! project's main checkout root (Cargo.toml → rs, package.json → js, …).

use crate::bus::{Consumer, Ctx, Event, EventKind};
use crate::report::WorktreeKind;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

pub struct EcosystemConsumer;

#[async_trait::async_trait(?Send)]
impl Consumer for EcosystemConsumer {
    fn name(&self) -> &str {
        "ecosystem"
    }
    fn subscribes_to(&self) -> &[EventKind] {
        &[EventKind::ProjectsGrouped]
    }
    async fn on_event(
        &self,
        event: &Event,
        _ctx: &Ctx<'_>,
        _stage: &crate::bus::Stage,
    ) -> Result<Vec<Event>> {
        let Event::ProjectsGrouped { projects, .. } = event else {
            return Ok(vec![]);
        };
        let mut by_project: HashMap<String, Vec<String>> = HashMap::new();
        for p in projects.iter() {
            let root = p
                .worktrees
                .iter()
                .find(|w| w.kind == WorktreeKind::Main)
                .or(p.worktrees.first())
                .map(|w| w.path.clone());
            if let Some(root) = root {
                by_project.insert(p.project_id.clone(), crate::ecosystem::detect(&root));
            }
        }
        Ok(vec![Event::EcosystemsDetected {
            by_project: Arc::new(by_project),
        }])
    }
}
