//! Adversarial audit of #177 (G5, PR #204 head d1c4d08). Harness copied
//! from tool_removal_adversarial.rs; every fake is a sh script in a temp
//! dir. A test that FAILS on d1c4d08 is a finding.
#![allow(dead_code, unused_imports)]

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use swamp_core::fs_gate::spawn::{Program, ToolResolver, is_tool_exec, is_tool_read};
use swamp_core::ledger::{Ledger, Verb};
use swamp_core::tool_removal::{
    self, Host, Manager, Preview, Refusal, Status, Target, command_line, parse_command_line,
};

const UUID: &str = "5FF350CD-0800-4015-B796-BE66B16D154E";

struct Sandbox {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        for d in ["bin", "home", "state", "shim"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        let s = Sandbox { _tmp: tmp, root };
        s.write_fake("mise");
        s.write_fake("xcrun");
        s
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn state(&self, name: &str, text: &str) {
        fs::write(self.root.join("state").join(name), text).unwrap();
    }

    /// A fake manager: logs `$0` and every argument (tab-separated) and
    /// its environment, then answers from `state/<key>.out` (stdout),
    /// `state/<key>.err` (stderr), `state/<key>.code` (exit) and, for a
    /// removal, runs `state/<key>.sh` first. `<key>` is the argv joined
    /// with `_`, `/` as `S` (per sandbox, so
    /// mise and xcrun never share one).
    fn write_fake(&self, name: &str) {
        let s = self.root.display();
        let script = format!(
            r#"#!/bin/sh
S='{s}'
{{ printf '%s' "$0"; for a in "$@"; do printf '\t%s' "$a"; done; printf '\n'; }} >> "$S/calls.log"
env | sort > "$S/env.last"
key=$(printf '%s' "$*" | tr ' /' '_S')
if [ -f "$S/state/$key.sh" ]; then sh "$S/state/$key.sh"; fi
if [ -f "$S/state/$key.out" ]; then cat "$S/state/$key.out"; fi
if [ -f "$S/state/$key.err" ]; then cat "$S/state/$key.err" >&2; fi
if [ -f "$S/state/$key.code" ]; then exit "$(cat "$S/state/$key.code")"; fi
if [ -f "$S/state/$key.out" ] || [ -f "$S/state/$key.err" ] || [ -f "$S/state/$key.sh" ]; then exit 0; fi
echo "fake {name}: no fixture for $key" >&2
exit 97
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

    fn answer(&self, argv: &str, stdout: &str, stderr: &str, code: i32) {
        let k = Self::key(argv);
        self.state(&format!("{k}.out"), stdout);
        self.state(&format!("{k}.err"), stderr);
        self.state(&format!("{k}.code"), &code.to_string());
    }

    fn on_run(&self, argv: &str, sh: &str) {
        self.state(&format!("{}.sh", Self::key(argv)), sh);
    }

    fn host(&self) -> Host {
        Host::sandboxed(&self.root)
    }

    fn calls(&self) -> Vec<Vec<String>> {
        fs::read_to_string(self.root.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| l.split('\t').map(str::to_string).collect())
            .collect()
    }

    /// Calls whose arguments (without `$0`) are exactly `argv`.
    fn ran(&self, argv: &str) -> usize {
        let want: Vec<&str> = argv.split(' ').collect();
        self.calls()
            .iter()
            .filter(|c| c[1..].iter().map(String::as_str).eq(want.iter().copied()))
            .count()
    }

    fn ledger(&self) -> Ledger {
        Ledger::open(self.root.join("store/ledger.parquet")).unwrap()
    }

    // ---- mise fixtures -------------------------------------------------

    fn install_dir(&self, tool: &str, version: &str) -> PathBuf {
        self.home()
            .join(".local/share/mise/installs")
            .join(tool)
            .join(version)
    }

    fn ls_json(&self, entries: &[(&str, &str, Option<&str>)]) -> String {
        let mut by_tool: std::collections::BTreeMap<&str, Vec<String>> = Default::default();
        for (tool, version, source) in entries {
            let path = self.install_dir(tool, version);
            let src = source
                .map(|s| {
                    format!(
                        r#","requested_version":"latest","source":{{"type":"mise.toml","path":"{s}"}}"#
                    )
                })
                .unwrap_or_default();
            by_tool.entry(tool).or_default().push(format!(
                r#"{{"version":"{version}","install_path":"{}"{src},"installed":true,"active":false}}"#,
                path.display()
            ));
        }
        let body: Vec<String> = by_tool
            .iter()
            .map(|(t, e)| format!(r#""{t}":[{}]"#, e.join(",")))
            .collect();
        format!("{{{}}}", body.join(","))
    }

    fn prune_text(&self, tvs: &[&str]) -> String {
        let mut out = String::new();
        for tv in tvs {
            let (tool, version) = tv.split_once('@').unwrap();
            out.push_str(&format!(
                "mise {tv} is prunable: {tool} is required at other by ~/src/p/mise.toml\n\
                 mise {tv} [dryrun]  uninstall\n\
                 mise {tv} [dryrun]  remove ~/.local/share/mise/installs/{tool}/{version}\n\
                 mise {tv} [dryrun]  ✓ done\n"
            ));
        }
        out
    }

    fn uninstall_dry(tv: &str) -> String {
        let (tool, version) = tv.split_once('@').unwrap();
        format!(
            "mise {tv}       uninstall\n\
             mise {tv}       remove ~/.local/share/mise/installs/{tool}/{version}\n\
             mise {tv}     ✓ uninstalled (dry-run)\n"
        )
    }

    /// The reporter's mise: node 24.14.1 from the global config, go and
    /// java temurin prunable.
    fn standard_mise(&self) {
        let global = self.home().join(".config/mise/config.toml");
        let global = global.to_str().unwrap();
        for (t, v) in [
            ("node", "24.14.1"),
            ("go", "1.23.5"),
            ("java", "temurin-17.0.20+101"),
        ] {
            fs::create_dir_all(self.install_dir(t, v)).unwrap();
        }
        self.answer("--version", "2026.9.15 macos-arm64 (2026-09-27)\n", "", 0);
        self.answer(
            "-C / ls --json --installed",
            &self.ls_json(&[
                ("go", "1.23.5", None),
                ("java", "temurin-17.0.20+101", None),
                ("node", "24.14.1", Some(global)),
            ]),
            "",
            0,
        );
        self.answer(
            "-C / prune --tools --dry-run",
            "",
            &self.prune_text(&["go@1.23.5", "java@temurin-17.0.20+101"]),
            0,
        );
        for tv in ["go@1.23.5", "node@24.14.1", "java@temurin-17.0.20+101"] {
            self.answer(
                &format!("-C / uninstall --dry-run {tv}"),
                "",
                &Self::uninstall_dry(tv),
                0,
            );
        }
    }

    /// `mise uninstall go@1.23.5` really removes (the dir and the list
    /// entry), then exits `code`.
    fn go_uninstall_removes(&self, code: i32) {
        let after = self.ls_json(&[
            ("java", "temurin-17.0.20+101", None),
            (
                "node",
                "24.14.1",
                Some(
                    self.home()
                        .join(".config/mise/config.toml")
                        .to_str()
                        .unwrap(),
                ),
            ),
        ]);
        self.state("ls_after.json", &after);
        let ls_key = Self::key("-C / ls --json --installed");
        self.on_run(
            "-C / uninstall go@1.23.5",
            &format!(
                "rm -r '{}'; cp '{}/state/ls_after.json' '{}/state/{ls_key}.out'; exit {code}\n",
                self.install_dir("go", "1.23.5").display(),
                self.root.display(),
                self.root.display()
            ),
        );
        self.state(
            &format!("{}.code", Self::key("-C / uninstall go@1.23.5")),
            &code.to_string(),
        );
    }

    // ---- simctl fixtures ----------------------------------------------

    fn standard_simctl(&self, device_state: &str) {
        self.answer("--version", "xcrun version 72.\n", "", 0);
        self.answer(
            "simctl runtime list -j",
            &format!(
                r#"{{"{UUID}":{{"build":"23C54","deletable":true,"identifier":"{UUID}","mountPath":"{}/vol/iOS_23C54","runtimeIdentifier":"com.apple.CoreSimulator.SimRuntime.iOS-26-2","sizeBytes":8381044573,"state":"Ready","version":"26.2"}}}}"#,
                self.root.display()
            ),
            "",
            0,
        );
        self.answer(
            "simctl list devices -j",
            &format!(
                r#"{{"devices":{{"com.apple.CoreSimulator.SimRuntime.iOS-26-2":[{{"name":"iPhone 17 Pro","state":"{device_state}"}},{{"name":"iPhone Air","state":"Shutdown"}}]}}}}"#
            ),
            "",
            0,
        );
        self.answer(
            &format!("simctl runtime delete {UUID} --dry-run"),
            &format!("Would delete P: {UUID} iOS (26.2 - 23C54) (Ready)\n"),
            "",
            0,
        );
    }
}

fn go() -> Target {
    Target::MiseVersion {
        tool: "go".into(),
        version: "1.23.5".into(),
    }
}

fn refused(r: Result<Preview, Refusal>) -> Refusal {
    match r {
        Ok(p) => panic!("expected a refusal, got a preview for {}", p.command_line()),
        Err(r) => r,
    }
}

/// No verdict word and no em dash in anything shown to the human.
fn assert_plain(text: &str) {
    assert!(!text.contains('\u{2014}'), "em dash in {text:?}");
    let lower = text.to_lowercase();
    for w in [
        "unused", "obsolete", "stale", "orphan", "safe to", "junk", "garbage",
    ] {
        assert!(!lower.contains(w), "verdict word `{w}` in {text:?}");
    }
}

fn assert_refusal_plain(r: &Refusal) {
    assert_plain(&r.reason);
    assert_plain(&r.next);
    assert!(!r.next.is_empty(), "a refusal always says what to do next");
}

fn preview_text(p: &Preview) -> String {
    let mut all = vec![
        p.title().to_string(),
        p.command_line(),
        p.regen().to_string(),
        p.open_files().to_string(),
    ];
    all.extend(p.evidence().iter().cloned());
    all.extend(p.warnings().iter().cloned());
    all.join("\n")
}

/// Tempting wrong patch: "trust `mise uninstall` to refuse a version a
/// config asks for". mise's own dry run exits 0 for the global node; even
/// with a fake prune that (wrongly) lists it, swamp refuses and names the
/// global config.

// ======================= audit attacks =======================

fn review(sb: &Sandbox, t: &Target) -> Result<Preview, Refusal> {
    tool_removal::review_target(&sb.host(), t, &[])
}

/// Attack 4 / diff bug: `check_removes` accepts any path under mise's
/// dirs whose last component equals the version, compared lexically.
/// Tempting wrong patch: "Path::starts_with is a real prefix check" (it
/// keeps `..`). A dry run naming `installs/../../../../Documents/1.23.5`
/// must refuse.
#[test]
fn adv_dry_run_path_with_dotdot_escaping_mise_dirs_refuses() {
    let sb = Sandbox::new();
    sb.standard_mise();
    sb.answer(
        "-C / uninstall --dry-run go@1.23.5",
        "",
        "mise go@1.23.5       uninstall\n\
         mise go@1.23.5       remove ~/.local/share/mise/installs/go/1.23.5\n\
         mise go@1.23.5       remove ~/.local/share/mise/installs/../../../../Documents/1.23.5\n\
         mise go@1.23.5     ✓ uninstalled (dry-run)\n",
        0,
    );
    let r = refused(review(&sb, &go()));
    assert!(r.reason.contains("Documents"), "{}", r.reason);
}

/// Tempting wrong patch: "a path under mise's dirs ending in the version
/// is this version's". Another tool's install dir with the same version
/// string is not go's; a dry run naming it must refuse.
#[test]
fn adv_dry_run_naming_another_tools_install_refuses() {
    let sb = Sandbox::new();
    sb.standard_mise();
    sb.answer(
        "-C / uninstall --dry-run go@1.23.5",
        "",
        "mise go@1.23.5       uninstall\n\
         mise go@1.23.5       remove ~/.local/share/mise/installs/go/1.23.5\n\
         mise go@1.23.5       remove ~/.local/share/mise/installs/node/1.23.5\n\
         mise go@1.23.5     ✓ uninstalled (dry-run)\n",
        0,
    );
    let r = refused(review(&sb, &go()));
    assert!(r.reason.contains("node"), "{}", r.reason);
}

/// Tempting wrong patch: "`.and_then(as_str)` on `source.path`": a
/// `source` of an unexpected type reads as "nothing requests it". A
/// listing swamp cannot read must refuse, never pass the consumer guard.
#[test]
fn adv_ls_source_of_wrong_type_fails_closed() {
    let sb = Sandbox::new();
    sb.standard_mise();
    let global = sb.home().join(".config/mise/config.toml");
    let path = sb.install_dir("go", "1.23.5");
    for src in [
        format!(r#""source":"{}""#, global.display()),
        format!(
            r#""source":{{"type":"mise.toml","path":["{}"]}}"#,
            global.display()
        ),
        r#""source":{"type":"mise.toml"}"#.to_string(),
    ] {
        sb.answer(
            "-C / ls --json --installed",
            &format!(
                r#"{{"go":[{{"version":"1.23.5","install_path":"{}",{src},"installed":true,"active":false}}]}}"#,
                path.display()
            ),
            "",
            0,
        );
        assert!(review(&sb, &go()).is_err(), "passed with {src}");
    }
}

/// Tempting wrong patch: "`find` the first entry for t@v". A listing with
/// the same t@v twice, the second requested by the global config, must
/// not pass on the first.
#[test]
fn adv_ls_duplicate_entries_are_not_resolved_by_first_match() {
    let sb = Sandbox::new();
    sb.standard_mise();
    let global = sb.home().join(".config/mise/config.toml");
    let path = sb.install_dir("go", "1.23.5");
    sb.answer(
        "-C / ls --json --installed",
        &format!(
            r#"{{"go":[{{"version":"1.23.5","install_path":"{p}","installed":true,"active":false}},{{"version":"1.23.5","install_path":"{p}","requested_version":"1.23.5","source":{{"type":"mise.toml","path":"{g}"}},"installed":true,"active":true}}]}}"#,
            p = path.display(),
            g = global.display()
        ),
        "",
        0,
    );
    assert!(review(&sb, &go()).is_err());
}

fn sim_runtimes(sb: &Sandbox, body: &str) {
    sb.answer(
        "simctl runtime list -j",
        &format!(r#"{{"{UUID}":{{{body}}}}}"#),
        "",
        0,
    );
}

fn sim() -> Target {
    Target::SimRuntime { uuid: UUID.into() }
}

/// Tempting wrong patch: "`unwrap_or_default()` on runtimeIdentifier".
/// Without it the booted-device lookup keys on "" and finds nobody: the
/// booted iPhone on this runtime is never seen. Must refuse.
#[test]
fn adv_runtime_without_identifier_does_not_skip_the_booted_check() {
    let sb = Sandbox::new();
    sb.standard_simctl("Booted");
    sim_runtimes(
        &sb,
        r#""build":"23C54","deletable":true,"sizeBytes":1,"state":"Ready","version":"26.2""#,
    );
    let r = review(&sb, &sim());
    assert!(
        r.is_err(),
        "previewed a runtime whose devices swamp could not match"
    );
}

/// Tempting wrong patch: "`state == \"Booted\"` means nothing is running".
/// A device Booting or Shutting Down, or with no state, is not known to
/// be shut down; simctl would shut it down and delete anyway.
#[test]
fn adv_device_booting_or_without_state_refuses() {
    for dev in [
        r#"{"name":"iPhone 17 Pro","state":"Booting"}"#,
        r#"{"name":"iPhone 17 Pro","state":"Shutting Down"}"#,
        r#"{"name":"iPhone 17 Pro"}"#,
        r#"{"name":"iPhone 17 Pro","state":7}"#,
    ] {
        let sb = Sandbox::new();
        sb.standard_simctl("Shutdown");
        sb.answer(
            "simctl list devices -j",
            &format!(r#"{{"devices":{{"com.apple.CoreSimulator.SimRuntime.iOS-26-2":[{dev}]}}}}"#),
            "",
            0,
        );
        assert!(review(&sb, &sim()).is_err(), "previewed with device {dev}");
    }
}

/// Attack 7: the test-build guard trusts whatever directory the test
/// calls a sandbox. Tempting wrong patch: "sandbox = any dir the test
/// passes". A sandbox rooted at a system directory resolves the REAL
/// /usr/bin/xcrun and the guard (path starts_with sandbox) would let it
/// run. Only resolution is exercised here; nothing is spawned.
#[test]
fn adv_a_sandbox_rooted_at_a_system_dir_does_not_resolve_a_real_manager() {
    let (r, counted) = swamp_core::work_counters::measured(|| {
        ToolResolver::sandboxed(Path::new("/usr")).resolve(Program::Xcrun)
    });
    assert_eq!(counted.subprocess_spawns, 0);
    if let Ok(bin) = r {
        panic!(
            "a 'sandbox' of /usr resolved the real {} (the spawn guard would accept it)",
            bin.path().display()
        );
    }
}

/// Attack 3: command == shown, over hostile operands. Whatever the
/// formatter renders must parse back to the same argv, and no hostile
/// word is ever an exec shape.
#[test]
fn adv_command_line_round_trips_hostile_words_and_hostile_words_never_exec() {
    let hostile = [
        "-rf",
        "--yes",
        "a b",
        "a\nb",
        "a\tb",
        "it's",
        "''",
        "\\",
        "\\'",
        "$(id)",
        "`id`",
        "日本語",
        "e\u{301}",
        "\u{202e}evil",
        "",
        " ",
        "@",
        "+",
        "x".repeat(4000).leak(),
        "--",
        "-C",
        "a'b'c",
        "'\\''",
        "\u{1b}[2K",
    ];
    for h in hostile {
        let argv: Vec<OsString> = ["-C", "/", "uninstall", h]
            .iter()
            .map(OsString::from)
            .collect();
        let line = command_line("mise", &argv);
        let words = parse_command_line(&line);
        let back: Vec<OsString> = words[1..].iter().map(OsString::from).collect();
        assert_eq!(back, argv, "round trip of {h:?} via {line:?}");
        assert!(!line.contains('\n') || h.contains('\n'));
        assert!(
            !is_tool_exec(Program::Mise, &argv),
            "hostile operand {h:?} is an exec shape"
        );
        for tv in [format!("go@{h}"), format!("{h}@1")] {
            let a: Vec<OsString> = ["-C", "/", "uninstall", tv.as_str()]
                .iter()
                .map(OsString::from)
                .collect();
            let plausible = !h.is_empty()
                && h.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c))
                && h.chars().next().unwrap().is_ascii_alphanumeric();
            if !plausible {
                assert!(!is_tool_exec(Program::Mise, &a), "{tv:?} is an exec shape");
            }
        }
        let s: Vec<OsString> = ["simctl", "runtime", "delete", h]
            .iter()
            .map(OsString::from)
            .collect();
        assert!(!is_tool_exec(Program::Xcrun, &s), "{h:?} as a runtime");
    }
    for bad in [
        "all",
        "ALL",
        "5ff350cd-0800-4015-b796-be66b16d154e",
        "--outdated",
    ] {
        let s: Vec<OsString> = ["simctl", "runtime", "delete", bad]
            .iter()
            .map(OsString::from)
            .collect();
        assert!(!is_tool_exec(Program::Xcrun, &s), "{bad}");
    }
}

/// Attack 9: a rendered line can hide text. Bidi overrides and zero-width
/// characters are not `is_control` in Rust (they are format chars) and
/// survive `clean_line`; a dry-run line with U+202E can display a path
/// reversed. Tempting wrong patch: "strip `char::is_control`".
#[test]
fn adv_dry_run_bidi_and_zero_width_characters_are_refused_or_stripped() {
    let sb = Sandbox::new();
    sb.standard_mise();
    sb.answer(
        "-C / uninstall --dry-run go@1.23.5",
        "",
        "mise go@1.23.5       uninstall\n\
         mise go@1.23.5       remove ~/.local/share/mise/installs/go/1.23.5\n\
         mise WARN \u{202e}5.32.1/og/sllatsni\u{200b}\n\
         mise go@1.23.5     ✓ uninstalled (dry-run)\n",
        0,
    );
    match review(&sb, &go()) {
        Err(_) => {}
        Ok(p) => {
            for l in p.dry_output() {
                assert!(
                    !l.chars().any(|c| matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200b}'..='\u{200f}')),
                    "bidi/zero-width reached the confirm: {l:?}"
                );
            }
        }
    }
}

/// Attack 9: an OSC sequence (`ESC ] 52 ; c ; <base64> BEL`, clipboard
/// write) must leave no ESC or BEL on screen. clean_line drops ESC and the
/// next char only; the payload stays as visible text (acceptable) but no
/// control character may remain.
#[test]
fn adv_osc52_and_c1_controls_never_reach_the_confirm() {
    let sb = Sandbox::new();
    sb.standard_mise();
    sb.answer(
        "-C / uninstall --dry-run go@1.23.5",
        "",
        "mise WARN \u{1b}]52;c;cm0gLXJmIH4=\u{7} hi \u{9b}2J \u{1b}[?1049l\n\
         mise go@1.23.5       uninstall\n\
         mise go@1.23.5       remove ~/.local/share/mise/installs/go/1.23.5\n\
         mise go@1.23.5     ✓ uninstalled (dry-run)\n",
        0,
    );
    let p = review(&sb, &go()).expect("a WARN line is shown, not refused");
    for l in p.dry_output() {
        assert!(!l.chars().any(|c| c.is_control()), "{l:?}");
    }
}

/// Attack 4: CRLF output (a mise running under a pty wrapper) must not
/// silently pass or silently break; the \r is stripped and it parses, or
/// it refuses. And non-UTF-8 bytes in a remove path must refuse (lossy
/// U+FFFD is not the path mise removes).
#[test]
fn adv_non_utf8_remove_path_refuses() {
    let sb = Sandbox::new();
    sb.standard_mise();
    let mut err: Vec<u8> = b"mise go@1.23.5       uninstall\nmise go@1.23.5       remove ~/.local/share/mise/installs/go/1.23.5\nmise go@1.23.5       remove ~/.local/share/mise/installs/go/\xff1.23.5\nmise go@1.23.5     \xe2\x9c\x93 uninstalled (dry-run)\n".to_vec();
    let k = Sandbox::key("-C / uninstall --dry-run go@1.23.5");
    fs::write(sb.root.join(format!("state/{k}.err")), &mut err).unwrap();
    // `\u{FFFD}1.23.5` is not the version, so check_removes should refuse
    // on the file name; assert only that it is not a preview.
    assert!(review(&sb, &go()).is_err());
}

/// Attack 8 / diff-adjacent bug (pre-existing in Ledger::append, now the
/// only record of a permanent removal): a ledger that does not parse
/// (truncated write, garbage) is read as empty by `append`
/// (`read_ledger_rows(..).unwrap_or_default()`) and rewritten with just
/// the new row: every earlier record, including `tool-remove` rows, is
/// silently gone. Tempting wrong patch: "an unreadable ledger is a fresh
/// ledger". Must refuse to append (Err), never drop history.
#[test]
fn adv_append_to_an_unreadable_ledger_never_drops_history() {
    let sb = Sandbox::new();
    sb.standard_mise();
    sb.go_uninstall_removes(0);
    let host = sb.host();
    let p = tool_removal::review_target(&host, &go(), &[]).unwrap();
    let ledger = sb.ledger();
    let first = tool_removal::execute(&host, &p, &[], &ledger);
    assert!(first.recorded.is_ok());
    let path = sb.root.join("store/ledger.parquet");
    let good = fs::read(&path).unwrap();
    // Truncate the file (a crash mid-write).
    fs::write(&path, &good[..good.len() / 2]).unwrap();
    // Any later append (here: a refused re-run, which records too).
    let again = tool_removal::execute(&host, &p, &[], &ledger);
    let after = Ledger::open(&path).unwrap().all();
    match (again.recorded, after) {
        (Err(_), _) => {} // refused to overwrite: fine
        (Ok(()), Ok(recs)) => assert!(
            recs.len() >= 2,
            "append over an unreadable ledger kept {} record(s); the earlier tool-remove row is gone",
            recs.len()
        ),
        (Ok(()), Err(e)) => panic!("recorded Ok but the ledger does not read: {e}"),
    }
}

/// Attack 8: writes a store copy's ledger with a tool-remove row, for
/// the v0.7.5 binary check (run by hand with ADV_LEDGER_DIR set).
#[test]
#[ignore]
fn adv_write_tool_remove_row_into_store_copy() {
    let dir = PathBuf::from(std::env::var_os("ADV_LEDGER_DIR").expect("ADV_LEDGER_DIR"));
    assert!(dir.starts_with("/private/tmp") || dir.starts_with("/tmp"), "a copy only");
    let sb = Sandbox::new();
    sb.standard_mise();
    sb.go_uninstall_removes(0);
    let host = sb.host();
    let p = tool_removal::review_target(&host, &go(), &[]).unwrap();
    let ledger = Ledger::open(dir.join("ledger.parquet")).unwrap();
    let before = ledger.all().map(|r| r.len()).unwrap_or(0);
    let out = tool_removal::execute(&host, &p, &[], &ledger);
    assert!(out.recorded.is_ok());
    assert_eq!(ledger.all().unwrap().len(), before + 1);
}
