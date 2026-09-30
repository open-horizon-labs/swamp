//! v0.8.0 G6 adversarial audit (core half): Reclaim to Trash. Real temp
//! directories, a fake Trash root and a temp store; nothing here touches
//! the developer's real caches, Trash or store.
//!
//! Each test names the tempting wrong patch it fails.

use std::path::{Path, PathBuf};

use swamp_core::actions::{ReclaimMoveFacts as MoveFacts, trash_reclaim as trash};
use swamp_core::external::ExternalUnit;
use swamp_core::fs_gate::StoreDir;
use swamp_core::last_used::LastUsed;
use swamp_core::ledger::Ledger;
use swamp_core::locations::{Provenance, StorageCategory};
use swamp_core::manager_facts::ManagerFacts;
use swamp_core::reclaim::{ReclaimInput, ReclaimView, build};
use swamp_core::reclaim_trash::{ReclaimTarget, find_target, review};

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
    root: PathBuf,
    home: PathBuf,
    store: PathBuf,
    trash: PathBuf,
}

fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("home");
    let store = home.join(".local/share/swamp");
    let trash = root.join("trash");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&store).unwrap();
    Fx {
        _tmp: tmp,
        root,
        home,
        store,
        trash,
    }
}

fn ledger(f: &Fx) -> Ledger {
    Ledger::resolved(&StoreDir::at(&f.store).unwrap())
}

fn make_dir(p: &Path) {
    std::fs::create_dir_all(p).unwrap();
    std::fs::write(p.join("f"), b"hello").unwrap();
}

fn target(path: &Path) -> ReclaimTarget {
    find_target(&view(&[unit(StorageCategory::Cache, path, 5)]), path).unwrap()
}

fn facts(t: &ReclaimTarget, warnings: Vec<String>) -> MoveFacts {
    MoveFacts {
        label: t.path.display().to_string(),
        bytes: t.bytes.unwrap_or(0),
        observed_at: NOW,
        warnings,
        category: t.category.clone(),
    }
}

/// Tempting wrong patch: the refusal names `swamp protect remove <the
/// row's path>` whatever the mark is on. When the mark is on an ancestor
/// (or a descendant), that command matches no entry and silently does
/// nothing, so the person follows the instruction and is refused again.
/// The command named must be the one that takes the covering mark off.
#[test]
fn adv_the_protect_remove_named_in_the_refusal_takes_an_ancestor_mark_off() {
    let f = fx();
    let parent = f.home.join("keep");
    let p = parent.join("cache");
    make_dir(&p);
    swamp_core::protection::protect_add(&f.store, &parent).unwrap();
    let t = target(&p);
    let err = review(&t, Some(&f.store), Some(&f.home)).unwrap_err();
    // Run exactly what the text tells the person to run.
    let named = err
        .split("`swamp protect remove ")
        .nth(1)
        .and_then(|s| s.split('`').next())
        .unwrap_or_else(|| panic!("no command named: {err}"));
    swamp_core::protection::protect_remove(&f.store, Path::new(named)).unwrap();
    let again = review(&t, Some(&f.store), Some(&f.home));
    assert!(
        again.is_ok(),
        "followed the refusal's own instruction (`swamp protect remove {named}`) and was refused again: {:?}",
        again.err()
    );
}

/// Same, for a mark on a descendant.
#[test]
fn adv_the_protect_remove_named_in_the_refusal_takes_a_descendant_mark_off() {
    let f = fx();
    let p = f.home.join("cache");
    let inner = p.join("keep");
    make_dir(&inner);
    swamp_core::protection::protect_add(&f.store, &inner).unwrap();
    let t = target(&p);
    let err = review(&t, Some(&f.store), Some(&f.home)).unwrap_err();
    let named = err
        .split("`swamp protect remove ")
        .nth(1)
        .and_then(|s| s.split('`').next())
        .unwrap_or_else(|| panic!("no command named: {err}"));
    swamp_core::protection::protect_remove(&f.store, Path::new(named)).unwrap();
    let again = review(&t, Some(&f.store), Some(&f.home));
    assert!(
        again.is_ok(),
        "followed `swamp protect remove {named}` and was refused again: {:?}",
        again.err()
    );
}

/// Tempting wrong patch: only paths outside home get a location line, so
/// a row whose path IS the home folder reviews with nothing said about
/// that (silence is "fine"). The plan must state it.
#[test]
fn adv_the_home_folder_itself_is_named_on_the_confirm() {
    let f = fx();
    let r = review(&target(&f.home), Some(&f.store), Some(&f.home)).unwrap();
    assert!(
        r.warnings.iter().any(|w| w.contains("home folder")),
        "a mark on the home folder itself says nothing about it: {:#?}",
        r.warnings
    );
}

/// Tempting wrong patch: `!path.starts_with(home)` means "outside home",
/// so the parent of home (like /Users) is described as outside the home
/// folder when it contains it. The plan must say it contains the home
/// folder.
#[test]
fn adv_a_folder_that_contains_home_is_not_called_outside_it() {
    let f = fx();
    let r = review(
        &target(&f.root),
        Some(&f.root.join("nostore")),
        Some(&f.home),
    )
    .unwrap();
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("contains") && w.contains("home")),
        "a folder holding the whole home folder is not named as such: {:#?}",
        r.warnings
    );
}

