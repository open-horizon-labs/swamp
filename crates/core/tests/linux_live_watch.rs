//! #81 on a real Linux kernel: the inotify watcher against the races and
//! losses the design names, and the incremental pipeline it feeds held
//! to a reference full walk.
//!
//! The state machine's rules are unit tests in `live_watch.rs` with a
//! fake kernel (they run on both platforms); this file is the evidence
//! that the rules hold against inotify itself.
#![cfg(target_os = "linux")]

#[path = "fixture/mod.rs"]
mod fixture;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use swamp_core::fs_events::{FsEventsPlan, FsEventsRequest, FsEventsSource, RefreshRefusal};
use swamp_core::live_watch::{Coverage, LiveTree, Loss, RawEvent, inotify, mask};
use swamp_core::report::report_full_mode_with_source;

type Tree = LiveTree<inotify::Inotify>;

fn open_tree(root: &Path, exclude: Vec<PathBuf>) -> Tree {
    let mut t = LiveTree::new(
        root,
        exclude,
        inotify::Inotify::new().unwrap(),
        inotify::limits(),
    )
    .unwrap();
    t.register_all();
    let evs = t.kernel_mut().read(0).unwrap();
    t.apply(&evs);
    t.open();
    t
}

/// Drains everything the kernel has queued, for up to `wait`.
fn pump(t: &mut Tree, wait: Duration) {
    let deadline = Instant::now() + wait;
    loop {
        let evs = t.kernel_mut().read(50).unwrap();
        let empty = evs.is_empty();
        t.apply(&evs);
        if empty && Instant::now() > deadline {
            return;
        }
    }
}

fn dirty(t: &Tree) -> HashSet<PathBuf> {
    t.dirty().map(|(p, _)| p.to_path_buf()).collect()
}

fn root() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    (tmp, root)
}

/// Directories created *while* registration is walking the tree end up
/// watched: registration adds a directory's watch before listing it, so
/// a subdirectory created after the listing is reported by its parent
/// and registered then. After the epoch opens, a write inside every one
/// of them is reported.
#[test]
fn directories_created_during_bootstrap_are_watched_once_the_epoch_opens() {
    let (_t, root) = root();
    for i in 0..40 {
        std::fs::create_dir_all(root.join(format!("pre/{i}/deep"))).unwrap();
    }
    let stop = Arc::new(AtomicBool::new(false));
    let made = Arc::new(Mutex::new(Vec::new()));
    let churn = {
        let (stop, made, root) = (stop.clone(), made.clone(), root.clone());
        std::thread::spawn(move || {
            let mut i = 0;
            while !stop.load(Ordering::Relaxed) && i < 400 {
                let d = root.join(format!("pre/{}/new-{i}/inner", i % 40));
                std::fs::create_dir_all(&d).unwrap();
                made.lock().unwrap().push(d);
                i += 1;
            }
        })
    };
    let mut t = open_tree(&root, Vec::new());
    stop.store(true, Ordering::Relaxed);
    churn.join().unwrap();
    pump(&mut t, Duration::from_millis(200));
    assert_eq!(t.coverage(), &Coverage::Complete);
    t.take_dirty();

    let made = made.lock().unwrap().clone();
    assert!(!made.is_empty(), "the churn thread created nothing");
    for d in &made {
        std::fs::write(d.join("after-epoch.o"), b"x").unwrap();
    }
    pump(&mut t, Duration::from_millis(300));
    let seen = dirty(&t);
    let missed: Vec<&PathBuf> = made.iter().filter(|d| !seen.contains(*d)).collect();
    assert!(
        missed.is_empty(),
        "{} of {} directories created during bootstrap were not watched: {:?}",
        missed.len(),
        made.len(),
        &missed[..missed.len().min(5)]
    );
}

#[test]
fn a_nested_new_tree_is_registered_and_a_write_deep_inside_is_seen() {
    let (_t, root) = root();
    let mut t = open_tree(&root, Vec::new());
    let deep = root.join("a/b/c/d");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("early"), b"1").unwrap();
    pump(&mut t, Duration::from_millis(200));
    let first = dirty(&t);
    for d in ["a", "a/b", "a/b/c", "a/b/c/d"] {
        assert!(first.contains(&root.join(d)), "{d} dirty: {first:?}");
    }
    t.take_dirty();
    std::fs::write(deep.join("later"), b"2").unwrap();
    pump(&mut t, Duration::from_millis(200));
    assert!(dirty(&t).contains(&deep), "the new subtree is watched");
}

