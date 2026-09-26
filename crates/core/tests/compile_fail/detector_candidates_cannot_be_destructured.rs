//! discovery-consumes-effective-scope: destructuring a detector summary
//! to reach its raw candidates does not compile either.
use swamp_core::scope::DetectorSummary;

fn raw(s: &DetectorSummary) {
    let DetectorSummary { locations, .. } = s;
}

fn main() {}
