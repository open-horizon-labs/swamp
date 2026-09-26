//! target: crates/core/src/report.rs
//! by: compile:E0061
//! ported: 2026-09-22 -- the history writers take the `bus::Stage`; everything else as the real signature
//! why: report assembly calling a stage's work directly
fn sweep_report_calls_stage_directly(dir: &std::path::Path, files: &mut [crate::report::FileRow]) {
    let _ = crate::growth::observe_and_annotate_files(dir, 1, files, 1_000, 30, 3600);
}
