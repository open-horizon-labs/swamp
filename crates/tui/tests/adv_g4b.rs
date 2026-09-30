//! Adversarial audit of G4b (#206) in the TUI. Fixture-only.
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

fn spans_width(s: &[ratatui::text::Span<'_>]) -> usize {
    s.iter()
        .map(|x| swamp_tui::model::display_width(&x.content))
        .sum()
}

/// Tempting wrong patch: the strip's width math forgets the `… ` / ` …`
/// cut markers or the `of <project>` suffix, so the strip is wider than the
/// terminal and the right edge (or the current tab) is cut by the
/// renderer. Fuzz every view at widths 1-300 with unicode project names.
#[test]
fn adv_strip_never_wider_than_the_terminal_and_current_always_shown() {
    let names = [
        None,
        Some("p"),
        Some("日本語のプロジェクト名前がとても長い"),
        Some("emoji😀😀😀😀😀😀😀😀😀😀"),
        Some("e\u{301}e\u{301}combining-accents-project-name"),
        Some("a-very-long-ascii-project-name-that-goes-on-and-on-and-on-forever"),
    ];
    let mut bad = Vec::new();
    for name in names {
        for v in ViewKind::ALL {
            let mut a = app();
            a.view = v;
            a.selected_project = name.map(str::to_string);
            let _ = ui::view_strip_spans(&a, 0);
            for w in 1..=300usize {
                let s = ui::view_strip_spans(&a, w);
                let got = spans_width(&s);
                let text: String = s.iter().map(|x| x.content.to_string()).collect();
                if got > w {
                    bad.push(format!("{v:?} {name:?} w={w} width={got}: {text:?}"));
                }
                if text.contains('\n') {
                    bad.push(format!("{v:?} w={w} newline"));
                }
                if w >= 12 && !text.contains(v.section().title()) {
                    bad.push(format!(
                        "{v:?} {name:?} w={w} current key missing: {text:?}"
                    ));
                }
            }
        }
    }
    let wide: Vec<&String> = bad
        .iter()
        .filter(|b| {
            !b.contains(" w=1 ")
                && b.split(" w=")
                    .nth(1)
                    .and_then(|r| r.split_whitespace().next())
                    .and_then(|n| n.parse::<usize>().ok())
                    .is_some_and(|n| n >= 20)
        })
        .collect();
    let missing = bad.iter().filter(|b| b.contains("missing")).count();
    assert!(
        bad.is_empty(),
        "{} cases ({} at w>=20, {} current-key-missing); w>=20 first: {:#?}; first: {:#?}",
        bad.len(),
        wide.len(),
        missing,
        &wide[..wide.len().min(6)],
        &bad[..bad.len().min(4)]
    );
}

/// Tempting wrong patch: the view-key arm is matched before a text field,
/// or a digit in the picker's text field goes to the `0` clear arm.
#[test]
fn adv_view_keys_land_as_text_in_filter_and_picker_text_field() {
    let typed = "cDI123v";
    let mut a = app();
    swamp_tui::handle_key(&mut a, KeyCode::Char(':'));
    for k in typed.chars() {
        swamp_tui::handle_key(&mut a, KeyCode::Char(k));
    }
    assert!(a.filter_text.ends_with(typed), "{}", a.filter_text);
    assert_eq!(a.view, ViewKind::Projects);
    let mut a = app();
    swamp_tui::handle_key(&mut a, KeyCode::Char('/'));
    a.picker.as_mut().unwrap().field = 3;
    for k in typed.chars() {
        swamp_tui::handle_key(&mut a, KeyCode::Char(k));
    }
    assert_eq!(a.view, ViewKind::Projects);
    assert_eq!(a.picker.as_ref().unwrap().project_query, typed);
    // Help and the cargo popup keep their keys.
    for k in typed.chars() {
        let mut a = app();
        a.help_open = true;
        swamp_tui::handle_key(&mut a, KeyCode::Char(k));
        assert_eq!(a.view, ViewKind::Projects, "help {k}");
        let mut a = app();
        a.cargo_inspection = Some(vec!["x".into()]);
        swamp_tui::handle_key(&mut a, KeyCode::Char(k));
        assert_eq!(a.view, ViewKind::Projects, "cargo popup {k}");
    }
}

/// Tempting wrong patch: an empty store has no rows and a direct key or a
/// draw at a degenerate size indexes row 0 or subtracts past zero.
#[test]
fn adv_every_key_on_an_empty_store_at_every_size_does_not_panic() {
    let mut a = App::new(
        Report::empty(PathBuf::from("/h/src")),
        PathBuf::from("/h/src"),
    );
    a.has_index = false;
    a.views_seen = false;
    for v in ViewKind::ALL {
        a.set_view(v);
        assert_eq!(a.view, v);
        // Every navigation key on an empty store, too.
        for k in [
            KeyCode::Tab,
            KeyCode::BackTab,
            KeyCode::Char('v'),
            KeyCode::Char('1'),
            KeyCode::Char('2'),
            KeyCode::Char('3'),
        ] {
            let mut b = App::new(
                Report::empty(PathBuf::from("/h/src")),
                PathBuf::from("/h/src"),
            );
            b.has_index = false;
            b.set_view(v);
            swamp_tui::handle_key(&mut b, k);
            let _ = buf(&b, 80, 24);
        }
        for (w, h) in [
            (0u16, 0u16),
            (1, 1),
            (2, 1),
            (1, 2),
            (5, 3),
            (10, 5),
            (39, 8),
            (40, 11),
            (40, 12),
            (300, 4),
        ] {
            let _ = buf(&a, w, h);
        }
        let f = frame(&a, 80, 24).join("\n");
        if matches!(v, ViewKind::Disk | ViewKind::DiskGaps) {
            assert!(
                f.contains("swamp observe"),
                "Disk empty state must say what to run:\n{f}"
            );
        }
        for word in [
            "unused", "obsolete", "stale", "safe to", "orphan", "\u{2014}",
        ] {
            assert!(
                !f.to_lowercase().contains(word),
                "{v:?} shows {word:?}:\n{f}"
            );
        }
    }
}

/// Tempting wrong patch: the current tab is drawn with a color (fg/bg)
/// instead of an attribute, so NO_COLOR / monochrome loses it.
#[test]
fn adv_strip_row_has_no_color_cells() {
    for v in ViewKind::ALL {
        let mut a = app();
        a.view = v;
        for w in [40u16, 80, 120] {
            let b = buf(&a, w, 24);
            let y = ui::headline_rows(24) + 1;
            let mut reversed = false;
            for x in 0..w {
                let c = &b[(x, y)];
                assert!(
                    c.fg == Color::Reset && c.bg == Color::Reset,
                    "{v:?} w={w} x={x} {:?}",
                    c
                );
                reversed |= c.modifier.contains(Modifier::REVERSED);
            }
            assert!(reversed, "{v:?} w={w}: current tab not highlighted");
        }
    }
}

/// Tempting wrong patch: the Disk view's "Not measured" row hard-codes
/// the plural, so one unreadable directory reads "1 directories".
#[test]
fn adv_disk_view_not_measured_row_is_singular_for_one() {
    // The fixture ledger has exactly one unreadable directory.
    let rows = swamp_tui::model::disk_rows(&measured(now() - 60), None);
    let nm: Vec<&String> = rows
        .iter()
        .map(|r| &r.label)
        .filter(|n| n.starts_with("Not measured"))
        .collect();
    assert_eq!(nm.len(), 1);
    assert!(!nm[0].contains("1 directories"), "{}", nm[0]);
}

/// Tempting wrong patch: a zero-used or over-used ledger adds a row or
/// the TUI says "no used figure" for a container that reported 0 bytes.
#[test]
fn adv_zero_and_over_used_ledgers_keep_rows_and_say_why() {
    let meta = |used: u64| VolumeMetaRow {
        measured_at: now() - 60,
        cycle_started_at: 1,
        cycle_complete_at: now() - 60,
        complete: true,
        budget_secs: 120,
        budget_used_ms: 1,
        statfs_at: now() - 60,
        container_total: Some(500 * GB),
        container_used: Some(used),
        container_free: Some(1),
        data_volume_used: Some(used),
    };
    for w in [40u16, 50, 80, 120] {
        let base = frame(&app(), w, 24);
        for used in [0u64, GB] {
            let mut a = app();
            a.set_ledger(LedgerReading::Measured(Box::new(account(&[], &meta(used)))));
            let f = frame(&a, w, 24);
            assert_eq!(
                f[ui::headline_rows(24) as usize + 1].is_empty(),
                base[ui::headline_rows(24) as usize + 1].is_empty()
            );
            assert!(!f[1].contains('%'), "used={used} w={w}: {}", f[1]);
            if used == 0 && w >= 80 {
                assert!(
                    !f.join("\n").contains("no used figure"),
                    "0 bytes used is reported as missing:\n{}",
                    f.join("\n")
                );
            }
        }
    }
}
