//! Adversarial audit G5 (#177) of the TUI sheet; harness copied from
//! tool_removal_sheet.rs. A test that FAILS on d1c4d08 is a finding.
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

/// Attack 9: `tool_enter` skips the fit check when `height == 0`
/// (`h != 0 && !confirm_fits`). Tempting wrong patch: "0 means not
/// measured yet, let it through". A confirm that was never drawn at any
/// size was never seen: Enter must run nothing.
#[test]
fn adv_enter_on_a_never_drawn_confirm_runs_nothing() {
    let f = Fakes::new();
    f.mise(0);
    let mut app = f.app();
    assert_eq!((app.width, app.height), (0, 0));
    choose(&mut app, "go@1.23.5");
    handle_key(&mut app, KeyCode::Enter);
    wait(&mut app);
    assert!(matches!(
        app.tool_sheet.as_ref().unwrap().stage,
        Stage::Confirm(_)
    ));
    handle_key(&mut app, KeyCode::Enter);
    wait(&mut app);
    assert_eq!(
        ran_uninstall(&f),
        0,
        "a never-drawn confirm ran: {:?}",
        f.calls()
    );
}

/// Attack 2: typeahead. A second Enter pressed while the review runs is
/// ignored (good), but an Enter arriving the instant the confirm lands
/// (key repeat, a pasted "\n\n", a double tap slower than the review) is
/// taken as the confirmation: there is no dwell and no input flush.
/// Tempting wrong patch: "any Enter while Stage::Confirm is the human's
/// yes". This test asserts an Enter within 250 ms of the confirm first
/// appearing runs nothing.
#[test]
fn adv_enter_the_instant_the_confirm_lands_is_not_a_confirmation() {
    let f = Fakes::new();
    f.mise(0);
    let mut app = f.app();
    rows(&mut app, 120, 30);
    choose(&mut app, "go@1.23.5");
    handle_key(&mut app, KeyCode::Enter);
    handle_key(&mut app, KeyCode::Enter); // repeat during review: ignored
    wait(&mut app);
    rows(&mut app, 120, 30); // the loop draws once before the next key
    handle_key(&mut app, KeyCode::Enter); // the queued repeat
    wait(&mut app);
    assert_eq!(
        ran_uninstall(&f),
        0,
        "typeahead Enter confirmed the removal"
    );
}

/// Attack 2: two rapid Enters on the confirm run the command once.
#[test]
fn adv_double_enter_on_the_confirm_runs_once() {
    let f = Fakes::new();
    f.mise(0);
    let mut app = f.app();
    choose(&mut app, "go@1.23.5");
    handle_key(&mut app, KeyCode::Enter);
    wait(&mut app);
    rows(&mut app, 120, 30);
    handle_key(&mut app, KeyCode::Enter);
    handle_key(&mut app, KeyCode::Enter);
    handle_key(&mut app, KeyCode::Enter);
    wait(&mut app);
    handle_key(&mut app, KeyCode::Enter);
    wait(&mut app);
    assert!(ran_uninstall(&f) <= 1, "{:?}", f.calls());
}

/// Attack 2: Esc, q and Ctrl-C while the removal runs: the sheet stays on
/// Running, keeps drawing, and the child is not killed (the removal
/// completes and is recorded).
#[test]
fn adv_keys_during_a_running_removal_neither_kill_nor_hide_it() {
    let f = Fakes::new();
    f.mise(0);
    let k = "-C_S_uninstall_go@1.23.5";
    let sh = f.root.join(format!("state/{k}.sh"));
    let body = fs::read_to_string(&sh).unwrap();
    fs::write(&sh, format!("sleep 1\n{body}")).unwrap();
    let mut app = f.app();
    choose(&mut app, "go@1.23.5");
    handle_key(&mut app, KeyCode::Enter);
    wait(&mut app);
    rows(&mut app, 120, 30);
    handle_key(&mut app, KeyCode::Enter);
    for code in [
        KeyCode::Esc,
        KeyCode::Char('q'),
        KeyCode::Char('R'),
        KeyCode::Backspace,
    ] {
        handle_key(&mut app, code);
        assert!(
            matches!(
                app.tool_sheet.as_ref().map(|s| &s.stage),
                Some(Stage::Running(_))
            ),
            "{code:?} left the running removal"
        );
    }
    swamp_tui::handle_terminal_key(
        &mut app,
        crossterm::event::KeyEvent::new(
            KeyCode::Char('c'),
            crossterm::event::KeyModifiers::CONTROL,
        ),
    );
    assert!(!app.quit);
    let r = rows(&mut app, 80, 24);
    assert!(
        r.iter().any(|l| l.contains("go@1.23.5")),
        "{}",
        r.join("\n")
    );
    wait(&mut app);
    assert!(
        !f.install("go", "1.23.5").exists(),
        "the child was killed mid-removal"
    );
}

/// Attack 9: frames at 40/50 columns: a confirm that cannot be shown in
/// full refuses Enter, and the keys row is visible.
#[test]
fn adv_narrow_terminals_refuse_enter_and_keep_keys_visible() {
    for (w, h) in [(40u16, 24u16), (50, 24), (50, 12)] {
        let f = Fakes::new();
        f.mise(0);
        let mut app = f.app();
        choose(&mut app, "go@1.23.5");
        handle_key(&mut app, KeyCode::Enter);
        wait(&mut app);
        let r = rows(&mut app, w, h);
        assert_plain(&r);
        let fits = r.iter().any(|l| l.contains("Enter run"));
        handle_key(&mut app, KeyCode::Enter);
        wait(&mut app);
        if ran_uninstall(&f) == 1 {
            // It ran: the whole command line must have been on screen.
            let text = r.join("");
            assert!(fits, "{w}x{h}: ran without the keys row:\n{}", r.join("\n"));
            assert!(text.contains("go@1.23.5"), "{w}x{h}:\n{}", r.join("\n"));
        }
    }
}
