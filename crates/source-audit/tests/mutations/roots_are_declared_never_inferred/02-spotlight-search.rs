//! target: crates/cli/src/main.rs
//! mode: append
//! by: audit:no_root_inference_sources
//! why: asking Spotlight for every directory that holds a `.git` finds the user's repositories without being told where they are
/// Sweep: the Spotlight query.
pub fn sweep_spotlight_program() -> &'static str {
    "mdfind"
}
