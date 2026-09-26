//! activity-and-consumer-evidence-have-limits: an Unknown fact's reason
//! is a `Reason`, never a plain string.
use swamp_core::evidence::{Evidence, EvidenceSource, FactKind, FactSubtype};

fn blank(kind: FactKind, subtype: FactSubtype, source: EvidenceSource) {
    let _ = Evidence::unknown(kind, subtype, source, 0, String::from(""));
}

fn main() {}
