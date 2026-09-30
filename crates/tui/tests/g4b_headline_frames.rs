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
                    // The strip row names the current section by its key.
                    let strip = &f[1 + hr];
                    let sec = view.section();
                    assert!(
                        strip.contains(&format!("{} {}", sec.key(), sec.title())),
                        "strip: {ctx}"
                    );
                    // The view line, then the body.
                    assert!(f[2 + hr].contains("filter:"), "view line: {ctx}");
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
        "Developer storage: 39.0GB across 1 project and 3 tool locations (39.0% of used)"
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
        f[4].contains("Reclaim: 3.0GB regenerable, 3 units (2)"),
        "{}",
        f[4]
    );
    assert!(f[4].contains("Disk: ledger 3 h ago (3)"), "{}", f[4]);
    // Wide enough, they spell the key out.
    let wide = frame(&a, 120, 30);
    assert!(
        wide[4].contains("Reclaim: 3.0GB regenerable across 3 units (2 for Tools)"),
        "{}",
        wide[4]
    );
    assert!(
        wide[4].contains("Disk: ledger measured 3 h ago (3)"),
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

/// Tempting wrong patch: the sub-view is only on the strip (so a narrow
/// screen loses it) or `v` runs into the next section. The view line names
/// section, sub-view and place; `v` wraps inside the section.
#[test]
fn the_view_line_names_the_sub_view_and_v_wraps_inside_the_section() {
    use swamp_tui::app::Section;
    for w in [40u16, 50, 80, 120] {
        let mut a = app();
        a.set_section(Section::Tools);
        let f = frame(&a, w, 24);
        let line = &f[2 + ui::headline_rows(24) as usize];
        assert!(
            line.contains("Tools › Reclaim (1 of 4") || (w < 80 && line.contains("Reclaim")),
            "{w}: {line}"
        );
        for _ in 0..4 {
            swamp_tui::handle_key(&mut a, KeyCode::Char('v'));
        }
        assert_eq!(a.view, ViewKind::Reclaim, "wrapped");
        let line = frame(&a, w, 24)[2 + ui::headline_rows(24) as usize].clone();
        assert!(
            line.contains("Tools › Reclaim (1 of 4") || (w < 80 && line.contains("Reclaim")),
            "{w}: {line}"
        );
    }
    let mut a = app();
    a.set_section(Section::Disk);
    swamp_tui::handle_key(&mut a, KeyCode::Char('v'));
    assert_eq!(a.view, ViewKind::DiskGaps);
    let line = frame(&a, 80, 24)[2 + ui::headline_rows(24) as usize].clone();
    assert!(line.contains("Disk › Not measured (2 of 2"), "{line}");
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
/// quit`. It is `Tab section  v view  / filter  R refresh  ⌫ delete` and
/// the row keys that already existed, and never a key per view.
#[test]
fn the_legend_names_tab_and_v_and_never_a_key_per_view() {
    let a = app();
    let f80 = frame(&a, 80, 24);
    let last = &f80[23];
    assert!(
        last.starts_with("Tab section  v view  / filter  R refresh  ⌫ delete"),
        "{last}"
    );
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
// The first-run pointer
// ---------------------------------------------------------------------

/// Tempting wrong patch: the pointer line is a permanent banner, or it is
/// remembered only in memory, or it hides on any key. It shows on a store
/// that has never opened Tools or Disk, ends when either is opened, and is
/// written to `ui_state.json` (additive; an older reader ignores it).
#[test]
fn the_first_run_line_hides_after_tools_or_disk_is_opened_and_stays_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = app();
    a.store_dir = Some(dir.path().to_path_buf());
    a.views_seen = false;
    let want = "New: Tab opens Tools (Reclaim) and Disk. Hides after you open either.";
    assert_eq!(frame(&a, 80, 24)[4], want);
    // Moving inside Projects does not end it.
    swamp_tui::handle_key(&mut a, KeyCode::Char('v'));
    swamp_tui::handle_key(&mut a, KeyCode::Char('1'));
    assert_eq!(frame(&a, 80, 24)[4], want);
    // Opening Disk does.
    swamp_tui::handle_key(&mut a, KeyCode::Char('3'));
    assert!(a.views_seen);
    let f = frame(&a, 80, 24);
    assert!(!f[4].starts_with("New:"), "{}", f[4]);
    assert!(f[4].contains("Reclaim:") && f[4].contains("(2"), "{}", f[4]);
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
        f.contains("Accounted: declared roots and catalog units"),
        "{f}"
    );
    assert!(f.contains("Everything else (not developer storage)"), "{f}");
    assert!(f.contains("System volumes"), "{f}");
    let pic = f
        .lines()
        .find(|l| l.contains("/Users/x/Pictures"))
        .expect("named");
    assert!(pic.contains("not read") && !pic.contains("0B"), "{pic}");
    a.set_view(ViewKind::DiskGaps);
    let f = frame(&a, 120, 30).join("\n");
    assert!(f.contains("Could not be read: 1 directory"), "{f}");
    let pic = f
        .lines()
        .find(|l| l.contains("/Users/x/Pictures"))
        .expect("named");
    assert!(pic.contains("not read") && !pic.contains("0B"), "{pic}");
    assert!(
        f.contains("Largest measured folders outside developer storage"),
        "{f}"
    );
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
        assert!(f.contains("opening the UI never scans"), "{f}");
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

/// Tempting wrong patch: a narrow screen cuts the view line at the edge, so
/// the active filter (which hides rows) is the part that vanishes. The view
/// part shortens first; the filter is always on screen.
#[test]
fn an_active_filter_is_always_visible_on_the_view_line() {
    for w in [40u16, 50, 80, 120] {
        for v in ViewKind::ALL {
            let mut a = app();
            a.set_view(v);
            a.filter_text = "growth > 100MB in 7d".into();
            let f = frame(&a, w, 24);
            let line = &f[2 + ui::headline_rows(24) as usize];
            assert!(
                line.contains("filter: growth > 100MB in 7d"),
                "{w} {v:?}: {line}"
            );
        }
    }
}

/// The pointer names the jump key, which works from every section (Tab
/// from Tools goes to Disk, not back to Tools).
#[test]
fn the_reclaim_pointer_names_the_jump_key_not_tab() {
    let mut a = app();
    for w in [40u16, 80, 120] {
        for v in [ViewKind::Projects, ViewKind::Reclaim, ViewKind::Disk] {
            a.set_view(v);
            let l = frame(&a, w, 24)[4].clone();
            assert!(
                l.contains("Reclaim") && !l.contains("Tab"),
                "{w} {v:?}: {l}"
            );
            assert!(l.contains("(2") || w < 40, "{l}");
        }
    }
    // Disk views hide the keys they cannot use.
    a.set_view(ViewKind::Disk);
    let last = frame(&a, 120, 24).pop().unwrap();
    assert!(
        !last.contains("⌫ delete") && !last.contains("Space mark"),
        "{last}"
    );
}
