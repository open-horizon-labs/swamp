//! target: crates/tui/src/app.rs
//! expect: accept
//! source: accept
//! why: a blocking occupancy probe started from code the event thread runs (a `Drop`, which the rule treats as reachable from anywhere) -- inside `worker::spawn`
/// Accept: the probe is the worker's, never the event loop's.
struct SweepAcceptProbe(PathBuf);

impl Drop for SweepAcceptProbe {
    fn drop(&mut self) {
        let p = self.0.clone();
        crate::worker::spawn(move || {
            let _ = swamp_core::occupancy::open_file_evidence(&p);
        });
    }
}