/// Rename a directory, write inside it under its new name, delete it,
/// recreate it, write again: each step is reported at the path it
/// happened at, and none of it is a loss of coverage.
#[test]
fn rename_delete_and_recreate_are_followed() {
    let (_t, root) = root();
    std::fs::create_dir_all(root.join("x/sub")).unwrap();
    std::fs::create_dir_all(root.join("elsewhere")).unwrap();
    let mut t = open_tree(&root, Vec::new());

    std::fs::rename(root.join("x"), root.join("elsewhere/y")).unwrap();
    pump(&mut t, Duration::from_millis(150));
    t.take_dirty();
    std::fs::write(root.join("elsewhere/y/sub/f"), b"1").unwrap();
    pump(&mut t, Duration::from_millis(150));
    assert!(
        dirty(&t).contains(&root.join("elsewhere/y/sub")),
        "{:?}",
        dirty(&t)
    );

    t.take_dirty();
    std::fs::remove_dir_all(root.join("elsewhere/y")).unwrap();
    std::fs::create_dir_all(root.join("elsewhere/y/sub")).unwrap();
    pump(&mut t, Duration::from_millis(150));
    t.take_dirty();
    std::fs::write(root.join("elsewhere/y/sub/g"), b"2").unwrap();
    pump(&mut t, Duration::from_millis(150));
    assert!(
        dirty(&t).contains(&root.join("elsewhere/y/sub")),
        "{:?}",
        dirty(&t)
    );
    assert_eq!(
        t.coverage(),
        &Coverage::Complete,
        "deletes and renames are changes, not losses"
    );

    // Moved out of the tree entirely: dropped, not a loss either.
    let outside = tempfile::tempdir().unwrap();
    std::fs::rename(root.join("elsewhere"), outside.path().join("gone")).unwrap();
    pump(&mut t, Duration::from_millis(150));
    assert_eq!(t.coverage(), &Coverage::Complete);
    std::fs::write(outside.path().join("gone/y/sub/h"), b"3").unwrap();
    pump(&mut t, Duration::from_millis(150));
    assert!(
        !dirty(&t).iter().any(|p| !p.starts_with(&root)),
        "nothing outside the root is reported"
    );
}

/// An overflow, injected into the source (the privileged way -- lowering
/// `fs.inotify.max_queued_events` -- is not available to a test): the
/// claim ends with the overflow named, and only a new epoch restores it.
#[test]
fn an_injected_overflow_ends_the_claim_until_a_new_epoch() {
    let (_t, root) = root();
    let mut t = open_tree(&root, Vec::new());
    t.apply(&[RawEvent {
        wd: -1,
        mask: mask::IN_Q_OVERFLOW,
        cookie: 0,
        name: None,
    }]);
    match t.coverage() {
        Coverage::Lost { loss, .. } => assert_eq!(*loss, Loss::QueueOverflow),
        other => panic!("{other:?}"),
    }
    assert!(t.reopen_after_loss());
    assert_eq!(t.coverage(), &Coverage::Complete);
}

/// And a real one, without privileges: stop reading, queue more events
/// than `max_queued_events`, then read. The kernel reports
/// `IN_Q_OVERFLOW`, and the watch names it.
#[test]
fn a_real_queue_overflow_is_reported_as_a_loss() {
    let limit = inotify::limits().max_queued_events.unwrap_or(16_384);
    if limit > 200_000 {
        eprintln!(
            "SKIP a_real_queue_overflow: max_queued_events is {limit}, too many to exceed quickly"
        );
        return;
    }
    let (_t, root) = root();
    // The directory exists, and is watched, before the burst: files
    // created in a directory whose watch is not yet installed produce no
    // events at all, and so no overflow.
    let dir = root.join("burst");
    std::fs::create_dir(&dir).unwrap();
    let mut t = open_tree(&root, Vec::new());
    // Each create is at least two events (IN_CREATE, IN_CLOSE_WRITE).
    for i in 0..(limit as usize / 2 + 2_000) {
        std::fs::write(dir.join(format!("f{i}")), b"").unwrap();
    }
    pump(&mut t, Duration::from_millis(200));
    match t.coverage() {
        Coverage::Lost { loss, detail, .. } => {
            assert_eq!(*loss, Loss::QueueOverflow);
            eprintln!("MEASURE real_overflow limit={limit} detail={detail:?}");
        }
        other => panic!(
            "{} files queued past a limit of {limit} and no overflow: {other:?}",
            limit / 2 + 2000
        ),
    }
}

