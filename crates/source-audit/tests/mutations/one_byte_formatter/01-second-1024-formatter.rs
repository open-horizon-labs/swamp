//! target: crates/tui/src/model.rs
//! by: audit:byte_units_only_in_the_formatter
//! why: sweep slip -- a second byte formatter dividing by 1024 beside the re-exported SI one
pub fn sweep_pretty_bytes(n: u64) -> String {
    let kib = n / 1024;
    format!("{kib} KiB")
}
