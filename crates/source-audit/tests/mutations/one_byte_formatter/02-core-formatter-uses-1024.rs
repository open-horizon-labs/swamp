//! target: crates/core/src/render.rs
//! mode: append
//! expect: retired
//! retired: 2026-09-22 -- a second formatting function *inside* render.rs, the formatter module: which function in the formatter renders a size is review's business, not a rule's (the rule is "only render.rs formats sizes").
//! why: a 1024 divisor anywhere in the one byte formatter's module family
pub fn sweep_human_bytes_binary(n: u64) -> String {
    format!("{} KiB", n / 1024)
}
