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
                let mut ids: Vec<&String> = by_worktree.keys().collect();
                ids.sort();
                let candidates: Vec<(&String, String, String)> = ids
                    .into_iter()
                    .filter_map(|worktree_id| {
                        let remote = remotes.get(worktree_id)?;
                        let (owner, repo) = crate::github::github_owner_repo(remote)?;
                        Some((worktree_id, owner, repo))
                    })
                    .collect();
                // Each tip is one repository open (#181: 83 worktrees opened
                // one after another were ~1 s of a warm pass, ~2 s at
                // background priority); the opens are independent, so they
                // run on a small pool and keep their order.
                let paths: Vec<&std::path::Path> = candidates
                    .iter()
                    .map(|(id, _, _)| by_worktree[*id].path.as_path())
                    .collect();
                let tips = tip_shas_parallel(&paths);
                let owned: Vec<(String, String, String, Option<String>, String)> = candidates
                    .into_iter()
                    .zip(tips)
                    .map(|((worktree_id, owner, repo), tip_sha)| {
                        (
                            worktree_id.clone(),
                            owner,
                            repo,
                            by_worktree[worktree_id].branch.clone(),
                            tip_sha,
                        )
                    })
                    .collect();
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
                    let summary = crate::github::observe_all_forced(
                        &responder,
                        &dir,
                        volume_id,
                        &inputs,
                        ctx.observed_at,
                        crate::github::DEFAULT_GITHUB_TTL_SECS,
                        crate::github::DEFAULT_RUN_BUDGET_SECS,
                        crate::github::DEFAULT_CONCURRENCY,
                        crate::github::force_refresh(),
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

/// `signals::tip_sha` for every path, in input order, on up to four
/// threads (a scheduled observe runs at low priority; four keeps the opens
/// from crowding the rest of the pass).
fn tip_shas_parallel(paths: &[&std::path::Path]) -> Vec<String> {
    let workers = std::thread::available_parallelism()
        .map(|c| c.get())
        .unwrap_or(4)
        .min(4)
        .min(paths.len())
        .max(1);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out: Vec<std::sync::Mutex<String>> = paths
        .iter()
        .map(|_| std::sync::Mutex::new(String::new()))
        .collect();
    let scope = crate::work_counters::current();
    std::thread::scope(|s| {
        for _ in 0..workers {
            let scope = scope.clone();
            let (next, out) = (&next, &out);
            s.spawn(move || {
                crate::work_counters::install(scope);
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(path) = paths.get(i) else { break };
                    *out[i].lock().unwrap() = crate::signals::tip_sha(path).unwrap_or_default();
                }
            });
        }
    });
    out.into_iter().map(|m| m.into_inner().unwrap()).collect()
}

#[cfg(test)]
mod tests {
    /// Tempting wrong patch: collect the tips as the threads finish, so a
    /// tip lands on another worktree's row. Order and values equal the
    /// serial opens.
    #[test]
    fn parallel_tips_keep_input_order_and_equal_the_serial_answer() {
        let tmp = tempfile::tempdir().unwrap();
        let mut paths = Vec::new();
        for i in 0..6 {
            let repo = tmp.path().join(format!("r{i}"));
            std::fs::create_dir_all(&repo).unwrap();
            if i % 3 != 2 {
                let git = |args: &[&str]| {
                    crate::work_counters::record_spawn();
                    let ok = std::process::Command::new("git")
                        .args(args)
                        .current_dir(&repo)
                        .env("GIT_AUTHOR_NAME", "t")
                        .env("GIT_AUTHOR_EMAIL", "t@example.com")
                        .env("GIT_COMMITTER_NAME", "t")
                        .env("GIT_COMMITTER_EMAIL", "t@example.com")
                        .env("GIT_CONFIG_GLOBAL", "/dev/null")
                        .status()
                        .unwrap()
                        .success();
                    assert!(ok, "{args:?}");
                };
                git(&["init", "-q"]);
                std::fs::write(repo.join("f"), format!("{i}")).unwrap();
                git(&["add", "f"]);
                git(&["commit", "-q", "-m", "c", "--no-gpg-sign"]);
            }
            paths.push(repo);
        }
        let refs: Vec<&std::path::Path> = paths.iter().map(|p| p.as_path()).collect();
        let serial: Vec<String> = refs
            .iter()
            .map(|p| crate::signals::tip_sha(p).unwrap_or_default())
            .collect();
        assert_eq!(serial.iter().filter(|s| !s.is_empty()).count(), 4);
        assert_eq!(super::tip_shas_parallel(&refs), serial);
    }
}
