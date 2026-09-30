//! v0.8.0 G6: Reclaim to Trash (core half). Real temp directories, a
//! fake Trash root, a temp store; nothing here touches the developer's
//! real caches, Trash or store.
//!
//! Policy (maintainer, 2026-09-30): what the person can see in Reclaim,
//! the person may move to Trash. Category, location, tool records and a
//! process holding a file are lines on the confirm, never refusals. What
//! refuses: not a real entry, the OS, the person's own protect marks,
//! a target that changed after the review, a ledger that cannot take the
//! started row.
//!
//! Each test names the tempting wrong patch it fails.

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use swamp_core::actions::{ReclaimMoveFacts as MoveFacts, trash_reclaim as trash};
use swamp_core::drilldown::{ChildKind, ChildMeasure, UnitChild};
use swamp_core::external::ExternalUnit;
use swamp_core::fs_gate::StoreDir;
use swamp_core::last_used::LastUsed;
use swamp_core::ledger::Ledger;
use swamp_core::locations::{Provenance, StorageCategory};
use swamp_core::manager_facts::ManagerFacts;
use swamp_core::reclaim::{ReclaimInput, ReclaimView, build};
use swamp_core::reclaim_trash::{
    ReclaimTarget, Reviewed, child_not_markable, find_target, recheck, review, row_path,
};

const NOW: u64 = 1_790_000_000;

fn unit(category: StorageCategory, path: &Path, bytes: u64) -> ExternalUnit {
    ExternalUnit {
        detector_id: "fixture".to_string(),
        detector_name: "fixture".to_string(),
        category,
        provenance: Provenance::BuiltinConvention,
        path: path.to_path_buf(),
        bytes,
        mtime_max: 0,
        hardlinked: false,
        growth_bytes: None,
        regrowth_count: 0,
        observed_at: NOW,
        consumers: Vec::new(),
        note: None,
        evidence: Vec::new(),
        bytes_counted_elsewhere: 0,
        overlap_count: 0,
        last_used: LastUsed::default(),
        children: Vec::new(),
    }
}

fn child(kind: ChildKind, name: &str, bytes: Option<i64>) -> UnitChild {
    UnitChild {
        kind,
        name: name.to_string(),
        bytes,
        measure: if bytes.is_some() {
            ChildMeasure::Complete
        } else {
            ChildMeasure::NotMeasured
        },
        mtime_max: 0,
        entries: 0,
        not_measured: 0,
        last_used: LastUsed::default(),
    }
}

fn view(units: &[ExternalUnit]) -> ReclaimView {
    build(&ReclaimInput {
        units,
        interiors: &[],
        unowned: &[],
        manager_facts: &ManagerFacts::default(),
        declared_roots: &[],
        explicit_scope: false,
        projects: 1,
        observed_at: NOW,
    })
}

struct Fx {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    store: PathBuf,
    trash: PathBuf,
}

fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("home");
    let store = root.join("store");
    let trash = root.join("trash");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&store).unwrap();
    Fx {
        _tmp: tmp,
        home,
        store,
        trash,
    }
}

fn ledger(f: &Fx) -> Ledger {
    Ledger::resolved(&StoreDir::at(&f.store).unwrap())
}

fn target_of(v: &ReclaimView, path: &Path) -> ReclaimTarget {
    find_target(v, path).unwrap()
}

fn facts_of(t: &ReclaimTarget, warnings: Vec<String>) -> MoveFacts {
    MoveFacts {
        label: t.path.display().to_string(),
        bytes: t.bytes.unwrap_or(0),
        observed_at: NOW,
        warnings,
        category: t.category.clone(),
    }
}

fn make_dir(p: &Path) {
    std::fs::create_dir_all(p).unwrap();
    std::fs::write(p.join("f"), b"hello").unwrap();
}

fn reviewed(f: &Fx, cat: StorageCategory, path: &Path) -> (ReclaimTarget, Reviewed, Vec<String>) {
    let v = view(&[unit(cat, path, 5)]);
    let t = target_of(&v, path);
    let r = review(&t, Some(&f.store), Some(&f.home)).expect("review");
    (t, r.reviewed, r.warnings)
}

