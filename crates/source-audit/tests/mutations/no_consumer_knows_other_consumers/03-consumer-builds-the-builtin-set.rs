//! target: crates/core/src/consumers/cache.rs
//! by: audit:bus_static_registration
//! ported: 2026-09-22 -- the original called a local stand-in named `with_builtins`; the real registrar
//! why: a consumer constructing the whole builtin set, which is registration knowledge it must not have
fn sweep_rebuild_bus() {
    let _ = crate::bus::EventBus::with_builtins();
}
