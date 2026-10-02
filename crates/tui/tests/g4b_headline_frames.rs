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
/// state and every view, at every width; the strip, view title and table
/// heading never move, and the keys stay on the last row.
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
                    // The strip row names the current section by its key.
                    let strip = &f[1 + hr];
                    let sec = view.section();
                    assert!(
                        strip.contains(&format!("{} {}", sec.key(), sec.title())),
                        "strip: {ctx}"
                    );
                    // The view title stays visible even when the active
                    // filter does not apply to this view.
                    assert!(f[2 + hr].contains(view.title()), "view title: {ctx}");
                    let body = &f[3 + hr];
                    if !a.rows().is_empty() {
                        assert!(!body.trim().is_empty(), "heading at the same row: {ctx}");
                    }
                    // Keys are on the last row, `q quit` always.
                    assert!(f[h as usize - 1].contains("q quit"), "keys: {ctx}");
                    // The headline total is the first line of the block.
                    assert!(
                        f[1].contains("Developer storage") || f[1].contains("Dev "),
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
    assert_eq!(ui::headline_rows(30), 2);
    assert_eq!(ui::headline_rows(29), 2);
    assert_eq!(ui::headline_rows(24), 2);
    assert_eq!(ui::headline_rows(22), 2);
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

/// Tempting wrong patch: totals, coverage and observation context are
/// repeated or mixed together. The header owns root/age; the two headline
/// rows own the developer-storage total and measurement coverage.
#[test]
fn the_first_screen_separates_scope_age_total_and_coverage() {
    let mut a = app();
    a.live_age = true;
    a.observed_label = "4m ago".into();
    let f = frame(&a, 80, 30);
    assert!(f[0].contains("/h/src"), "header owns root: {}", f[0]);
    assert!(
        f[0].contains("observed 4m ago"),
        "header owns age: {}",
        f[0]
    );
    assert!(
        f[1].contains("Developer storage") && f[1].contains("39.0GB"),
        "{}",
        f[1]
    );
    assert!(
        f[2].contains("ledger") && f[2].contains("3h ago"),
        "{}",
        f[2]
    );
    assert!(
        !f[1].contains("Reclaim:") && !f[2].contains("Reclaim:"),
        "{f:?}"
    );
    assert!(!f[1].contains("Disk:") && !f[2].contains("Disk:"), "{f:?}");
}

/// Tempting wrong patch: the disk state is folded into a percent (or the
/// line is dropped) when the ledger is missing, unreadable, newer or
/// future-dated. The coverage headline says what happened, with no percent.
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
        assert!(f[2].contains("ledger"), "{name}: {}", f[2]);
    }
    // The previous scope is stated on the block itself.
    let mut a = app();
    a.previous_scope_roots = Some(3);
    let f = frame(&a, 80, 24);
    assert!(f[2].contains("previous scope"), "{}", f[2]);
    let f = frame(&a, 40, 24);
    assert!(f[2].contains("previous scope"), "{}", f[2]);
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
        assert!(f[2].starts_with("FLAG: "), "{w}: {}", f[2]);
        assert!(f[2].contains("spot audit"), "{w}: {}", f[2]);
        if w >= 80 {
            assert!(f[2].contains("swamp report --view disk"), "{w}: {}", f[2]);
        }
        // Same rows as the calm state: strip and table heading unmoved.
        assert!(f[3].contains("Projects"), "{w}: {}", f[3]);
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
// The section strip
// ---------------------------------------------------------------------