/// Tempting wrong patch: local-state and models (or an installation, or
/// Claude scratch) are refused by category. The maintainer's rule is that
/// what they can see, they may move: each is reviewable, moves, and its
/// consequence is a line on the confirm, not a verdict.
#[test]
fn every_category_is_allowed_and_says_what_swamp_does_not_know() {
    for (cat, needle) in [
        (StorageCategory::LocalState, "cannot be regenerated"),
        (StorageCategory::Models, "cannot be regenerated"),
        (
            StorageCategory::Installation,
            "the tool's own removal command",
        ),
        (
            StorageCategory::Unclassified,
            "regeneration cost not established",
        ),
        (StorageCategory::Cache, "last used: no record"),
        (StorageCategory::BuildOutput, "last used: no record"),
        (StorageCategory::Environments, "last used: no record"),
        (StorageCategory::Downloads, "last used: no record"),
    ] {
        let f = fx();
        let p = f.home.join("thing");
        make_dir(&p);
        let (t, r, warnings) = reviewed(&f, cat, &p);
        assert!(
            warnings.iter().any(|w| w.contains(needle)),
            "{cat:?}: {warnings:#?}"
        );
        let dest = trash(
            &r,
            &facts_of(&t, warnings),
            Some(&f.store),
            &ledger(&f),
            &f.trash,
        )
        .unwrap_or_else(|e| panic!("{cat:?}: {e}"));
        assert!(!p.exists(), "{cat:?} moved");
        assert!(dest.join("f").exists(), "{cat:?} is in the Trash, whole");
    }
}

/// Tempting wrong patch: no warning carries the words of a verdict.
#[test]
fn warnings_never_use_verdict_words() {
    let f = fx();
    let p = f.home.join("Library/Caches");
    make_dir(&p);
    std::fs::create_dir_all(p.join("app")).unwrap();
    let mut u = unit(StorageCategory::Unclassified, &p, 5);
    u.children = vec![child(ChildKind::Entry, "app", Some(5))];
    u.note = Some("part of it could not be read".into());
    let v = view(&[u]);
    let t = target_of(&v, &p);
    let r = review(&t, Some(&f.store), Some(&f.home)).unwrap();
    let all = r.warnings.join("\n").to_lowercase();
    for w in ["unused", "stale", "safe", "orphan", "obsolete", "unneeded"] {
        assert!(!all.contains(w), "{w} in {all}");
    }
}

/// Tempting wrong patch: the whole ~/Library/Caches root is refused (or
/// silently allowed). It is allowed, and its confirm says it is the
/// folder every app keeps its cache in and that N folders are inside,
/// and that a coverage gap makes the bytes a lower bound.
#[test]
fn the_caches_root_is_allowed_with_the_whole_folder_named() {
    let f = fx();
    let p = f.home.join("Library/Caches");
    make_dir(&p);
    std::fs::create_dir_all(p.join("a")).unwrap();
    std::fs::create_dir_all(p.join("b")).unwrap();
    let mut u = unit(StorageCategory::Unclassified, &p, 10);
    u.children = vec![
        child(ChildKind::Entry, "a", Some(5)),
        child(ChildKind::Entry, "b", Some(5)),
    ];
    u.note = Some("coverage incomplete".into());
    let v = view(&[u]);
    let t = target_of(&v, &p);
    let r = review(&t, Some(&f.store), Some(&f.home)).unwrap();
    let all = r.warnings.join("\n");
    assert!(
        all.contains("every app on this Mac keeps its cache in"),
        "{all}"
    );
    assert!(
        all.contains("including the 2 folders listed under it"),
        "{all}"
    );
    assert!(all.contains("bytes are a lower bound"), "{all}");
}

