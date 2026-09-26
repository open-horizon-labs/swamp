//! target: crates/core/src/growth.rs
//! expect: retired
//! retired: 2026-09-22 -- only defines a function (in a stand-in module) that nothing calls; `crate::walk::full_walk` never existed and the real `growth::full_walk` takes a `bus::Stage`. Walk-before-replay ordering inside the real `stage_tracked_with_source` is behaviour, asserted by fsevents_incremental.rs (04-sweep3 and sweep4-M4 keep the audit-visible shape).
//! why: sweep slip -- the entry point runs a full walk itself instead of delegating replay planning
pub fn observe_tracked_with_source(root: &std::path::Path) -> u64 {
    let _ = crate::walk::full_walk(root);
    0
}
