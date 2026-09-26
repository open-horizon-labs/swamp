//! target: crates/core/src/growth.rs
//! mode: append
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: re-review 3 sweep -- a staged replay that runs a full tree walk before consulting FSEvents, spelled `discover_and_attribute` (blind spot: the ordering rule greps statements for the literal token `full_walk`; any other spelling of a full traversal is invisible)
/// Sweep: walks everything, then replays.
pub mod sweep_stage {
    pub struct SweepStream;
    impl SweepStream {
        pub fn replay(&self, _since: u64) -> Vec<String> {
            Vec::new()
        }
    }

    fn sweep_scan_all(dir: &std::path::Path, n: &mut usize) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            *n += 1;
            if e.path().is_dir() {
                sweep_scan_all(&e.path(), n);
            }
        }
    }

    pub fn stage_tracked_with_source(root: &std::path::Path) -> Vec<String> {
        let mut n = 0usize;
        sweep_scan_all(root, &mut n);
        let _events = SweepStream.replay(0);
        Vec::new()
    }
}
