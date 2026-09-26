//! `ReportAssembled` → `ReportCached`: the pipeline's terminal
//! checkpoint gate, not a cache write any more.
//!
//! Until R18a-4 this consumer wrote the whole assembled `Report` to
//! `last_report-<key>.json.zst` so a `--no-observe`/TUI-start call could
//! paint the last known truth without walking anything. That per-root
//! replay cache is now two typed tables written earlier in the same
//! pass, by the consumers that actually need a previous value to replay
//! from (`consumers::signals`'s `git_signals.parquet`,
//! `consumers::cargo`'s `cargo_replay_cache.parquet`) -- each right
//! after it computes the value the *next* pass will read back, rather
//! than from a full-report snapshot taken at the very end. Nothing else
//! ever read `load_last_report` for anything but those two replay
//! decisions, so there is nothing left for this stage to persist.
//!
//! It still has a job: `bus::run_report`'s FSEvents checkpoint commits
//! only after `ReportCached` fires (`consumers::walk`), so a pass that
//! errors anywhere -- including inside `signals`/`cargo`'s own table
//! writes above, which now abort the whole bus run via `?` exactly like
//! any other consumer error -- never reaches here and never advances
//! the checkpoint past evidence nothing actually persisted for.

use crate::bus::{Consumer, Ctx, Event, EventKind};
use anyhow::Result;

pub struct CacheWriter;

#[async_trait::async_trait(?Send)]
impl Consumer for CacheWriter {
    fn name(&self) -> &str {
        "cache"
    }
    fn subscribes_to(&self) -> &[EventKind] {
        &[EventKind::ReportAssembled]
    }
    async fn on_event(
        &self,
        _event: &Event,
        _ctx: &Ctx<'_>,
        _stage: &crate::bus::Stage,
    ) -> Result<Vec<Event>> {
        Ok(vec![Event::ReportCached])
    }
}
