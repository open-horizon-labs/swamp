//! v0.8.0 G6: what the person sees in Reclaim and External, they may move
//! to Trash (maintainer, 2026-09-30). Real temp directories, a fake Trash
//! root and a temp store; nothing here touches the developer's real
//! caches, Trash, store or managers.
//!
//! Each test names the tempting wrong patch it fails.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::KeyCode;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use swamp_core::drilldown::{ChildKind, ChildMeasure, UnitChild};
use swamp_core::external::ExternalUnit;
use swamp_core::last_used::LastUsed;
use swamp_core::locations::{Provenance, StorageCategory};
use swamp_tui::actions::execute_plan;
use swamp_tui::app::{App, ViewKind};
use swamp_tui::{handle_key, ui};

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    store: PathBuf,
    trash: PathBuf,
}

fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let store = root.join("store");
    std::fs::create_dir_all(&store).unwrap();
    Fx {
        trash: root.join("trash"),
        store,
        root,
        _tmp: tmp,
    }
}

fn dir(p: &Path) {
    std::fs::create_dir_all(p).unwrap();
    std::fs::write(p.join("f"), b"hello").unwrap();
}

fn entry(name: &str, bytes: Option<i64>) -> UnitChild {
    child(ChildKind::Entry, name, bytes)
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

fn unit(cat: StorageCategory, path: &Path, bytes: u64, children: Vec<UnitChild>) -> ExternalUnit {
    ExternalUnit {
        detector_id: "fixture".into(),
        detector_name: "fixture".into(),
        category: cat,
        provenance: Provenance::BuiltinConvention,
        path: path.to_path_buf(),
        bytes,
        mtime_max: 0,
        hardlinked: false,
        growth_bytes: None,
        regrowth_count: 0,
        observed_at: 1_000,
        consumers: Vec::new(),
        note: None,
        evidence: Vec::new(),
        bytes_counted_elsewhere: 0,
        overlap_count: 0,
        last_used: LastUsed::default(),
        children,
    }
}

fn app_with(f: &Fx, units: Vec<ExternalUnit>, view: ViewKind) -> App {
    let mut report = swamp_core::report::Report::empty(f.root.clone());
    report.observed_at = 1_000;
    let mut a = App::new(report, f.root.clone());
    a.set_external_units(units);
    a.store_dir = Some(f.store.clone());
    a.views_seen = true;
    a.set_view(view);
    a
}

fn wait(a: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while a.operation.is_some() {
        assert!(Instant::now() < deadline, "operation did not finish");
        a.poll_operation();
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn select(a: &mut App, needle: &str) {
    let rows = a.rows();
    let at = rows
        .iter()
        .position(|r| r.label.trim() == needle)
        .or_else(|| rows.iter().position(|r| r.label.contains(needle)))
        .unwrap_or_else(|| panic!("no row with {needle:?}"));
    a.selected = at;
}

fn frame(a: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, a)).unwrap();
    t.backend().to_string()
}

fn space(a: &mut App) {
    handle_key(a, KeyCode::Char(' '));
    wait(a);
}

fn run_plan(a: &App, f: &Fx) -> Vec<swamp_tui::actions::UnitResult> {
    let units: Vec<_> = a.marked.values().cloned().collect();
    let store = swamp_core::fs_gate::StoreDir::at(&f.store).unwrap();
    let ledger = swamp_core::ledger::Ledger::resolved(&store);
    execute_plan(&units, &ledger, &f.trash, false)
}

/// Tempting wrong patch: Reclaim rows stay `view only` (no unit), so Space
/// answers "nothing to delete on this row". The unit and its listed folder
/// both mark, the confirm names the exact paths and sizes, and nothing
/// moves until the plan runs.
#[test]
fn a_reclaim_unit_and_a_depth_two_folder_mark_and_move_to_trash() {
    let f = fx();
    let caches = f.root.join("Caches");
    dir(&caches);
    dir(&caches.join("hiphi"));
    let mut a = app_with(
        &f,
        vec![unit(
            StorageCategory::Unclassified,
            &caches,
            10,
            vec![
                entry("hiphi", Some(7)),
                child(ChildKind::Remainder, "", Some(3)),
            ],
        )],
        ViewKind::Reclaim,
    );
    // The folder first: open the unit, select the listed folder.
    select(&mut a, "Caches");
    handle_key(&mut a, KeyCode::Right);
    select(&mut a, "hiphi");
    space(&mut a);
    assert_eq!(a.marked.len(), 1, "refusal: {:?}", a.refusal_active());
    assert!(caches.join("hiphi").exists(), "marking moves nothing");
    let summary = a.confirm_summary();
    assert!(
        summary.contains(&caches.join("hiphi").display().to_string()),
        "{summary}"
    );
    assert!(summary.contains("7B"), "{summary}");
    assert!(summary.contains("Cost unknown"), "{summary}");
    assert!(summary.contains("Last used · no record"), "{summary}");
    assert!(
        summary.contains("Trash can be restored until emptied"),
        "{summary}"
    );
    let res = run_plan(&a, &f);
    assert!(res[0].outcome.is_ok(), "{:?}", res[0].outcome);
    assert!(!caches.join("hiphi").exists());
    assert!(caches.exists(), "only the folder moved, not its unit");
}

/// Tempting wrong patch: the cursor row is acted on at Backspace even
/// though Space marked a different one ("unmarked row removed because the
/// cursor moved"). The plan is what was marked, whatever the cursor does.
#[test]
fn backspace_acts_on_the_marks_not_on_the_cursor() {
    let f = fx();
    let a1 = f.root.join("a");
    let b1 = f.root.join("b");
    dir(&a1);
    dir(&b1);
    let mut a = app_with(
        &f,
        vec![
            unit(StorageCategory::Cache, &a1, 9, vec![]),
            unit(StorageCategory::Cache, &b1, 5, vec![]),
        ],
        ViewKind::Reclaim,
    );
    select(&mut a, "/a");
    space(&mut a);
    select(&mut a, "/b");
    handle_key(&mut a, KeyCode::Backspace);
    wait(&mut a);
    assert!(
        a.confirm_open,
        "Backspace opened the confirm, it did not act"
    );
    assert_eq!(a.marked.len(), 1);
    assert!(a.marked.contains_key(&a1.display().to_string()));
    assert!(a1.exists() && b1.exists(), "Backspace never acts directly");
}

/// Tempting wrong patch: Backspace on an unmarked row removes it with no
/// confirm. It marks the row and opens the confirm; nothing has moved.
#[test]
fn backspace_on_an_unmarked_row_opens_the_confirm_and_moves_nothing() {
    let f = fx();
    let p = f.root.join("c");
    dir(&p);
    let mut a = app_with(
        &f,
        vec![unit(StorageCategory::Cache, &p, 5, vec![])],
        ViewKind::Reclaim,
    );
    select(&mut a, "/c");
    handle_key(&mut a, KeyCode::Backspace);
    wait(&mut a);
    assert!(a.confirm_open);
    assert!(p.exists());
    assert!(a.operation.is_none());
}

/// Tempting wrong patch: categories are refused (local-state, models,
/// installations, the whole Caches root). Each marks and carries its
/// own plain consequence; none is refused.
#[test]
fn no_category_and_not_the_caches_root_is_refused() {
    let f = fx();
    let mut units = Vec::new();
    let mut names = Vec::new();
    for (i, cat) in [
        StorageCategory::LocalState,
        StorageCategory::Models,
        StorageCategory::Installation,
        StorageCategory::Cache,
        StorageCategory::BuildOutput,
        StorageCategory::Environments,
        StorageCategory::Unclassified,
    ]
    .into_iter()
    .enumerate()
    {
        let p = f.root.join(format!("u{i}"));
        dir(&p);
        names.push(p.clone());
        units.push(unit(cat, &p, 5, vec![]));
    }
    let mut a = app_with(&f, units, ViewKind::Reclaim);
    for (i, p) in names.iter().enumerate() {
        select(&mut a, &format!("u{i}"));
        space(&mut a);
        assert!(
            a.marked.contains_key(&p.display().to_string()),
            "u{i} refused: {:?}",
            a.refusal_active()
        );
    }
    let s = a.confirm_summary();
    assert!(s.contains("Cannot be regenerated"), "{s}");
}

/// Tempting wrong patch: a protected folder is markable without telling
/// the person why it is not, or the review worker has no store and skips
/// the protect check (the pre-G6 worker did). The person's own mark is
/// respected through the real Space path, with the command that removes it.
#[test]
fn a_protected_folder_is_refused_with_the_persons_own_mark_named() {
    let f = fx();
    let p = f.root.join("kept");
    dir(&p);
    swamp_core::protection::protect_add(&f.store, &p).unwrap();
    let mut a = app_with(
        &f,
        vec![unit(StorageCategory::Cache, &p, 5, vec![])],
        ViewKind::Reclaim,
    );
    select(&mut a, "kept");
    space(&mut a);
    assert!(a.marked.is_empty());
    let why = a.refusal_active().expect("a reason is shown").to_string();
    assert!(why.contains("protected by you"), "{why}");
    assert!(why.contains("swamp protect remove"), "{why}");
    // Backspace opens no confirm for it either.
    handle_key(&mut a, KeyCode::Backspace);
    wait(&mut a);
    assert!(!a.confirm_open && p.exists());
}

/// Tempting wrong patch: the marked folder is swapped for a symlink after
/// the mark and the move follows it. The plan refuses as changed since
/// review; nothing moves; the mark stays.
#[test]
fn a_symlink_swapped_in_after_the_mark_moves_nothing() {
    let f = fx();
    let p = f.root.join("cache");
    let precious = f.root.join("precious");
    dir(&p);
    dir(&precious);
    let mut a = app_with(
        &f,
        vec![unit(StorageCategory::Cache, &p, 5, vec![])],
        ViewKind::Reclaim,
    );
    select(&mut a, "cache");
    space(&mut a);
    assert_eq!(a.marked.len(), 1);
    std::fs::rename(&p, f.root.join("aside")).unwrap();
    std::os::unix::fs::symlink(&precious, &p).unwrap();
    let res = run_plan(&a, &f);
    let err = res[0].outcome.as_ref().unwrap_err();
    assert!(err.contains("changed since review"), "{err}");
    assert!(precious.join("f").exists());
    assert!(
        std::fs::symlink_metadata(&p)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

/// Tempting wrong patch: a unit and a folder inside it are both queued,
/// so the second move hits a path the first already took (or Trash gets
/// two copies). Marking either while the other is marked refuses, naming
/// the overlap, in both orders.
#[test]
fn overlapping_marks_refuse_in_both_orders() {
    let f = fx();
    let caches = f.root.join("Caches");
    dir(&caches);
    dir(&caches.join("x"));
    let mk = |f: &Fx| {
        app_with(
            f,
            vec![unit(
                StorageCategory::Unclassified,
                &caches,
                10,
                vec![entry("x", Some(5))],
            )],
            ViewKind::Reclaim,
        )
    };
    let mut a = mk(&f);
    select(&mut a, "Caches");
    handle_key(&mut a, KeyCode::Right);
    select(&mut a, "x");
    space(&mut a);
    select(&mut a, "Caches");
    space(&mut a);
    assert_eq!(a.marked.len(), 1);
    assert!(a.refusal_active().unwrap().contains("overlaps"));
    let mut b = mk(&f);
    select(&mut b, "Caches");
    space(&mut b);
    handle_key(&mut b, KeyCode::Right);
    select(&mut b, "x");
    space(&mut b);
    assert_eq!(b.marked.len(), 1);
    assert!(b.refusal_active().unwrap().contains("overlaps"));
}

/// Tempting wrong patch: marking twice queues the path twice. Space again
/// unmarks it.
#[test]
fn marking_twice_unmarks() {
    let f = fx();
    let p = f.root.join("c");
    dir(&p);
    let mut a = app_with(
        &f,
        vec![unit(StorageCategory::Cache, &p, 5, vec![])],
        ViewKind::Reclaim,
    );
    select(&mut a, "/c");
    space(&mut a);
    assert_eq!(a.marked.len(), 1);
    space(&mut a);
    assert!(a.marked.is_empty());
}

/// Tempting wrong patch: `A` in Reclaim marks the unit and every listed
/// folder (overlap refusals as noise), or toggles marked rows off. It marks
/// each top-level row once and leaves what is marked marked.
#[test]
fn mark_all_marks_each_unit_once_and_never_toggles() {
    let f = fx();
    let p1 = f.root.join("one");
    let p2 = f.root.join("two");
    dir(&p1);
    dir(&p2);
    dir(&p1.join("x"));
    let mut a = app_with(
        &f,
        vec![
            unit(StorageCategory::Cache, &p1, 9, vec![entry("x", Some(4))]),
            unit(StorageCategory::Cache, &p2, 5, vec![]),
        ],
        ViewKind::Reclaim,
    );
    select(&mut a, "one");
    handle_key(&mut a, KeyCode::Right);
    handle_key(&mut a, KeyCode::Char('A'));
    wait(&mut a);
    assert_eq!(a.marked.len(), 2, "{:?}", a.refusal_active());
    assert!(a.confirm_open);
    a.confirm_open = false;
    handle_key(&mut a, KeyCode::Char('A'));
    wait(&mut a);
    assert_eq!(a.marked.len(), 2, "a second A adds, it does not unmark");
}

/// Tempting wrong patch: the confirm lets keys through that change what is
/// under it (a mark, the view, the section, the cursor) or that start the
/// move. Tab, v, 1-3, Space, A, Backspace, arrows and the filter keys are
/// swallowed, the marks are the same after, and nothing moved.
#[test]
fn the_open_confirm_swallows_every_key_but_its_own() {
    let f = fx();
    let p = f.root.join("c");
    let q = f.root.join("d");
    dir(&p);
    dir(&q);
    let mut a = app_with(
        &f,
        vec![
            unit(StorageCategory::Cache, &p, 5, vec![]),
            unit(StorageCategory::Cache, &q, 5, vec![]),
        ],
        ViewKind::Reclaim,
    );
    select(&mut a, "/c");
    handle_key(&mut a, KeyCode::Backspace);
    wait(&mut a);
    assert!(a.confirm_open);
    let marks: Vec<_> = a.marked.keys().cloned().collect();
    let (view, sel) = (a.view, a.selected);
    for k in [
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Char('v'),
        KeyCode::Char('1'),
        KeyCode::Char('2'),
        KeyCode::Char('3'),
        KeyCode::Char(' '),
        KeyCode::Char('A'),
        KeyCode::Backspace,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Char('/'),
        KeyCode::Char(':'),
    ] {
        handle_key(&mut a, k);
        assert!(a.operation.is_none(), "{k:?} started work");
        assert!(a.confirm_open, "{k:?} closed the confirm");
        assert_eq!(a.marked.keys().cloned().collect::<Vec<_>>(), marks, "{k:?}");
        assert_eq!((a.view, a.selected), (view, sel), "{k:?}");
    }
    assert!(p.exists() && q.exists());
    // Esc cancels: nothing moved, the mark the press made is taken back.
    handle_key(&mut a, KeyCode::Esc);
    assert!(!a.confirm_open && a.marked.is_empty() && p.exists());
}

/// Tempting wrong patch: Enter typed ahead during the review (before the
/// confirm exists) confirms it. While the review runs, Enter, Space and
/// Backspace do nothing; only the confirm's own Enter acts, and only when
/// the plan fits the screen.
#[test]
fn enter_typed_ahead_during_the_review_does_not_confirm() {
    let f = fx();
    let p = f.root.join("c");
    dir(&p);
    let mut a = app_with(
        &f,
        vec![unit(StorageCategory::Cache, &p, 5, vec![])],
        ViewKind::Reclaim,
    );
    select(&mut a, "/c");
    handle_key(&mut a, KeyCode::Backspace);
    // The review may still be running: type ahead.
    handle_key(&mut a, KeyCode::Enter);
    handle_key(&mut a, KeyCode::Enter);
    wait(&mut a);
    assert!(p.exists(), "no Enter reached the move");
    assert!(a.confirm_open, "the confirm is open and waiting");
}

/// Tempting wrong patch: Enter is offered on a plan whose warnings are cut
/// off behind "+N more". On a terminal too small to show the whole plan
/// Enter does nothing and the footer says to enlarge it.
#[test]
fn an_unreviewed_plan_offers_no_enter_even_when_the_overlay_fits() {
    let f = fx();
    let p = f.root.join("c");
    dir(&p);
    let mut a = app_with(
        &f,
        vec![unit(StorageCategory::Unclassified, &p, 5, vec![])],
        ViewKind::Reclaim,
    );
    select(&mut a, "/c");
    handle_key(&mut a, KeyCode::Backspace);
    wait(&mut a);
    assert!(a.confirm_open);
    a.width = 40;
    a.height = 12;
    assert!(a.confirm_fits(40, 12));
    assert!(!a.confirm_review_is_complete(40, 12));
    handle_key(&mut a, KeyCode::Enter);
    assert!(a.operation.is_none() && p.exists());
    let small = frame(&a, 40, 12);
    assert!(small.contains("Esc cancel"), "{small}");
    assert!(!small.contains("Enter move to Trash"), "{small}");
    // A roomy terminal draws the entire plan and arms Enter.
    a.width = 120;
    a.height = 50;
    assert!(a.confirm_fits(120, 50));
    let big = frame(&a, 120, 50);
    assert!(big.contains("Cost unknown"), "{big}");
    assert!(big.contains("Last used · no record"), "{big}");
    assert!(a.confirm_review_is_complete(120, 50));
}

/// Tempting wrong patch: the legend names `Space mark` and `Backspace`
/// on every Reclaim row, including one that is not a folder, where the
/// key answers with a refusal. The keys are named only on a row that has a
/// mark to make; the other row says why in the detail pane.
#[test]
fn the_legend_names_the_keys_only_where_they_act() {
    let f = fx();
    let p = f.root.join("c");
    dir(&p);
    dir(&p.join("x"));
    let mut a = app_with(
        &f,
        vec![unit(
            StorageCategory::Cache,
            &p,
            10,
            vec![
                entry("x", Some(6)),
                child(ChildKind::Remainder, "", Some(4)),
            ],
        )],
        ViewKind::Reclaim,
    );
    a.width = 120;
    a.height = 30;
    select(&mut a, "/c");
    handle_key(&mut a, KeyCode::Right);
    let on_folder = frame(&a, 120, 30);
    let last = on_folder.lines().last().unwrap();
    assert!(
        last.contains("Space mark") && last.contains("⌫ trash"),
        "{last}"
    );
    // The remainder row: not a folder.
    a.selected = a.rows().len() - 1;
    assert!(!a.selected_row_markable());
    let on_rest = frame(&a, 120, 30);
    let last = on_rest.lines().last().unwrap();
    assert!(
        !last.contains("Space mark") && !last.contains("⌫"),
        "{last}"
    );
    assert!(
        on_rest.contains("mark the unit"),
        "the reason is in the detail pane:\n{on_rest}"
    );
    // And pressing Space there marks nothing and says why.
    handle_key(&mut a, KeyCode::Char(' '));
    wait(&mut a);
    assert!(a.marked.is_empty());
}

/// Tempting wrong patch: the External view keeps "nothing to delete on
/// this row" for detector-resolved units and their folders. Both mark.
#[test]
fn external_rows_and_their_folders_mark_too() {
    let f = fx();
    let p = f.root.join("hf");
    dir(&p);
    dir(&p.join("model"));
    let mut a = app_with(
        &f,
        vec![unit(
            StorageCategory::Models,
            &p,
            10,
            vec![entry("model", Some(8))],
        )],
        ViewKind::External,
    );
    select(&mut a, "hf");
    space(&mut a);
    assert_eq!(a.marked.len(), 1, "{:?}", a.refusal_active());
    space(&mut a);
    handle_key(&mut a, KeyCode::Right);
    select(&mut a, "model");
    space(&mut a);
    assert_eq!(a.marked.len(), 1, "{:?}", a.refusal_active());
    let rows = a.rows();
    assert!(
        rows.iter()
            .all(|r| r.signals.iter().all(|s| s != "blocked"))
    );
    assert!(
        rows.iter()
            .all(|r| !r.signals.iter().any(|s| s.contains("inspection only")))
    );
}

/// Tempting wrong patch: a huge list is cut to the first screen, hiding
/// the marked paths. 400 folders under one unit mark and list; the plan
/// names the ones it shows and counts the rest, never claiming fewer.
#[test]
fn a_huge_marked_list_groups_summary_and_keeps_every_path_in_inventory() {
    let f = fx();
    let base = f.root.join("Caches");
    std::fs::create_dir_all(&base).unwrap();
    let mut kids = Vec::new();
    let mut units = Vec::new();
    for i in 0..12 {
        let p = f.root.join(format!("unit{i:02}"));
        dir(&p);
        units.push(unit(StorageCategory::Cache, &p, 5, vec![]));
    }
    for i in 0..400 {
        kids.push(entry(&format!("d{i}"), Some(1)));
    }
    units.push(unit(StorageCategory::Unclassified, &base, 400, kids));
    let mut a = app_with(&f, units, ViewKind::Reclaim);
    handle_key(&mut a, KeyCode::Char('A'));
    wait(&mut a);
    assert_eq!(a.marked.len(), 13);
    let s = a.confirm_summary();
    assert!(s.contains("Review 13 actions"), "{s}");
    assert!(s.contains("Trash · 13 actions"), "{s}");
    let details = a.confirm_details();
    assert!(
        details.contains("/unit11"),
        "the last target was omitted: {details}"
    );
    assert_eq!(details.matches("TRASH ·").count(), 13, "{details}");
}

/// Tempting wrong patch: the move is recorded after it happens, so a
/// store that cannot be written leaves a moved folder with no record (or
/// the failure is swallowed). Through the real plan: the started row
/// cannot be written, nothing moves, the result says so.
#[test]
fn a_ledger_that_cannot_be_written_means_nothing_moves_through_the_plan() {
    use std::os::unix::fs::PermissionsExt;
    let f = fx();
    let p = f.root.join("c");
    dir(&p);
    let mut a = app_with(
        &f,
        vec![unit(StorageCategory::Cache, &p, 5, vec![])],
        ViewKind::Reclaim,
    );
    select(&mut a, "/c");
    space(&mut a);
    assert_eq!(a.marked.len(), 1);
    std::fs::set_permissions(&f.store, std::fs::Permissions::from_mode(0o500)).unwrap();
    let res = run_plan(&a, &f);
    std::fs::set_permissions(&f.store, std::fs::Permissions::from_mode(0o700)).unwrap();
    let err = res[0].outcome.as_ref().unwrap_err();
    assert!(err.contains("nothing moved"), "{err}");
    assert!(p.join("f").exists());
}

/// Tempting wrong patch: a moved folder stays on screen until the next
/// observation, so a second Space marks a path that is gone, and the unit
/// still claims bytes that left. The rows drop it and the unit's size
/// comes down by the folder's bytes.
#[test]
fn a_moved_folder_leaves_the_rows_and_the_unit_shrinks() {
    let f = fx();
    let caches = f.root.join("Caches");
    dir(&caches);
    dir(&caches.join("x"));
    dir(&caches.join("y"));
    let mut a = app_with(
        &f,
        vec![unit(
            StorageCategory::Unclassified,
            &caches,
            10,
            vec![entry("x", Some(6)), entry("y", Some(4))],
        )],
        ViewKind::Reclaim,
    );
    select(&mut a, "Caches");
    handle_key(&mut a, KeyCode::Right);
    select(&mut a, "x");
    space(&mut a);
    let res = run_plan(&a, &f);
    assert!(res[0].outcome.is_ok());
    a.prune_removed(&res);
    let rows = a.rows();
    assert!(
        rows.iter().all(|r| r.label.trim() != "x"),
        "x is gone from the rows"
    );
    assert_eq!(rows[0].bytes, 4, "the unit lost the folder's bytes");
}

/// Tempting wrong patch: a store-interior folder is "inspection only" and
/// the mark refuses it, or it is planned through a project and refused for
/// having no owning checkout. A folder no checkout owns marks as the path
/// plus what its adapter said it could not establish, and moves.
#[test]
fn a_store_interior_folder_marks_with_what_its_adapter_did_not_establish() {
    use swamp_core::artifact::{AccountingBasis, ArtifactRole};
    use swamp_core::build_adapters::{BuildContainer, NestedUnitBuilder};
    let f = fx();
    let store = f.root.join("go-build");
    let inner = store.join("ab");
    dir(&inner);
    let c = BuildContainer::shared_store_of(
        "go",
        store.clone(),
        swamp_core::locations::BuildStoreKind::GoBuildCache,
    );
    let n = NestedUnitBuilder::new(&c, ArtifactRole::Intermediate, inner.clone())
        .is_dir(true)
        .bytes_on_basis(300, AccountingBasis::Allocated)
        .supported_with_reason("fixture")
        .consequence("recompiled")
        .no_action_because("shared by every Go project on this machine")
        .build();
    let mut a = app_with(
        &f,
        vec![unit(StorageCategory::Cache, &store, 300, vec![])],
        ViewKind::External,
    );
    a.set_store_interiors(vec![n]);
    // Open the unit, then each group under it, until the folder is a row.
    for _ in 0..4 {
        if a.rows().iter().any(|r| r.label == "ab") {
            break;
        }
        let at = a
            .rows()
            .iter()
            .position(|r| r.expandable && r.collapsed_children.is_some());
        let Some(at) = at else { break };
        a.selected = at;
        handle_key(&mut a, KeyCode::Right);
    }
    select(&mut a, "ab");
    assert!(
        a.selected_row_markable(),
        "the interior folder has a mark to make"
    );
    space(&mut a);
    assert_eq!(a.marked.len(), 1, "{:?}", a.refusal_active());
    let s = a.confirm_summary();
    assert!(s.contains("shared by every Go project"), "{s}");
    assert!(s.contains("recompiled"), "{s}");
    let res = run_plan(&a, &f);
    assert!(res[0].outcome.is_ok(), "{:?}", res[0].outcome);
    assert!(!inner.exists());
}

/// Tempting wrong patch: the Disk view's measured folders stay read-only
/// rows. Each measured folder is a path that marks; a folder that could
/// not be read is not (the OS said no, there is no number either).
#[test]
fn a_measured_folder_in_the_disk_view_marks_and_an_unreadable_one_does_not() {
    use swamp_core::growth::VolumeMetaRow;
    use swamp_core::volume_ledger::{
        Category as LC, Exactness, LedgerReading, Row as LRow, account,
    };
    let f = fx();
    let big = f.root.join("Movies");
    dir(&big);
    let now = swamp_core::entities::now();
    let lrow = |path: &Path, cat: LC, bytes: Option<u64>| LRow {
        path: path.display().to_string(),
        category: cat,
        bytes,
        overlap_bytes: 0,
        entries: None,
        unreadable: 0,
        measured_at: now,
        method: "walk".into(),
        exactness: if bytes.is_some() {
            Exactness::Exact
        } else {
            Exactness::NotMeasured
        },
        note: None,
    };
    let rows = vec![
        lrow(&big, LC::Other, Some(5)),
        lrow(&f.root.join("Pictures"), LC::Unreadable, None),
    ];
    let meta = VolumeMetaRow {
        measured_at: now,
        cycle_started_at: 1,
        cycle_complete_at: now,
        complete: true,
        budget_secs: 120,
        budget_used_ms: 1,
        statfs_at: now,
        container_total: Some(500),
        container_used: Some(100),
        container_free: Some(400),
        data_volume_used: Some(90),
    };
    let mut a = app_with(&f, vec![], ViewKind::Disk);
    a.set_ledger(LedgerReading::Measured(Box::new(account(&rows, &meta))));
    select(&mut a, &big.display().to_string());
    assert!(a.selected_row_markable());
    space(&mut a);
    assert_eq!(a.marked.len(), 1, "{:?}", a.refusal_active());
    assert!(a.confirm_summary().contains("no record of what uses it"));
    assert!(big.exists(), "marking moves nothing");
    let res = run_plan(&a, &f);
    assert!(res[0].outcome.is_ok() && !big.exists());
    // The unreadable folder has no unit: no key is named for it.
    let unreadable = a.rows().into_iter().find(|r| r.label.contains("Pictures"));
    assert!(unreadable.is_none_or(|r| r.unit.is_none()));
}

/// Tempting wrong patch: Space on a manager-removed row (mise) still says
/// "never moved to Trash". Space marks the folder for Trash (the confirm
/// says the manager will not know it is gone); Backspace on an unmarked
/// row still opens the manager's own list.
#[test]
fn a_tool_managed_row_marks_for_trash_and_keeps_its_own_command_on_backspace() {
    let f = fx();
    let p = f.root.join("mise/installs");
    dir(&p);
    let mut u = unit(StorageCategory::Installation, &p, 5, vec![]);
    u.detector_id = "mise".into();
    let mut a = app_with(&f, vec![u], ViewKind::External);
    select(&mut a, "installs");
    assert!(a.rows()[a.selected].tool.is_some());
    space(&mut a);
    assert_eq!(a.marked.len(), 1, "{:?}", a.refusal_active());
    // Backspace on the row that is itself marked is the Trash confirm.
    handle_key(&mut a, KeyCode::Backspace);
    wait(&mut a);
    assert!(a.confirm_open && a.tool_sheet.is_none());
    assert!(p.exists());
    handle_key(&mut a, KeyCode::Esc);
    // With nothing marked, Backspace opens mise's own sheet instead.
    a.marked.clear();
    handle_key(&mut a, KeyCode::Backspace);
    assert!(a.tool_sheet.is_some() || a.operation.is_some());
}

/// Long Reclaim plans scroll, and `k keep executables` is not offered for
/// Reclaim folders alone.
#[test]
fn the_does_not_fit_footer_is_true_and_k_is_not_offered_for_reclaim_plans() {
    let f = fx();
    let mut units = Vec::new();
    for i in 0..6 {
        let p = f.root.join(format!("u{i}"));
        dir(&p);
        units.push(unit(StorageCategory::LocalState, &p, 5, vec![]));
    }
    let mut a = app_with(&f, units, ViewKind::Reclaim);
    handle_key(&mut a, KeyCode::Char('A'));
    wait(&mut a);
    assert!(a.confirm_open);
    a.width = 60;
    a.height = 14;
    assert!(a.confirm_fits(60, 14));
    let small = frame(&a, 60, 14);
    let last = small.lines().last().unwrap();
    assert!(last.contains("Read every line"), "{last}");
    assert!(small.contains("Review actions"), "{small}");
    a.width = 200;
    a.height = 80;
    let big = frame(&a, 200, 80);
    assert!(
        !big.lines().last().unwrap().contains("keep executables"),
        "{big}"
    );
}

/// Tempting wrong patch: only Reclaim plans escape what they print. An
/// ordinary plan's warning with a newline forges a plan line, and a bidi
/// override in a Reclaim path reaches the produced string (the frame drops
/// it, so the string is what is asserted).
#[test]
fn plan_strings_hold_no_forged_lines_or_bidi_overrides() {
    use swamp_tui::actions::{MarkedUnit, confirm_summary};
    let plain = |path: &str, warning: &str| MarkedUnit {
        cargo_unit: None,
        agent_unit: None,
        session_members: None,
        reclaim: None,
        path: PathBuf::from(path),
        docker: None,
        worktree_path: PathBuf::from(path),
        bytes: 1,
        observed_at: 0,
        worktree: None,
        label: path.to_string(),
        warnings: vec![warning.to_string()],
    };
    let u = plain("/x/a", "tracked\n⚠ nothing will be moved\u{202e}");
    let s = confirm_summary(std::slice::from_ref(&u));
    assert!(
        !s.lines().any(|l| l.starts_with("⚠ nothing will be moved")),
        "{s}"
    );
    assert!(!s.contains('\u{202e}'), "{s}");

    let f = fx();
    let folder = f.root.join("abc\u{202e}gnp.exe");
    dir(&folder);
    let mut a = app_with(
        &f,
        vec![unit(StorageCategory::Cache, &folder, 5, vec![])],
        ViewKind::Reclaim,
    );
    handle_key(&mut a, KeyCode::Char(' '));
    wait(&mut a);
    assert_eq!(a.marked.len(), 1, "{:?}", a.refusal_active());
    assert!(!a.confirm_summary().contains('\u{202e}'));
}

/// Tempting wrong patch: the generic mark path names the row's own path in
/// `swamp protect remove`, though the mark is on a folder above it. The
/// command named is the entry's.
#[test]
fn the_generic_mark_path_names_the_covering_protect_entry() {
    let f = fx();
    let parent = f.root.join("keep");
    let p = parent.join("leftover");
    dir(&p);
    swamp_core::protection::protect_add(&f.store, &parent).unwrap();
    let mut report = swamp_core::report::Report::empty(f.root.clone());
    report.observed_at = 1_000;
    report.unowned.push(swamp_core::report::UnownedRow {
        measurement: None,
        path_or_object: p.display().to_string(),
        bytes: 5,
        reason: swamp_core::report::UnownedReason::SharedCache,
        docker_kind: None,
        shared_bytes: None,
        note: None,
        created_at: None,
        containers: Vec::new(),
        shared_with: Vec::new(),
        dangling: false,
        evidence: Vec::new(),
    });
    let mut a = App::new(report, f.root.clone());
    a.store_dir = Some(f.store.clone());
    a.views_seen = true;
    a.set_view(ViewKind::Unowned);
    let row = a.rows().into_iter().next().unwrap();
    a.mark_row(&row);
    assert!(a.marked.is_empty());
    let why = a.refusal_active().unwrap().to_string();
    assert!(
        why.contains(&format!("swamp protect remove {}`", parent.display())),
        "{why}"
    );
}

/// Tempting wrong patch: the simulator runtime volumes row keeps saying the
/// tool command is "not available yet", or the removal sentence sits last
/// on the clipped signals line and is cut at the edge. The detail pane
/// shows both ways (Trash, simctl's own command, permanent) in full at 80
/// columns, the row knows its manager (Backspace with nothing marked opens
/// simctl's list) and Space still marks it for Trash.
#[test]
fn simulator_volumes_row_says_both_removal_paths_and_both_are_reachable() {
    let f = fx();
    let vol = f.root.join("CoreSimulator").join("Volumes");
    dir(&vol);
    let mut u = unit(StorageCategory::Installation, &vol, 10, vec![]);
    u.detector_id = "core-simulator".into();
    let mut a = app_with(&f, vec![u], ViewKind::Reclaim);
    select(&mut a, "Volumes");
    let rows = a.rows();
    assert_eq!(
        rows[a.selected].tool,
        Some(swamp_core::tool_removal::Manager::Simulator),
        "Backspace with nothing marked must reach simctl's sheet"
    );
    for w in [80u16, 120] {
        let s = frame(&a, w, 30);
        assert!(s.contains("simctl's own command, permanent"), "{w}: {s}");
        assert!(!s.contains("not available yet"), "{w}: {s}");
    }
    space(&mut a);
    assert_eq!(a.marked.len(), 1, "refusal: {:?}", a.refusal_active());
}
