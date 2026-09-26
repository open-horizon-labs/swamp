//! A full measurement stages a pre-walk anchor, never an end-of-walk cursor.
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use swamp_core::fs_events::{FsEventsPlan, FsEventsRequest, FsEventsSource};
use swamp_core::growth::{read_fsevents_anchor, stage_tracked_with_source, volume_store_dir};

struct Baseline(AtomicUsize);
impl FsEventsSource for Baseline {
    fn replay(&self, _: &FsEventsRequest) -> FsEventsPlan {
        panic!("full observation must not replay history")
    }
    fn anchor_before_full(&self, root: &Path) -> Option<(u64, u64)> {
        self.0.fetch_add(1, Ordering::SeqCst);
        // If the hook moves after the walk this file is absent from its total.
        std::fs::write(root.join("during-anchor"), vec![1u8; 512 * 1024]).unwrap();
        Some((71, std::fs::metadata(root).unwrap().dev()))
    }
}

#[test]
fn full_anchor_precedes_walk_and_is_not_published_until_commit() {
    let root = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let source = Baseline(AtomicUsize::new(0));
    let stage = swamp_core::bus::Stage::for_tests();
    let (walk, checkpoint) = stage_tracked_with_source(
        &stage,
        store.path(),
        root.path(),
        1000,
        0,
        true,
        true,
        &source,
        &[],
    )
    .unwrap();
    assert!(
        walk.attribution.walked_total >= 512 * 1024,
        "anchor must precede measurement"
    );
    let dir = volume_store_dir(store.path(), root.path());
    assert_eq!(read_fsevents_anchor(&dir).event_id, None);
    drop(checkpoint); // later persistence failure: staged anchor is not published
    assert_eq!(read_fsevents_anchor(&dir).event_id, None);
    let (_, checkpoint) = stage_tracked_with_source(
        &stage,
        store.path(),
        root.path(),
        1001,
        0,
        true,
        true,
        &source,
        &[],
    )
    .unwrap();
    // A mutation after the walk must remain after the stored baseline.
    std::fs::write(root.path().join("after-walk"), b"later event").unwrap();
    checkpoint.unwrap().commit().unwrap();
    assert_eq!(read_fsevents_anchor(&dir).event_id, Some(71));
    assert_eq!(
        source.0.load(Ordering::SeqCst),
        2,
        "exactly one pre-walk capture per pass"
    );
    struct Next;
    impl FsEventsSource for Next {
        fn replay(&self, request: &FsEventsRequest) -> FsEventsPlan {
            assert_eq!(request.since.event_id, Some(71));
            FsEventsPlan::from_live(vec![request.root.clone()], 72, request.since.device)
        }
    }
    let (next, _) = stage_tracked_with_source(
        &stage,
        store.path(),
        root.path(),
        1100,
        0,
        false,
        true,
        &Next,
        &[],
    )
    .unwrap();
    assert!(
        next.attribution.walked_total > walk.attribution.walked_total,
        "a post-walk mutation must remain observable from the saved baseline"
    );
}

#[test]
fn full_unit_anchor_is_staged_without_earning_reuse_coverage() {
    let root = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let source = Baseline(AtomicUsize::new(0));
    let roots = vec![root.path().to_path_buf()];
    let replay =
        swamp_core::growth::replay_unit_roots(Some(store.path()), &roots, 1000, true, &source);
    assert!(!replay.covered(root.path()));
    let dir = volume_store_dir(store.path(), root.path());
    assert!(read_fsevents_anchor(&dir).unit_root.is_none());
    drop(replay);
    assert!(read_fsevents_anchor(&dir).unit_root.is_none());
    let replay =
        swamp_core::growth::replay_unit_roots(Some(store.path()), &roots, 1001, true, &source);
    replay.commit().unwrap();
    assert_eq!(
        read_fsevents_anchor(&dir).unit_root.unwrap().event_id,
        Some(71)
    );
    assert_eq!(
        read_fsevents_anchor(&dir).event_id,
        None,
        "unit commit must not advance walk cursor"
    );
}
