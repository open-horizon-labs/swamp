//! target: crates/core/src/consumers/ecosystem.rs
//! by: audit:no_unreferenced_public_items
//! ported: 2026-09-22 -- the full `Consumer` trait (the old fixture implemented only `name`); registration is private to the bus's registrar, so an unregistered consumer is a public type nothing names
//! why: a new fact source written as a consumer and never registered in EventBus::with_builtins
pub struct SweepUnregisteredConsumer;

#[async_trait::async_trait(?Send)]
impl Consumer for SweepUnregisteredConsumer {
    fn name(&self) -> &str {
        "sweep-unregistered"
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