/// swamp's own store, under the root, is neither watched nor reported:
/// writing an observation's results cannot trigger the next refresh.
#[test]
fn the_store_is_excluded_from_the_watch() {
    let (_t, root) = root();
    let store = root.join(".swamp-store");
    std::fs::create_dir_all(store.join("123")).unwrap();
    let mut t = open_tree(&root, vec![store.clone()]);
    std::fs::write(store.join("123/dirs.parquet"), b"x").unwrap();
    std::fs::write(root.join("real-change"), b"y").unwrap();
    pump(&mut t, Duration::from_millis(200));
    let d = dirty(&t);
    assert!(d.contains(&root), "{d:?}");
    assert!(!d.iter().any(|p| p.starts_with(&store)), "{d:?}");
}

/// A live-epoch source over an in-process tree, as the TUI and the
/// collector use one: the tree's dirty list when the stored observation
/// is inside the epoch, the gap otherwise, the loss when coverage failed.
struct EpochSource(Mutex<Tree>);

impl FsEventsSource for EpochSource {
    fn replay(&self, req: &FsEventsRequest) -> FsEventsPlan {
        let mut t = self.0.lock().unwrap();
        pump(&mut t, Duration::from_millis(200));
        let dev = Some(t.device());
        if let Coverage::Lost { loss, .. } = t.coverage() {
            return FsEventsPlan::refused(loss.refusal(), dev);
        }
        match req.since.last_observed_at {
            Some(since) if t.opened_at().is_some_and(|o| o <= since) => {
                FsEventsPlan::from_live(t.take_dirty(), t.seq(), dev)
            }
            _ => FsEventsPlan::refused(RefreshRefusal::LiveWatchGap, dev),
        }
    }
}

fn artifact_bytes(r: &swamp_core::report::Report) -> BTreeMap<PathBuf, u64> {
    r.projects
        .iter()
        .flat_map(|p| p.worktrees.iter())
        .flat_map(|w| w.artifacts.iter())
        .map(|a| (a.path.clone(), a.bytes))
        .collect()
}

fn mode(r: &swamp_core::report::Report) -> String {
    r.notes
        .iter()
        .find_map(|n| n.strip_prefix("fsevents: "))
        .unwrap_or("")
        .to_string()
}

/// The pipeline the watcher feeds, held to a reference full walk: the
/// first observation after the epoch opens is a named full walk (the gap
/// before the watch is covered by nothing); the next, after a mutation
/// in one subtree, is incremental and agrees with a full walk of the
/// same tree row for row.
#[test]
fn watched_incremental_observations_equal_a_reference_full_walk() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = fixture::build(tmp.path());
    let root = std::fs::canonicalize(&fx.root).unwrap();
    let store = tempfile::tempdir().unwrap();
    let observe = |src: &dyn FsEventsSource, observe: bool, full: bool| {
        report_full_mode_with_source(
            &root,
            None,
            false,
            Some(store.path()),
            Some("1h"),
            observe,
            false,
            false,
            full,
            src,
        )
        .unwrap()
    };
    // A baseline from before the watch.
    let before = EpochSource(Mutex::new(open_tree(
        &root,
        vec![store.path().to_path_buf()],
    )));
    let _ = observe(&before, true, false);
    // Now a watch opens (epoch after the stored observation).
    std::thread::sleep(Duration::from_millis(1100));
    let src = EpochSource(Mutex::new(open_tree(
        &root,
        vec![store.path().to_path_buf()],
    )));
    std::thread::sleep(Duration::from_millis(1100));
    let first = observe(&src, true, false);
    assert!(
        mode(&first).contains("mode=full reason=live_watch_gap"),
        "the first observation inside a new epoch walks fully and says why: {}",
        mode(&first)
    );

    std::fs::write(fx.node_modules.join("grown.bin"), vec![7u8; 64 * 1024]).unwrap();
    std::fs::create_dir_all(fx.target_dir.join("debug/new-deps")).unwrap();
    std::fs::write(fx.target_dir.join("debug/new-deps/x.rlib"), vec![1u8; 8192]).unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    let second = observe(&src, true, false);
    assert!(
        mode(&second).starts_with("mode=incremental"),
        "{}",
        mode(&second)
    );

    let reference = observe(&src, false, true);
    assert_eq!(
        artifact_bytes(&second),
        artifact_bytes(&reference),
        "the watched incremental observation and a full walk disagree"
    );
    assert!(artifact_bytes(&second)[&fx.node_modules] > fx.node_modules_bytes);
}
