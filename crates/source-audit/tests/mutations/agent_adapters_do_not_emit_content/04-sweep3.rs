//! target: crates/core/src/agents/aider.rs
//! mode: append
//! by: audit:adapters_do_not_reach_gates, compile:clippy::disallowed_methods
//! why: re-review 3 sweep -- a transcript path put on stderr by a panic message (blind spot: `EMITTERS` lists the print/log macros and the stdout/stderr handles; `panic!`/`expect` write the same bytes to stderr and are on none of them)
/// Sweep: leaks the transcript path to stderr.
pub fn sweep_require_session(p: &std::path::Path) {
    if !p.exists() {
        panic!("no session at {}", p.display());
    }
}