/// Tempting wrong patch: the store is just another path. Marking swamp's
/// own store (or a folder holding it) moves the ledger the move is
/// recorded in; the plan must say so.
#[test]
fn adv_swamps_own_store_is_named_on_the_confirm() {
    let f = fx();
    for p in [f.store.clone(), f.home.join(".local/share")] {
        let r = review(&target(&p), Some(&f.store), Some(&f.home)).unwrap();
        assert!(
            r.warnings
                .iter()
                .any(|w| w.contains("swamp") && (w.contains("store") || w.contains("ledger"))),
            "{}: marking swamp's own store says nothing about it: {:#?}",
            p.display(),
            r.warnings
        );
    }
}

/// Tempting wrong patch: a Reclaim unit is never a checkout, so nothing
/// looks for `.git`. A marked folder that is (or contains) a git checkout
/// takes its history and unpushed work to Trash; the plan must say so.
#[test]
fn adv_a_git_checkout_is_named_on_the_confirm() {
    let f = fx();
    let p = f.home.join("src/app");
    make_dir(&p.join(".git"));
    for t in [p.clone(), f.home.join("src")] {
        let r = review(&target(&t), Some(&f.store), Some(&f.home)).unwrap();
        assert!(
            r.warnings.iter().any(|w| w.contains("git")),
            "{}: a checkout goes to Trash with nothing said: {:#?}",
            t.display(),
            r.warnings
        );
    }
}

/// Tempting wrong patch: the stored `hardlinked` flag is trusted, so a
/// file with other links reviews as ordinary bytes. Moving one link to
/// Trash frees nothing even when Trash is emptied; the plan must say so.
#[test]
fn adv_a_hardlinked_file_says_its_bytes_stay() {
    let f = fx();
    let dir = f.home.join("farm");
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("blob");
    std::fs::write(&a, vec![7u8; 8192]).unwrap();
    std::fs::hard_link(&a, f.home.join("other-link")).unwrap();
    let r = review(&target(&a), Some(&f.store), Some(&f.home)).unwrap();
    assert!(
        r.warnings.iter().any(|w| w.contains("link")),
        "a file with 2 links reviews as if Trash would free it: {:#?}",
        r.warnings
    );
}

/// Tempting wrong patch: the started row is written, the move fails, the
/// error is returned, and the started row stays forever, reading as a
/// move in progress. A failed move must leave a final row that is not
/// `started`.
#[test]
fn adv_a_failed_move_does_not_leave_a_started_row_forever() {
    let f = fx();
    let p = f.home.join("cache");
    make_dir(&p);
    let t = target(&p);
    let r = review(&t, Some(&f.store), Some(&f.home)).unwrap();
    // A Trash root that is a plain file: the move cannot happen.
    std::fs::write(&f.trash, b"not a folder").unwrap();
    let err = trash(
        &r.reviewed,
        &facts(&t, r.warnings),
        Some(&f.store),
        &ledger(&f),
        &f.trash,
    )
    .unwrap_err();
    assert!(p.join("f").exists(), "{err}");
    let rows = ledger(&f).all().unwrap();
    assert!(
        rows.iter().all(|row| row.outcome != "started"),
        "the move failed ({err}) and the ledger still says it started: {:#?}",
        rows.iter().map(|r| &r.outcome).collect::<Vec<_>>()
    );
}

/// Tempting wrong patch: the Trash name is `<basename>-<seconds>`, so two
/// marked folders with the same name moved in one confirm (the usual
/// case: `cache`, `target`, `node_modules`) collide, and the second is
/// refused as "already exists in the Trash" though nothing is wrong with
/// it.
#[test]
fn adv_two_folders_with_the_same_name_both_move_in_one_confirm() {
    let f = fx();
    let a = f.home.join("a/cache");
    let b = f.home.join("b/cache");
    make_dir(&a);
    make_dir(&b);
    for p in [&a, &b] {
        let t = target(p);
        let r = review(&t, Some(&f.store), Some(&f.home)).unwrap();
        let res = trash(
            &r.reviewed,
            &facts(&t, r.warnings),
            Some(&f.store),
            &ledger(&f),
            &f.trash,
        );
        assert!(res.is_ok(), "{}: {:?}", p.display(), res.err());
    }
}

/// Tempting wrong patch: the recheck compares identity only, so a folder
/// that a process opened after the review moves with the confirm still
/// saying nothing held it. (Tool-managed removal refuses when its warnings
/// changed since review; the Trash move must at least not be silent.)
#[test]
fn adv_a_folder_opened_after_review_does_not_move_silently() {
    let f = fx();
    let p = f.home.join("cache");
    make_dir(&p);
    let t = target(&p);
    let r = review(&t, Some(&f.store), Some(&f.home)).unwrap();
    assert!(
        !r.warnings.iter().any(|w| w.contains("in use right now")),
        "precondition: nothing held it at review: {:#?}",
        r.warnings
    );
    let _held = std::fs::File::open(p.join("f")).unwrap();
    let res = trash(
        &r.reviewed,
        &facts(&t, r.warnings),
        Some(&f.store),
        &ledger(&f),
        &f.trash,
    );
    assert!(
        res.is_err(),
        "a folder held open since the review moved with no new word: {:?}",
        res
    );
}
