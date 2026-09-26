//! #82: a collector's dirty list is consumed only after the observation
//! that re-walked it has written its history, and two writers of one
//! root never interleave. Platform-neutral: the consumption and the
//! locks are the same code on both, driven here by a canned source that
//! carries a consumption the way the Linux collector source does.

#[path = "fixture/mod.rs"]
mod fixture;

use std::time::Duration;
use swamp_core::continuity::{
    Checkpoint, Consumption, DirtyEntry, paths, read_checkpoint, write_checkpoint,
};
use swamp_core::fs_events::{FsEventsPlan, FsEventsRequest, FsEventsSource};
use swamp_core::growth::stage_tracked_with_source;

struct WithConsumption(FsEventsPlan);

impl FsEventsSource for WithConsumption {
    fn replay(&self, _req: &FsEventsRequest) -> FsEventsPlan {
        self.0.clone()
    }
}

struct Refuse;

impl FsEventsSource for Refuse {
    fn replay(&self, _req: &FsEventsRequest) -> FsEventsPlan {
        FsEventsPlan::refused(swamp_core::fs_events::RefreshRefusal::NoStoredEventId, None)
    }
}

fn checkpoint(root: &std::path::Path, dirty: Vec<DirtyEntry>) -> Checkpoint {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(root).unwrap();
    Checkpoint {
        root: root.to_path_buf(),
        device: m.dev(),
        root_ino: m.ino(),
        boot_id: None,
        epoch_id: "epoch-1".into(),
        opened_at: 0,
        pid: 1,
        lost: None,
        previous_loss: None,
        seq: 5,
        dirty,
        excluded: Vec::new(),
        sync_token: None,
        flushed_at: 0,
        stopped_at: None,
        watches: 1,
        max_user_watches: None,
        kernel_bytes_estimate: 0,
    }
}

#[test]
fn the_dirty_list_survives_a_crash_before_or_after_the_history_write_and_goes_after_the_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = fixture::build(tmp.path());
    let root = std::fs::canonicalize(&fx.root).unwrap();
    let store = tempfile::tempdir().unwrap();
    // A baseline, so the next pass has a topology to be incremental from.
    let (_w, c) = stage_tracked_with_source(
        &swamp_core::bus::Stage::for_tests(),
        store.path(),
        &root,
        1_000,
        1 << 20,
        false,
        true,
        &Refuse,
        &[],
    )
    .unwrap();
    c.unwrap().commit().unwrap();

    let p = paths(store.path(), &root);
    let rel = fx
        .node_modules
        .strip_prefix(&fx.root)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    write_checkpoint(
        &p,
        &checkpoint(
            &root,
            vec![
                DirtyEntry {
                    p: rel.clone(),
                    s: 5,
                },
                // Dirtied again after the plan was read: must survive.
                DirtyEntry {
                    p: "late".into(),
                    s: 9,
                },
            ],
        ),
    )
    .unwrap();
    let mut plan = FsEventsPlan::from_live(vec![root.join(&rel)], 5, None);
    plan.consume = Some(Consumption {
        checkpoint: p.checkpoint.clone(),
        dirty_lock: p.dirty_lock.clone(),
        epoch_id: "epoch-1".into(),
        through_seq: 5,
    });
    let src = WithConsumption(plan);
    let pending = || read_checkpoint(&p).unwrap().dirty;

    // Crash before the history commit: the checkpoint is dropped.
    let (walk, c) = stage_tracked_with_source(
        &swamp_core::bus::Stage::for_tests(),
        store.path(),
        &root,
        2_000,
        1 << 20,
        false,
        true,
        &src,
        &[],
    )
    .unwrap();
    assert_eq!(walk.mode, "incremental");
    drop(c);
    assert_eq!(
        pending().len(),
        2,
        "nothing is consumed when nothing was written"
    );

    // Crash between the history write and the consumption.
    let (_w, c) = stage_tracked_with_source(
        &swamp_core::bus::Stage::for_tests(),
        store.path(),
        &root,
        3_000,
        1 << 20,
        false,
        true,
        &src,
        &[],
    )
    .unwrap();
    c.unwrap().commit_without_consuming_for_test().unwrap();
    assert_eq!(
        pending().len(),
        2,
        "the entries stay; the next run re-walks them"
    );

    // The real commit consumes exactly what the plan covered.
    let (_w, c) = stage_tracked_with_source(
        &swamp_core::bus::Stage::for_tests(),
        store.path(),
        &root,
        4_000,
        1 << 20,
        false,
        true,
        &src,
        &[],
    )
    .unwrap();
    c.unwrap().commit().unwrap();
    assert_eq!(
        pending(),
        vec![DirtyEntry {
            p: "late".into(),
            s: 9
        }]
    );
}

