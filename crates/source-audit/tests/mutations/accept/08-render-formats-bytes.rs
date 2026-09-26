//! target: crates/core/src/render.rs
//! expect: accept
//! source: accept
//! why: the formatter module formatting a byte quantity with a unit (the one place allowed to)
/// Accept: a size rendered where sizes are rendered.
fn sweep_accept_kib(n: u64) -> String {
    format!("{:.1} KiB", n as f64 / 1024.0)
}
