//! Independent verification of #177 (G5, PR #204 head 21603ae): attacks
//! round 1 did not try. Harness copied from adv_g5_sheet.rs.
#![allow(dead_code, unused_imports)]

use crossterm::event::KeyCode;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use swamp_core::external::ExternalUnit;
use swamp_core::report::{Reconciliation, Report};
use swamp_tui::app::{App, ViewKind};
use swamp_tui::tool_sheet::Stage;
use swamp_tui::{handle_key, ui};

struct Fakes {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Fakes {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        for d in ["bin", "home", "state", "store"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        let f = Fakes { _tmp: tmp, root };
        f.write_fake("mise");
        f
    }

    fn write_fake(&self, name: &str) {
        let s = self.root.display();
        let script = format!(
            r#"#!/bin/sh
S='{s}'
{{ printf '%s' "$0"; for a in "$@"; do printf '\t%s' "$a"; done; printf '\n'; }} >> "$S/calls.log"
key=$(printf '%s' "$*" | tr ' /' '_S')
if [ -f "$S/state/$key.sh" ]; then sh "$S/state/$key.sh"; fi
if [ -f "$S/state/$key.out" ]; then cat "$S/state/$key.out"; fi
if [ -f "$S/state/$key.err" ]; then cat "$S/state/$key.err" >&2; fi
if [ -f "$S/state/$key.code" ]; then exit "$(cat "$S/state/$key.code")"; fi
[ -f "$S/state/$key.out" ] || [ -f "$S/state/$key.err" ] || [ -f "$S/state/$key.sh" ] || exit 97
"#
        );
        let p = self.root.join("bin").join(name);
        fs::write(&p, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn key(argv: &str) -> String {
        argv.replace(' ', "_").replace('/', "S")
    }

    fn answer(&self, argv: &str, stdout: &str, stderr: &str) {
        let k = Self::key(argv);
        fs::write(self.root.join(format!("state/{k}.out")), stdout).unwrap();
        fs::write(self.root.join(format!("state/{k}.err")), stderr).unwrap();
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn install(&self, tool: &str, version: &str) -> PathBuf {
        self.home()
            .join(".local/share/mise/installs")
            .join(tool)
            .join(version)
    }

    fn entry(&self, tool: &str, version: &str, source: Option<&Path>) -> String {
        let src = source
            .map(|s| {
                format!(
                    r#","requested_version":"latest","source":{{"type":"mise.toml","path":"{}"}}"#,
                    s.display()
                )
            })
            .unwrap_or_default();
        format!(
            r#"{{"version":"{version}","install_path":"{}"{src},"installed":true,"active":false}}"#,
            self.install(tool, version).display()
        )
    }

    fn ls(&self, with_go: bool) -> String {
        let global = self.home().join(".config/mise/config.toml");
        let mut tools = vec![format!(
            r#""node":[{}]"#,
            self.entry("node", "24.14.1", Some(&global))
        )];
        if with_go {
            tools.insert(0, format!(r#""go":[{}]"#, self.entry("go", "1.23.5", None)));
        }
        format!("{{{}}}", tools.join(","))
    }

    /// node 24.14.1 from the global config, go 1.23.5 prunable. With
    /// `dry_lines` extra WARN lines in go's dry run (a longer preview).
    fn mise(&self, dry_lines: usize) {
        fs::create_dir_all(self.install("go", "1.23.5")).unwrap();
        self.answer("--version", "2026.9.15 macos-arm64 (2026-09-27)\n", "");
        self.answer("-C / ls --json --installed", &self.ls(true), "");
        self.answer(
            "-C / prune --tools --dry-run",
            "",
            "mise go@1.23.5 is prunable: go is required at 1.24.0 by ~/src/p/mise.toml\n\
             mise go@1.23.5 [dryrun]  uninstall\n\
             mise go@1.23.5 [dryrun]  remove ~/.local/share/mise/installs/go/1.23.5\n\
             mise go@1.23.5 [dryrun]  ✓ done\n",
        );
        let warns = "mise WARN  a note mise printed\n".repeat(dry_lines);
        self.answer(
            "-C / uninstall --dry-run go@1.23.5",
            "",
            &format!(
                "{warns}mise go@1.23.5       uninstall\n\
                 mise go@1.23.5       remove ~/.local/share/mise/installs/go/1.23.5\n\
                 mise go@1.23.5     ✓ uninstalled (dry-run)\n"
            ),
        );
        let after = self.ls(false);
        fs::write(self.root.join("state/ls_after.json"), after).unwrap();
        fs::write(
            self.root.join(format!(
                "state/{}.sh",
                Self::key("-C / uninstall go@1.23.5")
            )),
            format!(
                "rm -r '{}'; cp '{1}/state/ls_after.json' '{1}/state/{2}.out'\n",
                self.install("go", "1.23.5").display(),
                self.root.display(),
                Self::key("-C / ls --json --installed")
            ),
        )
        .unwrap();
    }

    fn calls(&self) -> Vec<Vec<String>> {
        fs::read_to_string(self.root.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| l.split('\t').map(str::to_string).collect())
            .collect()
    }

    fn app(&self) -> App {
        let mut app = App::new(empty_report(), "/root".into());
        app.tool_host = swamp_core::tool_removal::Host::sandboxed(&self.root);
        app.set_external_units(vec![ExternalUnit {
            detector_id: swamp_core::locations::mise::MISE_DETECTOR_ID.to_string(),
            detector_name: "mise".into(),
            category: swamp_core::locations::StorageCategory::Installation,
            provenance: swamp_core::locations::Provenance::BuiltinConvention,
            path: self.home().join(".local/share/mise/installs"),
            bytes: 3_800_000_000,
            mtime_max: 0,
            hardlinked: false,
            growth_bytes: None,
            regrowth_count: 0,
            observed_at: 0,
            consumers: Vec::new(),
            note: None,
            evidence: Vec::new(),
            bytes_counted_elsewhere: 0,
            overlap_count: 0,
            last_used: Default::default(),
            children: Vec::new(),
        }]);
        app.set_view(ViewKind::External);
        app.selected = 0;
        app
    }
}

fn empty_report() -> Report {
    Report {
        store_dir: None,
        observed_at: 0,
        root: "/root".into(),
        projects: vec![],
        unowned: vec![],
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
        dirs_by_worktree: None,
        files_by_worktree: None,
        schedule_line: None,
        github_enrichment: None,
        nested_artifacts: Vec::new(),
    }
}

fn wait(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.tool_sheet.as_ref().is_some_and(|s| s.waiting()) {
        assert!(Instant::now() < deadline, "the tool worker did not finish");
        app.poll_tool();
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn rows(app: &mut App, w: u16, h: u16) -> Vec<String> {
    app.width = w;
    app.height = h;
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| ui::draw(f, app)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect()
}

fn row_of(rows: &[String], needle: &str) -> usize {
    rows.iter()
        .position(|r| r.contains(needle))
        .unwrap_or_else(|| panic!("`{needle}` not on screen:\n{}", rows.join("\n")))
}

fn assert_plain(rows: &[String]) {
    let text = rows.join("\n");
    assert!(!text.contains('\u{2014}'), "em dash on screen:\n{text}");
    let lower = text.to_lowercase();
    for w in ["unused", "obsolete", "stale", "orphan", "safe to"] {
        assert!(!lower.contains(w), "verdict word `{w}` on screen:\n{text}");
    }
}

/// Opens the sheet on the mise row and moves to `label`.
fn choose(app: &mut App, label: &str) {
    handle_key(app, KeyCode::Backspace);
    assert!(app.tool_sheet.is_some(), "{:?}", app.refusal_active());
    wait(app);
    let sheet = app.tool_sheet.as_ref().unwrap();
    assert!(matches!(sheet.stage, Stage::Choose), "{:?}", sheet.stage);
    let at = sheet
        .listing
        .as_ref()
        .unwrap()
        .candidates
        .iter()
        .position(|c| c.label == label)
        .unwrap_or_else(|| panic!("{label} not listed"));
    for _ in 0..at {
        handle_key(app, KeyCode::Down);
    }
}

fn ran_uninstall(f: &Fakes) -> usize {
    f.calls()
        .iter()
        .filter(|c| c[1..] == ["-C", "/", "uninstall", "go@1.23.5"])
        .count()
}

fn confirm_stage(app: &App) -> bool {
    matches!(app.tool_sheet.as_ref().unwrap().stage, Stage::Confirm(_))
}

fn hold_off() {
    std::thread::sleep(swamp_tui::tool_sheet::HOLD_OFF + Duration::from_millis(100));
}

/// Round 2, resize: the confirm is first painted on a terminal too small
/// to show it in full (the human sees a cut sheet), the hold-off passes,
/// then the terminal grows and `Y` arrives before (or right after) the
/// full confirm is painted. Tempting wrong patch: "the hold-off counts
/// from the first paint of any size" (`note_drawn` runs on every paint,
/// fit or not). The human has seen the whole confirm for 0 ms: nothing
/// may run.
#[test]
fn g5b_hold_off_restarts_when_the_confirm_first_fits() {
    let f = Fakes::new();
    f.mise(0);
    let mut app = f.app();
    rows(&mut app, 120, 30);
    choose(&mut app, "go@1.23.5");
    handle_key(&mut app, KeyCode::Enter);
    wait(&mut app);
    assert!(confirm_stage(&app));
    rows(&mut app, 40, 12); // painted, but does not fit
    hold_off();
    rows(&mut app, 120, 30); // grows: the full confirm appears now
    handle_key(&mut app, KeyCode::Char('Y'));
    wait(&mut app);
    assert_eq!(
        ran_uninstall(&f),
        0,
        "Y ran a confirm that had been fully visible for 0 ms: {:?}",
        f.calls()
    );
}

/// Round 2, resize without a paint: the terminal grows and `Y` is read
/// before the loop repaints (both events queued together). Tempting
/// wrong patch: "fit is judged at the current size" (it is, but the
/// human saw the old size). Nothing may run.
#[test]
fn g5b_y_after_a_resize_that_was_not_yet_painted_runs_nothing() {
    let f = Fakes::new();
    f.mise(0);
    let mut app = f.app();
    rows(&mut app, 120, 30);
    choose(&mut app, "go@1.23.5");
    handle_key(&mut app, KeyCode::Enter);
    wait(&mut app);
    rows(&mut app, 40, 12);
    hold_off();
    app.width = 120; // advance() saw the resize; no paint yet
    app.height = 30;
    handle_key(&mut app, KeyCode::Char('Y'));
    wait(&mut app);
    assert_eq!(ran_uninstall(&f), 0, "{:?}", f.calls());
}

/// Round 2, out-of-date preview: between the review the human read and `Y`,
/// the version became requested by the global config (a `mise use -g`
/// in another shell). Tempting wrong patch: "Y runs the preview it
/// holds". The re-review at Y must refuse, and the sheet must say
/// nothing ran.
#[test]
fn g5b_y_on_a_preview_that_went_out_of_date_runs_nothing() {
    let f = Fakes::new();
    f.mise(0);
    let mut app = f.app();
    rows(&mut app, 120, 30);
    choose(&mut app, "go@1.23.5");
    handle_key(&mut app, KeyCode::Enter);
    wait(&mut app);
    rows(&mut app, 120, 30);
    // Now go@1.23.5 is requested by the global config.
    let global = f.home().join(".config/mise/config.toml");
    let ls = format!(
        r#"{{"go":[{}],"node":[{}]}}"#,
        f.entry("go", "1.23.5", Some(&global)),
        f.entry("node", "24.14.1", Some(&global))
    );
    f.answer("-C / ls --json --installed", &ls, "");
    hold_off();
    rows(&mut app, 120, 30);
    handle_key(&mut app, KeyCode::Char('Y'));
    wait(&mut app);
    assert_eq!(ran_uninstall(&f), 0, "{:?}", f.calls());
    let screen = rows(&mut app, 120, 30).join("\n");
    assert!(screen.contains("Nothing ran"), "{screen}");
}

/// Round 2, lowercase and look-alike keys: `y`, `Y` as part of pasted
/// text read without bracketed paste arrive as keys. Only a capital `Y`
/// is the remove key; `y`, Enter, Space and `A` never run a removal.
/// Tempting wrong patch: "accept y/Y" (a pasted "yes" runs it).
#[test]
fn g5b_only_capital_y_runs_and_nothing_else_does() {
    let f = Fakes::new();
    f.mise(0);
    let mut app = f.app();
    rows(&mut app, 120, 30);
    choose(&mut app, "go@1.23.5");
    handle_key(&mut app, KeyCode::Enter);
    wait(&mut app);
    rows(&mut app, 120, 30);
    hold_off();
    rows(&mut app, 120, 30);
    for c in ['y', 'e', 's', ' ', 'A', 'a', 'D', 'd'] {
        handle_key(&mut app, KeyCode::Char(c));
    }
    handle_key(&mut app, KeyCode::Enter);
    handle_key(&mut app, KeyCode::Delete);
    wait(&mut app);
    assert_eq!(ran_uninstall(&f), 0, "{:?}", f.calls());
    assert!(confirm_stage(&app));
}

/// Round 2, G4b coordination: while a sheet is open, the main keymap's
/// section and view keys (Tab, Shift-Tab, `v`, `1`-`3`), sort keys,
/// help and filter keys do nothing: the view underneath does not
/// change and the sheet stays on its confirm. Tempting wrong patch (at
/// merge): "put the new IA keys above the overlay checks".
#[test]
fn g5b_view_and_section_keys_do_nothing_while_a_sheet_is_open() {
    let f = Fakes::new();
    f.mise(0);
    let mut app = f.app();
    rows(&mut app, 120, 30);
    choose(&mut app, "go@1.23.5");
    handle_key(&mut app, KeyCode::Enter);
    wait(&mut app);
    rows(&mut app, 120, 30);
    let view = app.view;
    for code in [
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Char('v'),
        KeyCode::Char('1'),
        KeyCode::Char('2'),
        KeyCode::Char('3'),
        KeyCode::Char('?'),
        KeyCode::Char('/'),
        KeyCode::Char(':'),
        KeyCode::Char('s'),
        KeyCode::Char('0'),
    ] {
        handle_key(&mut app, code);
        assert_eq!(app.view, view, "{code:?} changed the view under the sheet");
        assert!(confirm_stage(&app), "{code:?} left the confirm");
    }
    assert!(!app.help_open, "help opened over the sheet");
    assert!(
        !app.editing_filter,
        "filter editing started under the sheet"
    );
    assert_eq!(ran_uninstall(&f), 0);
}