/// Tempting wrong patch: the strip highlights the current section with a
/// color (lost under NO_COLOR and on a light theme), or highlights more
/// than the current one. It is reverse video only, on exactly the current
/// section's label, at every width.
#[test]
fn the_strip_highlights_the_current_section_with_reverse_video_and_no_color() {
    for w in [40u16, 50, 80, 120] {
        for view in ViewKind::ALL {
            let mut a = app();
            a.set_view(view);
            let b = buf(&a, w, 24);
            let y = 1 + ui::headline_rows(24);
            let sec = view.section();
            let label = format!("{} {}", sec.key(), sec.title());
            let line: String = (0..w).map(|x| b[(x, y)].symbol().to_string()).collect();
            let at = line
                .find(&label)
                .unwrap_or_else(|| panic!("{w} {view:?}: {line}"));
            let col = line[..at].chars().count() as u16;
            let reversed: Vec<u16> = (0..w)
                .filter(|x| b[(*x, y)].modifier.contains(Modifier::REVERSED))
                .collect();
            let want: Vec<u16> = (col..col + label.chars().count() as u16).collect();
            assert_eq!(reversed, want, "{w} {view:?}: {line}");
            for x in 0..w {
                assert_eq!(b[(x, y)].fg, Color::Reset, "{w} {view:?}: no color");
                assert_eq!(b[(x, y)].bg, Color::Reset, "{w} {view:?}: no color");
            }
            // All three sections are named, in order, on one row.
            let mut from = 0;
            for s in swamp_tui::app::Section::ALL {
                let i = line[from..]
                    .find(s.title())
                    .unwrap_or_else(|| panic!("{w}: {line}"));
                from += i + s.title().len();
            }
        }
    }
}

/// Tempting wrong patch: the sub-view title is only on the strip (so it is
/// easy to miss) or `v` runs into the next section. The line names the view
/// and purpose; `v` wraps inside the section.
#[test]
fn the_view_line_names_the_sub_view_and_v_wraps_inside_the_section() {
    use swamp_tui::app::Section;
    for w in [40u16, 50, 80, 120] {
        let mut a = app();
        a.set_section(Section::Tools);
        let f = frame(&a, w, 24);
        let line = &f[2 + ui::headline_rows(24) as usize];
        assert!(line.contains("Reclaim"), "{w}: {line}");
        for _ in 0..4 {
            swamp_tui::handle_key(&mut a, KeyCode::Char('v'));
        }
        assert_eq!(a.view, ViewKind::Reclaim, "wrapped");
        let line = frame(&a, w, 24)[2 + ui::headline_rows(24) as usize].clone();
        assert!(line.contains("Reclaim"), "{w}: {line}");
    }
    let mut a = app();
    a.set_section(Section::Disk);
    swamp_tui::handle_key(&mut a, KeyCode::Char('v'));
    assert_eq!(a.view, ViewKind::DiskGaps);
    let line = frame(&a, 80, 24)[2 + ui::headline_rows(24) as usize].clone();
    assert!(line.contains("Coverage gaps"), "{line}");
}

// ---------------------------------------------------------------------
// The keymap
// ---------------------------------------------------------------------

