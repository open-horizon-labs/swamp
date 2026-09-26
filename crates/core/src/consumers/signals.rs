//! `ProjectsGrouped` → `SignalsComputed`: git activity per worktree (last
//! commit age, dirty, unpushed, locked, idle) plus the current branch,
//! computed in parallel at walk time.

use crate::bus::{Consumer, Ctx, Event, EventKind, WorktreeSignals};
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub struct SignalsConsumer;

#[async_trait::async_trait(?Send)]
impl Consumer for SignalsConsumer {
    fn name(&self) -> &str {
        "signals"
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
            projects, rewalked, ..
        } = event
        else {
            return Ok(vec![]);
        };
        let all: Vec<(String, PathBuf)> = projects
            .iter()
            .flat_map(|p| {
                p.worktrees
                    .iter()
                    .map(|w| (w.worktree_id.clone(), w.path.clone()))
            })
            .collect();
        let mut by_worktree: HashMap<String, WorktreeSignals> = HashMap::new();
        // Incremental walk: a worktree FSEvents reported nothing under has
        // the same git state it had last time; age its signals instead of
        // running git on it again. Only the re-walked ones are recomputed.
        //
        // The previous pass's signals come from `git_signals.parquet`
        // (R18a-4), root-keyed and written by this same consumer at the
        // end of `on_event` below -- never from a scope-keyed table, so
        // this replay decision works identically for a single-root,
        // scope-less call and for one root inside a multi-root scope.
        let mut todo: Vec<(String, PathBuf)> = all.clone();
        if let (Some(rewalked), Some(store)) = (rewalked, &ctx.store_dir)
            && let Some((prev_observed_at, prev_by_worktree)) =
                crate::growth::read_git_signals_table(store, &crate::growth::root_key(&ctx.root))
        {
            let elapsed = ctx.observed_at.saturating_sub(prev_observed_at);
            let rewalked: std::collections::HashSet<&String> = rewalked.iter().collect();
            todo.retain(|(id, path)| {
                if rewalked.contains(id) {
                    return true;
                }
                let Some(prev) = prev_by_worktree.get(id.as_str()) else {
                    return true;
                };
                let (rows, raw) = crate::signals::age_signals(&prev.rows, &prev.raw, elapsed);
                by_worktree.insert(
                    id.clone(),
                    WorktreeSignals {
                        branch: prev.branch.clone(),
                        path: path.clone(),
                        rows,
                        raw,
                    },
                );
                false
            });
        }
        let paths: Vec<PathBuf> = todo.iter().map(|(_, p)| p.clone()).collect();
        let computed = crate::signals::compute_signals_raw_parallel(&paths, ctx.observed_at);
        let mut it = computed.into_iter();
        for (id, path) in todo {
            let (rows, raw) = it.next().unwrap_or_else(|| {
                (
                    Vec::new(),
                    crate::signals::RawSignals {
                        last_commit_age_secs: None,
                        dirty: None,
                        unpushed: None,
                        locked: None,
                        idle_for_secs: None,
                    },
                )
            });
            by_worktree.insert(
                id,
                WorktreeSignals {
                    branch: crate::github::current_branch(&path),
                    path,
                    rows,
                    raw,
                },
            );
        }
        // Persist this pass's full `by_worktree` (replayed rows aged
        // forward and freshly walked ones alike) as the next pass's
        // replay cache. Gated on `ctx.observe` like every other
        // current-state table write: a `--no-observe`/pure-read call
        // must never advance what a later real observation replays
        // from.
        if ctx.observe
            && let Some(store) = &ctx.store_dir
        {
            crate::growth::write_git_signals_table(
                store,
                &crate::growth::root_key(&ctx.root),
                ctx.observed_at,
                &by_worktree,
            )?;
        }
        Ok(vec![Event::SignalsComputed {
            by_worktree: Arc::new(by_worktree),
        }])
    }
}
