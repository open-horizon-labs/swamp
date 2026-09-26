//! target: crates/core/src/report.rs
//! by: compile:E0061
//! ported: 2026-09-22 -- the history writers now take the `bus::Stage` too; everything else as the real signature
//! why: a history write from report assembly, off the bus
fn sweep_direct_history(dir: &std::path::Path, rows: &mut [crate::report::DirRollup]) {
    let _ = crate::growth::observe_and_annotate_dirs(dir, 1, rows, 1_000, 30, 3600);
}
