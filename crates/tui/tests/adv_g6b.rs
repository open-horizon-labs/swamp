//! v0.8.0 G6 independent verification (TUI half): keys, sizes and sheets
//! the first audit did not try. Real temp directories, a fake Trash root
//! (SWAMP_TRASH_DIR) and a temp store (SWAMP_DIR) for this whole process,
//! so an Enter here can never reach the developer's real Trash or store.
//!
//! Each test names the tempting wrong patch it fails.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crossterm::event::KeyCode;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use swamp_core::drilldown::{ChildKind, ChildMeasure, UnitChild};
use swamp_core::external::ExternalUnit;
use swamp_core::last_used::LastUsed;
use swamp_core::locations::{Provenance, StorageCategory};
use swamp_tui::app::{App, ViewKind};
use swamp_tui::{handle_key, ui};

/// One fake Trash and store for the process, set before any test acts.
fn sandbox() -> &'static (PathBuf, PathBuf) {
    static S: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
    S.get_or_init(|| {
        let tmp = tempfile::tempdir().unwrap().keep();
        let root = std::fs::canonicalize(tmp).unwrap();
        let trash = root.join("trash");
        let store = root.join("store");
        std::fs::create_dir_all(&store).unwrap();
        // An empty manager sandbox: no real mise/simctl can ever resolve.
        let tools = root.join("tools");
        std::fs::create_dir_all(tools.join("bin")).unwrap();
        std::fs::create_dir_all(tools.join("home")).unwrap();
        // SAFETY: set once, before any test in this process reads them.
        unsafe {
            std::env::set_var("SWAMP_TRASH_DIR", &trash);
            std::env::set_var("SWAMP_DIR", &store);
            std::env::set_var("SWAMP_TEST_TOOL_SANDBOX", &tools);
        }
        (trash, store)
    })
}

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    store: PathBuf,
}

fn fx() -> Fx {
    let (_, store) = sandbox();
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    Fx {
        store: store.clone(),
        root,
        _tmp: tmp,
    }
}

fn dir(p: &Path) {
    std::fs::create_dir_all(p).unwrap();
    std::fs::write(p.join("f"), b"hello").unwrap();
}

