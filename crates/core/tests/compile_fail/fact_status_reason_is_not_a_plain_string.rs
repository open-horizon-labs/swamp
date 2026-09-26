//! activity-and-consumer-evidence-have-limits: a `FactStatus` literal
//! built outside the evidence module still needs a `Reason`.
use swamp_core::evidence::FactStatus;

fn main() {
    let _ = FactStatus::Unknown { reason: String::new() };
}
