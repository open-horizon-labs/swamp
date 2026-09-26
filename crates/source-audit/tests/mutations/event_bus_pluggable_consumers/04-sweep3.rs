//! target: crates/core/src/consumers/mod.rs
//! mode: append
//! by: compile:E0050
//! why: re-review 3 sweep -- a consumer defined in consumers/mod.rs: it names the bus, registers at run time and is never in with_builtins (blind spot: `consumer_files` filters out `/mod.rs`, so the whole umbrella is blind to a consumer that lives there)
/// Sweep: a consumer the bus audits cannot see.
#[derive(Default)]
pub struct SweepShadowConsumer;

#[async_trait::async_trait(?Send)]
impl crate::bus::Consumer for SweepShadowConsumer {
    fn name(&self) -> &str {
        "sweep-shadow"
    }
    fn subscribes_to(&self) -> &[crate::bus::EventKind] {
        &[]
    }
    async fn on_event(
        &self,
        _event: &crate::bus::Event,
        _ctx: &crate::bus::Ctx<'_>,
    ) -> anyhow::Result<Vec<crate::bus::Event>> {
        let _bus_is_known_here = crate::bus::EventBus::with_builtins();
        Ok(Vec::new())
    }
}