fn entry(name: &str, bytes: Option<i64>) -> UnitChild {
    UnitChild {
        kind: ChildKind::Entry,
        name: name.to_string(),
        bytes,
        measure: ChildMeasure::Complete,
        mtime_max: 0,
        entries: 0,
        not_measured: 0,
        last_used: LastUsed::default(),
        access_evidence: None,
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
    let deadline = Instant::now() + Duration::from_secs(60);
    while a.operation.is_some() {
        assert!(Instant::now() < deadline, "operation did not finish");
        a.poll_operation();
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn select(a: &mut App, needle: &str) {
    let rows = a.rows();
    a.selected = rows
        .iter()
        .position(|r| r.label.trim() == needle)
        .or_else(|| rows.iter().position(|r| r.label.contains(needle)))
        .or_else(|| {
            rows.iter()
                .position(|r| r.unit.as_ref().is_some_and(|u| u.0.contains(needle)))
        })
        .unwrap_or_else(|| panic!("no row with {needle:?}"));
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

/// Tempting wrong patch: only a confirm that opens at the END of a review
/// asks the event loop to drop typeahead. With a mark already made (Space,
/// earlier), Backspace opens the confirm at once; an Enter typed in the
/// same key batch (typed before the plan was ever drawn) then confirms it.
/// Every opening of a confirm must ask for the drain.
#[test]
fn adv_b_enter_in_the_same_batch_as_backspace_never_confirms_an_unseen_plan() {
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
    assert_eq!(a.marked.len(), 1, "{:?}", a.refusal_active());
    let _ = a.take_confirm_drain();
    handle_key(&mut a, KeyCode::Backspace);
    assert!(a.confirm_open, "Backspace with a mark opens the confirm");
    assert!(
        a.take_confirm_drain(),
        "the confirm opened with no drain: an Enter already queued behind this Backspace \
         confirms a plan nobody saw"
    );
}

/// Tempting wrong patch: the overlap check looks only at marks from the
/// current view. A parent marked in External and its child marked in
/// Reclaim both land in one plan: the child moves, then the parent's move
/// is short of what its confirm said. One of them must be refused (or the
/// view switch must drop the first mark).
#[test]
fn adv_b_a_parent_and_child_marked_in_different_views_never_both_mark() {
    let f = fx();
    let p = f.root.join("hf");
    dir(&p);
    dir(&p.join("model"));
    let units = vec![unit(
        StorageCategory::Models,
        &p,
        10,
        vec![entry("model", Some(8))],
    )];
    let mut a = app_with(&f, units, ViewKind::External);
    select(&mut a, "hf");
    space(&mut a);
    assert_eq!(a.marked.len(), 1, "{:?}", a.refusal_active());
    a.set_view(ViewKind::Reclaim);
    select(&mut a, "hf");
    handle_key(&mut a, KeyCode::Right);
    select(&mut a, "model");
    space(&mut a);
    let keys: Vec<_> = a.marked.keys().cloned().collect();
    assert!(
        keys.len() <= 1,
        "parent and child are both marked across views: {keys:?}"
    );
    handle_key(&mut a, KeyCode::Backspace);
    wait(&mut a);
    assert!(p.join("model/f").exists() && p.join("f").exists());
}

/// Tempting wrong patch: `A` over a very long Reclaim view either takes
/// seconds per row, draws a plan that overflows 80x24 while offering
/// Enter, or panics on the frame. 1000 rows: the review finishes, the frame
/// draws at 80x24 and 120x30, Enter is offered only if the whole plan fits,
/// and an Enter at 80x24 on a plan that does not fit moves nothing.
#[test]
fn adv_b_mark_all_over_a_thousand_rows_at_80x24_never_offers_an_unread_plan() {
    let f = fx();
    let mut units = Vec::new();
    for i in 0..1000 {
        let p = f.root.join(format!("u{i:04}"));
        dir(&p);
        let cat = if i % 7 == 0 {
            StorageCategory::LocalState
        } else {
            StorageCategory::Cache
        };
        units.push(unit(cat, &p, 5, vec![]));
    }
    let mut a = app_with(&f, units, ViewKind::Reclaim);
    let t0 = Instant::now();
    handle_key(&mut a, KeyCode::Char('A'));
    wait(&mut a);
    let took = t0.elapsed();
    assert!(
        took < Duration::from_secs(45),
        "review of 1000 rows took {took:?}"
    );
    assert!(a.confirm_open, "{:?}", a.refusal_active());
    for (w, h) in [(80u16, 24u16), (120, 30)] {
        a.width = w;
        a.height = h;
        let fr = frame(&a, w, h);
        let last = fr.lines().last().unwrap_or("").to_string();
        if a.confirm_review_is_complete(w, h) {
            assert!(
                fr.contains("Enter move to Trash"),
                "{w}x{h} offers Enter over a cut plan:\n{fr}"
            );
        } else {
            assert!(!last.contains("Enter move to Trash"), "{w}x{h}: {last}");
        }
    }
    a.width = 80;
    a.height = 24;
    handle_key(&mut a, KeyCode::Enter);
    wait(&mut a);
    assert!(
        f.root.join("u0000/f").exists(),
        "Enter moved an unread plan"
    );
    handle_key(&mut a, KeyCode::Esc);
}

/// Tempting wrong patch: the fit is decided when the review starts. The
/// terminal shrinks while the review runs; the confirm that opens at the
/// new size offers no Enter if the plan does not fit it, and the frame
/// draws at every size in between.
#[test]
fn adv_b_a_resize_during_the_review_is_honoured_by_the_confirm() {
    let f = fx();
    let mut units = Vec::new();
    for i in 0..5 {
        let p = f.root.join(format!("s{i}"));
        dir(&p);
        units.push(unit(StorageCategory::LocalState, &p, 5, vec![]));
    }
    let mut a = app_with(&f, units, ViewKind::Reclaim);
    a.width = 200;
    a.height = 60;
    handle_key(&mut a, KeyCode::Char('A'));
    for (w, h) in [(120u16, 30u16), (60, 14), (30, 8), (20, 5)] {
        a.width = w;
        a.height = h;
        let _ = frame(&a, w, h);
        a.poll_operation();
    }
    wait(&mut a);
    assert!(a.confirm_open);
    assert!(!a.confirm_fits(20, 5));
    handle_key(&mut a, KeyCode::Enter);
    wait(&mut a);
    assert!(f.root.join("s0/f").exists(), "Enter moved at 20x5");
    let fr = frame(&a, 20, 5);
    assert!(!fr.lines().last().unwrap_or("").contains("Enter"), "{fr}");
    handle_key(&mut a, KeyCode::Esc);
}

/// Tempting wrong patch: Esc during the review only hides the progress,
/// and the review's result still lands as a confirm (with Enter live).
/// Esc cancels: no confirm opens, nothing moves.
#[test]
fn adv_b_esc_during_the_review_opens_no_confirm() {
    let f = fx();
    let mut units = Vec::new();
    for i in 0..300 {
        let p = f.root.join(format!("e{i:03}"));
        dir(&p);
        units.push(unit(StorageCategory::Cache, &p, 5, vec![]));
    }
    let mut a = app_with(&f, units, ViewKind::Reclaim);
    handle_key(&mut a, KeyCode::Char('A'));
    let running = a.operation.is_some();
    handle_key(&mut a, KeyCode::Esc);
    wait(&mut a);
    if running {
        assert!(
            !a.confirm_open,
            "Esc during the review, and the confirm opened anyway ({} marks)",
            a.marked.len()
        );
    }
    handle_key(&mut a, KeyCode::Enter);
    wait(&mut a);
    assert!(f.root.join("e000/f").exists());
}

/// Tempting wrong patch: the recheck at Enter trusts the path. The parent
/// folder is renamed after the mark and before Enter (and a new folder is
/// made at the old name): Enter moves neither, and says why.
#[test]
fn adv_b_a_parent_renamed_between_mark_and_enter_moves_nothing() {
    let f = fx();
    let parent = f.root.join("proj");
    let p = parent.join("cache");
    dir(&p);
    let mut a = app_with(
        &f,
        vec![unit(StorageCategory::Cache, &p, 5, vec![])],
        ViewKind::Reclaim,
    );
    a.width = 200;
    a.height = 60;
    select(&mut a, "cache");
    handle_key(&mut a, KeyCode::Backspace);
    wait(&mut a);
    assert!(a.confirm_open, "{:?}", a.refusal_active());
    let reviewed = frame(&a, 200, 60);
    assert!(a.confirm_review_is_complete(200, 60), "{reviewed}");
    let renamed = f.root.join("proj-renamed");
    std::fs::rename(&parent, &renamed).unwrap();
    dir(&p);
    handle_key(&mut a, KeyCode::Enter);
    wait(&mut a);
    assert!(renamed.join("cache/f").exists(), "the renamed folder moved");
    assert!(p.join("f").exists(), "the new folder at the old name moved");
    let said = format!(
        "{:?} {:?} {:?}",
        a.last_result,
        a.refusal_active(),
        a.blocked
    );
    assert!(
        said.contains("changed since review") || said.contains("different entry"),
        "Enter said nothing about why: {said}"
    );
}

fn mise_app(f: &Fx) -> (App, PathBuf) {
    let p = f.root.join("mise/installs");
    dir(&p);
    let mut u = unit(StorageCategory::Installation, &p, 5, vec![]);
    u.detector_id = "mise".into();
    let mut a = app_with(f, vec![u], ViewKind::External);
    select(&mut a, "installs");
    assert!(a.rows()[a.selected].tool.is_some());
    (a, p)
}

/// Tempting wrong patch: making tool rows markable for Trash rerouted
/// Backspace. On a mise row with nothing marked, Backspace still opens the
/// manager's own sheet (never the Trash confirm), and Space marks the
/// folder for Trash with a line saying the manager will not know.
#[test]
fn adv_b_a_mise_row_keeps_its_sheet_on_backspace_and_offers_trash_on_space() {
    let f = fx();
    let (mut a, p) = mise_app(&f);
    handle_key(&mut a, KeyCode::Backspace);
    assert!(
        a.tool_sheet.is_some() && !a.confirm_open,
        "Backspace on a mise row did not open mise's sheet"
    );
    handle_key(&mut a, KeyCode::Esc);
    for _ in 0..200 {
        if a.tool_sheet.is_none() {
            break;
        }
        handle_key(&mut a, KeyCode::Esc);
        std::thread::sleep(Duration::from_millis(5));
    }
    a.tool_sheet = None;
    space(&mut a);
    assert_eq!(a.marked.len(), 1, "{:?}", a.refusal_active());
    let w = a.marked.values().next().unwrap().warnings.join("\n");
    assert!(w.to_lowercase().contains("mise"), "{w}");
    assert!(p.join("f").exists());
}

/// Tempting wrong patch: the confirm block's keys come before the sheet's.
/// With a Trash confirm open underneath and the sheet on top, Enter and
/// Backspace belong to the sheet: nothing is moved to Trash.
#[test]
fn adv_b_the_tool_sheet_is_above_the_confirm_in_key_order() {
    let f = fx();
    let (mut a, p) = mise_app(&f);
    space(&mut a);
    assert_eq!(a.marked.len(), 1);
    a.confirm_open = true;
    a.marked.clear();
    handle_key(&mut a, KeyCode::Esc);
    a.confirm_open = false;
    handle_key(&mut a, KeyCode::Backspace);
    assert!(a.tool_sheet.is_some());
    // Put a live Trash confirm under the sheet.
    space_under_sheet(&mut a);
    a.confirm_open = !a.marked.is_empty();
    for k in [KeyCode::Enter, KeyCode::Backspace] {
        if a.tool_sheet.is_none() {
            break;
        }
        handle_key(&mut a, k);
        assert!(
            !a.operation.as_ref().is_some_and(|o| o.label == "Deleting"),
            "{k:?} with the sheet on top started the Trash move"
        );
        wait(&mut a);
    }
    assert!(p.join("f").exists(), "the folder under the sheet moved");
}

/// Marks the selected row directly (the sheet swallows Space).
fn space_under_sheet(a: &mut App) {
    let row = a.rows()[a.selected].clone();
    a.mark_row(&row);
}

/// Shared facts are summarized once; the explicit inventory still keeps
/// every marked path available without making a large list the primary view.
#[test]
fn adv_b_a_warning_shared_by_more_than_three_folders_names_every_one_while_enter_is_offered() {
    let f = fx();
    let mut units = Vec::new();
    let mut state = Vec::new();
    for i in 0..13 {
        let p = f.root.join(format!("m{i:02}"));
        dir(&p);
        // The last five sort after the eighth listed path.
        let cat = if i >= 8 {
            state.push(p.clone());
            StorageCategory::LocalState
        } else {
            StorageCategory::Cache
        };
        units.push(unit(cat, &p, 5, vec![]));
    }
    let mut a = app_with(&f, units, ViewKind::Reclaim);
    handle_key(&mut a, KeyCode::Char('A'));
    wait(&mut a);
    assert_eq!(a.marked.len(), 13, "{:?}", a.refusal_active());
    let s = a.confirm_summary();
    assert!(s.contains("affects 5 actions"), "{s}");
    let details =
        swamp_tui::actions::confirm_details(&a.marked.values().cloned().collect::<Vec<_>>());
    for path in &state {
        assert!(
            details.contains(&path.display().to_string()),
            "missing {}:\n{details}",
            path.display()
        );
    }
    handle_key(&mut a, KeyCode::Esc);
}
