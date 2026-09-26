//! `GrowthAnnotated` → `HistoryLoaded`: per-row byte history over the
//! effective growth window, read from the store's current file plus
//! reverse deltas, after this run's observation has been written.

use crate::bus::{Consumer, Ctx, Event, EventKind};
use anyhow::Result;
use std::sync::Arc;

pub struct HistoryConsumer;

#[async_trait::async_trait(?Send)]
impl Consumer for HistoryConsumer {
    fn name(&self) -> &str {
        "history"
    }
    fn subscribes_to(&self) -> &[EventKind] {
        &[EventKind::GrowthAnnotated]
    }
    async fn on_event(
        &self,
        _event: &Event,
        ctx: &Ctx<'_>,
        _stage: &crate::bus::Stage,
    ) -> Result<Vec<Event>> {
        let Some(dir) = &ctx.store_dir else {
            return Ok(vec![Event::HistoryLoaded {
                series_by_key: Arc::new(Default::default()),
                total_series: Vec::new(),
                window_secs: 0,
            }]);
        };
        let vol = crate::growth::volume_store_dir(dir, &ctx.root);
        // R20: the observation's own timestamp, not a second clock read
        // -- `runs.parquet` records it, and a read re-buckets the same
        // history at the same instant.
        let now_s = ctx.observed_at;
        let asked = ctx
            .since_override
            .as_deref()
            .and_then(crate::growth::parse_duration_secs)
            .unwrap_or_else(|| {
                crate::growth::parse_duration_secs(&crate::growth::load_config(dir).since)
                    .unwrap_or(86_400)
            });
        let hist = crate::growth::history_span_secs(&vol, now_s).unwrap_or(asked);
        let window = asked.min(hist).max(60);
        let (series, total) = crate::growth::history_series(&vol, window, 24, now_s);
        Ok(vec![Event::HistoryLoaded {
            series_by_key: Arc::new(series),
            total_series: total,
            window_secs: window,
        }])
    }
}
