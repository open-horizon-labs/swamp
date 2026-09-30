//! v0.8.0 G4b in the TUI: the developer-storage headline block, the view
//! strip and the direct keys for Reclaim and Disk. Fixture-only.
//!
//! Each test names the tempting wrong patch it fails.

use std::path::PathBuf;

use crossterm::event::KeyCode;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};
use swamp_core::external::ExternalUnit;
use swamp_core::growth::VolumeMetaRow;
use swamp_core::last_used::LastUsed;
use swamp_core::locations::{Provenance, StorageCategory};
use swamp_core::report::Report;
use swamp_core::volume_ledger::{
    Category as LedgerCategory, Exactness, LedgerReading, Row, account,
};
use swamp_tui::app::{App, ViewKind};
use swamp_tui::ui;

const GB: u64 = 1_000_000_000;

fn now() -> u64 {
    swamp_core::entities::now()
}

fn unit(detector: &str, category: StorageCategory, path: &str, bytes: u64) -> ExternalUnit {
    ExternalUnit {
        detector_id: detector.to_string(),
        detector_name: detector.to_string(),
        category,
        provenance: Provenance::BuiltinConvention,
        path: PathBuf::from(path),
        bytes,
        mtime_max: 0,
        hardlinked: false,
        growth_bytes: None,
        regrowth_count: 0,
        observed_at: now(),
        consumers: Vec::new(),
        note: None,
        evidence: Vec::new(),
        bytes_counted_elsewhere: 0,
        overlap_count: 0,
        last_used: LastUsed::default(),
        children: Vec::new(),
    }
}

fn lrow(path: &str, cat: LedgerCategory, bytes: Option<u64>) -> Row {
    Row {
        path: path.to_string(),
        category: cat,
        bytes,
        overlap_bytes: 0,
        entries: None,
        unreadable: 0,
        measured_at: now() - 3 * 3600,
        method: "walk: allocated bytes, lstat only".to_string(),
        exactness: if bytes.is_some() {
            Exactness::Exact
        } else {
            Exactness::NotMeasured
        },
        note: None,
    }
}

fn measured(measured_at: u64) -> LedgerReading {
    let rows = vec![
        lrow("/h/src", LedgerCategory::Declared, Some(30 * GB)),
        lrow(
            "/h/.rustup/toolchains",
            LedgerCategory::Catalog,
            Some(5 * GB),
        ),
        lrow("/Users/x/Movies", LedgerCategory::Other, Some(9 * GB)),
        lrow(
            "APFS volume Preboot (disk3s2)",
            LedgerCategory::System,
            Some(10 * GB),
        ),
        lrow("/Users/x/Pictures", LedgerCategory::Unreadable, None),
    ];
    let meta = VolumeMetaRow {
        measured_at,
        cycle_started_at: 1,
        cycle_complete_at: measured_at,
        complete: true,
        budget_secs: 120,
        budget_used_ms: 1000,
        statfs_at: measured_at,
        container_total: Some(500 * GB),
        container_used: Some(100 * GB),
        container_free: Some(400 * GB),
        data_volume_used: Some(90 * GB),
    };
    LedgerReading::Measured(Box::new(account(&rows, &meta)))
}

fn app() -> App {
    let mut r = Report::empty(PathBuf::from("/h/src"));
    r.observed_at = now() - 240;
    r.reconciliation.walked_total = 30 * GB;
    let mut a = App::new(r, PathBuf::from("/h/src"));
    a.filter_text = "0".into();
    a.set_external_units(vec![
        unit(
            "rustup",
            StorageCategory::Installation,
            "/h/.rustup/toolchains",
            5 * GB,
        ),
        unit("uv", StorageCategory::Cache, "/h/.cache/uv", 3 * GB),
        unit("claude-code", StorageCategory::LocalState, "/h/.claude", GB),
    ]);
    a.set_ledger(measured(now() - 3 * 3600));
    a
}

fn buf(app: &App, w: u16, h: u16) -> ratatui::buffer::Buffer {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    t.backend().buffer().clone()
}

