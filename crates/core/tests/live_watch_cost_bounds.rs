//! #89: the regression thresholds for watched (live-epoch) observation,
//! as tests with real bounds.
//!
//! Wall time is not a merge criterion on a shared runner (it is recorded
//! in the Linux validation artifact instead). What *is* deterministic is
//! the work: directories listed. The thresholds, and why each is the
//! right one:
//!
//! * **Unchanged under proven continuity lists O(1) directories**, not
//!   O(tree): a watched epoch that reports nothing changed must not walk.
//!   Bound: at most 4 listings for a 300-directory tree (the root and the
//!   worktree roots the incremental merge re-confirms), and under 2% of
//!   the full walk.
//! * **A one-subtree mutation lists what it touched, not the tree.** The
//!   pipeline's granularity is the artifact for a change inside a build
//!   output (re-sized from its own subtree) and the *worktree* for a
//!   change in authored source (handed back to the walker, because a new
//!   file can change what the worktree's rows are). Bounds: a change in
//!   `node_modules` lists at most that artifact's own directories plus
//!   the reported ones plus 4; a
//!   change in one worktree's source lists that worktree and not the
//!   300 directories of the sibling worktree next to it.
//! * **The full walk it replaces lists every directory** -- the baseline
//!   that makes the other two meaningful, asserted so a change that
//!   stopped counting listings cannot pass them vacuously.
//!
//! The plans are the ones a live watch hands the pipeline
//! (`FsEventsPlan::from_live`); the pipeline is the same on both
//! platforms, so this runs on both.

#[path = "fixture/mod.rs"]
mod fixture;

use swamp_core::fs_events::{FsEventsPlan, FsEventsRequest, FsEventsSource, RefreshRefusal};
use swamp_core::work_counters::measured;

struct Canned(FsEventsPlan);

impl FsEventsSource for Canned {
    fn replay(&self, _req: &FsEventsRequest) -> FsEventsPlan {
        self.0.clone()
    }
}

const DIRS: usize = 300;

fn count_dirs(p: &std::path::Path) -> u64 {
    let mut n = 1;
    for e in std::fs::read_dir(p).unwrap().flatten() {
        if e.file_type().unwrap().is_dir() {
            n += count_dirs(&e.path());
        }
    }
    n
}

#[test]
fn watched_observation_work_is_bounded_by_what_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = fixture::build(tmp.path());
    let root = std::fs::canonicalize(&fx.root).unwrap();
    let checkout = std::fs::canonicalize(&fx.checkout).unwrap();
    // The bulk lives in a *sibling* worktree, so "the tree" and "the
    // worktree that changed" are different sizes.
    let sibling = std::fs::canonicalize(&fx.linked_worktree).unwrap();
    for i in 0..DIRS {
        let d = sibling.join(format!("docs/section-{}/page-{i}", i % 30));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("index.md"), format!("page {i}")).unwrap();
    }
    let store = tempfile::tempdir().unwrap();
    // The whole report pipeline, as `observe`/the TUI run it: the
    // history rows the incremental merge carries forward are written by
    // it, not by the walk stage alone.
    let stage = |_at: u64, plan: FsEventsPlan| {
        measured(|| {
            let r = swamp_core::report::report_full_mode_with_source(
                &root,
                None,
                false,
                Some(store.path()),
                Some("1h"),
                true,
                false,
                false,
                false,
                &Canned(plan),
            )
            .unwrap();
            r.notes
                .iter()
                .find_map(|n| n.strip_prefix("fsevents: mode="))
                .and_then(|m| m.split(' ').next())
                .unwrap_or("")
                .to_string()
        })
    };
    let (mode, full) = stage(
        1_000,
        FsEventsPlan::refused(RefreshRefusal::LiveWatchGap, None),
    );
    assert_eq!(mode, "full");
    assert!(
        full.dirs_listed as usize >= DIRS,
        "the baseline full walk listed {} directories for a {DIRS}-directory tree",
        full.dirs_listed
    );

    let (mode, unchanged) = stage(2_000, FsEventsPlan::from_live(Vec::new(), 1, None));
    assert_eq!(mode, "incremental");
    assert!(
        unchanged.dirs_listed <= 4 && unchanged.dirs_listed * 50 < full.dirs_listed,
        "unchanged under a proven epoch listed {} directories (full: {})",
        unchanged.dirs_listed,
        full.dirs_listed
    );

    // Inside a build output: that artifact is re-sized, from its own
    // directories -- a folded artifact keeps no interior rows, so its
    // subtree is the unit of work, and nothing outside it is listed.
    let modules = std::fs::canonicalize(&fx.node_modules).unwrap();
    let artifact_dirs = count_dirs(&modules);
    std::fs::write(modules.join("grown.bin"), vec![1u8; 4096]).unwrap();
    // What inotify reports for a write in node_modules: node_modules.
    let reported = vec![modules.clone()];
    let n = reported.len() as u64;
    let (mode, artifact) = stage(3_000, FsEventsPlan::from_live(reported, 2, None));
    assert_eq!(mode, "incremental");
    assert!(
        artifact.dirs_listed <= artifact_dirs + n + 4
            && artifact.dirs_listed * 4 < full.dirs_listed,
        "a change inside node_modules listed {} directories (its own {artifact_dirs}, reported \
         {n}, full {})",
        artifact.dirs_listed,
        full.dirs_listed
    );

    // In one worktree's source: that worktree, not its 300-directory
    // sibling.
    let touched = checkout.join("src");
    std::fs::create_dir_all(&touched).unwrap();
    std::fs::write(touched.join("new.rs"), "x").unwrap();
    let reported = vec![touched.clone()];
    let (mode, one) = stage(4_000, FsEventsPlan::from_live(reported, 3, None));
    assert_eq!(mode, "incremental");
    assert!(
        (one.dirs_listed as usize) < DIRS / 2 && one.dirs_listed * 4 < full.dirs_listed,
        "a change in one worktree's source listed {} directories: the {DIRS}-directory sibling \
         worktree was walked too (full {})",
        one.dirs_listed,
        full.dirs_listed
    );
    eprintln!(
        "MEASURE live_watch_cost dirs={DIRS} full_listed={} unchanged_listed={} \
         artifact_change_listed={} source_change_listed={}",
        full.dirs_listed, unchanged.dirs_listed, artifact.dirs_listed, one.dirs_listed
    );
}
