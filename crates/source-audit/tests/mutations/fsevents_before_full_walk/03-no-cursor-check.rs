//! target: crates/core/src/growth.rs
//! expect: retired
//! retired: 2026-09-22 -- only defines a function (in a stand-in module) that nothing calls; `crate::walk::full_walk` never existed and the real `growth::full_walk` takes a `bus::Stage`. Walk-before-replay ordering inside the real `stage_tracked_with_source` is behaviour, asserted by fsevents_incremental.rs (04-sweep3 and sweep4-M4 keep the audit-visible shape).
//! why: a staged replay that walks fully before consulting FSEvents is the original non-incremental defect
pub fn stage_tracked_with_source(root: &std::path::Path, src: &dyn crate::fs_events::FsEventsSource) -> u64 {
    let _ = crate::walk::full_walk(root);
    let _ = src.replay(root);
    0
}
