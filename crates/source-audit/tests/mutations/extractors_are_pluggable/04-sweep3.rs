//! target: crates/core/src/bus/mod.rs
//! mode: append
//! by: compile:E0624
//! why: re-review 3 sweep -- a consumer registered twice, so every event it handles is handled twice (blind spot: registration is checked for *presence* in `with_builtins`, never for a count -- unlike the adapter registry, which does count)
/// Sweep: the same consumer, registered a second time.
pub fn sweep_with_builtins_twice(bus: &mut EventBus) -> Result<()> {
    bus.register(Box::new(crate::consumers::CacheWriter))?;
    bus.register(Box::new(crate::consumers::CacheWriter))?;
    Ok(())
}
