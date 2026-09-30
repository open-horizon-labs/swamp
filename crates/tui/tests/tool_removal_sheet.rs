//! The tool-managed removal sheet (#177) end to end in the TUI, against
//! fake managers in a temp dir (the core resolver panics in a test build
//! on any binary outside the sandbox). Frames at 80 and 120 columns for
//! the list, the confirm, a refusal and the result; each test names the
//! tempting wrong patch it fails.

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

/// Tempting wrong patch: "treat a mise install like any other directory
/// and mark it for Trash". Space refuses with the reason; Backspace opens
/// the manager's own list and marks nothing.
#[test]
fn the_mise_row_opens_the_managers_list_and_never_marks_for_trash() {
    let f = Fakes::new();
    f.mise(0);
    let mut app = f.app();
    handle_key(&mut app, KeyCode::Char(' '));
    assert!(app.marked.is_empty());
    assert!(
        app.refusal_active()
            .unwrap_or("")
            .contains("never moved to Trash"),
        "{:?}",
        app.refusal_active()
    );
    assert!(f.calls().is_empty(), "Space starts nothing");
    handle_key(&mut app, KeyCode::Backspace);
    wait(&mut app);
    assert!(app.marked.is_empty());
    for (w, h) in [(80u16, 24u16), (120, 30)] {
        let r = rows(&mut app, w, h);
        row_of(&r, "go@1.23.5  mise reports prunable");
        row_of(&r, "node@24.14.1");
        assert!(
            r[h as usize - 3].contains("Enter review"),
            "keys row:\n{}",
            r.join("\n")
        );
        assert_plain(&r);
    }
}

/// Tempting wrong patch: "let the confirm grow with the dry run". The
/// what, no-Trash and command rows stay put whatever the manager printed,
/// at 80 and 120 columns, and the keys are always the last inner row.
#[test]
fn the_confirm_rows_never_move_and_the_keys_stay_visible() {
    let mut command_rows = Vec::new();
    for extra in [0usize, 12] {
        let f = Fakes::new();
        f.mise(extra);
        let mut app = f.app();
        choose(&mut app, "go@1.23.5");
        handle_key(&mut app, KeyCode::Enter);
        wait(&mut app);
        assert!(
            matches!(app.tool_sheet.as_ref().unwrap().stage, Stage::Confirm(_)),
            "{:?}",
            app.tool_sheet.as_ref().unwrap().stage
        );
        for (w, h) in [(80u16, 24u16), (120, 30)] {
            let r = rows(&mut app, w, h);
            assert!(r.iter().all(|l| l.chars().count() == w as usize));
            let no_trash = row_of(&r, "No Trash recovery: this cannot be undone.");
            let cmd = row_of(&r, "mise -C / uninstall go@1.23.5");
            assert_eq!(cmd, no_trash + 2);
            command_rows.push((w, cmd));
            row_of(&r, "Remove go@1.23.5 with mise, permanently.");
            row_of(&r, "Size: not measured");
            row_of(&r, "Reinstall is a download");
            row_of(&r, "Open files: none held");
            row_of(&r, "mise says:");
            row_of(&r, "mise's dry run (verbatim):");
            assert!(
                r[h as usize - 3].contains("Y remove (cannot be undone) · Esc cancel"),
                "keys row:\n{}",
                r.join("\n")
            );
            assert_plain(&r);
        }
    }
    for w in [80, 120] {
        let at: Vec<usize> = command_rows
            .iter()
            .filter(|(cw, _)| *cw == w)
            .map(|(_, r)| *r)
            .collect();
        assert!(at.windows(2).all(|p| p[0] == p[1]), "{w} columns: {at:?}");
    }
}

/// Tempting wrong patch: "the dry run exited 0, so show the confirm".
/// The global node is refused with the reason and the next step, and Esc
/// goes back to the list with nothing run.
#[test]
fn a_refusal_shows_reason_and_next_step_and_runs_nothing() {
    let f = Fakes::new();
    f.mise(0);
    let mut app = f.app();
    choose(&mut app, "node@24.14.1");
    handle_key(&mut app, KeyCode::Enter);
    wait(&mut app);
    for (w, h) in [(80u16, 24u16), (120, 30)] {
        let r = rows(&mut app, w, h);
        row_of(
            &r,
            "Swamp will not run this removal (node@24.14.1). Nothing ran.",
        );
        row_of(&r, "Reason: node@24.14.1 is requested by");
        row_of(&r, "Next: Edit that file or run");
        assert!(r[h as usize - 3].contains("Esc back"));
        assert_plain(&r);
    }
    handle_key(&mut app, KeyCode::Esc);
    assert!(matches!(
        app.tool_sheet.as_ref().unwrap().stage,
        Stage::Choose
    ));
    assert!(
        f.calls()
            .iter()
            .all(|c| c[1..] != ["-C", "/", "uninstall", "node@24.14.1"]),
        "{:?}",
        f.calls()
    );
}

