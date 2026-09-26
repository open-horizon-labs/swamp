//! discovery-consumes-effective-scope: a detector summary's raw candidate
//! locations are private; discovery reads the authorized scope roots.
use swamp_core::scope::DetectorSummary;

fn raw(s: &DetectorSummary) {
    let _ = &s.locations;
}

fn main() {}