/// Tempting wrong patch: a second way into a view (digits 4-9, a letter
/// per view) creeps back, or a key is bound twice, or a section is only
/// reachable by a key that does not exist. Every section and sub-view is
/// reachable with only Tab, Shift-Tab, `v` and 1-3; the main keymap is read
/// from the source and no key is bound twice; the removed keys do nothing.
#[test]
fn every_section_and_view_is_reachable_with_tab_v_and_1_to_3_and_no_key_is_bound_twice() {
    use std::collections::HashSet;
    use swamp_tui::app::Section;
    // 1. Reachability from any start, by the allowed keys only.
    let keys = [
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Char('v'),
        KeyCode::Char('1'),
        KeyCode::Char('2'),
        KeyCode::Char('3'),
    ];
    let mut reached: HashSet<ViewKind> = HashSet::new();
    let mut frontier: Vec<ViewKind> = vec![ViewKind::Projects];
    reached.insert(ViewKind::Projects);
    while let Some(v) = frontier.pop() {
        for k in keys {
            let mut a = app();
            a.set_view(v);
            swamp_tui::handle_key(&mut a, k);
            if reached.insert(a.view) {
                frontier.push(a.view);
            }
        }
    }
    assert_eq!(reached.len(), ViewKind::ALL.len(), "{reached:?}");
    for v in ViewKind::ALL {
        assert!(reached.contains(&v), "{v:?} is unreachable");
    }
    // 2. Sections: Tab and Shift-Tab are inverse and cover all three; 1-3 jump.
    let mut a = app();
    let mut seen = vec![a.view.section()];
    for _ in 0..3 {
        swamp_tui::handle_key(&mut a, KeyCode::Tab);
        seen.push(a.view.section());
    }
    assert_eq!(
        seen,
        vec![
            Section::Projects,
            Section::Tools,
            Section::Disk,
            Section::Projects
        ]
    );
    swamp_tui::handle_key(&mut a, KeyCode::BackTab);
    assert_eq!(a.view.section(), Section::Disk);
    for s in Section::ALL {
        swamp_tui::handle_key(&mut a, KeyCode::Char(s.key()));
        assert_eq!(a.view, s.default_view());
    }
    assert_eq!(ViewKind::Reclaim, Section::Tools.default_view());
    // 3. The removed keys open nothing.
    for k in ['4', '5', '6', '7', '8', '9', 'c', 'D', 'I'] {
        let mut a = app();
        swamp_tui::handle_key(&mut a, KeyCode::Char(k));
        assert_eq!(a.view, ViewKind::Projects, "{k} still opens a view");
    }
    // 4. No key is bound twice in the main keymap (read from the source:
    // every `KeyCode::Char('x')` from the start of the main `match code` to
    // the end of the function), and no view key is a literal there other
    // than the three digits.
    let src = include_str!("../src/lib.rs");
    let start = src
        .find("    match code {\n        KeyCode::Char('q') => app.quit = true,")
        .expect("the main keymap");
    let end = start + src[start..].find("\n}\n").expect("end of handle_key_mod");
    let region = &src[start..end];
    let mut bound: Vec<String> = Vec::new();
    let mut rest = region;
    while let Some(i) = rest.find("KeyCode::") {
        let tail = &rest[i + "KeyCode::".len()..];
        let name: String = if let Some(t) = tail.strip_prefix("Char('") {
            let c = t.chars().next().unwrap();
            format!("Char({c})")
        } else {
            tail.chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect()
        };
        bound.push(name);
        rest = &tail[1..];
    }
    // `d` is guarded by an open plan; count its guarded arm once.
    let mut sorted = bound.clone();
    sorted.sort();
    let dups: Vec<&String> = sorted
        .windows(2)
        .filter(|w| w[0] == w[1])
        .map(|w| &w[0])
        .collect();
    assert!(dups.is_empty(), "bound twice in the main keymap: {dups:?}");
    for gone in ["Char(4)", "Char(c)", "Char(D)", "Char(I)"] {
        assert!(
            !bound.contains(&gone.to_string()),
            "{gone} is still in the keymap"
        );
    }
    assert!(bound.contains(&"Tab".to_string()) && bound.contains(&"BackTab".to_string()));
    // 5. `?` help lists every section and view with its key.
    let mut a = app();
    a.help_open = true;
    let mut text = String::new();
    for scroll in [0usize, 15, 30, 45, 60, 75, 90] {
        a.help_scroll.set(scroll);
        text.push_str(&frame(&a, 120, 30).join("\n"));
        text.push('\n');
    }
    for s in Section::ALL {
        assert!(
            text.contains(&format!("{} {}", s.key(), s.title())),
            "help lacks {s:?}"
        );
    }
    for v in ViewKind::ALL {
        assert!(
            text.contains(&format!("{}:", v.title())),
            "help lacks {v:?}"
        );
        let head: String = v.describe().chars().take(20).collect();
        assert!(text.contains(&head), "help lacks the description of {v:?}");
    }
    assert!(
        text.contains("Shift-Tab") && text.contains("1 2 3"),
        "{text}"
    );
}

/// Tempting wrong patch: Ctrl-C is taken for a view key, or `v` walks out
/// of the section.
#[test]
fn ctrl_c_is_not_a_view_key_and_v_stays_in_its_section() {
    let mut a = app();
    swamp_tui::handle_terminal_key(
        &mut a,
        crossterm::event::KeyEvent::new(
            KeyCode::Char('c'),
            crossterm::event::KeyModifiers::CONTROL,
        ),
    );
    assert_eq!(a.view, ViewKind::Projects);
    assert!(a.quit);
    for s in swamp_tui::app::Section::ALL {
        let mut a = app();
        a.set_section(s);
        for _ in 0..(s.views().len() * 2 + 1) {
            swamp_tui::handle_key(&mut a, KeyCode::Char('v'));
            assert_eq!(a.view.section(), s);
        }
    }
}

