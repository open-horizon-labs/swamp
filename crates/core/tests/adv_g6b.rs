//! v0.8.0 G6 independent verification (core half): what the first audit
//! did not try. Real temp directories, a fake Trash root and a temp store;
//! nothing here touches the developer's real caches, Trash or store.
//!
//! Each test names the tempting wrong patch it fails.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

use swamp_core::actions::{ReclaimMoveFacts as MoveFacts, trash_reclaim as trash};
use swamp_core::fs_gate::StoreDir;
use swamp_core::ledger::Ledger;
use swamp_core::reclaim_trash::{ReclaimTarget, review};

const NOW: u64 = 1_790_000_000;

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
    let store = root.join("store");
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
    ReclaimTarget::for_path(path.to_path_buf(), "cache", Some(5), None, Vec::new())
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

fn move_it(f: &Fx, p: &Path) -> Result<PathBuf, String> {
    let t = target(p);
    let r = review(&t, Some(&f.store), Some(&f.home))?;
    trash(
        &r.reviewed,
        &facts(&t, r.warnings),
        Some(&f.store),
        &ledger(&f),
        &f.trash,
    )
}

fn outcomes(f: &Fx) -> Vec<String> {
    ledger(f)
        .all()
        .unwrap_or_default()
        .into_iter()
        .map(|r| r.outcome)
        .collect()
}

/// Tempting wrong patch: the unit path is used as stored. A record that
/// spells a symlink with a trailing slash (`.../link/`) makes lstat and
/// rename follow the link on macOS, so the review never says "symlink"
/// and the rename moves the TARGET directory into Trash, leaving the link
/// dangling. The link may move; what it points to must stay put.
#[test]
fn adv_b_a_trailing_slash_on_a_symlink_never_moves_the_target() {
    let f = fx();
    let real = f.home.join("real-cache");
    make_dir(&real);
    let link = f.home.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let spelled = PathBuf::from(format!("{}/", link.display()));
    let res = move_it(&f, &spelled);
    assert!(
        real.join("f").exists(),
        "the symlink's target was moved to Trash (result {res:?}); the link is {}",
        if link.symlink_metadata().is_ok() {
            "still there, now dangling"
        } else {
            "gone too"
        }
    );
    assert!(
        !outcomes(&f).iter().any(|o| o == "started"),
        "{:?}",
        outcomes(&f)
    );
}

/// Tempting wrong patch: the same, spelled `.../dir/.`. Rust's components
/// drop the `.`, so the dot-segment check passes; the rename is then
/// refused by the OS after the started row. Nothing may move, and the
/// ledger must not keep a started row.
#[test]
fn adv_b_a_dot_segment_path_moves_nothing_and_leaves_no_started_row() {
    let f = fx();
    let real = f.home.join("cache");
    make_dir(&real);
    let spelled = PathBuf::from(format!("{}/.", real.display()));
    let _ = move_it(&f, &spelled);
    assert!(real.join("f").exists(), "the folder moved");
    assert!(
        !outcomes(&f).iter().any(|o| o == "started"),
        "{:?}",
        outcomes(&f)
    );
}

/// Tempting wrong patch: a unit that is itself a symlink to a folder is
/// resolved first, so the target folder goes to Trash. Only the link moves;
/// the confirm says so; the target and its content are untouched.
#[test]
fn adv_b_a_symlink_unit_moves_the_link_and_never_the_target() {
    let f = fx();
    let real = f.home.join("real");
    make_dir(&real);
    let link = f.home.join("cache-link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let t = target(&link);
    let r = review(&t, Some(&f.store), Some(&f.home)).unwrap();
    assert!(
        r.warnings.iter().any(|w| w.contains("symlink")),
        "{:#?}",
        r.warnings
    );
    let dest = trash(
        &r.reviewed,
        &facts(&t, r.warnings),
        Some(&f.store),
        &ledger(&f),
        &f.trash,
    )
    .unwrap();
    assert!(real.join("f").exists(), "the target moved");
    assert!(link.symlink_metadata().is_err(), "the link stayed");
    assert!(
        dest.symlink_metadata().unwrap().file_type().is_symlink(),
        "what reached Trash is not the link"
    );
}

/// Tempting wrong patch: a folder already in Trash with the destination's
/// own name (or its `<name>-<second>` spelling) is overwritten or makes the
/// move fail. It stays byte-for-byte, and the new move lands beside it.
#[test]
fn adv_b_a_same_named_entry_already_in_trash_is_never_touched() {
    let f = fx();
    let p = f.home.join("cache");
    make_dir(&p);
    let items = f.trash.clone();
    std::fs::create_dir_all(&items).unwrap();
    let now = swamp_core::entities::now();
    let mut existing = Vec::new();
    for name in [
        "cache".to_string(),
        format!("cache-{now}"),
        format!("cache-{}", now + 1),
    ] {
        let e = items.join(&name);
        std::fs::create_dir_all(&e).unwrap();
        std::fs::write(e.join("keep"), name.as_bytes()).unwrap();
        existing.push((e, name));
    }
    let dest = move_it(&f, &p).unwrap();
    for (e, name) in &existing {
        assert_ne!(&dest, e);
        assert_eq!(std::fs::read(e.join("keep")).unwrap(), name.as_bytes());
        assert!(!e.join("f").exists(), "{} was merged into", e.display());
    }
    assert!(dest.join("f").exists());
}

/// Tempting wrong patch: the uniqueness suffix comes from the clock. Five
/// siblings with one basename moved in the same second all land, each in
/// its own Trash entry, with one final ledger row each.
#[test]
fn adv_b_same_basename_siblings_moved_in_one_second_all_land_apart() {
    let f = fx();
    let mut dests = Vec::new();
    for i in 0..5 {
        let p = f.home.join(format!("p{i}/node_modules"));
        make_dir(&p);
        dests.push(move_it(&f, &p).unwrap());
    }
    let mut uniq = dests.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), 5, "{dests:#?}");
    let o = outcomes(&f);
    assert_eq!(o.len(), 5, "{o:?}");
    assert!(o.iter().all(|o| o == "completed"), "{o:?}");
}

