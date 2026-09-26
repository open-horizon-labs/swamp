//! target: crates/core/src/report.rs
//! by: compile:E0061
//! ported: 2026-09-22 -- the current signature; the missing argument is the `bus::Stage`
//! why: the same stage call, renamed on import
use crate::growth::observe_tracked_with_source as sweep_stage;

fn sweep_aliased_stage(dir: &std::path::Path, root: &std::path::Path) {
    let source = crate::fs_events::platform_source();
    let _ = sweep_stage(dir, root, 1_000, 0, false, true, source.as_ref(), &[]);
}