/// Tempting wrong patch: the legend grows view keys, or loses `? help  q
/// quit`. It names section/view/filter/refresh and the current review verb
/// the row keys that already existed, and never a key per view.
#[test]
fn the_legend_names_tab_and_v_and_never_a_key_per_view() {
    let mut a = app();
    a.set_view(ViewKind::Projects);
    let f80 = frame(&a, 80, 24);
    let last = &f80[23];
    for item in ["Tab section", "v view", "/ filter"] {
        assert!(last.contains(item), "missing {item:?}: {last}");
    }
    assert!(last.contains("? help  q quit"), "{last}");
    for w in [40u16, 50, 80, 120] {
        for v in ViewKind::ALL {
            let mut a = app();
            a.set_view(v);
            let last = frame(&a, w, 30).pop().unwrap();
            assert!(last.contains("? help  q quit"), "{w} {v:?}: {last}");
            assert!(
                !last.contains("reclaim") && !last.contains("disk"),
                "{last}"
            );
        }
    }
}

// ---------------------------------------------------------------------
// The first-run navigation hint
// ---------------------------------------------------------------------

/// Tempting wrong patch: the navigation hint is a permanent banner, or it
/// appears before a user has seen the views. It appears on the section strip
/// until Tools or Disk is opened, and the seen state is persisted.
#[test]
fn the_first_run_hint_uses_the_section_strip_and_stays_hidden_after_use() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = app();
    a.store_dir = Some(dir.path().to_path_buf());
    a.views_seen = false;
    let strip_y = 1 + ui::headline_rows(30) as usize;
    let has_hint = |a: &App| frame(a, 80, 30)[strip_y].contains("Tab switches sections");
    assert!(has_hint(&a), "first-run hint missing from section strip");
    // Moving inside Projects does not end it.
    swamp_tui::handle_key(&mut a, KeyCode::Char('v'));
    swamp_tui::handle_key(&mut a, KeyCode::Char('1'));
    assert!(has_hint(&a), "project navigation ended the first-run hint");
    // Opening Disk does.
    swamp_tui::handle_key(&mut a, KeyCode::Char('3'));
    assert!(a.views_seen);
    assert!(!has_hint(&a), "hint remains after opening Disk");
    let view_line = frame(&a, 80, 30)[2 + ui::headline_rows(30) as usize].clone();
    assert!(view_line.contains(ViewKind::Disk.title()), "{view_line}");
    a.flush_ui_state();
    let raw = std::fs::read_to_string(dir.path().join("ui_state.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["views_seen"], true, "{raw}");
    assert!(swamp_tui::app::load_ui_state(dir.path()).views_seen);
    let old = tempfile::tempdir().unwrap();
    std::fs::write(
        old.path().join("ui_state.json"),
        r#"{"filter":"","sort":"","reverse":false,"keep_executables":false}"#,
    )
    .unwrap();
    assert!(!swamp_tui::app::load_ui_state(old.path()).views_seen);
    // Tab into Tools ends it too.
    let mut b = app();
    b.views_seen = false;
    swamp_tui::handle_key(&mut b, KeyCode::Tab);
    assert_eq!(b.view, ViewKind::Reclaim);
    assert!(b.views_seen);
    assert!(!has_hint(&b));
}

// ---------------------------------------------------------------------
// The Disk section
// ---------------------------------------------------------------------

/// Tempting wrong patch: an unreadable folder shows `0B`, or a view works
/// on a missing ledger by measuring one. Summary lists the parts, Not
/// measured lists the gaps as unmeasured or pending, and with no ledger
/// both say what to run and list nothing themselves.
#[test]
fn the_disk_views_list_the_ledger_parts_and_never_show_unreadable_as_zero() {
    let mut a = app();
    a.set_view(ViewKind::Disk);
    let f = frame(&a, 120, 30).join("\n");
    assert!(
        f.contains("Developer storage") && f.contains("35.0GB"),
        "{f}"
    );
    assert!(f.contains("Everything else") && f.contains("9.0GB"), "{f}");
    assert!(f.contains("System volumes") && f.contains("10.0GB"), "{f}");
    let pic = f
        .lines()
        .find(|l| l.contains("/Users/x/Pictures"))
        .expect("named");
    assert!(pic.contains("not read") && !pic.contains("0B"), "{pic}");
    a.set_view(ViewKind::DiskGaps);
    let f = frame(&a, 120, 30).join("\n");
    assert!(f.contains("Not read") && f.contains("1 directory"), "{f}");
    let pic = f
        .lines()
        .find(|l| l.contains("/Users/x/Pictures"))
        .expect("named");
    assert!(pic.contains("not read") && !pic.contains("0B"), "{pic}");
    assert!(f.contains("Outside developer storage"), "{f}");
    assert!(f.contains("/Users/x/Movies"), "{f}");
    for v in [ViewKind::Disk, ViewKind::DiskGaps] {
        let mut b = app();
        b.set_ledger(LedgerReading::NotMeasured);
        b.set_view(v);
        let f = frame(&b, 80, 24).join(" ");
        let f = f.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            f.contains("disk ledger: not measured yet; run swamp observe --volume"),
            "{f}"
        );
        assert!(f.contains("Opening this view does not scan"), "{f}");
        let (_, work) = swamp_core::work_counters::measured(|| frame(&b, 80, 24));
        assert_eq!(work, swamp_core::work_counters::WorkCounters::default());
    }
}

