//! target: crates/tui/src/model.rs
//! mode: append
//! by: audit:byte_units_only_in_the_formatter
//! why: re-review 3 sweep -- a second byte formatter dividing by 1024 with binary labels taken from a unit table (blind spot: the `labels` half requires a KiB/MiB literal *inside the format! macro*; a unit table moves the label out of the macro)
const SWEEP_BINARY_UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];

/// Sweep: a second formatter, binary divisor, binary labels.
pub fn sweep_format_bytes(n: u64) -> String {
    let mut v = n as f64;
    let mut i = 0usize;
    while v >= 1024.0 && i + 1 < SWEEP_BINARY_UNITS.len() {
        v /= 1024.0;
        i += 1;
    }
    format!("{:.1} {}", v, SWEEP_BINARY_UNITS[i])
}