/// Tempting wrong patch: "rebuild the argv at Enter". The argv the fake
/// recorded is the one the confirm drew, and the result line says what the
/// re-read observed, plainly; the ledger has the record.
#[test]
fn enter_runs_the_drawn_command_and_the_result_is_plain() {
    let f = Fakes::new();
    f.mise(0);
    let mut app = f.app();
    choose(&mut app, "go@1.23.5");
    handle_key(&mut app, KeyCode::Enter);
    wait(&mut app);
    let r = rows(&mut app, 120, 30);
    let drawn = r[row_of(&r, "mise -C / uninstall go@1.23.5")]
        .trim_matches(|c: char| c == '│' || c.is_whitespace())
        .to_string();
    let words = swamp_core::tool_removal::parse_command_line(&drawn);
    // `Y`, after the confirm has been on screen for the hold-off (the
    // sandbox host records in its own store).
    std::thread::sleep(swamp_tui::tool_sheet::HOLD_OFF + Duration::from_millis(50));
    handle_key(&mut app, KeyCode::Char('Y'));
    wait(&mut app);
    let ran: Vec<Vec<String>> = f
        .calls()
        .into_iter()
        .filter(|c| c[1..] == words[1..])
        .collect();
    assert_eq!(
        ran.len(),
        1,
        "exactly the drawn command ran once: {:?}",
        f.calls()
    );
    assert!(ran[0][0].ends_with("/bin/mise"));
    for (w, h) in [(80u16, 24u16), (120, 30)] {
        let r = rows(&mut app, w, h);
        row_of(&r, "Removed go@1.23.5 with mise, permanently.");
        row_of(&r, "Recorded in swamp's ledger.");
        assert!(r[h as usize - 3].contains("Esc close"));
        assert_plain(&r);
    }
    let recs = swamp_core::ledger::Ledger::open(f.root.join("store/ledger.parquet"))
        .unwrap()
        .all()
        .unwrap();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].verb, swamp_core::ledger::Verb::ToolRemove);
}

/// Tempting wrong patch: "Enter on a cut-off confirm is fine, the human
/// saw most of it". On a terminal too small for the whole command block,
/// Enter runs nothing.
#[test]
fn enter_on_a_confirm_that_does_not_fit_runs_nothing() {
    let f = Fakes::new();
    f.mise(0);
    let mut app = f.app();
    choose(&mut app, "go@1.23.5");
    handle_key(&mut app, KeyCode::Enter);
    wait(&mut app);
    rows(&mut app, 60, 10);
    std::thread::sleep(swamp_tui::tool_sheet::HOLD_OFF + Duration::from_millis(50));
    handle_key(&mut app, KeyCode::Char('Y'));
    assert!(matches!(
        app.tool_sheet.as_ref().unwrap().stage,
        Stage::Confirm(_)
    ));
    assert!(
        f.calls()
            .iter()
            .all(|c| c[1..] != ["-C", "/", "uninstall", "go@1.23.5"])
    );
}

const UUID: &str = "5FF350CD-0800-4015-B796-BE66B16D154E";