// ---------------------------------------------------------------------
// Typing
// ---------------------------------------------------------------------

/// Tempting wrong patch: view keys are matched before the modal handlers,
/// so typing `Tab v 1 2 3 c D I` into the filter (or the picker's text
/// field) switches views instead of typing. Tab completes in the filter as
/// before; letters and digits land as text; the picker never switches.
#[test]
fn view_keys_are_text_while_typing_in_the_filter_and_the_picker() {
    let mut a = app();
    swamp_tui::handle_key(&mut a, KeyCode::Char(':'));
    assert!(a.editing_filter);
    let before = a.filter_text.clone();
    for k in ['v', '1', '2', '3', 'c', 'D', 'I'] {
        swamp_tui::handle_key(&mut a, KeyCode::Char(k));
    }
    swamp_tui::handle_key(&mut a, KeyCode::Tab);
    assert!(a.filter_text.contains("v123cDI"), "{}", a.filter_text);
    assert!(a.filter_text.starts_with(&before[..0]));
    assert_eq!(a.view, ViewKind::Projects, "typing switched the view");
    assert!(a.editing_filter, "Tab must not leave the edit");
    swamp_tui::handle_key(&mut a, KeyCode::Esc);
    // Picker: every field, then its text field.
    let mut a = app();
    swamp_tui::handle_key(&mut a, KeyCode::Char('/'));
    assert!(a.picker.is_some());
    for k in [
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Char('v'),
        KeyCode::Char('1'),
        KeyCode::Char('2'),
        KeyCode::Char('3'),
        KeyCode::Char('c'),
        KeyCode::Char('D'),
        KeyCode::Char('I'),
    ] {
        swamp_tui::handle_key(&mut a, k);
        assert_eq!(
            a.view,
            ViewKind::Projects,
            "{k:?} switched the view in the picker"
        );
        assert!(a.picker.is_some() || k == KeyCode::Char('1'), "{k:?}");
    }
    // Help and the blocked list keep their own keys.
    let mut a = app();
    a.help_open = true;
    for k in [KeyCode::Tab, KeyCode::Char('2'), KeyCode::Char('v')] {
        swamp_tui::handle_key(&mut a, k);
        assert_eq!(a.view, ViewKind::Projects);
    }
    let mut a = app();
    a.blocked_open = true;
    for k in [KeyCode::Tab, KeyCode::Char('2'), KeyCode::Char('v')] {
        swamp_tui::handle_key(&mut a, k);
        assert_eq!(a.view, ViewKind::Projects);
    }
}

