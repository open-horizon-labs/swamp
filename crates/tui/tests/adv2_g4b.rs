//! Round-2 adversarial audit of the G4b section/view rework (#206).
//! Fixture-only; no test touches the real store or spawns a scan.
//!
//! Each test names the tempting wrong patch it fails.

use std::path::PathBuf;

use crossterm::event::KeyCode;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use swamp_core::entities::Confidence;
use swamp_core::report::{
    ArtifactKind, ArtifactRow, ProjectRow, Reconciliation, Report, Signal, Source, UnownedReason,
    UnownedRow, WorktreeKind, WorktreeRow,
};
use swamp_tui::app::{App, Section, ViewKind};
use swamp_tui::{handle_key, ui};

fn art(kind: ArtifactKind, path: &str, bytes: u64, growth: Option<i64>) -> ArtifactRow {
    ArtifactRow {
        kind,
        path: PathBuf::from(path),
        bytes,
        mtime_max: 0,
        ecosystem: None,
        hardlinked: false,
        dedup_stale: false,
        allocated_bytes: None,
        allocated_growth_bytes: None,
        local_bytes: 0,
        track: None,
        growth_bytes: growth,
        regrowth_count: 0,
        observed_at: 1_726_000_000,
        confidence: Confidence::High,
        source: Source::new("fixture"),
        note: None,
        created_at: None,
        containers: Vec::new(),
        shared_with: Vec::new(),
        dangling: false,
        evidence: Vec::new(),
    }
}

fn fixture_report() -> Report {
    Report {
        store_dir: None,
        observed_at: 1_726_000_000,
        root: PathBuf::from("/Users/dev/src"),
        projects: vec![
            ProjectRow {
                project_id: "p-mole".into(),
                name: "mole".into(),
                remote: None,
                ecosystems: Vec::new(),
                worktrees: vec![WorktreeRow {
                    worktree_id: "w-mole".into(),
                    path: PathBuf::from("/Users/dev/src/mole"),
                    kind: WorktreeKind::Main,
                    artifacts: vec![
                        art(
                            ArtifactKind::DependencyTree,
                            "/Users/dev/src/mole/node_modules",
                            420 * 1024 * 1024,
                            Some(180 * 1024 * 1024),
                        ),
                        art(
                            ArtifactKind::BuildOutput,
                            "/Users/dev/src/mole/target",
                            2_147_483_648,
                            Some(1_073_741_824),
                        ),
                        art(
                            ArtifactKind::Source,
                            "/Users/dev/src/mole/src",
                            12 * 1024 * 1024,
                            Some(2048),
                        ),
                    ],
                    signals: vec![Signal {
                        name: "dirty".into(),
                        value: "clean".into(),
                    }],
                    branch: None,
                    github: None,
                    merge_complete: None,
                    idle_secs: None,
                }],
            },
            ProjectRow {
                project_id: "p-slop".into(),
                name: "swamp".into(),
                remote: None,
                ecosystems: Vec::new(),
                worktrees: vec![WorktreeRow {
                    worktree_id: "w-slop".into(),
                    path: PathBuf::from("/Users/dev/src/swamp"),
                    kind: WorktreeKind::Main,
                    artifacts: vec![
                        art(
                            ArtifactKind::BuildOutput,
                            "/Users/dev/src/swamp/target",
                            6_442_450_944,
                            Some(2_684_354_560),
                        ),
                        art(
                            ArtifactKind::DockerImage,
                            "/Users/dev/src/swamp/.docker/img",
                            1_610_612_736,
                            Some(-104_857_600),
                        ),
                    ],
                    signals: vec![],
                    branch: None,
                    github: None,
                    merge_complete: None,
                    idle_secs: None,
                }],
            },
        ],
        unowned: vec![UnownedRow {
            measurement: None,
            path_or_object: "/Users/dev/.cache/leftover".into(),
            bytes: 209_715_200,
            reason: UnownedReason::SharedCache,
            docker_kind: None,
            shared_bytes: None,
            note: None,
            created_at: None,
            containers: Vec::new(),
            shared_with: Vec::new(),
            dangling: false,
            evidence: Vec::new(),
        }],
        dirs_by_worktree: None,
        files_by_worktree: None,
        schedule_line: None,
        github_enrichment: None,
        nested_artifacts: Vec::new(),
        reconciliation: Reconciliation {
            unique_estimate: None,
            attributed: 0,
            unowned: 0,
            walked_total: 0,
            du_total: None,
            docker_attributed: 0,
            docker_unowned: 0,
        },
        notes: vec![],
        series_by_key: Default::default(),
        total_series: Vec::new(),
        series_window_secs: 0,
        summary: Default::default(),
    }
}

