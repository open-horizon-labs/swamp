//! target: crates/core/src/render.rs
//! expect: accept
//! source: accept
//! why: a new public item that another module names (so it is not dead public API)
/// Accept: public, and used.
pub fn sweep_accept_plural(n: usize, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") }
}
//! file: crates/core/src/report.rs
//! mode: append
/// Accept: the caller that keeps it live.
fn sweep_accept_units_line(n: usize) -> String {
    crate::render::sweep_accept_plural(n, "unit")
}