/// Tempting wrong patch: the folder row under a unit is looked up by its
/// bare name, or its name is joined unchecked. A depth-2 folder resolves
/// to unit/name; `../x`, `a/b`, empty and `..` resolve to nothing; a
/// remainder row never names a path.
#[test]
fn a_child_row_is_one_plain_folder_under_its_unit() {
    let f = fx();
    let p = f.home.join("Library/Caches");
    make_dir(&p);
    let mut u = unit(StorageCategory::Unclassified, &p, 10);
    u.children = vec![
        child(ChildKind::Entry, "hiphi-endpoints", Some(7)),
        child(ChildKind::Remainder, "", Some(3)),
    ];
    let v = view(&[u]);
    let want = p.join("hiphi-endpoints");
    assert_eq!(
        row_path(
            p.to_str().unwrap(),
            Some((ChildKind::Entry, "hiphi-endpoints"))
        ),
        Some(want.clone())
    );
    let t = find_target(&v, &want).unwrap();
    assert!(t.is_folder && t.bytes == Some(7));
    for bad in ["../x", "a/b", "", "..", "."] {
        assert_eq!(
            row_path(p.to_str().unwrap(), Some((ChildKind::Entry, bad))),
            None,
            "{bad:?}"
        );
        assert!(
            !child_not_markable(ChildKind::Entry, bad).is_empty(),
            "{bad:?} has a reason"
        );
    }
    assert_eq!(
        row_path(p.to_str().unwrap(), Some((ChildKind::Remainder, ""))),
        None
    );
    assert!(child_not_markable(ChildKind::Remainder, "").contains("mark the unit"));
    // A path that is not a row of the view is not a target.
    assert!(find_target(&v, &f.home.join("elsewhere")).is_err());
}

/// Tempting wrong patch: an unmeasured folder is marked as zero bytes
/// without saying so.
#[test]
fn an_unmeasured_folder_says_its_size_is_not_measured() {
    let f = fx();
    let p = f.home.join("c");
    make_dir(&p);
    std::fs::create_dir_all(p.join("x")).unwrap();
    let mut u = unit(StorageCategory::Cache, &p, 5);
    u.children = vec![child(ChildKind::Entry, "x", None)];
    let v = view(&[u]);
    let t = find_target(&v, &p.join("x")).unwrap();
    assert_eq!(t.bytes, None);
    let r = review(&t, Some(&f.store), Some(&f.home)).unwrap();
    assert!(
        r.warnings[0].contains("size not measured"),
        "{:?}",
        r.warnings
    );
}

/// Tempting wrong patch: the symlink check runs only at mark time, so a
/// folder swapped for a symlink after the review moves the LINK (and
/// with a follow-the-link rename, the target's parent). Nothing moves,
/// the ledger stays empty, and the link's target is untouched.
#[test]
fn a_symlink_swapped_in_after_review_refuses_and_moves_nothing() {
    let f = fx();
    let p = f.home.join("cache");
    make_dir(&p);
    let (t, r, w) = reviewed(&f, StorageCategory::Cache, &p);
    let precious = f.home.join("precious");
    make_dir(&precious);
    std::fs::rename(&p, f.home.join("cache.aside")).unwrap();
    symlink(&precious, &p).unwrap();
    let err = trash(&r, &facts_of(&t, w), Some(&f.store), &ledger(&f), &f.trash).unwrap_err();
    assert!(err.contains("changed since review"), "{err}");
    assert!(err.contains("a symlink"), "{err}");
    assert!(
        std::fs::symlink_metadata(&p)
            .unwrap()
            .file_type()
            .is_symlink(),
        "link untouched"
    );
    assert!(precious.join("f").exists());
    assert!(
        ledger(&f).all().map(|r| r.is_empty()).unwrap_or(true),
        "no ledger row"
    );
    assert!(!f.trash.exists() || std::fs::read_dir(&f.trash).unwrap().next().is_none());
}

/// Tempting wrong patch: the recheck compares only the path and the
/// kind. A different folder renamed into the same place is a different
/// entry.
#[test]
fn a_different_folder_renamed_into_place_refuses() {
    let f = fx();
    let p = f.home.join("cache");
    make_dir(&p);
    let (t, r, w) = reviewed(&f, StorageCategory::Cache, &p);
    let other = f.home.join("other");
    make_dir(&other);
    std::fs::rename(&p, f.home.join("aside")).unwrap();
    std::fs::rename(&other, &p).unwrap();
    let err = trash(&r, &facts_of(&t, w), Some(&f.store), &ledger(&f), &f.trash).unwrap_err();
    assert!(err.contains("a different entry"), "{err}");
    assert!(p.join("f").exists());
}

