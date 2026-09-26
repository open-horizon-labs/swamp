//! target: crates/core/src/consumers/projects.rs
//! by: audit:no_unreferenced_public_items
//! ported: 2026-09-22 -- the full `Consumer` trait
//! why: alias/rename variant -- the trait imported under another name, so a "impl Consumer for" needle would miss it
use crate::bus::Consumer as SweepStage;

pub struct SweepAliasedConsumer;

#[async_trait::async_trait(?Send)]
impl SweepStage for SweepAliasedConsumer {
    fn name(&self) -> &str {
        "sweep-aliased"
    }
    fn subscribes_to(&self) -> &[EventKind] {
        &[EventKind::ProjectsGrouped]
    }
    async fn on_event(
        &self,
        _event: &Event,
        _ctx: &Ctx<'_>,
        _stage: &crate::bus::Stage,
    ) -> Result<Vec<Event>> {
        Ok(Vec::new())
    }
}