fn app() -> App {
    let mut a = App::new(fixture_report(), "/Users/dev/src".into());
    a.filter_text = "0".into();
    a
}

fn frame(app: &App, w: u16, h: u16) -> Vec<String> {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    let b = t.backend().buffer().clone();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| b[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect()
}

fn with_confirm() -> App {
    let mut a = app();
    a.mark_selected();
    a.open_confirm();
    assert!(a.confirm_open, "fixture: confirm must be open");
    a
}

/// Tempting wrong patch: the delete confirm is drawn as a sheet but its keys
/// fall through to the main match, so Tab / Shift-Tab / v / 1-3 switch the
/// view under an open confirm and the next Enter confirms a plan the person
/// is no longer looking at.
#[test]
fn adv2_section_and_view_keys_do_nothing_under_the_delete_confirm() {
    let mut bad = Vec::new();
    for k in [
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Char('v'),
        KeyCode::Char('1'),
        KeyCode::Char('2'),
        KeyCode::Char('3'),
    ] {
        let mut a = with_confirm();
        let marks = a.marked.len();
        handle_key(&mut a, k);
        if a.view != ViewKind::Projects || !a.confirm_open || a.marked.len() != marks {
            bad.push(format!(
                "{k:?}: view {:?} confirm_open {} marks {}->{}",
                a.view,
                a.confirm_open,
                marks,
                a.marked.len()
            ));
        }
        assert!(a.operation.is_none(), "{k:?} started an operation");
    }
    assert!(bad.is_empty(), "keys leaked under the confirm: {bad:#?}");
}

/// Tempting wrong patch: the blocked list or help drawer lets Tab through.
#[test]
fn adv2_section_keys_do_nothing_under_blocked_help_and_picker() {
    for k in [
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Char('v'),
        KeyCode::Char('2'),
    ] {
        let mut a = app();
        a.blocked_open = true;
        handle_key(&mut a, k);
        assert_eq!(a.view, ViewKind::Projects, "blocked {k:?}");
        let mut a = app();
        a.help_open = true;
        handle_key(&mut a, k);
        assert_eq!(a.view, ViewKind::Projects, "help {k:?}");
        for field in 0..4 {
            let mut a = app();
            handle_key(&mut a, KeyCode::Char('/'));
            a.picker.as_mut().unwrap().field = field;
            handle_key(&mut a, k);
            assert_eq!(a.view, ViewKind::Projects, "picker field {field} {k:?}");
            assert!(a.picker.is_some(), "picker closed by {k:?}");
        }
        let mut a = app();
        handle_key(&mut a, KeyCode::Char(':'));
        handle_key(&mut a, k);
        assert_eq!(a.view, ViewKind::Projects, "filter {k:?}");
        assert!(a.editing_filter, "filter closed by {k:?}");
    }
}

/// Tempting wrong patch: `prev` is `(at + 1) % 3` copied from `next`, or
/// `v` stops at the last view instead of wrapping, or Tab walks views.
#[test]
fn adv2_wraps_both_ways_at_both_ends() {
    let mut a = app();
    handle_key(&mut a, KeyCode::BackTab);
    assert_eq!(a.view, ViewKind::Disk, "Shift-Tab on the first section");
    handle_key(&mut a, KeyCode::Tab);
    assert_eq!(a.view, ViewKind::Projects, "Tab on the last section");
    for sec in Section::ALL {
        let mut a = app();
        a.set_section(sec);
        let vs = sec.views();
        for i in 0..vs.len() * 2 + 1 {
            assert_eq!(a.view, vs[i % vs.len()], "{sec:?} step {i}");
            assert_eq!(a.view.section(), sec);
            handle_key(&mut a, KeyCode::Char('v'));
        }
    }
    // Three Tabs and three Shift-Tabs are identities from every view's section.
    for v in ViewKind::ALL {
        let mut a = app();
        a.set_view(v);
        for _ in 0..3 {
            handle_key(&mut a, KeyCode::Tab);
        }
        assert_eq!(a.view.section(), v.section());
        for _ in 0..3 {
            handle_key(&mut a, KeyCode::BackTab);
        }
        assert_eq!(a.view.section(), v.section());
    }
}

/// Tempting wrong patch: the cursor is carried across views unclamped, so a
/// row index valid in a long view points past a shorter one.
#[test]
fn adv2_cursor_is_clamped_and_restored_across_sections() {
    let mut a = app();
    a.set_view(ViewKind::Builds);
    handle_key(&mut a, KeyCode::End);
    let builds_at = a.selected;
    for k in [
        KeyCode::Tab,
        KeyCode::Tab,
        KeyCode::Char('v'),
        KeyCode::Char('1'),
    ] {
        handle_key(&mut a, k);
        let n = a.rows().len();
        assert!(
            a.selected < n.max(1),
            "{:?}: selected {} of {n}",
            a.view,
            a.selected
        );
        let _ = frame(&a, 80, 24);
    }
    assert_eq!(a.view, ViewKind::Projects);
    handle_key(&mut a, KeyCode::Char('v'));
    handle_key(&mut a, KeyCode::Char('v'));
    assert_eq!(a.view, ViewKind::Builds);
    assert_eq!(a.selected, builds_at, "Builds cursor not restored");
}

/// Tempting wrong patch: `views_seen` is set by any view change (so `v`
/// inside Projects hides the hint), or never persisted.
#[test]
fn adv2_views_seen_only_by_leaving_projects() {
    let mut a = app();
    a.views_seen = false;
    let hint = |a: &App| {
        let h = 30;
        frame(a, 120, h)[1 + ui::headline_rows(h) as usize].contains("Tab switches sections")
    };
    assert!(hint(&a), "first-run hint missing");
    for _ in 0..ViewKind::ALL.len() {
        handle_key(&mut a, KeyCode::Char('v'));
        assert_eq!(a.view.section(), Section::Projects);
    }
    assert!(!a.views_seen, "v inside Projects ended the hint");
    handle_key(&mut a, KeyCode::Char('3'));
    assert!(a.views_seen);
    handle_key(&mut a, KeyCode::Char('1'));
    assert!(!hint(&a), "hint shown again after Disk was opened");
}

/// Tempting wrong patch: `UiState` gains `deny_unknown_fields`, or the loader
/// fails the whole file on a key an older (or newer) swamp wrote, dropping
/// the person's filter and sort.
#[test]
fn adv2_old_ui_state_with_removed_view_keys_loads() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("ui_state.json"),
        r#"{"filter":"kind:build","sort":"size","reverse":true,"keep_executables":true,
            "view":"external","view_key":"9","last_view":7,"reclaim_key":"c"}"#,
    )
    .unwrap();
    let s = swamp_tui::app::load_ui_state(d.path());
    assert_eq!(s.filter, "kind:build");
    assert_eq!(s.sort, "size");
    assert!(s.reverse && s.keep_executables);
    assert!(!s.views_seen, "an old file has never seen the new views");
    // The v0.7.5 file had no views_seen at all.
    std::fs::write(d.path().join("ui_state.json"), r#"{"filter":"x"}"#).unwrap();
    let s = swamp_tui::app::load_ui_state(d.path());
    assert_eq!(s.filter, "x");
}

