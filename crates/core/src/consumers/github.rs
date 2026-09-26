//! `ProjectsGrouped` + `SignalsComputed` → `GithubEnriched`: PR and merge
//! facts for every worktree whose remote is on github.com. Reads the
//! volume-keyed `enrich.parquet` cache; refreshes it live only when the
//! run asked (`enrich`), since that shells out to `gh`.

use crate::bus::{Consumer, Ctx, Event, EventKind};
use crate::report::GithubEnrichmentSummary;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct GithubConsumer {
    /// worktree_id -> raw remote URL, remembered from `ProjectsGrouped`
    /// until the signals (branch) arrive.
    remotes: Mutex<Option<Arc<HashMap<String, String>>>>,
}

#[async_trait::async_trait(?Send)]
impl Consumer for GithubConsumer {
    fn name(&self) -> &str {
        "github"
    }
    fn subscribes_to(&self) -> &[EventKind] {
        &[EventKind::ProjectsGrouped, EventKind::SignalsComputed]
    }
    async fn on_event(
        &self,
        event: &Event,
        ctx: &Ctx<'_>,
        _stage: &crate::bus::Stage,
    ) -> Result<Vec<Event>> {
        match event {
            Event::ProjectsGrouped {
                worktree_remotes, ..
            } => {
                *self.remotes.lock().unwrap() = Some(worktree_remotes.clone());
                Ok(vec![])
            }
            Event::SignalsComputed { by_worktree } => {
                let remotes = self.remotes.lock().unwrap().clone().unwrap_or_default();
                let dir = ctx
                    .store_dir
                    .clone()
                    .or_else(crate::report::default_github_cache_dir);
                let Some(dir) = dir else {
                    return Ok(vec![Event::GithubEnriched {
                        facts_by_worktree: Arc::new(HashMap::new()),
                        summary: None,
                        notes: Vec::new(),
                    }]);
                };
                let volume_id = crate::fs_gate::metadata_following(&ctx.root)
                    .map(|m| crate::fs_gate::MetadataExt::dev(&m))
                    .unwrap_or(0);
                // Only worktrees whose remote resolves to a github.com owner/repo.
                let mut owned: Vec<(String, String, String, Option<String>, String)> = Vec::new();
                let mut ids: Vec<&String> = by_worktree.keys().collect();
                ids.sort();
                for worktree_id in ids {
                    let Some(remote) = remotes.get(worktree_id) else {
                        continue;
                    };
                    let Some((owner, repo)) = crate::github::github_owner_repo(remote) else {
                        continue;
                    };
                    let sig = &by_worktree[worktree_id];
                    let tip_sha = crate::signals::tip_sha(&sig.path).unwrap_or_default();
                    owned.push((
                        worktree_id.clone(),
                        owner,
                        repo,
                        sig.branch.clone(),
                        tip_sha,
                    ));
                }
                let inputs: Vec<crate::github::EnrichInput> = owned
                    .iter()
                    .map(
                        |(worktree_id, owner, repo, branch, tip_sha)| crate::github::EnrichInput {
                            worktree_id,
                            tip_sha,
                            branch: branch.as_deref(),
                            owner,
                            repo,
                        },
                    )
                    .collect();
                let t_github = std::time::Instant::now();
                let (facts, notes, summary) = if ctx.enrich {
                    let responder = crate::github::GhCliResponder;
                    let summary = crate::github::observe_all(
                        &responder,
                        &dir,
                        volume_id,
                        &inputs,
                        ctx.observed_at,
                        crate::github::DEFAULT_GITHUB_TTL_SECS,
                        crate::github::DEFAULT_RUN_BUDGET_SECS,
                        crate::github::DEFAULT_CONCURRENCY,
                    );
                    let (facts, mut read_notes) = crate::github::read_cached(
                        &dir,
                        volume_id,
                        &inputs,
                        ctx.observed_at,
                        crate::github::DEFAULT_GITHUB_TTL_SECS,
                    );
                    read_notes.retain(|n| {
                        !n.contains("not enriched") && !n.contains("older than the refresh window")
                    });
                    read_notes.extend(summary.notes);
                    (
                        facts,
                        read_notes,
                        Some(GithubEnrichmentSummary {
                            calls_made: summary.calls_made,
                            worktrees_enriched: summary.worktrees_enriched,
                            elapsed_secs: t_github.elapsed().as_secs_f64(),
                        }),
                    )
                } else {
                    let (facts, notes) = crate::github::read_cached(
                        &dir,
                        volume_id,
                        &inputs,
                        ctx.observed_at,
                        crate::github::DEFAULT_GITHUB_TTL_SECS,
                    );
                    (facts, notes, None)
                };
                Ok(vec![Event::GithubEnriched {
                    facts_by_worktree: Arc::new(facts),
                    summary,
                    notes,
                }])
            }
            _ => Ok(vec![]),
        }
    }
}