fn simulator_app(f: &Fakes, state: &str) -> App {
    f.write_fake("xcrun");
    f.answer("--version", "xcrun version 72.\n", "");
    f.answer(
        "simctl runtime list -j",
        &format!(
            r#"{{"{UUID}":{{"build":"23C54","deletable":true,"mountPath":"/Library/Developer/CoreSimulator/Volumes/iOS_23C54","runtimeIdentifier":"com.apple.CoreSimulator.SimRuntime.iOS-26-2","sizeBytes":8381044573,"state":"Ready","version":"26.2"}}}}"#
        ),
        "",
    );
    f.answer(
        "simctl list devices -j",
        &format!(
            r#"{{"devices":{{"com.apple.CoreSimulator.SimRuntime.iOS-26-2":[{{"name":"iPhone 17 Pro","state":"{state}"}},{{"name":"iPhone Air","state":"Shutdown"}}]}}}}"#
        ),
        "",
    );
    f.answer(
        &format!("simctl runtime delete {UUID} --dry-run"),
        &format!("Would delete P: {UUID} iOS (26.2 - 23C54) (Ready)\n"),
        "",
    );
    let mut app = App::new(empty_report(), "/root".into());
    app.tool_host = swamp_core::tool_removal::Host::sandboxed(&f.root);
    app.set_external_units(vec![ExternalUnit {
        detector_id: swamp_core::locations::core_simulator::CORE_SIMULATOR_DETECTOR_ID.to_string(),
        detector_name: "CoreSimulator".into(),
        category: swamp_core::locations::StorageCategory::Installation,
        provenance: swamp_core::locations::Provenance::BuiltinConvention,
        path: PathBuf::from("/Library/Developer/CoreSimulator/Volumes"),
        bytes: 20_800_000_000,
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

/// Owner decision 5 on screen: unbooted devices are counted and named on
/// the confirm (the confirm is still required); a booted one refuses.
#[test]
fn simulator_runtime_confirm_names_devices_and_a_booted_one_refuses() {
    let f = Fakes::new();
    let mut app = simulator_app(&f, "Shutdown");
    choose(&mut app, "iOS 26.2 (23C54)");
    handle_key(&mut app, KeyCode::Enter);
    wait(&mut app);
    for (w, h) in [(80u16, 24u16), (120, 30)] {
        let r = rows(&mut app, w, h);
        row_of(&r, &format!("xcrun simctl runtime delete {UUID}"));
        row_of(&r, "Warning: 2 simulator devices on this runtime");
        row_of(&r, "(simctl's sizeBytes)");
        row_of(&r, "Reinstall is a download of");
        assert!(r[h as usize - 3].contains("Y remove (cannot be undone)"));
        assert_plain(&r);
    }
    let f = Fakes::new();
    let mut app = simulator_app(&f, "Booted");
    choose(&mut app, "iOS 26.2 (23C54)");
    handle_key(&mut app, KeyCode::Enter);
    wait(&mut app);
    for (w, h) in [(80u16, 24u16), (120, 30)] {
        let r = rows(&mut app, w, h);
        row_of(&r, "iPhone 17 Pro (Booted) is not shut down");
        row_of(&r, "Next: Shut it down in Simulator");
        assert_plain(&r);
    }
    assert!(
        f.calls().iter().all(|c| !c.iter().any(|w| w == "delete")),
        "{:?}",
        f.calls()
    );
}

fn uninstalls(f: &Fakes) -> usize {
    f.calls()
        .iter()
        .filter(|c| c[1..] == ["-C", "/", "uninstall", "go@1.23.5"])
        .count()
}

/// Owner decision C1. Tempting wrong patch: "Enter on the confirm runs
/// it". Typeahead, a double or held Enter, and a pasted newline (it
/// arrives as Enter without bracketed paste) never run a removal: Enter
/// only opens the review; running takes `Y`.
#[test]
fn no_enter_ever_runs_a_removal() {
    let f = Fakes::new();
    f.mise(0);
    let mut app = f.app();
    rows(&mut app, 120, 30);
    choose(&mut app, "go@1.23.5");
    for _ in 0..3 {
        handle_key(&mut app, KeyCode::Enter); // held Enter
    }
    wait(&mut app);
    rows(&mut app, 120, 30);
    std::thread::sleep(swamp_tui::tool_sheet::HOLD_OFF + Duration::from_millis(50));
    for _ in 0..5 {
        handle_key(&mut app, KeyCode::Enter); // typeahead, paste, repeat
        rows(&mut app, 120, 30);
    }
    wait(&mut app);
    assert_eq!(uninstalls(&f), 0, "{:?}", f.calls());
    assert!(matches!(
        app.tool_sheet.as_ref().unwrap().stage,
        Stage::Confirm(_)
    ));
}

/// Tempting wrong patch: "Y is enough". `Y` before the confirm has been
/// drawn, and `Y` within the hold-off after its first draw, run nothing;
/// `Y` after it runs exactly once.
#[test]
fn y_runs_only_after_the_confirm_has_been_seen_for_the_hold_off() {
    let f = Fakes::new();
    f.mise(0);
    let mut app = f.app();
    rows(&mut app, 120, 30);
    choose(&mut app, "go@1.23.5");
    handle_key(&mut app, KeyCode::Enter);
    handle_key(&mut app, KeyCode::Char('Y')); // typed ahead, during review
    wait(&mut app);
    handle_key(&mut app, KeyCode::Char('Y')); // never drawn yet
    rows(&mut app, 120, 30);
    handle_key(&mut app, KeyCode::Char('Y')); // drawn, inside the hold-off
    wait(&mut app);
    assert_eq!(uninstalls(&f), 0, "{:?}", f.calls());
    std::thread::sleep(swamp_tui::tool_sheet::HOLD_OFF + Duration::from_millis(50));
    rows(&mut app, 120, 30);
    handle_key(&mut app, KeyCode::Char('Y'));
    handle_key(&mut app, KeyCode::Char('Y'));
    wait(&mut app);
    assert_eq!(uninstalls(&f), 1, "{:?}", f.calls());
}

/// The keys row names the remove key and says it cannot be undone.
#[test]
fn the_confirm_keys_row_names_the_remove_key() {
    let f = Fakes::new();
    f.mise(0);
    let mut app = f.app();
    choose(&mut app, "go@1.23.5");
    handle_key(&mut app, KeyCode::Enter);
    wait(&mut app);
    for (w, h) in [(80u16, 24u16), (120, 30)] {
        let r = rows(&mut app, w, h);
        assert!(
            r[h as usize - 3].contains("Y remove (cannot be undone) · Esc cancel"),
            "{}",
            r.join("\n")
        );
    }
}
