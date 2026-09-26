//! target: crates/core/src/agents/roo_code.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (W group, reviewer_counterexamples_stack4_sweep.rs), W12
//! by: compile:clippy::disallowed_methods
//! why: W12: `Path::read_dir` passed as a method value to `.map(..)`
//! blind-spot (old model): a path-to-associated-function used as a value, to std: dropped
/// Sweep 4 W12.
pub fn sweep4_w12_list(dirs: &[std::path::PathBuf]) -> usize {
    dirs.iter()
        .map(std::path::PathBuf::as_path)
        .map(Path::read_dir)
        .filter_map(Result::ok)
        .map(|rd| rd.count())
        .sum()
}
