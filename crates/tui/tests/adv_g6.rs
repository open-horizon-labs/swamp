//! v0.8.0 G6 adversarial audit (TUI half). Real temp directories, a fake
//! Trash root and a temp store; nothing here touches the developer's real
//! caches, Trash, store or managers. Each test names the tempting wrong
//! patch it fails.

use std::path::{Path, PathBuf};
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

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    store: PathBuf,
}

fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let store = root.join("store");
    std::fs::create_dir_all(&store).unwrap();
    Fx {
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
    UnitChild {
        kind: ChildKind::Entry,
        name: name.to_string(),
        bytes,
        measure: ChildMeasure::Complete,
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
    let deadline = Instant::now() + Duration::from_secs(30);
    while a.operation.is_some() {
        assert!(Instant::now() < deadline, "operation did not finish");
        a.poll_operation();
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn frame(a: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, a)).unwrap();
    t.backend().to_string()
}

fn mark_child(f: &Fx, name: &str) -> App {
    let caches = f.root.join("Caches");
    dir(&caches);
    dir(&caches.join(name));
    let mut a = app_with(
        f,
        vec![unit(
            StorageCategory::Cache,
            &caches,
            10,
            vec![entry(name, Some(7))],
        )],
        ViewKind::Reclaim,
    );
    let rows = a.rows();
    a.selected = rows
        .iter()
        .position(|r| r.label.contains("Caches"))
        .unwrap();
    handle_key(&mut a, KeyCode::Right);
    let rows = a.rows();
    a.selected = rows
        .iter()
        .position(|r| r.depth > 0 && r.unit.is_some())
        .expect("the child row is markable");
    handle_key(&mut a, KeyCode::Char(' '));
    wait(&mut a);
    assert_eq!(a.marked.len(), 1, "{:?}", a.refusal_active());
    a
}

/// Tempting wrong patch: the stored folder name is joined and printed
/// raw. A real folder whose name holds a newline writes its own line into
/// the plan, here a forged warning line that reads as swamp's.
#[test]
fn adv_a_folder_name_with_a_newline_cannot_forge_a_plan_line() {
    let f = fx();
    let a = mark_child(&f, "x\n⚠ nothing will be moved");
    let s = a.confirm_summary();
    assert!(
        !s.lines().any(|l| l.starts_with("⚠ nothing will be moved")),
        "the folder name wrote its own warning line into the plan:\n{s}"
    );
}

/// Tempting wrong patch: only the tool sheet strips format characters. A
/// right-to-left override in a folder name reaches the Trash plan and
/// reorders what the person reads.
#[test]
fn adv_a_bidi_override_in_a_folder_name_never_reaches_the_plan() {
    let f = fx();
    let mut a = mark_child(&f, "abc\u{202e}gnp.exe");
    a.confirm_open = true;
    let fr = frame(&a, 160, 50);
    // The drawn frame is what the person reads (the summary string keeps
    // the raw name; ratatui drops the control on draw).
    assert!(
        !fr.contains('\u{202e}'),
        "a U+202E override is drawn:\n{fr}"
    );
}

/// A unique consequential warning names its action in the summary even
/// though the complete path inventory lives in the details view.
#[test]
fn adv_a_merged_plan_never_hides_which_folder_a_warning_is_about() {
    let f = fx();
    let mut units = Vec::new();
    for i in 0..12 {
        let p = f.root.join(format!("a{i:02}"));
        dir(&p);
        units.push(unit(StorageCategory::Cache, &p, 5, vec![]));
    }
    let state = f.root.join("zz-state");
    dir(&state);
    units.push(unit(StorageCategory::LocalState, &state, 5, vec![]));
    let mut a = app_with(&f, units, ViewKind::Reclaim);
    handle_key(&mut a, KeyCode::Char('A'));
    wait(&mut a);
    assert_eq!(a.marked.len(), 13, "{:?}", a.refusal_active());
    let s = a.confirm_summary();
    assert!(s.contains("Cannot be downloaded or rebuilt"), "{s}");
    let fits = a.confirm_fits(200, 60);
    assert!(
        !fits || s.contains(&state.display().to_string()),
        "Enter offered (fits at 200x60: {fits}) while the one folder that cannot be regenerated is only in the count:\n{s}"
    );
}

/// Tempting wrong patch: the credentials/config rows became markable with
/// their warnings, but the confirm still keeps the first three warnings and
/// folds the rest into "+N more warnings", with Enter offered (no Docker in
/// the plan, so it always "fits"). What swamp keeps by default must never
/// be the part that is folded away.
#[test]
fn adv_a_kept_by_default_warning_is_never_folded_while_enter_is_offered() {
    use swamp_core::agents::{AgentActionCapability as Act, AgentCategory as Cat};
    let f = fx();
    let home = f.root.join("claude");
    std::fs::create_dir_all(&home).unwrap();
    let cred = home.join("credentials.db");
    std::fs::write(&cred, b"secret-token-value").unwrap();
    let u = swamp_core::agents::AgentUnit {
        tool_id: "claude-code".into(),
        tool_name: "Claude Code".into(),
        tool_home: home.clone(),
        category: Cat::ProtectedConfig,
        id: swamp_core::agents::unit_id("claude-code", Cat::ProtectedConfig, "credentials.db"),
        relative_path: "credentials.db".into(),
        path: cred.clone(),
        members: Vec::new(),
        bytes: 18,
        hardlinked: false,
        complete: true,
        growth_bytes: None,
        regrowth_count: 0,
        observed_at: 1_000,
        mtime_max: 1_000,
        protected: true,
        protect_reason: Some("credentials".into()),
        project_link: swamp_core::agents::ProjectLinkState::NotApplicable,
        action: Act::None,
        note: None,
        evidence: Vec::new(),
    };
    let mut a = app_with(&f, vec![], ViewKind::Agents);
    a.set_agent_units(vec![u.clone()]);
    let row = swamp_tui::model::agent_rows(&a.agent_units)
        .into_iter()
        .find(|r| r.label.contains("credentials.db"))
        .unwrap();
    a.mark_row(&row);
    assert_eq!(a.marked.len(), 1, "{:?}", a.refusal_active());
    let s = a.confirm_summary();
    assert!(!s.contains("secret-token-value"), "credentials printed");
    let fits = a.confirm_fits(80, 24);
    assert!(
        !(fits && s.contains("more warnings")),
        "Enter is offered (fits: {fits}) while warnings are folded; the marked unit's warnings are {:#?}\nplan:\n{s}",
        a.marked.values().next().unwrap().warnings
    );
    assert!(
        s.contains("swamp keeps this by default"),
        "the kept-by-default warning is not on the plan:\n{s}"
    );
}

/// Width/height fuzz: a very long folder name in an open Reclaim confirm
/// never panics the frame and never offers Enter where the plan does not
/// fit.
#[test]
fn adv_a_long_name_confirm_draws_at_every_size() {
    let f = fx();
    let long = "n".repeat(250);
    let mut a = mark_child(&f, &long);
    a.confirm_open = true;
    for w in (20u16..=200).step_by(7) {
        for h in [5u16, 8, 12, 24, 40, 60] {
            a.width = w;
            a.height = h;
            let fr = frame(&a, w, h);
            if a.confirm_fits(w, h) {
                assert!(!fr.contains("more lines"), "{w}x{h}:\n{fr}");
            }
        }
    }
}