/// Tempting wrong patch: the recheck compares the entry's identity but
/// not where its path resolves. The same folder reached through a parent
/// that became a symlink is at a different place than the one reviewed.
#[test]
fn a_path_that_now_resolves_elsewhere_refuses() {
    let f = fx();
    let parent = f.home.join("p");
    let p = parent.join("c");
    make_dir(&p);
    let (t, r, w) = reviewed(&f, StorageCategory::Cache, &p);
    let moved_parent = f.home.join("p2");
    std::fs::rename(&parent, &moved_parent).unwrap();
    symlink(&moved_parent, &parent).unwrap();
    let err = trash(&r, &facts_of(&t, w), Some(&f.store), &ledger(&f), &f.trash).unwrap_err();
    assert!(err.contains("now resolves to"), "{err}");
    assert!(moved_parent.join("c/f").exists());
}

/// Tempting wrong patch: a stale plan whose folder is gone moves
/// something else, or counts as done. It is refused as gone.
#[test]
fn a_folder_gone_since_review_refuses() {
    let f = fx();
    let p = f.home.join("cache");
    make_dir(&p);
    let (t, r, w) = reviewed(&f, StorageCategory::Cache, &p);
    std::fs::remove_dir_all(&p).unwrap();
    let err = trash(&r, &facts_of(&t, w), Some(&f.store), &ledger(&f), &f.trash).unwrap_err();
    assert!(err.contains("is gone"), "{err}");
}

/// Tempting wrong patch: protection is checked only at mark time, or
/// only for the target and not for what it contains. The person's mark
/// is respected at review, for a protected descendant, and again at the
/// move when it was added after the review; the refusal names the way to
/// take the mark off.
#[test]
fn the_persons_protect_mark_is_respected_at_review_and_at_the_move() {
    let f = fx();
    let p = f.home.join("cache");
    make_dir(&p);
    let inner = p.join("keep");
    make_dir(&inner);
    swamp_core::protection::protect_add(&f.store, &inner).unwrap();
    let v = view(&[unit(StorageCategory::Cache, &p, 5)]);
    let t = target_of(&v, &p);
    let err = review(&t, Some(&f.store), Some(&f.home)).unwrap_err();
    assert!(err.contains("protected by you"), "{err}");
    assert!(err.contains("swamp protect remove"), "{err}");
    swamp_core::protection::protect_remove(&f.store, &inner).unwrap();
    let r = review(&t, Some(&f.store), Some(&f.home)).unwrap();
    swamp_core::protection::protect_add(&f.store, &p).unwrap();
    let err = trash(
        &r.reviewed,
        &facts_of(&t, r.warnings),
        Some(&f.store),
        &ledger(&f),
        &f.trash,
    )
    .unwrap_err();
    assert!(err.contains("protected by you"), "{err}");
    assert!(p.join("f").exists(), "nothing moved");
}

/// Tempting wrong patch: an unreadable protect list (or no store at all)
/// is read as "nothing is protected".
#[test]
fn protect_marks_that_cannot_be_read_refuse() {
    let f = fx();
    let p = f.home.join("cache");
    make_dir(&p);
    let v = view(&[unit(StorageCategory::Cache, &p, 5)]);
    let t = target_of(&v, &p);
    assert!(
        review(&t, None, Some(&f.home))
            .unwrap_err()
            .contains("could not be checked")
    );
    std::fs::write(
        swamp_core::protection::protect_path(&f.store),
        b"not parquet",
    )
    .unwrap();
    let err = review(&t, Some(&f.store), Some(&f.home)).unwrap_err();
    assert!(err.contains("could not be read"), "{err}");
}

