//! activity-and-consumer-evidence-have-limits: a reason written as a
//! literal is checked non-blank at compile time.
fn main() {
    let _ = swamp_core::reason!("   ");
}
