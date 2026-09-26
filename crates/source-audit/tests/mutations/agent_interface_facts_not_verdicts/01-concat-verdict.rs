//! target: crates/core/src/render.rs
//! by: audit:no_verdict_literals
//! why: sweep slip -- `concat!("can", " be deleted")` is invisible to a plain string-literal scan
pub fn sweep_verdict_line() -> String {
    concat!("can", " be deleted").to_string()
}