/// Tempting wrong patch: string prefix instead of path components. A
/// store at `.../swamp-preview` does not make `.../swamp` "hold swamp's
/// ledger", a keep mark on `cache` does not cover `cache2`, and a file open
/// in `cache2` does not make `cache` "in use".
#[test]
fn adv_b_a_name_that_is_a_prefix_of_another_is_its_own_folder() {
    let f = fx();
    let share = f.home.join(".local/share");
    let preview = share.join("swamp-preview");
    make_dir(&preview);
    let sibling = share.join("swamp");
    make_dir(&sibling);
    let t = target(&sibling);
    let r = review(&t, Some(&preview), Some(&f.home));
    assert!(
        r.is_ok(),
        "a sibling whose name is a prefix of the store's is refused: {r:?}"
    );

    let cache = f.home.join("cache");
    let cache2 = f.home.join("cache2");
    make_dir(&cache);
    make_dir(&cache2);
    swamp_core::protection::protect_add(&f.store, &cache).unwrap();
    let r2 = review(&target(&cache2), Some(&f.store), Some(&f.home));
    assert!(r2.is_ok(), "a keep mark on cache covers cache2: {r2:?}");
    assert!(review(&target(&cache), Some(&f.store), Some(&f.home)).is_err());

    let other = f.home.join("busy");
    let other2 = f.home.join("busy2");
    make_dir(&other);
    make_dir(&other2);
    let _held = std::fs::File::open(other2.join("f")).unwrap();
    let r3 = review(&target(&other), Some(&f.store), Some(&f.home)).unwrap();
    assert!(
        !r3.warnings.iter().any(|w| w.contains("in use right now")),
        "a file open in busy2 makes busy read as in use: {:#?}",
        r3.warnings
    );
}

/// Tempting wrong patch: the recheck compares the entry's own identity
/// only by inode, which a rename of the PARENT keeps. The marked path no
/// longer names anything (the parent was renamed after the mark): nothing
/// moves, the renamed folder is untouched, and no started row is left.
#[test]
fn adv_b_a_parent_renamed_after_the_mark_moves_nothing() {
    let f = fx();
    let parent = f.home.join("proj");
    let p = parent.join("cache");
    make_dir(&p);
    let t = target(&p);
    let r = review(&t, Some(&f.store), Some(&f.home)).unwrap();
    let renamed = f.home.join("proj-renamed");
    std::fs::rename(&parent, &renamed).unwrap();
    // Something new now sits at the old parent's name: still not the entry.
    make_dir(&p);
    let res = trash(
        &r.reviewed,
        &facts(&t, r.warnings),
        Some(&f.store),
        &ledger(&f),
        &f.trash,
    );
    assert!(res.is_err(), "moved after the parent was renamed: {res:?}");
    assert!(renamed.join("cache/f").exists() && p.join("f").exists());
    assert!(outcomes(&f).is_empty(), "{:?}", outcomes(&f));
}

