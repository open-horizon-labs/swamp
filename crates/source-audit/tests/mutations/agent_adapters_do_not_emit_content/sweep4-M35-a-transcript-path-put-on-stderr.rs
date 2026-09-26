//! target: crates/core/src/agents/aider.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M35
//! by: audit:adapters_do_not_reach_gates, compile:clippy::disallowed_methods
//! why: a transcript path put on stderr by `.unwrap()` of an `Err` that carries it
//! blind-spot (old model): `expect(..)` counts when its message carries content; `unwrap()` prints the error's `Debug` -- here the path -- to stderr and is not an emitter (inline `{p:?}` in `panic!` *is* caught: placeholders are normalised)
/// Sweep 4: leaks the transcript path to stderr.
pub fn sweep4_require_session(p: &Path) {
    let found: Result<(), String> = if p.exists() {
        Ok(())
    } else {
        Err(format!("no session header in {}", p.display()))
    };
    found.unwrap();
}