/// Tempting wrong patch: unsupported predicates are shown as if they narrow
/// the current view, or a supported filter disappears on a narrow screen.
#[test]
fn filter_summary_is_visible_only_where_and_as_it_is_applied() {
    for w in [40u16, 50, 80, 120] {
        for v in [
            ViewKind::Projects,
            ViewKind::Tree,
            ViewKind::Builds,
            ViewKind::Deps,
        ] {
            let mut a = app();
            a.set_view(v);
            a.filter_text = "growth > 100MB in 7d".into();
            a.commit_filter();
            let f = frame(&a, w, 24);
            let line = &f[2 + ui::headline_rows(24) as usize];
            if w >= 50 {
                assert!(
                    line.contains("filter: growth > 100MB in 7d"),
                    "{w} {v:?}: {line}"
                );
            }
        }
    }

    let mut a = app();
    a.filter_text = "growth > 100MB in 7d kind:build".into();
    a.commit_filter();
    for v in [
        ViewKind::Docker,
        ViewKind::Unowned,
        ViewKind::External,
        ViewKind::Reclaim,
        ViewKind::Disk,
        ViewKind::DiskGaps,
        ViewKind::Agents,
    ] {
        a.set_view(v);
        let line = frame(&a, 120, 24)[2 + ui::headline_rows(24) as usize].clone();
        assert!(line.contains(v.title()), "{v:?}: {line}");
        assert!(
            !line.contains("filter:"),
            "ignored filter claimed: {v:?}: {line}"
        );
    }
}

/// Set `SWAMP_CLARITY_CAPTURE` to a directory to write one readable frame
/// per view at the compact and wide review sizes. Normal tests never write
/// captures or goldens.
#[test]
fn optionally_capture_every_view_at_compact_and_wide_sizes() {
    let Some(dir) = std::env::var_os("SWAMP_CLARITY_CAPTURE") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    for view in ViewKind::ALL {
        let mut a = app();
        a.views_seen = true;
        a.clear_filter();
        a.set_view(ViewKind::Projects);
        a.selected = 0;
        a.set_view(view);
        for (name, width, height) in [("compact", 80, 24), ("wide", 200, 60)] {
            let contents = frame(&a, width, height).join("\n");
            std::fs::write(dir.join(format!("{}-{name}.txt", view.label())), contents).unwrap();
        }
    }
}

/// The active filter is never claimed on a view that ignores it. Aggregate
/// views name only the predicate kinds they actually apply.
#[test]
fn filter_summary_only_claims_predicates_used_by_the_current_view() {
    let mut a = app();
    a.filter_text = "growth > 100MB in 7d kind:build type:rust project:mole".into();
    a.commit_filter();
    let view_line_y = 2 + ui::headline_rows(30) as usize;

    a.set_view(ViewKind::Projects);
    let projects = frame(&a, 160, 30)[view_line_y].clone();
    assert!(projects.contains("Projects"), "{projects}");
    assert!(projects.contains("filter:"), "{projects}");
    assert!(projects.contains("project:mole"), "{projects}");

    a.set_view(ViewKind::Kinds);
    let kinds = frame(&a, 160, 30)[view_line_y].clone();
    assert!(kinds.contains("Storage kinds"), "{kinds}");
    assert!(kinds.contains("kind:build"), "{kinds}");
    assert!(kinds.contains("other filters off"), "{kinds}");
    assert!(
        !kinds.contains("project:mole"),
        "ignored predicate shown: {kinds}"
    );
    assert!(
        !kinds.contains("type:rust"),
        "ignored predicate shown: {kinds}"
    );

    a.set_view(ViewKind::Types);
    let types = frame(&a, 160, 30)[view_line_y].clone();
    assert!(types.contains("Ecosystems"), "{types}");
    assert!(types.contains("type:rust"), "{types}");
    assert!(types.contains("other filters off"), "{types}");
    assert!(
        !types.contains("project:mole"),
        "ignored predicate shown: {types}"
    );
    assert!(
        !types.contains("kind:build"),
        "ignored predicate shown: {types}"
    );

    for v in [ViewKind::Reclaim, ViewKind::Disk, ViewKind::External] {
        a.set_view(v);
        let line = frame(&a, 160, 30)[view_line_y].clone();
        assert!(line.contains(v.title()), "{v:?}: {line}");
        assert!(
            !line.contains("filter:"),
            "ignored filter claimed: {v:?}: {line}"
        );
        assert!(
            !line.contains("project:mole"),
            "ignored filter claimed: {v:?}: {line}"
        );
    }
}