/// A read-only pass consumes nothing, whatever its plan says.
#[test]
fn a_read_only_observation_consumes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = fixture::build(tmp.path());
    let root = std::fs::canonicalize(&fx.root).unwrap();
    let store = tempfile::tempdir().unwrap();
    let p = paths(store.path(), &root);
    write_checkpoint(
        &p,
        &checkpoint(
            &root,
            vec![DirtyEntry {
                p: "a".into(),
                s: 1,
            }],
        ),
    )
    .unwrap();
    let mut plan = FsEventsPlan::from_live(vec![root.clone()], 5, None);
    plan.consume = Some(Consumption {
        checkpoint: p.checkpoint.clone(),
        dirty_lock: p.dirty_lock.clone(),
        epoch_id: "epoch-1".into(),
        through_seq: 5,
    });
    let (_w, c) = stage_tracked_with_source(
        &swamp_core::bus::Stage::for_tests(),
        store.path(),
        &root,
        1_000,
        1 << 20,
        false,
        false,
        &WithConsumption(plan),
        &[],
    )
    .unwrap();
    assert!(c.is_none(), "a read-only pass has nothing to commit");
    assert_eq!(read_checkpoint(&p).unwrap().dirty.len(), 1);
}

/// Two observations of one root (a TUI refresh and a scheduled run, say)
/// cannot interleave: the second waits for the first to commit, so it
/// never reads a baseline the first is about to replace, nor consumes
/// changes the first has not written.
#[test]
fn a_second_writer_of_the_same_root_waits_for_the_first_to_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = fixture::build(tmp.path());
    let root = std::fs::canonicalize(&fx.root).unwrap();
    let store = tempfile::tempdir().unwrap();
    let (_w, first) = stage_tracked_with_source(
        &swamp_core::bus::Stage::for_tests(),
        store.path(),
        &root,
        1_000,
        1 << 20,
        false,
        true,
        &Refuse,
        &[],
    )
    .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let (store2, root2) = (store.path().to_path_buf(), root.clone());
    let second = std::thread::spawn(move || {
        let (_w, c) = stage_tracked_with_source(
            &swamp_core::bus::Stage::for_tests(),
            &store2,
            &root2,
            2_000,
            1 << 20,
            false,
            true,
            &Refuse,
            &[],
        )
        .unwrap();
        tx.send(()).unwrap();
        c.unwrap().commit().unwrap();
    });
    assert!(
        rx.recv_timeout(Duration::from_millis(500)).is_err(),
        "the second writer staged while the first still held the root"
    );
    first.unwrap().commit().unwrap();
    rx.recv_timeout(Duration::from_secs(20))
        .expect("the second writer proceeds once the first commits");
    second.join().unwrap();

    // A read-only pass takes no lock at all.
    let (_w, held) = stage_tracked_with_source(
        &swamp_core::bus::Stage::for_tests(),
        store.path(),
        &root,
        3_000,
        1 << 20,
        false,
        true,
        &Refuse,
        &[],
    )
    .unwrap();
    let (_w, ro) = stage_tracked_with_source(
        &swamp_core::bus::Stage::for_tests(),
        store.path(),
        &root,
        3_001,
        1 << 20,
        false,
        false,
        &Refuse,
        &[],
    )
    .unwrap();
    assert!(ro.is_none());
    drop(held);
}