/// Tempting wrong patch: a path with `..` or a relative path is
/// normalized and then moved, so a stored row can walk out of where it
/// was shown. The path must name one place as written.
#[test]
fn a_path_with_dotdot_or_relative_is_not_a_target() {
    let f = fx();
    let real = f.home.join("a");
    make_dir(&real);
    let sneaky = f.home.join("a/../a");
    let v = view(&[unit(StorageCategory::Cache, &sneaky, 5)]);
    let err = review(&target_of(&v, &sneaky), Some(&f.store), Some(&f.home)).unwrap_err();
    assert!(err.contains("does not name one place"), "{err}");
    let rel = Path::new("relative/dir");
    let v = view(&[unit(StorageCategory::Cache, rel, 5)]);
    let err = review(&target_of(&v, rel), Some(&f.store), Some(&f.home)).unwrap_err();
    assert!(err.contains("not an absolute path"), "{err}");
}

/// Tempting wrong patch: a socket or a missing path gets a generic
/// "done"; the only refusal for a path is that it is not a real entry.
#[test]
fn only_a_real_entry_can_be_marked() {
    let f = fx();
    let missing = f.home.join("nope");
    let v = view(&[unit(StorageCategory::Cache, &missing, 5)]);
    let err = review(&target_of(&v, &missing), Some(&f.store), Some(&f.home)).unwrap_err();
    assert!(err.contains("is gone"), "{err}");
    let sock = f.home.join("s");
    let _l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    let v = view(&[unit(StorageCategory::Cache, &sock, 5)]);
    let err = review(&target_of(&v, &sock), Some(&f.store), Some(&f.home)).unwrap_err();
    assert!(err.contains("not a folder or a file"), "{err}");
}

/// Tempting wrong patch: a symlink row follows the link, moving (or
/// probing) the target. The link itself goes to Trash; the target stays.
#[test]
fn a_symlink_entry_moves_the_link_never_its_target() {
    let f = fx();
    let target = f.home.join("target");
    make_dir(&target);
    let link = f.home.join("link");
    symlink(&target, &link).unwrap();
    let v = view(&[unit(StorageCategory::Cache, &link, 0)]);
    let t = target_of(&v, &link);
    let r = review(&t, Some(&f.store), Some(&f.home)).unwrap();
    assert!(
        r.warnings[0].contains("only the link goes to Trash"),
        "{:?}",
        r.warnings
    );
    trash(
        &r.reviewed,
        &facts_of(&t, r.warnings),
        Some(&f.store),
        &ledger(&f),
        &f.trash,
    )
    .unwrap();
    assert!(std::fs::symlink_metadata(&link).is_err(), "link moved");
    assert!(target.join("f").exists(), "target untouched");
}

/// Tempting wrong patch: a path outside the home is refused by location.
/// It is allowed with a line; only the OS can refuse it.
#[test]
fn outside_home_is_a_line_not_a_refusal() {
    let f = fx();
    let outside = std::fs::canonicalize(f._tmp.path())
        .unwrap()
        .join("elsewhere");
    make_dir(&outside);
    let v = view(&[unit(StorageCategory::Cache, &outside, 5)]);
    let t = target_of(&v, &outside);
    let r = review(&t, Some(&f.store), Some(&f.home)).unwrap();
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("outside your home folder"))
    );
}

/// Tempting wrong patch: Claude session scratch is refused. It is allowed
/// with the plain consequence for a running session.
#[test]
fn claude_scratch_warns_about_running_sessions() {
    let f = fx();
    let scratch = Path::new("/private/tmp/claude-502-g6-test-nonexistent");
    let mut t = {
        let p = f.home.join("x");
        make_dir(&p);
        let v = view(&[unit(StorageCategory::Unclassified, &p, 5)]);
        target_of(&v, &p)
    };
    t.path = scratch.to_path_buf();
    let w = swamp_core::reclaim_trash::warnings_for(&t, "", Some(&f.home));
    assert!(w.iter().any(|l| l.contains("breaks when it goes")), "{w:?}");
}

