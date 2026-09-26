//! `TrackingAnnotated` + `HistoryLoaded` → `ReportAssembled`: folds the
//! final facts into the `Report` contract. The only place a `Report` is
//! constructed on the pipeline.

use crate::bus::{Consumer, Ctx, Draft, Event, EventKind};
use crate::report::Report;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

type History = (
    Arc<HashMap<String, Vec<Option<u64>>>>,
    Vec<Option<u64>>,
    u64,
);

#[derive(Default)]
struct Pending {
    draft: Option<Arc<Draft>>,
    history: Option<History>,
    emitted: bool,
}

#[derive(Default)]
pub struct ReportAssembler {
    pending: Mutex<Pending>,
}

#[async_trait::async_trait(?Send)]
impl Consumer for ReportAssembler {
    fn name(&self) -> &str {
        "assemble"
    }
    fn subscribes_to(&self) -> &[EventKind] {
        &[EventKind::TrackingAnnotated, EventKind::HistoryLoaded]
    }
    async fn on_event(
        &self,
        event: &Event,
        ctx: &Ctx<'_>,
        _stage: &crate::bus::Stage,
    ) -> Result<Vec<Event>> {
        let mut p = self.pending.lock().unwrap();
        match event {
            Event::TrackingAnnotated(d) => p.draft = Some(d.clone()),
            Event::HistoryLoaded {
                series_by_key,
                total_series,
                window_secs,
            } => p.history = Some((series_by_key.clone(), total_series.clone(), *window_secs)),
            _ => {}
        }
        let (Some(draft), Some((series_by_key, total_series, window))) =
            (p.draft.clone(), p.history.clone())
        else {
            return Ok(vec![]);
        };
        if p.emitted {
            return Ok(vec![]);
        }
        p.emitted = true;
        let mut d: Draft = (*draft).clone();
        let stale = d
            .projects
            .iter()
            .flat_map(|p| &p.worktrees)
            .flat_map(|w| &w.artifacts)
            .filter(|a| a.dedup_stale)
            .count();
        if stale > 0 {
            d.notes.push(format!("{stale} artifact unique-byte totals await full-scan reconciliation; directory allocations are current but may count hardlinks multiple times."));
        }
        let summary = crate::report::summarize(&d.projects);
        let report = Report {
            observed_at: ctx.observed_at,
            root: ctx.root.clone(),
            store_dir: ctx.store_dir.clone(),
            projects: d.projects,
            unowned: d.unowned,
            series_by_key: (*series_by_key).clone(),
            total_series,
            series_window_secs: window,
            reconciliation: d.reconciliation,
            notes: d.notes,
            dirs_by_worktree: d.dirs_by_worktree,
            files_by_worktree: d.files_by_worktree,
            schedule_line: d.schedule_line,
            summary,
            github_enrichment: d.github_enrichment,
            nested_artifacts: Arc::try_unwrap(d.nested_artifacts).unwrap_or_else(|a| (*a).clone()),
        };
        Ok(vec![Event::ReportAssembled(Arc::new(report))])
    }
}
