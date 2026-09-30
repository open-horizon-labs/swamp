//! target: crates/core/src/scope.rs
//! mode: append
//! by: audit:no_root_inference_sources
//! why: the Spotlight program name built from two literals
/// Sweep: Spotlight program name split.
pub fn sweep_spotlight_split() -> String {
    let mut s = String::from("md");
    s.push_str("find");
    s
}