/// Tempting wrong patch: the move is written to the ledger only after it
/// happens, so a ledger that cannot be written leaves a moved folder
/// with no record. A started row must be writable first; otherwise
/// nothing moves.
#[test]
fn a_ledger_that_cannot_take_the_started_row_means_nothing_moves() {
    use std::os::unix::fs::PermissionsExt;
    let f = fx();
    let p = f.home.join("cache");
    make_dir(&p);
    let (t, r, w) = reviewed(&f, StorageCategory::Cache, &p);
    std::fs::set_permissions(&f.store, std::fs::Permissions::from_mode(0o500)).unwrap();
    let res = trash(&r, &facts_of(&t, w), Some(&f.store), &ledger(&f), &f.trash);
    std::fs::set_permissions(&f.store, std::fs::Permissions::from_mode(0o700)).unwrap();
    let err = res.unwrap_err();
    assert!(err.contains("could not write its ledger"), "{err}");
    assert!(err.contains("nothing moved"), "{err}");
    assert!(p.join("f").exists(), "the folder is where it was");
    assert!(!f.trash.exists() || std::fs::read_dir(&f.trash).unwrap().next().is_none());
}

/// Tempting wrong patch: a second row is appended for the final outcome,
/// leaving a `started` row that reads as an unfinished move forever. The
/// started row is replaced by the final one, which names the Trash
/// location.
#[test]
fn one_ledger_row_ends_completed_with_the_way_back() {
    let f = fx();
    let p = f.home.join("cache");
    make_dir(&p);
    let (t, r, w) = reviewed(&f, StorageCategory::Cache, &p);
    let dest = trash(&r, &facts_of(&t, w), Some(&f.store), &ledger(&f), &f.trash).unwrap();
    let rows = ledger(&f).all().unwrap();
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(rows[0].outcome, "completed");
    assert_eq!(rows[0].recovery_location.as_deref(), Some(dest.as_path()));
    assert!(rows[0].evidence.iter().any(|e| e.key == "warnings_shown"));
}

/// Tempting wrong patch: `recheck` takes the lock the observer holds (or
/// any shared lock), so a move while another swamp instance observes
/// blocks or fails. The move touches the ledger and the folder, not the
/// observe lock.
#[test]
fn a_move_while_another_instance_holds_the_observe_lock_still_moves() {
    let f = fx();
    let p = f.home.join("cache");
    make_dir(&p);
    let (t, r, w) = reviewed(&f, StorageCategory::Cache, &p);
    // Another instance mid-observation: its lock file is held for the
    // whole move.
    let lock = f.store.join("observe.lock");
    std::fs::write(&lock, b"pid 1").unwrap();
    let hold = std::fs::File::open(&lock).unwrap();
    let res = trash(&r, &facts_of(&t, w), Some(&f.store), &ledger(&f), &f.trash);
    drop(hold);
    assert!(res.is_ok(), "{res:?}");
    assert!(!p.exists());
}

/// Tempting wrong patch: a huge list is truncated, or reviewed with one
/// `lsof` per folder (minutes). Under one scoped open-file snapshot,
/// reviewing 500 folders is 500 exact reviews.
#[test]
fn a_huge_list_reviews_every_folder() {
    let f = fx();
    let base = f.home.join("Library/Caches");
    std::fs::create_dir_all(&base).unwrap();
    let mut u = unit(StorageCategory::Unclassified, &base, 500);
    u.children = (0..500)
        .map(|i| {
            std::fs::create_dir_all(base.join(format!("d{i}"))).unwrap();
            child(ChildKind::Entry, &format!("d{i}"), Some(1))
        })
        .collect();
    let v = view(&[u]);
    let n = swamp_core::occupancy::OccupancySnapshot::scoped(|| {
        let mut n = 0;
        for i in 0..500 {
            let p = base.join(format!("d{i}"));
            let t = find_target(&v, &p).unwrap();
            let r = review(&t, Some(&f.store), Some(&f.home)).unwrap();
            recheck(&r.reviewed, Some(&f.store)).unwrap();
            n += 1;
        }
        n
    });
    assert_eq!(n, 500);
}

// ---------------------------------------------------------------------
// Nested build units: an adapter that has no cleanup rule for a folder
// inside a checkout no longer makes it unplannable.
// ---------------------------------------------------------------------

