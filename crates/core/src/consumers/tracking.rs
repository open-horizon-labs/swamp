//! `GrowthAnnotated` → `TrackingAnnotated`: git tracking status
//! (tracked / ignored / untracked) on every artifact row and top-level
//! Source directory, and the ecosystem each artifact belongs to.

use crate::bus::{Consumer, Ctx, Draft, Event, EventKind};
use anyhow::Result;
use std::sync::Arc;

pub struct TrackingConsumer;

#[async_trait::async_trait(?Send)]
impl Consumer for TrackingConsumer {
    fn name(&self) -> &str {
        "tracking"
    }
    fn subscribes_to(&self) -> &[EventKind] {
        &[EventKind::GrowthAnnotated]
    }
    async fn on_event(
        &self,
        event: &Event,
        _ctx: &Ctx<'_>,
        _stage: &crate::bus::Stage,
    ) -> Result<Vec<Event>> {
        let Event::GrowthAnnotated(draft) = event else {
            return Ok(vec![]);
        };
        let mut d: Draft = (**draft).clone();
        crate::report::annotate_tracking(&mut d.projects, d.dirs_by_worktree.as_mut());
        Ok(vec![Event::TrackingAnnotated(Arc::new(d))])
    }
}