/// Tempting wrong patch: two swamp instances both pass the recheck and
/// both rename; or the ledger's read-modify-write loses one row. Exactly
/// one move happens, the entry is in Trash once, and the ledger ends with
/// a final row for EACH attempt (one moved, one failed), no `started`.
#[test]
fn adv_b_two_instances_racing_one_move_never_double_move_or_lose_a_row() {
    for round in 0..8 {
        let f = Arc::new(fx());
        let p = f.home.join(format!("race{round}"));
        make_dir(&p);
        let t = target(&p);
        let r = review(&t, Some(&f.store), Some(&f.home)).unwrap();
        let gate = Arc::new(Barrier::new(2));
        let hands: Vec<_> = (0..2)
            .map(|_| {
                let (f, t, r, gate) = (f.clone(), t.clone(), r.clone(), gate.clone());
                std::thread::spawn(move || {
                    gate.wait();
                    trash(
                        &r.reviewed,
                        &facts(&t, r.warnings.clone()),
                        Some(&f.store),
                        &ledger(&f),
                        &f.trash,
                    )
                })
            })
            .collect();
        let res: Vec<_> = hands.into_iter().map(|h| h.join().unwrap()).collect();
        let ok = res.iter().filter(|r| r.is_ok()).count();
        assert_eq!(ok, 1, "round {round}: {res:?}");
        let in_trash = std::fs::read_dir(&f.trash).map(|d| d.count()).unwrap_or(0);
        assert_eq!(in_trash, 1, "round {round}");
        let o = outcomes(&f);
        assert!(!o.iter().any(|o| o == "started"), "round {round}: {o:?}");
        assert_eq!(o.len(), 2, "round {round}: a ledger row was lost: {o:?}");
    }
}

/// Tempting wrong patch: the agent-facing plan was relaxed along with the
/// person's. An AI agent's `propose` for a Reclaim/External path, and its
/// `propose_agents` for a unit swamp keeps by default or has no rule for,
/// still refuse: the human-only rule for the agent path is unchanged.
#[test]
fn adv_b_the_agent_plan_still_refuses_the_audits_rows() {
    use swamp_core::agents::{AgentActionCapability as Act, AgentCategory as Cat};
    let f = fx();
    let ext = f.home.join("Library/Caches/Homebrew");
    make_dir(&ext);
    let report = swamp_core::report::Report::empty(f.root.clone());
    let err = swamp_core::actions::propose(&report, None, std::slice::from_ref(&ext), "agent")
        .unwrap_err()
        .to_string();
    assert!(err.contains("not plannable"), "{err}");

    let home = f.home.join(".claude");
    let cred = home.join(".credentials.json");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(&cred, b"x").unwrap();
    let todos = home.join("todos");
    make_dir(&todos);
    let mk = |cat: Cat, act: Act, path: &Path, protected: bool| swamp_core::agents::AgentUnit {
        tool_id: "claude-code".into(),
        tool_name: "Claude Code".into(),
        tool_home: home.clone(),
        category: cat,
        id: swamp_core::agents::unit_id(
            "claude-code",
            cat,
            &path.file_name().unwrap().to_string_lossy(),
        ),
        relative_path: path.file_name().unwrap().to_string_lossy().into(),
        path: path.to_path_buf(),
        members: Vec::new(),
        bytes: 1,
        hardlinked: false,
        complete: true,
        growth_bytes: None,
        regrowth_count: 0,
        observed_at: NOW,
        mtime_max: NOW,
        protected,
        protect_reason: protected.then(|| "credentials".into()),
        project_link: swamp_core::agents::ProjectLinkState::NotApplicable,
        action: act,
        note: None,
        evidence: Vec::new(),
    };
    let units = vec![
        mk(Cat::ProtectedConfig, Act::None, &cred, true),
        mk(Cat::Caches, Act::None, &todos, false),
    ];
    for u in &units {
        let e = swamp_core::actions::propose_agents(&units, std::slice::from_ref(&u.path), "agent");
        assert!(
            e.is_err(),
            "{}: agent plan accepted: {e:?}",
            u.path.display()
        );
        let h = swamp_core::actions::propose_agents_for_human(
            &units,
            std::slice::from_ref(&u.path),
            "human:tui",
        );
        assert!(
            h.is_ok(),
            "{}: the person's plan refused: {h:?}",
            u.path.display()
        );
    }
}

/// Tempting wrong patch: the ledger's append reads the table, adds a row
/// and rewrites it with no lock across processes. Two swamp instances
/// moving DIFFERENT folders at the same moment: both folders move, and
/// the ledger must keep both final rows (the record of where each went).
#[test]
fn adv_b_two_instances_moving_different_folders_keep_both_ledger_rows() {
    for round in 0..8 {
        let f = Arc::new(fx());
        let gate = Arc::new(Barrier::new(2));
        let hands: Vec<_> = (0..2)
            .map(|i| {
                let p = f.home.join(format!("r{round}-{i}"));
                make_dir(&p);
                let t = target(&p);
                let r = review(&t, Some(&f.store), Some(&f.home)).unwrap();
                let (f, gate) = (f.clone(), gate.clone());
                std::thread::spawn(move || {
                    gate.wait();
                    trash(
                        &r.reviewed,
                        &facts(&t, r.warnings.clone()),
                        Some(&f.store),
                        &ledger(&f),
                        &f.trash,
                    )
                })
            })
            .collect();
        let res: Vec<_> = hands.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(res.iter().all(|r| r.is_ok()), "round {round}: {res:?}");
        let o = outcomes(&f);
        assert_eq!(
            o.iter().filter(|o| *o == "completed").count(),
            2,
            "round {round}: two folders moved, the ledger records {o:?}"
        );
    }
}