fn frame(app: &App, w: u16, h: u16) -> Vec<String> {
    let b = buf(app, w, h);
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| b[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

type Setter = Box<dyn Fn(&mut App)>;

fn states() -> Vec<(&'static str, Setter)> {
    vec![
        ("fresh", Box::new(|_a: &mut App| {})),
        (
            "stale",
            Box::new(|a: &mut App| {
                a.live_age = true;
                a.report.observed_at = now() - 3 * 3600;
            }),
        ),
        (
            "ledger missing",
            Box::new(|a: &mut App| a.set_ledger(LedgerReading::NotMeasured)),
        ),
        (
            "ledger unreadable",
            Box::new(|a: &mut App| a.set_ledger(LedgerReading::Unreadable("bad file".into()))),
        ),
        (
            "ledger newer",
            Box::new(|a: &mut App| a.set_ledger(LedgerReading::Newer { unknown_rows: 3 })),
        ),
        (
            "ledger future",
            Box::new(|a: &mut App| a.set_ledger(measured(now() + 86_400))),
        ),
        (
            "scan running",
            Box::new(|a: &mut App| {
                a.observing = Some((0, 0));
                a.observing_started = Some(std::time::Instant::now());
            }),
        ),
        (
            "previous scope",
            Box::new(|a: &mut App| a.previous_scope_roots = Some(3)),
        ),
        ("no index", Box::new(|a: &mut App| a.has_index = false)),
        (
            "first-run hint",
            Box::new(|a: &mut App| a.views_seen = false),
        ),
    ]
}

/// Tempting wrong patch: a state (a warning, a missing ledger, a running
/// scan, a first-run line) is drawn by inserting a row, so the table shifts
/// down when it appears. The headline block is the same rows in every
/// state and every view, at every width; the strip, the filter line and the
/// table heading never move, and the keys stay on the last row.
#[test]
fn no_row_moves_between_states_or_views_at_any_width() {
    for w in [40u16, 50, 80, 120] {
        for h in [24u16, 30] {
            let hr = ui::headline_rows(h) as usize;
            let want_rows = h as usize;
            for (name, set) in states() {
                for view in ViewKind::ALL {
                    let mut a = app();
                    set(&mut a);
                    a.width = w;
                    a.height = h;
                    a.set_view(view);
                    let f = frame(&a, w, h);
                    let ctx = format!("{w}x{h} {name} {view:?}\n{}", f.join("\n"));
                    assert_eq!(f.len(), want_rows);
                    // The strip row names the current view by its key.
                    let strip = &f[1 + hr];
                    assert!(
                        strip.contains(&format!("{} {}", view.key(), view.title())),
                        "strip: {ctx}"
                    );
                    // The filter line, then the body.
                    assert!(f[2 + hr].starts_with("filter:"), "filter line: {ctx}");
                    let body = &f[3 + hr];
                    if !a.rows().is_empty() {
                        assert!(body.starts_with("Name"), "heading at the same row: {ctx}");
                    }
                    // Keys are on the last row, `q quit` always.
                    assert!(f[h as usize - 1].contains("q quit"), "keys: {ctx}");
                    // The headline is the first line of the block.
                    assert!(
                        f[1].starts_with("Developer storage")
                            || f[1].starts_with("Dev storage")
                            || f[1].starts_with("Dev "),
                        "headline row: {ctx}"
                    );
                    assert!(!f[1..=hr].iter().any(|l| l.contains('\u{2014}')), "{ctx}");
                }
            }
        }
    }
}

/// Tempting wrong patch: the headline block's height depends on the
/// terminal size only through one rule, and every size gets a defined
/// answer (no size makes the table vanish).
#[test]
fn the_headline_block_height_is_a_function_of_the_terminal_height_only() {
    assert_eq!(ui::headline_rows(24), 4);
    assert_eq!(ui::headline_rows(22), 4);
    assert_eq!(ui::headline_rows(21), 2);
    assert_eq!(ui::headline_rows(16), 2);
    assert_eq!(ui::headline_rows(15), 1);
    assert_eq!(ui::headline_rows(12), 1);
    assert_eq!(ui::headline_rows(11), 0);
    assert_eq!(ui::headline_rows(0), 0);
    for h in [0u16, 1, 5, 10, 12, 15, 16, 21, 22, 40] {
        for w in [0u16, 1, 10, 40, 80] {
            let a = app();
            let _ = frame(&a, w.max(1), h.max(1));
        }
    }
}

/// Tempting wrong patch: the percent or the ages are only in the report,
/// or only at wide sizes. The first screen at 80 columns shows the
/// headline with its percent, the ages, and the pointers with their keys.
#[test]
fn the_first_screen_at_80_columns_shows_the_headline_ages_and_pointers() {
    let a = app();
    let f = frame(&a, 80, 24);
    assert_eq!(
        f[1],
        "Developer storage: 39.0GB across 4 locations (39.0% of used)"
    );
    // Breakdown: projects, toolchains, caches, agents.
    assert!(f[2].contains("projects 30.0GB"), "{}", f[2]);
    // Ages and the ledger's parts.
    assert!(f[3].contains("observed 4 min ago"), "{}", f[3]);
    assert!(
        f[3].contains("ledger 3 h ago") || f[3].contains("disk ledger measured 3 h ago"),
        "{}",
        f[3]
    );
    // The pointers name the views and the keys; at 80 columns they take
    // their shorter form.
    assert!(
        f[4].contains("Reclaim: 3.0GB regenerable, 3 units (c)"),
        "{}",
        f[4]
    );
    assert!(f[4].contains("Disk: ledger 3 h ago (D)"), "{}", f[4]);
    // Wide enough, they spell the key out.
    let wide = frame(&a, 120, 30);
    assert!(
        wide[4].contains("Reclaim: 3.0GB regenerable across 3 units (press c)"),
        "{}",
        wide[4]
    );
    assert!(
        wide[4].contains("Disk: ledger measured 3 h ago (press D)"),
        "{}",
        wide[4]
    );
}

/// Tempting wrong patch: the disk state is folded into a percent (or the
/// line is dropped) when the ledger is missing, unreadable, newer or
/// future-dated. Each says what it is, at 80 columns, with no percent.
#[test]
fn a_ledger_that_cannot_be_used_is_named_and_gives_no_percent() {
    for (name, set) in states() {
        if !name.starts_with("ledger") {
            continue;
        }
        let mut a = app();
        set(&mut a);
        let f = frame(&a, 80, 24);
        assert!(!f[1].contains('%'), "{name}: {}", f[1]);
        assert!(f[3].contains("ledger"), "{name}: {}", f[3]);
        assert!(f[4].contains("Disk"), "{name}: {}", f[4]);
    }
    // The previous scope is stated on the block itself.
    let mut a = app();
    a.previous_scope_roots = Some(3);
    let f = frame(&a, 80, 24);
    assert!(f[3].contains("previous scope (3 roots)"), "{}", f[3]);
    let f = frame(&a, 40, 24);
    assert!(f[3].contains("previous scope"), "{}", f[3]);
}

/// Tempting wrong patch: drawing the block asks the disk or a tool for a
/// number. It is a pure function of stored facts.
#[test]
fn drawing_the_block_and_every_view_does_no_work() {
    let (_, work) = swamp_core::work_counters::measured(|| {
        let mut a = app();
        for v in ViewKind::ALL {
            a.set_view(v);
            let _ = frame(&a, 80, 24);
            let _ = frame(&a, 120, 30);
        }
    });
    assert_eq!(work, swamp_core::work_counters::WorkCounters::default());
}

/// Tempting wrong patch: the audit warning is only in `swamp report`, so a
/// person who lives in the TUI never sees that the walk is being
/// contradicted. It is the first clause of the block's third line at every
/// width, and the row does not move.
#[test]
fn a_spot_audit_that_disagrees_is_the_first_thing_on_the_disk_line_at_every_width() {
    let mut rows = vec![
        lrow("/h/src", LedgerCategory::Declared, Some(30 * GB)),
        lrow("/Users/x/Movies", LedgerCategory::Other, Some(9 * GB)),
    ];
    let mut audit = lrow(
        "spot audit: /Users/x/Library",
        LedgerCategory::Audit,
        Some(130 * GB),
    );
    audit.entries = Some(100 * GB);
    audit.exactness = Exactness::Estimated;
    rows.push(audit);
    let meta = VolumeMetaRow {
        measured_at: now() - 3600,
        cycle_started_at: 1,
        cycle_complete_at: now() - 3600,
        complete: true,
        budget_secs: 120,
        budget_used_ms: 1000,
        statfs_at: now() - 3600,
        container_total: Some(500 * GB),
        container_used: Some(100 * GB),
        container_free: Some(400 * GB),
        data_volume_used: Some(90 * GB),
    };
    let mut a = app();
    a.set_ledger(LedgerReading::Measured(Box::new(account(&rows, &meta))));
    for w in [40u16, 50, 80, 120] {
        let f = frame(&a, w, 24);
        assert!(f[3].starts_with("FLAG: "), "{w}: {}", f[3]);
        assert!(f[3].contains("spot audit"), "{w}: {}", f[3]);
        if w >= 80 {
            assert!(f[3].contains("swamp report --view disk"), "{w}: {}", f[3]);
        }
        // Same rows as the calm state: strip and table heading unmoved.
        assert!(f[5].contains("Projects"), "{w}: {}", f[5]);
    }
    // The Disk view shows the audit as its own row.
    a.set_view(ViewKind::Disk);
    let f = frame(&a, 120, 30).join("\n");
    assert!(
        f.contains("FLAG: walk spot audit disagrees with the ledger"),
        "{f}"
    );
}

// ---------------------------------------------------------------------
// The view strip
// ---------------------------------------------------------------------

/// Tempting wrong patch: the strip highlights the current view with a
/// color (lost under NO_COLOR and on a light theme), or shows a window that
/// forgets the current view. It is reverse video only, on exactly the
/// current tab, at every width.
#[test]
fn the_strip_highlights_the_current_view_with_reverse_video_and_no_color() {
    for w in [40u16, 50, 80, 120] {
        for view in ViewKind::ALL {
            let mut a = app();
            a.set_view(view);
            let b = buf(&a, w, 24);
            let y = 1 + ui::headline_rows(24);
            let label = format!("{} {}", view.key(), view.title());
            let cells: Vec<String> = (0..w).map(|x| b[(x, y)].symbol().to_string()).collect();
            let line: String = cells.concat();
            let at = line
                .find(&label)
                .unwrap_or_else(|| panic!("{w} {view:?}: {line}"));
            let col = line[..at].chars().count() as u16;
            let reversed: Vec<u16> = (0..w)
                .filter(|x| b[(*x, y)].modifier.contains(Modifier::REVERSED))
                .collect();
            let want: Vec<u16> = (col..col + label.chars().count() as u16).collect();
            assert_eq!(
                reversed, want,
                "{w} {view:?}: reverse video is the current tab only: {line}"
            );
            for x in 0..w {
                assert_eq!(b[(x, y)].fg, Color::Reset, "{w} {view:?}: no color");
                assert_eq!(b[(x, y)].bg, Color::Reset, "{w} {view:?}: no color");
            }
        }
    }
}

/// Tempting wrong patch: at 80 columns the strip is the whole 113-character
/// list clipped at the edge (Reclaim and Disk fall off the end), or
/// narrower screens show a partial word. It shows a window of whole tabs
/// around the current one with `…` where it is cut.
#[test]
fn the_strip_shows_whole_tabs_around_the_current_view_with_ellipses() {
    let strip = |view: ViewKind, w: u16| -> String {
        let mut a = app();
        a.set_view(view);
        frame(&a, w, 24)[1 + ui::headline_rows(24) as usize].clone()
    };
    // Wide enough for everything: all twelve tabs, in order.
    let wide = strip(ViewKind::Projects, 200);
    let mut at = 0;
    for v in ViewKind::ALL {
        let label = format!("{} {}", v.key(), v.title());
        let i = wide[at..]
            .find(&label)
            .unwrap_or_else(|| panic!("{label} in {wide}"));
        at += i + label.len();
    }
    assert!(wide.contains("v next"), "{wide}");
    // At 80: Projects shows its first tabs then `…`, and names the jump keys.
    let s80 = strip(ViewKind::Projects, 80);
    assert!(s80.starts_with("1 Projects  2 Tree"), "{s80}");
    assert!(s80.contains('…'), "{s80}");
    assert!(s80.contains("v next"), "{s80}");
    // Reclaim at 80: cut on the left, the current tab and its neighbors on screen.
    let r80 = strip(ViewKind::Reclaim, 80);
    assert!(r80.contains("c Reclaim"), "{r80}");
    assert!(r80.contains('…'), "{r80}");
    // Every tab is a whole label wherever the window sits, at 40 and 50.
    for w in [40u16, 50] {
        for v in ViewKind::ALL {
            let s = strip(v, w);
            assert!(
                s.contains(&format!("{} {}", v.key(), v.title())),
                "{w} {v:?}: {s}"
            );
            assert!(s.chars().count() <= w as usize);
        }
    }
}

// ---------------------------------------------------------------------
// The direct keys
// ---------------------------------------------------------------------

/// Tempting wrong patch: a new view gets a key another handler already
/// owns (`d` opens the blocked list on a plan, `a` sorts by age), or two
/// views share one. Every view has exactly one key, the keys are distinct,
/// they open their view, none is a key the main keymap binds otherwise, and
/// `?` help documents each.
#[test]
fn every_view_is_reachable_by_one_documented_key_and_no_key_is_bound_twice() {
    // 1. One key per view, all distinct, and `from_key` inverts `key`.
    let mut seen: Vec<char> = Vec::new();
    for v in ViewKind::ALL {
        let k = v.key();
        assert!(!seen.contains(&k), "{k} is the key of two views");
        seen.push(k);
        assert_eq!(ViewKind::from_key(k), Some(v));
    }
    assert_eq!(seen.len(), ViewKind::ALL.len());
    assert_eq!(ViewKind::from_key('0'), None, "0 clears the filter");
    // 2. Pressing the key opens the view.
    for v in ViewKind::ALL {
        let mut a = app();
        a.set_view(if v == ViewKind::Projects {
            ViewKind::Tree
        } else {
            ViewKind::Projects
        });
        swamp_tui::handle_key(&mut a, KeyCode::Char(v.key()));
        assert_eq!(a.view, v, "key {} opens {v:?}", v.key());
    }
    // 3. No other arm of the main keymap binds a view key. The keymap is
    // read from the source: every `KeyCode::Char('x')` between the start of
    // the main `match code` and the end of the function.
    let src = include_str!("../src/lib.rs");
    let start = src
        .find("    match code {\n        KeyCode::Char('q') => app.quit = true,")
        .expect("the main keymap");
    let end = start + src[start..].find("\n}\n").expect("end of handle_key_mod");
    let region = &src[start..end];
    let mut bound: Vec<char> = Vec::new();
    let mut rest = region;
    while let Some(i) = rest.find("KeyCode::Char('") {
        let tail = &rest[i + "KeyCode::Char('".len()..];
        let c = tail.chars().next().unwrap();
        // `Char(k) if ...` (the view arm) has no literal; a range arm is
        // `Char('1'..='9')` and would show as '1' here.
        bound.push(c);
        rest = &tail[c.len_utf8()..];
    }
    for v in ViewKind::ALL {
        assert!(
            !bound.contains(&v.key()),
            "the main keymap binds {:?} as well as the {v:?} view",
            v.key()
        );
    }
    // No literal key is bound twice in the main keymap either (`d` appears
    // once, guarded by the plan being open).
    let mut sorted = bound.clone();
    sorted.sort_unstable();
    let dups: Vec<char> = sorted
        .windows(2)
        .filter(|w| w[0] == w[1])
        .map(|w| w[0])
        .collect();
    assert!(dups.is_empty(), "bound twice in the main keymap: {dups:?}");
    // 4. `?` help lists every view with its key and its description.
    let mut a = app();
    a.help_open = true;
    let mut text = String::new();
    for scroll in [0usize, 20, 40, 60, 80] {
        a.help_scroll.set(scroll);
        text.push_str(&frame(&a, 120, 30).join("\n"));
        text.push('\n');
    }
    for v in ViewKind::ALL {
        assert!(
            text.contains(&format!("{}:", v.title())),
            "help lacks {v:?}"
        );
        let head: String = v.describe().chars().take(24).collect();
        assert!(
            text.contains(&head),
            "help lacks the description of {v:?}: {head}"
        );
    }
}

/// Tempting wrong patch: `c`, `D` or `I` are handled in a modal that a
/// later key can never reach, or Ctrl-C is taken for `c`.
#[test]
fn the_direct_keys_work_from_any_view_and_ctrl_c_is_not_c() {
    let mut a = app();
    swamp_tui::handle_key(&mut a, KeyCode::Char('c'));
    assert_eq!(a.view, ViewKind::Reclaim);
    swamp_tui::handle_key(&mut a, KeyCode::Char('D'));
    assert_eq!(a.view, ViewKind::Disk);
    swamp_tui::handle_key(&mut a, KeyCode::Char('I'));
    assert_eq!(a.view, ViewKind::Agents);
    swamp_tui::handle_key(&mut a, KeyCode::Char('9'));
    assert_eq!(a.view, ViewKind::External);
    // v cycles through all twelve and comes back.
    let mut a = app();
    let mut order = vec![a.view];
    for _ in 0..12 {
        swamp_tui::handle_key(&mut a, KeyCode::Char('v'));
        order.push(a.view);
    }
    assert_eq!(order.first(), order.last());
    for v in ViewKind::ALL {
        assert!(order.contains(&v), "{v:?} is not reached by v");
    }
    // Ctrl-C quits (or cancels), it never opens Reclaim.
    let mut a = app();
    swamp_tui::handle_terminal_key(
        &mut a,
        crossterm::event::KeyEvent::new(
            KeyCode::Char('c'),
            crossterm::event::KeyModifiers::CONTROL,
        ),
    );
    assert_eq!(a.view, ViewKind::Projects);
}

/// Tempting wrong patch: the legend grows and pushes `? help  q quit` (or
/// refresh and delete) off an 80 column screen to make room for the new
/// keys. The priority order stays; the new keys appear only where they fit.
#[test]
fn the_legend_keeps_its_priority_order_and_adds_the_direct_keys_only_where_they_fit() {
    let a = app();
    let f80 = frame(&a, 80, 24);
    let last = &f80[23];
    assert!(
        last.starts_with("/ filter  v view  R refresh  ⌫ delete"),
        "{last}"
    );
    assert!(last.contains("? help  q quit"), "{last}");
    let f120 = frame(&a, 120, 30);
    let last = &f120[29];
    assert!(last.contains("c reclaim  D disk"), "{last}");
    assert!(last.contains("? help  q quit"), "{last}");
    // In the Reclaim view (nothing to mark) the direct keys reach 80.
    let mut a = app();
    a.set_view(ViewKind::Reclaim);
    let last = &frame(&a, 80, 24)[23];
    assert!(last.contains("? help  q quit"), "{last}");
}

// ---------------------------------------------------------------------
// The first-run pointer
// ---------------------------------------------------------------------

/// Tempting wrong patch: the pointer line is a permanent banner, or it is
/// remembered only in memory, or it hides on any key. It shows on a store
/// that has never opened Reclaim or Disk, ends when either is opened, and
/// is written to `ui_state.json` (additive; an older reader ignores it).
#[test]
fn the_first_run_line_hides_after_reclaim_or_disk_is_opened_and_stays_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = app();
    a.store_dir = Some(dir.path().to_path_buf());
    a.views_seen = false;
    let want = "New: press c for Reclaim, D for Disk. Hides after you open either.";
    let f = frame(&a, 80, 24);
    assert_eq!(f[4], want);
    // Other keys do not end it.
    swamp_tui::handle_key(&mut a, KeyCode::Char('v'));
    swamp_tui::handle_key(&mut a, KeyCode::Char('1'));
    assert_eq!(frame(&a, 80, 24)[4], want);
    // Opening Disk does.
    swamp_tui::handle_key(&mut a, KeyCode::Char('D'));
    assert!(a.views_seen);
    let f = frame(&a, 80, 24);
    assert!(!f[4].starts_with("New:"), "{}", f[4]);
    assert!(f[4].contains("Reclaim:"), "{}", f[4]);
    // The state is on disk, written off the event thread, and a new
    // session over the same store starts with it set.
    a.flush_ui_state();
    let raw = std::fs::read_to_string(dir.path().join("ui_state.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["views_seen"], true, "{raw}");
    assert!(swamp_tui::app::load_ui_state(dir.path()).views_seen);
    // An older ui_state.json (no such key) loads as "not seen"; an older
    // reader would ignore the key it does not know.
    let old = tempfile::tempdir().unwrap();
    std::fs::write(
        old.path().join("ui_state.json"),
        r#"{"filter":"","sort":"","reverse":false,"keep_executables":false}"#,
    )
    .unwrap();
    assert!(!swamp_tui::app::load_ui_state(old.path()).views_seen);
    // Reclaim ends it too, on a fresh app.
    let mut b = app();
    b.views_seen = false;
    swamp_tui::handle_key(&mut b, KeyCode::Char('c'));
    assert!(b.views_seen);
}

// ---------------------------------------------------------------------
// The Disk view
// ---------------------------------------------------------------------

/// Tempting wrong patch: an unreadable folder shows `0B` in the Disk
/// view, or the view works on a missing ledger by measuring one. It lists
/// the parts, shows unreadable as unmeasured, and with no ledger says what
/// to run, listing nothing itself.
#[test]
fn the_disk_view_lists_the_ledger_parts_and_never_shows_unreadable_as_zero() {
    let mut a = app();
    a.set_view(ViewKind::Disk);
    let f = frame(&a, 120, 30).join("\n");
    assert!(
        f.contains("Accounted: declared roots and catalog units"),
        "{f}"
    );
    assert!(f.contains("Everything else (not developer storage)"), "{f}");
    assert!(f.contains("/Users/x/Movies"), "{f}");
    assert!(f.contains("System volumes"), "{f}");
    let pic = f
        .lines()
        .find(|l| l.contains("/Users/x/Pictures"))
        .expect("named");
    assert!(pic.contains("unmeasured") && !pic.contains("0B"), "{pic}");
    // No ledger: the empty state names the command, and nothing is listed.
    let mut b = app();
    b.set_ledger(LedgerReading::NotMeasured);
    b.set_view(ViewKind::Disk);
    let f = frame(&b, 80, 24).join(" ");
    let f = f.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        f.contains("disk ledger: not measured yet; run swamp observe --volume"),
        "{f}"
    );
    assert!(f.contains("opening the UI never scans"), "{f}");
    let (_, work) = swamp_core::work_counters::measured(|| frame(&b, 80, 24));
    assert_eq!(work, swamp_core::work_counters::WorkCounters::default());
}

/// Tempting wrong patch: the view keys are matched before the modal
/// handlers, so typing `c`, `D` or `I` into the filter (or the picker's
/// text field), or pressing them on the plan, the blocked list or help,
/// switches views instead. Every modal owns its keys first.
#[test]
fn the_view_keys_type_letters_in_the_filter_and_do_nothing_in_other_modals() {
    // Filter text editing: the letters land in the text.
    let mut a = app();
    swamp_tui::handle_key(&mut a, KeyCode::Char(':'));
    assert!(a.editing_filter);
    for k in ['c', 'D', 'I'] {
        swamp_tui::handle_key(&mut a, KeyCode::Char(k));
    }
    assert!(a.filter_text.ends_with("cDI"), "{}", a.filter_text);
    assert_eq!(a.view, ViewKind::Projects);
    swamp_tui::handle_key(&mut a, KeyCode::Esc);
    // Picker: its text field types them; its other fields ignore them.
    let mut a = app();
    swamp_tui::handle_key(&mut a, KeyCode::Char('/'));
    for k in ['c', 'D', 'I'] {
        swamp_tui::handle_key(&mut a, KeyCode::Char(k));
    }
    assert_eq!(a.view, ViewKind::Projects, "picker field");
    swamp_tui::handle_key(&mut a, KeyCode::Esc);
    // Help, the plan sheet and the blocked list keep their own keys.
    let mut a = app();
    a.help_open = true;
    swamp_tui::handle_key(&mut a, KeyCode::Char('c'));
    assert_eq!(a.view, ViewKind::Projects);
    // On an open plan the view keys act as the digits always did (the
    // plan is not a modal here); they bind nothing the plan uses (its keys
    // are Enter, Esc, d and k).
    let mut a = app();
    a.confirm_open = true;
    swamp_tui::handle_key(&mut a, KeyCode::Char('9'));
    let digit_switches = a.view != ViewKind::Projects;
    let mut b = app();
    b.confirm_open = true;
    swamp_tui::handle_key(&mut b, KeyCode::Char('c'));
    assert_eq!(b.view != ViewKind::Projects, digit_switches);
    let mut a = app();
    a.blocked_open = true;
    swamp_tui::handle_key(&mut a, KeyCode::Char('D'));
    assert_eq!(a.view, ViewKind::Projects);
    assert!(a.blocked_open);
}