fn project_with_target(f: &Fx) -> (swamp_core::report::Report, PathBuf) {
    use swamp_core::entities::Confidence;
    use swamp_core::report::{
        ArtifactKind, ArtifactRow, ProjectRow, Report, Source, WorktreeKind, WorktreeRow,
    };
    let wt = f.home.join("proj");
    let target = wt.join("target");
    make_dir(&target);
    let mut report = Report::empty(f.home.clone());
    report.observed_at = NOW;
    report.projects.push(ProjectRow {
        project_id: "p".into(),
        name: "proj".into(),
        remote: None,
        ecosystems: Vec::new(),
        worktrees: vec![WorktreeRow {
            worktree_id: "w".into(),
            path: wt.clone(),
            kind: WorktreeKind::Main,
            artifacts: vec![ArtifactRow {
                kind: ArtifactKind::BuildOutput,
                path: target.clone(),
                bytes: 100,
                mtime_max: 0,
                ecosystem: None,
                hardlinked: false,
                dedup_stale: false,
                allocated_bytes: None,
                allocated_growth_bytes: None,
                local_bytes: 0,
                track: None,
                growth_bytes: None,
                regrowth_count: 0,
                observed_at: NOW,
                confidence: Confidence::High,
                source: Source::new("fixture"),
                note: None,
                created_at: None,
                containers: Vec::new(),
                shared_with: Vec::new(),
                dangling: false,
                evidence: Vec::new(),
            }],
            signals: Vec::new(),
            branch: None,
            github: None,
            merge_complete: None,
            idle_secs: None,
        }],
    });
    (report, target)
}

/// Tempting wrong patch: a nested unit an adapter gave no cleanup rule
/// ("inspection only", or "unsupported: shared") is refused by propose.
/// It plans as that exact path, verb delete, and its confirm carries the
/// adapter's reason, what it could not read, and a lock warning.
#[test]
fn a_nested_unit_with_no_cleanup_rule_plans_as_its_exact_path_with_the_reason() {
    use swamp_core::artifact::{AccountingBasis, ArtifactRole};
    use swamp_core::build_adapters::{BuildContainer, NestedUnitBuilder};
    let f = fx();
    let (mut report, target) = project_with_target(&f);
    let wt = f.home.join("proj");
    let c = BuildContainer::project("cargo", target.clone(), wt);
    let shared = target.join("debug/deps/libshared");
    make_dir(&shared);
    let inspect = target.join("debug/deps/libinspect");
    make_dir(&inspect);
    report.nested_artifacts = vec![
        NestedUnitBuilder::new(&c, ArtifactRole::Dependency, shared.clone())
            .is_dir(true)
            .bytes_on_basis(10, AccountingBasis::Allocated)
            .no_action_because("shared by other projects")
            .limit("the walk could not read all of this directory")
            .build(),
        NestedUnitBuilder::new(&c, ArtifactRole::Dependency, inspect.clone())
            .is_dir(true)
            .bytes_on_basis(10, AccountingBasis::Allocated)
            .build(),
    ];
    for (p, needle) in [
        (&shared, "shared by other projects"),
        (&shared, "could not read all of this directory"),
        (&inspect, "has no cleanup rule"),
    ] {
        let units = swamp_core::actions::propose(&report, None, std::slice::from_ref(p), "t")
            .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        assert_eq!(units[0].path(), p.as_path());
        assert_eq!(units[0].verb(), "delete");
        let w = units[0].warnings().join("\n");
        assert!(w.contains(needle), "{needle}: {w}");
    }
    // A member that is not on disk is not a path: refused, never planned
    // as a no-op (the plan must not mask a missing member).
    let gone = target.join("debug/deps/libgone");
    let mut missing = report.nested_artifacts[1].clone();
    missing.path = gone.clone();
    report.nested_artifacts.push(missing);
    let err = swamp_core::actions::propose(&report, None, &[gone], "t").unwrap_err();
    assert!(err.to_string().contains("gone"), "{err}");
}
