//! `RootRequested` → `RootObserved`: discovery and attribution of the
//! root. With a store, the FSEvents-driven incremental path runs first and
//! falls back to a full walk on refusal; without one, a full walk.

use crate::bus::{Consumer, Ctx, Event, EventKind};
use anyhow::Result;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct WalkConsumer {
    checkpoint: Mutex<Option<crate::growth::ObservationCheckpoint>>,
}

#[async_trait::async_trait(?Send)]
impl Consumer for WalkConsumer {
    fn name(&self) -> &str {
        "walk"
    }
    fn subscribes_to(&self) -> &[EventKind] {
        &[EventKind::RootRequested, EventKind::ReportCached]
    }
    async fn on_event(
        &self,
        event: &Event,
        ctx: &Ctx<'_>,
        stage: &crate::bus::Stage,
    ) -> Result<Vec<Event>> {
        if matches!(event, Event::ReportCached) {
            if let Some(checkpoint) = self.checkpoint.lock().unwrap().take() {
                checkpoint.commit()?;
            }
            return Ok(vec![]);
        }
        *self.checkpoint.lock().unwrap() = None;
        let mut notes: Vec<String> = Vec::new();
        let mut rewalked: Option<Arc<Vec<String>>> = None;
        let mut changed_paths = None;
        let mut unconfirmed_worktree_ids: Vec<String> = Vec::new();
        let (discovered, attribution) = if let Some(dir) = &ctx.store_dir {
            let (tracked, checkpoint) = crate::growth::stage_tracked_with_source(
                stage,
                dir,
                &ctx.root,
                ctx.observed_at,
                ctx.large_file_min_bytes,
                ctx.force_full,
                ctx.observe,
                ctx.fs_events,
                &ctx.pruned_subtrees,
            )?;
            *self.checkpoint.lock().unwrap() = checkpoint;
            notes.push(format!(
                "fsevents: mode={} reason={} changed_dirs={}",
                tracked.mode, tracked.reason, tracked.changed_dirs
            ));
            if !tracked.unconfirmed_worktree_ids.is_empty() {
                notes.push(format!(
                    "coverage: {} worktree(s) could not be confirmed this pass (access lost, not deleted); history preserved",
                    tracked.unconfirmed_worktree_ids.len()
                ));
            }
            rewalked = tracked.rewalked.map(Arc::new);
            changed_paths = tracked.changed_paths.map(Arc::new);
            // The unit families read this after `run_report` returns:
            // it is the only trusted evidence that lets them replay a
            // stored measurement instead of re-taking it
            // (`crate::fs_events::EventCoverage`).
            *ctx.event_window.lock().unwrap() = tracked.event_window;
            unconfirmed_worktree_ids = tracked.unconfirmed_worktree_ids;
            (tracked.discovered, tracked.attribution)
        } else {
            notes.push("fsevents: mode=full reason=no_store changed_dirs=0".to_string());
            crate::walk::discover_and_attribute(
                stage,
                &ctx.root,
                ctx.observed_at,
                ctx.large_file_min_bytes,
                &ctx.pruned_subtrees,
            )?
        };
        Ok(vec![Event::RootObserved {
            changed_paths,
            discovered: Arc::new(discovered),
            attribution: Arc::new(attribution),
            notes,
            rewalked,
            unconfirmed_worktree_ids: Arc::new(unconfirmed_worktree_ids),
        }])
    }
}