/// Tempting wrong patch: the section strip or footer is laid out without a
/// height budget, so at small heights the key legend is pushed off, or a
/// line math underflows at width 1. Every view, widths 1..=200 x heights
/// 1..=60 (sampled on one axis each), no panic, legend kept at h >= 3.
#[test]
fn adv2_every_view_every_size_keeps_the_legend() {
    let mut bad = Vec::new();
    let mut sizes = Vec::new();
    // Every width to 24, then every 10th to 200 (and the edges); heights
    // likewise: enough to catch width and height math at small sizes and
    // at the thresholds (12, 16, 22) without a minute of debug rendering.
    let sampled = |max: u16| -> Vec<u16> {
        (1..=24u16)
            .chain((30..=max).step_by(10))
            .chain([max])
            .collect()
    };
    for w in sampled(200) {
        for h in [1u16, 2, 3, 4, 5, 8, 11, 12, 15, 16, 21, 22, 24, 60] {
            sizes.push((w, h));
        }
    }
    for h in sampled(60) {
        for w in [1u16, 10, 20, 40, 80, 200] {
            sizes.push((w, h));
        }
    }
    for v in ViewKind::ALL {
        let mut a = app();
        a.views_seen = false;
        a.set_view(v);
        for &(w, h) in &sizes {
            let f = frame(&a, w, h);
            let hr = ui::headline_rows(h) as usize;
            // At least one row is needed for each fixed chrome line, plus
            // a body row; below that, the key legend takes precedence.
            if w as usize >= v.title().len().max(16) && h as usize >= hr + 7 {
                assert!(
                    f[2 + hr].contains(v.title()),
                    "{v:?} {w}x{h} lost its title: {}",
                    f[2 + hr]
                );
            }
            if h >= 3 && w >= 16 {
                let last = f.last().unwrap();
                if !last.contains("q quit") && !last.contains("? help") {
                    bad.push(format!("{v:?} {w}x{h}: last row {last:?}"));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "{} sizes lose the legend, e.g. {:#?}",
        bad.len(),
        &bad[..bad.len().min(6)]
    );
}

/// Tempting wrong patch: the legend names a key nothing is bound to.
/// Every single-key token in the footer legend must change something when
/// pressed (derived by pressing it, not by a copied list).
#[test]
fn adv2_removed_view_keys_are_unbound() {
    for k in ['c', 'D', 'I', '4', '5', '6', '7', '8', '9'] {
        for v in ViewKind::ALL {
            let mut a = app();
            a.set_view(v);
            let sel = a.selected;
            handle_key(&mut a, KeyCode::Char(k));
            assert_eq!(a.view, v, "{k} moved {v:?}");
            assert_eq!(a.selected, sel, "{k} moved the cursor");
            assert!(!a.help_open && a.picker.is_none() && !a.editing_filter && !a.quit);
        }
    }
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs" || x == "md") {
            out.push(p);
        }
    }
}

/// Tempting wrong patch: the keymap is rewired but a doc or comment still
/// teaches a removed key (`c` for Reclaim, `D` Disk, `I`, digits 4-9 for
/// views).
#[test]
fn adv2_no_doc_or_help_teaches_a_removed_view_key() {
    let r = repo();
    let mut files = Vec::new();
    walk(&r.join("crates/tui/src"), &mut files);
    walk(&r.join("skills"), &mut files);
    for f in ["docs/usage.md", "DESIGN.md", "README.md"] {
        files.push(r.join(f));
    }
    let pats = [
        "press c ", "press c,", "press D", "press I", "`c`", "`D`", "`I`", "(`v`/`4`", "(`v`/`5`",
        "(`v`/`6`", "(`v`/`7`", "(`v`/`8`", "(`v`/`9`", "`4`", "`5`", "`6`", "`7`", "`8`", "`9`",
    ];
    let mut bad = Vec::new();
    for f in files {
        let Ok(t) = std::fs::read_to_string(&f) else {
            continue;
        };
        for (n, line) in t.lines().enumerate() {
            let l = line.to_lowercase();
            let about_keys =
                l.contains("key") || l.contains("press") || l.contains("view") || l.contains("tui");
            // A line recording what 0.7.x did is history, not teaching.
            if !about_keys || line.contains("0.7") {
                continue;
            }
            for p in pats {
                if line.contains(p) {
                    bad.push(format!(
                        "{}:{}: {}",
                        f.strip_prefix(&r).unwrap_or(&f).display(),
                        n + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "removed keys still taught:\n{}",
        bad.join("\n")
    );
}

/// Tempting wrong patch: a key mash reaches `confirm_delete` (Enter after a
/// view switch lands on a confirm opened elsewhere) or panics on an index. Keys that
/// spawn real work (Space/Backspace/A review, R, i) are left out so the fuzz
/// runs no worker; Enter, Esc, Tab, v, digits, arrows, sheets are in.
#[test]
fn adv2_seeded_key_mash_never_panics_or_deletes() {
    let keys = [
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Char('v'),
        KeyCode::Char('1'),
        KeyCode::Char('2'),
        KeyCode::Char('3'),
        KeyCode::Enter,
        KeyCode::Esc,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::Char('/'),
        KeyCode::Char(':'),
        KeyCode::Char('?'),
        KeyCode::Char('b'),
        KeyCode::Char('0'),
        KeyCode::Char('g'),
        KeyCode::Char('s'),
        KeyCode::Char('r'),
        KeyCode::Char('c'),
        KeyCode::Char('D'),
        KeyCode::Char('9'),
        KeyCode::Backspace,
    ];
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    for round in 0..40 {
        let mut a = app();
        a.views_seen = false;
        for step in 0..400 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let k = keys[(x % keys.len() as u64) as usize];
            // Backspace only as text: it opens a review outside a field.
            if k == KeyCode::Backspace && !(a.editing_filter || a.picker.is_some()) {
                continue;
            }
            handle_key(&mut a, k);
            assert!(
                a.operation.is_none(),
                "round {round} step {step} {k:?} started {:?}",
                a.operation.as_ref().map(|o| o.label)
            );
            if a.quit {
                break;
            }
            let n = a.rows().len();
            assert!(
                a.selected < n.max(1),
                "round {round} step {step}: {} of {n}",
                a.selected
            );
            if step % 37 == 0 {
                let _ = frame(&a, (x % 200) as u16 + 1, (x % 60) as u16 + 1);
            }
        }
    }
}
