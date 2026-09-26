//! target: crates/core/src/occupancy.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M46
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods, compile:clippy::disallowed_types
//! why: an uncounted `lsof` spawn written `<std::process::Command>::new(..)`
//! blind-spot (old model): a qualified-self path renders as `:: new`, which resolves to `new`; neither `Command::new` nor `spawn::command` matches, and it is not a value reference either
/// Sweep 4: a spawn the counter never hears about.
pub fn sweep4_probe_uncounted(p: &std::path::Path) -> bool {
    <std::process::Command>::new("lsof")
        .arg("-t")
        .arg(p)
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}