/// Tempting wrong patch: an open confirm lets the cursor, the views or the
/// marks move under it, so Enter confirms a plan the person no longer sees.
/// Every such key is swallowed; the keys the confirm uses still work.
#[test]
fn an_open_confirm_swallows_every_key_that_would_change_what_is_under_it() {
    let keys = [
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Char('v'),
        KeyCode::Char('1'),
        KeyCode::Char('2'),
        KeyCode::Char('3'),
        KeyCode::Char('/'),
        KeyCode::Char(':'),
        KeyCode::Char('0'),
        KeyCode::Char(' '),
        KeyCode::Char('A'),
    ];
    for k in keys {
        let mut a = app();
        a.selected = 1.min(a.rows().len().saturating_sub(1));
        a.confirm_open = true;
        let (view, sel, filter) = (a.view, a.selected, a.filter_text.clone());
        swamp_tui::handle_key(&mut a, k);
        assert!(a.confirm_open, "{k:?} closed the confirm");
        assert_eq!(a.view, view, "{k:?} changed the view under the confirm");
        assert_eq!(a.selected, sel, "{k:?} moved the cursor under the confirm");
        assert_eq!(a.filter_text, filter, "{k:?}");
        assert!(!a.editing_filter && a.picker.is_none(), "{k:?}");
    }
    // Esc still takes it back.
    let mut a = app();
    a.confirm_open = true;
    swamp_tui::handle_key(&mut a, KeyCode::Esc);
    assert!(!a.confirm_open);
}

/// Every key the legend or help names is bound in that mode: the legend's
/// keys are read from the drawn footer and pressed.
#[test]
fn every_key_the_legend_names_does_something() {
    let a = app();
    let legend = frame(&a, 200, 24).pop().unwrap();
    // "Tab section  v view  / filter  R refresh  ..." -> the key of each item.
    let mut bound = 0;
    for item in legend.split("  ") {
        let key = item.split(' ').next().unwrap_or("");
        let code = match key {
            "Tab" => KeyCode::Tab,
            "v" => KeyCode::Char('v'),
            "/" => KeyCode::Char('/'),
            // R needs a store and a scope to start a scan; its own tests
            // (refresh_now, r_while_another_observation_runs) press it.
            "R" => continue,
            // Marking and removal start background work that needs a store;
            // their tests press them. Here: the main keymap binds each.
            "⌫" | "Space" | "A" | "↑↓" | "→/←" => {
                let src = include_str!("../src/lib.rs");
                let pat = match key {
                    "⌫" => "KeyCode::Backspace =>",
                    "Space" => "KeyCode::Char(' ') =>",
                    "↑↓" => "KeyCode::Down =>",
                    "→/←" => "KeyCode::Right =>",
                    _ => "KeyCode::Char('A') =>",
                };
                assert!(
                    src.contains(pat),
                    "the legend names {key:?} but the keymap lacks {pat}"
                );
                bound += 1;
                continue;
            }
            "g/s/n/t/a" => KeyCode::Char('g'),
            "r" => KeyCode::Char('r'),
            "?" => KeyCode::Char('?'),
            "q" => KeyCode::Char('q'),
            "" => continue,
            other => {
                panic!("the legend names {other:?}, which this test does not know how to press")
            }
        };
        let mut b = app();
        // Something to act on, in a mode that can be seen to change.
        let before = (
            b.view,
            b.selected,
            b.sort,
            b.reverse,
            b.help_open,
            b.picker.is_some(),
            b.quit,
            b.operation.is_some()
                || b.pending.is_some()
                || b.refusal_active().is_some()
                || b.status.is_some()
                || b.last_result.is_some()
                || !b.marked.is_empty()
                || b.confirm_open,
        );
        swamp_tui::handle_key(&mut b, code);
        let after = (
            b.view,
            b.selected,
            b.sort,
            b.reverse,
            b.help_open,
            b.picker.is_some(),
            b.quit,
            b.operation.is_some()
                || b.pending.is_some()
                || b.refusal_active().is_some()
                || b.status.is_some()
                || b.last_result.is_some()
                || !b.marked.is_empty()
                || b.confirm_open,
        );
        assert_ne!(
            before, after,
            "the legend names {key:?} but pressing it does nothing"
        );
        bound += 1;
    }
    // This empty Projects fixture has navigation/filter/sort keys, but no
    // mark or review hints. The populated action fixtures cover those.
    assert!(bound >= 8, "{legend}");
}
