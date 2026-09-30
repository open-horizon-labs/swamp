//! Tool-managed removal (#177) against fake managers. Every fake is a
//! `sh` script under a temp dir that records its argv and environment
//! and touches nothing outside that dir; the resolver is a sandbox one,
//! and any spawn whose binary is not under the sandbox panics in a test
//! build (`spawn::guard_test_sandbox`). Each test names the tempting
//! wrong patch it fails.

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
#[test]
fn global_config_version_refused_even_though_mise_dry_run_exits_0() {
    let sb = Sandbox::new();
    sb.standard_mise();
    sb.answer(
        "-C / prune --tools --dry-run",
        "",
        &sb.prune_text(&["go@1.23.5", "node@24.14.1"]),
        0,
    );
    let target = Target::MiseVersion {
        tool: "node".into(),
        version: "24.14.1".into(),
    };
    let r = refused(tool_removal::review_target(&sb.host(), &target, &[]));
    assert!(r.reason.contains("global config"), "{}", r.reason);
    assert!(r.next.contains("mise unuse -g node"), "{}", r.next);
    assert_refusal_plain(&r);
    // And the prune set that contains it refuses as a whole.
    let r = refused(tool_removal::review_target(
        &sb.host(),
        &Target::MisePrune,
        &[],
    ));
    assert!(r.reason.contains("node@24.14.1"), "{}", r.reason);
    assert_eq!(sb.ran("-C / uninstall node@24.14.1"), 0);
    assert_eq!(sb.ran("-C / prune --tools"), 0);
}

/// Tempting wrong patch: "`source: null` in `mise ls` means nothing asks
/// for it". `mise ls` answers from its cwd; only mise's prune knows the
/// configs it tracks, so a version prune does not list is refused.
#[test]
fn source_null_from_cwd_is_not_no_consumer() {
    let sb = Sandbox::new();
    sb.standard_mise();
    sb.answer(
        "-C / prune --tools --dry-run",
        "",
        &sb.prune_text(&["java@temurin-17.0.20+101"]),
        0,
    );
    let r = refused(tool_removal::review_target(&sb.host(), &go(), &[]));
    assert!(
        r.reason.contains("prune does not list go@1.23.5"),
        "{}",
        r.reason
    );
    assert_refusal_plain(&r);
}

/// Tempting wrong patch: bare `mise prune` ("--tools is the default
/// anyway"). Bare prune also prunes tracked config links: the removal
/// shape without `--tools` does not exist, and what runs carries it.
#[test]
fn prune_runs_only_as_prune_tools() {
    let words = |s: &str| -> Vec<OsString> { s.split(' ').map(OsString::from).collect() };
    assert!(!is_tool_exec(Program::Mise, &words("-C / prune")));
    assert!(!is_tool_exec(Program::Mise, &words("prune --tools")));
    assert!(!is_tool_exec(
        Program::Mise,
        &words("-C / prune --tools --yes")
    ));
    assert!(is_tool_exec(Program::Mise, &words("-C / prune --tools")));

    let sb = Sandbox::new();
    sb.standard_mise();
    sb.on_run("-C / prune --tools", "exit 0\n");
    let p = tool_removal::review_target(&sb.host(), &Target::MisePrune, &[]).unwrap();
    assert_eq!(p.command_line(), "mise -C / prune --tools");
    tool_removal::execute(&sb.host(), &p, &[], &sb.ledger());
    assert_eq!(sb.ran("-C / prune --tools"), 1);
    assert_eq!(sb.ran("-C / prune"), 0, "bare prune never runs");
}

/// Tempting wrong patch: "one shape with an optional `--dry-run`". Each
/// dry-run shape minus its flag is a removal, which the read path refuses;
/// and the removal is exactly the dry run minus that one flag.
#[test]
fn a_dry_run_without_its_flag_is_not_a_read() {
    let words = |s: &str| -> Vec<OsString> { s.split(' ').map(OsString::from).collect() };
    for (program, dry) in [
        (
            Program::Mise,
            "-C / uninstall --dry-run go@1.23.5".to_string(),
        ),
        (Program::Mise, "-C / prune --tools --dry-run".to_string()),
        (
            Program::Xcrun,
            format!("simctl runtime delete {UUID} --dry-run"),
        ),
    ] {
        let dry_words = words(&dry);
        assert!(is_tool_read(program, &dry_words), "{dry}");
        let exec: Vec<OsString> = dry_words
            .iter()
            .filter(|w| w.as_os_str() != "--dry-run")
            .cloned()
            .collect();
        assert!(!is_tool_read(program, &exec), "{exec:?} must not be a read");
        assert!(is_tool_exec(program, &exec), "{exec:?} is the removal");
        assert!(!is_tool_exec(program, &dry_words));
    }
    let sb = Sandbox::new();
    sb.standard_mise();
    let p = tool_removal::review_target(&sb.host(), &go(), &[]).unwrap();
    assert_eq!(p.command_line(), "mise -C / uninstall go@1.23.5");
    assert_eq!(sb.ran("-C / uninstall --dry-run go@1.23.5"), 1);
    assert_eq!(
        sb.ran("-C / uninstall go@1.23.5"),
        0,
        "review never removes"
    );
}

/// Tempting wrong patch: "a DockerRef-style permissive operand slot".
/// `simctl runtime delete all` (its own alias) and the set flags can never
/// be built, reviewed or run.
#[test]
fn simctl_all_and_set_flags_are_never_an_operand() {
    let words = |s: &str| -> Vec<OsString> { s.split(' ').map(OsString::from).collect() };
    for bad in [
        "simctl runtime delete all",
        "simctl runtime delete --outdated",
        "simctl runtime delete --unusable",
        "simctl runtime delete --notUsedSinceDays 1",
        "simctl runtime delete 5ff350cd-0800-4015-b796-be66b16d154e",
        "simctl runtime delete iOS-26-2",
    ] {
        assert!(!is_tool_exec(Program::Xcrun, &words(bad)), "{bad}");
        assert!(
            !is_tool_read(Program::Xcrun, &words(&format!("{bad} --dry-run"))),
            "{bad}"
        );
    }
    let sb = Sandbox::new();
    sb.standard_simctl("Shutdown");
    for uuid in ["all", "--outdated", "-d"] {
        let r = refused(tool_removal::review_target(
            &sb.host(),
            &Target::SimRuntime { uuid: uuid.into() },
            &[],
        ));
        assert_refusal_plain(&r);
    }
    assert!(
        sb.calls()
            .iter()
            .all(|c| !c.contains(&"delete".to_string())),
        "nothing was ever asked to delete: {:?}",
        sb.calls()
    );
}

/// Tempting wrong patch: "simctl handles booted simulators". It shuts
/// them down and deletes anyway.
#[test]
fn a_booted_simulator_refuses_its_runtime() {
    let sb = Sandbox::new();
    sb.standard_simctl("Booted");
    let r = refused(tool_removal::review_target(
        &sb.host(),
        &Target::SimRuntime { uuid: UUID.into() },
        &[],
    ));
    assert!(r.reason.contains("iPhone 17 Pro is booted"), "{}", r.reason);
    assert!(r.next.contains("Shut it down"), "{}", r.next);
    assert_refusal_plain(&r);
    assert_eq!(
        sb.ran(&format!("simctl runtime delete {UUID} --dry-run")),
        0
    );
}

/// Owner decision 5: unbooted devices do not refuse, but the confirm
/// names how many and which.
#[test]
fn unbooted_devices_are_named_on_the_confirm() {
    let sb = Sandbox::new();
    sb.standard_simctl("Shutdown");
    let p = tool_removal::review_target(&sb.host(), &Target::SimRuntime { uuid: UUID.into() }, &[])
        .unwrap();
    let w = p.warnings().join("\n");
    assert!(w.contains("2 simulator devices"), "{w}");
    assert!(
        w.contains("iPhone 17 Pro") && w.contains("iPhone Air"),
        "{w}"
    );
    assert_eq!(p.size().unwrap().bytes, 8_381_044_573);
    assert!(p.regen().contains("download"), "{}", p.regen());
    assert_eq!(
        p.command_line(),
        format!("xcrun simctl runtime delete {UUID}")
    );
    assert_plain(&preview_text(&p));
}

/// Tempting wrong patch: "rebuild the argv at exec time from the target".
/// What the fake recorded is exactly what the confirm's command line reads
/// back as, program included.
#[test]
fn the_command_run_is_the_command_shown() {
    for manager in [Manager::Mise, Manager::Simulator] {
        let sb = Sandbox::new();
        let target = match manager {
            Manager::Mise => {
                sb.standard_mise();
                sb.go_uninstall_removes(0);
                go()
            }
            Manager::Simulator => {
                sb.standard_simctl("Shutdown");
                let gone = r#"{}"#;
                sb.state("rt_after.json", gone);
                let key = Sandbox::key("simctl runtime list -j");
                sb.on_run(
                    &format!("simctl runtime delete {UUID}"),
                    &format!(
                        "cp '{0}/state/rt_after.json' '{0}/state/{key}.out'\n",
                        sb.root.display()
                    ),
                );
                Target::SimRuntime { uuid: UUID.into() }
            }
        };
        let p = tool_removal::review_target(&sb.host(), &target, &[]).unwrap();
        let shown = p.command_line();
        let out = tool_removal::execute(&sb.host(), &p, &[], &sb.ledger());
        assert_eq!(out.status, Status::Removed, "{}", out.line);
        let shown_words = parse_command_line(&shown);
        let exec = sb
            .calls()
            .into_iter()
            .find(|c| c[1..] == shown_words[1..])
            .unwrap_or_else(|| panic!("`{shown}` never ran: {:?}", sb.calls()));
        assert_eq!(Path::new(&exec[0]), p.program_path());
        assert_eq!(
            Path::new(&exec[0]).file_name().unwrap().to_str().unwrap(),
            shown_words[0]
        );
        assert_eq!(
            sb.calls()
                .iter()
                .filter(|c| !c
                    .iter()
                    .any(|w| w == "--dry-run" || w == "-j" || w == "--version")
                    && !c.contains(&"ls".to_string()))
                .count(),
            1,
            "exactly one removal ran: {:?}",
            sb.calls()
        );
        assert_plain(&out.line);
    }
}

/// Tempting wrong patch: "Enter executes the stored preview". Between
/// review and Enter a project started requesting go: Enter refuses and
/// nothing runs.
#[test]
fn a_preview_gone_out_of_date_refuses_at_enter() {
    let sb = Sandbox::new();
    sb.standard_mise();
    sb.go_uninstall_removes(0);
    let p = tool_removal::review_target(&sb.host(), &go(), &[]).unwrap();
    // A config now requests go@1.23.5.
    sb.answer(
        "-C / ls --json --installed",
        &sb.ls_json(&[
            ("go", "1.23.5", Some("/Users/dev/src/p/mise.toml")),
            ("java", "temurin-17.0.20+101", None),
        ]),
        "",
        0,
    );
    let out = tool_removal::execute(&sb.host(), &p, &[], &sb.ledger());
    let Status::Refused(r) = &out.status else {
        panic!("expected a refusal: {}", out.line)
    };
    assert!(r.reason.contains("Since review"), "{}", r.reason);
    assert_eq!(sb.ran("-C / uninstall go@1.23.5"), 0);
    assert!(sb.install_dir("go", "1.23.5").exists());
    let rec = sb.ledger().all().unwrap();
    assert!(rec[0].outcome.starts_with("refused:"), "{}", rec[0].outcome);
    assert_eq!(rec[0].observed_path_state.as_deref(), Some("not run"));
    assert_plain(&out.line);
}

/// Tempting wrong patch: "exec after one dry run". The prune set grew
/// between review and Enter: refused, nothing runs.
#[test]
fn a_prune_set_that_changed_since_review_refuses() {
    let sb = Sandbox::new();
    sb.standard_mise();
    let p = tool_removal::review_target(&sb.host(), &Target::MisePrune, &[]).unwrap();
    sb.answer(
        "-C / prune --tools --dry-run",
        "",
        &sb.prune_text(&["go@1.23.5"]),
        0,
    );
    let out = tool_removal::execute(&sb.host(), &p, &[], &sb.ledger());
    assert!(matches!(out.status, Status::Refused(_)), "{}", out.line);
    assert_eq!(sb.ran("-C / prune --tools"), 0);
}

/// Tempting wrong patch: "`.ok()` means nothing is open". An open-file
/// check that could not finish blocks the removal (owner decision 4), and
/// a held file blocks too.
#[test]
fn an_open_file_check_that_could_not_finish_blocks() {
    let sb = Sandbox::new();
    sb.standard_mise();
    let r = refused(tool_removal::review_target(
        &sb.host().with_open_files_unknown("lsof: timed out"),
        &go(),
        &[],
    ));
    assert!(r.reason.contains("could not be checked"), "{}", r.reason);
    assert_refusal_plain(&r);
    let held = sb.install_dir("go", "1.23.5").join("bin/go");
    let r = refused(tool_removal::review_target(
        &sb.host().with_open_file_held(&held, "go"),
        &go(),
        &[],
    ));
    assert!(r.reason.contains("go has"), "{}", r.reason);
    assert_eq!(sb.ran("-C / uninstall go@1.23.5"), 0);
}

/// Tempting wrong patch: "exit 0 means removed". A fake that exits 0 and
/// removes nothing is recorded as still listed, never as removed.
#[test]
fn exit_0_without_removing_is_recorded_as_still_listed() {
    let sb = Sandbox::new();
    sb.standard_mise();
    sb.on_run("-C / uninstall go@1.23.5", "exit 0\n");
    let p = tool_removal::review_target(&sb.host(), &go(), &[]).unwrap();
    let out = tool_removal::execute(&sb.host(), &p, &[], &sb.ledger());
    assert_eq!(out.status, Status::StillListed, "{}", out.line);
    assert!(out.line.contains("still lists"), "{}", out.line);
    let rec = &sb.ledger().all().unwrap()[0];
    assert_eq!(rec.outcome, "failed:exit 0, still listed");
    assert_eq!(rec.verb, Verb::ToolRemove);
    assert!(rec.recovery_location.is_none());
}

/// Tempting wrong patch: "map a non-zero exit to failed". A fake that
/// removes and then exits 1 is recorded as removed with an error.
#[test]
fn nonzero_exit_after_removing_is_recorded_as_removed_with_error() {
    let sb = Sandbox::new();
    sb.standard_mise();
    sb.go_uninstall_removes(1);
    let p = tool_removal::review_target(&sb.host(), &go(), &[]).unwrap();
    let out = tool_removal::execute(&sb.host(), &p, &[], &sb.ledger());
    assert_eq!(
        out.status,
        Status::RemovedWithError(Some(1)),
        "{}",
        out.line
    );
    assert!(!sb.install_dir("go", "1.23.5").exists());
    let rec = &sb.ledger().all().unwrap()[0];
    assert_eq!(rec.outcome, "completed_with_error:1");
    assert_eq!(
        rec.observed_path_state.as_deref(),
        Some("removed via mise, no longer listed")
    );
}

/// Tempting wrong patch: "record failed and retry". A hanging removal is
/// killed at the deadline, the list is re-read, and the record says
/// `unknown:timed_out` with what the re-read found. Nothing is retried.
#[test]
fn a_hanging_removal_is_killed_reread_and_recorded_unknown() {
    let sb = Sandbox::new();
    sb.standard_mise();
    sb.on_run("-C / uninstall go@1.23.5", "sleep 600\n");
    let host = sb.host().with_exec_timeout(Duration::from_millis(800));
    let p = tool_removal::review_target(&host, &go(), &[]).unwrap();
    let started = std::time::Instant::now();
    let out = tool_removal::execute(&host, &p, &[], &sb.ledger());
    assert!(started.elapsed() < Duration::from_secs(30));
    assert_eq!(out.status, Status::TimedOut, "{}", out.line);
    let calls = sb.calls();
    let exec_at = calls
        .iter()
        .position(|c| c[1..] == ["-C", "/", "uninstall", "go@1.23.5"])
        .unwrap();
    assert!(
        calls[exec_at + 1..]
            .iter()
            .any(|c| c.contains(&"ls".to_string())),
        "the list is re-read after the kill"
    );
    assert_eq!(sb.ran("-C / uninstall go@1.23.5"), 1, "never retried");
    let rec = &sb.ledger().all().unwrap()[0];
    assert_eq!(rec.outcome, "unknown:timed_out");
    assert!(
        rec.observed_path_state
            .as_deref()
            .unwrap()
            .contains("still listed"),
        "{:?}",
        rec.observed_path_state
    );
}

/// Tempting wrong patch: "show whatever the manager printed as the
/// preview". Garbage, an error line, a missing marker and a huge output
/// all refuse; what is shown is bounded and stripped of escapes.
#[test]
fn garbage_huge_and_markerless_dry_runs_refuse() {
    let dry = "-C / uninstall --dry-run go@1.23.5";
    let cases: Vec<(&str, String)> = vec![
        (
            "garbage",
            "\u{1b}[31mnot what mise prints\u{1b}[0m\n".to_string(),
        ),
        (
            "marker",
            Sandbox::uninstall_dry("go@1.23.5").replace(" (dry-run)", ""),
        ),
        ("error", "mise ERROR something went wrong\n".to_string()),
        ("huge", "mise go@1.23.5 remove /x\n".repeat(60_000)),
    ];
    for (what, stderr) in cases {
        let sb = Sandbox::new();
        sb.standard_mise();
        sb.answer(dry, "", &stderr, 0);
        let r = refused(tool_removal::review_target(&sb.host(), &go(), &[]));
        assert_refusal_plain(&r);
        assert!(r.output.len() <= tool_removal::SHOWN_OUTPUT_LINES, "{what}");
        assert!(
            r.output.iter().all(|l| !l.chars().any(|c| c.is_control())),
            "{what}: control characters reached the screen"
        );
        assert_eq!(sb.ran("-C / uninstall go@1.23.5"), 0, "{what}");
    }
}

/// Tempting wrong patch: "reuse `Running::start` as is" (the parent's
/// whole environment). A poisoned `RUSTUP_TOOLCHAIN`, `MISE_*`,
/// `HOMEBREW_*` and `PATH` never reach the manager; the documented
/// pass-through variables do.
#[test]
fn the_child_environment_is_built_from_nothing() {
    let sb = Sandbox::new();
    sb.standard_mise();
    let host = sb.host().with_parent_env(&[
        ("RUSTUP_TOOLCHAIN", "poisoned"),
        ("MISE_ENV", "poisoned"),
        ("MISE_DATA_DIR", "/poisoned"),
        ("HOMEBREW_PREFIX", "/poisoned"),
        ("PATH", "/poisoned/bin:/usr/bin"),
        (
            "DEVELOPER_DIR",
            "/Applications/Xcode.app/Contents/Developer",
        ),
        ("USER", "dev"),
    ]);
    tool_removal::review_target(&host, &go(), &[]).unwrap();
    let env = fs::read_to_string(sb.root.join("env.last")).unwrap();
    assert!(!env.contains("poisoned"), "{env}");
    for name in [
        "RUSTUP_TOOLCHAIN=",
        "MISE_ENV=",
        "MISE_DATA_DIR=",
        "HOMEBREW_PREFIX=",
    ] {
        assert!(
            !env.lines().any(|l| l.starts_with(name)),
            "{name} leaked: {env}"
        );
    }
    assert!(env.contains("DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer"));
    assert!(env.contains("NO_COLOR=1") && env.contains("PAGER=cat"));
    assert!(
        env.contains(&format!("HOME={}", sb.home().display())),
        "{env}"
    );
    let path = env.lines().find_map(|l| l.strip_prefix("PATH=")).unwrap();
    assert!(
        path.starts_with(&sb.root.join("bin").display().to_string()),
        "{path}"
    );
}

/// Tempting wrong patch: "just call `Command::new(\"mise\")` like the other
/// programs". A `mise` earlier on PATH is never run: resolution reads a
/// fixed list, never PATH.
#[test]
fn a_mise_earlier_on_path_is_not_used() {
    let sb = Sandbox::new();
    sb.standard_mise();
    let evil = sb.root.join("shim/mise");
    let marker = sb.root.join("shim-ran");
    fs::write(
        &evil,
        format!("#!/bin/sh\ntouch '{}'\nexit 0\n", marker.display()),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&evil, fs::Permissions::from_mode(0o755)).unwrap();
    let shim_path = format!("{}:/usr/bin:/bin", sb.root.join("shim").display());
    let host = sb.host().with_parent_env(&[("PATH", shim_path.as_str())]);
    tool_removal::review_target(&host, &go(), &[]).unwrap();
    assert!(!marker.exists(), "the PATH shim ran");
    // Production candidates are a fixed list: none of them comes from PATH.
    let prod = ToolResolver::system();
    for c in prod.candidates(Program::Mise) {
        assert!(
            ["/opt/homebrew/bin/mise", "/usr/local/bin/mise"].contains(&c.to_str().unwrap())
                || c.ends_with(".local/bin/mise")
                || c.ends_with(".cargo/bin/mise"),
            "{c:?}"
        );
    }
    assert_eq!(
        prod.candidates(Program::Xcrun),
        vec![PathBuf::from("/usr/bin/xcrun")]
    );
}

/// Tempting wrong patch: "trust the test author to point at fakes". In a
/// test build the production resolver can never start a manager: either
/// nothing is found in the fixed list, or the spawn panics before it
/// starts. No spawn is counted either way.
#[test]
fn no_real_manager_is_reachable_from_a_test() {
    assert!(std::env::var_os("SWAMP_TEST_TOOL_SANDBOX").is_none());
    for target in [go(), Target::SimRuntime { uuid: UUID.into() }] {
        let (r, counted) = swamp_core::work_counters::measured(|| {
            std::panic::catch_unwind(|| {
                tool_removal::review_target(&Host::system(), &target, &[]).map(|_| ())
            })
        });
        match r {
            Err(panic) => {
                let msg = panic.downcast_ref::<String>().cloned().unwrap_or_default();
                assert!(msg.contains("not inside a test sandbox"), "{msg}");
            }
            Ok(Err(refusal)) => assert!(refusal.reason.contains("not found"), "{}", refusal.reason),
            Ok(Ok(())) => panic!("a test reached a real manager"),
        }
        assert_eq!(counted.subprocess_spawns, 0);
    }
}

/// Tempting wrong patch: "reuse Verb::Delete". The record reads back as
/// its own verb, and an older binary's reader (which maps any label it
/// does not know to `Delete`) sees an ordinary removal.
#[test]
fn the_ledger_record_is_its_own_verb_and_old_readers_see_a_removal() {
    let sb = Sandbox::new();
    sb.standard_mise();
    sb.go_uninstall_removes(0);
    let p = tool_removal::review_target(&sb.host(), &go(), &[]).unwrap();
    let out = tool_removal::execute(&sb.host(), &p, &[], &sb.ledger());
    assert_eq!(out.recorded, Ok(()));
    let rec = &sb.ledger().all().unwrap()[0];
    assert_eq!(rec.verb, Verb::ToolRemove);
    assert_eq!(rec.outcome, "completed");
    let fact = |k: &str| {
        rec.evidence
            .iter()
            .find(|f| f.key == k)
            .map(|f| f.value.clone())
            .unwrap_or_default()
    };
    assert_eq!(fact("argv_exec"), "mise -C / uninstall go@1.23.5");
    assert_eq!(fact("permanent"), "true");
    assert_eq!(fact("exit_code"), "0");
    assert!(fact("env_policy").starts_with("tool-env-1"));
    assert_eq!(
        command_line("mise", &[OsString::from("-C")]),
        "mise -C",
        "one formatter"
    );
}

/// Tempting wrong patch: "add a `swamp remove` CLI for convenience". No
/// code in the CLI crate names tool removal; the gate audit's
/// `tool_removal::execute` group allows only the TUI's actions.
#[test]
fn no_cli_path_reaches_tool_removal() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../cli/src");
    let mut stack = vec![root];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let text = fs::read_to_string(&p).unwrap();
                assert!(
                    !text.contains("tool_removal") && !text.contains("tool_remove"),
                    "{} names tool removal",
                    p.display()
                );
            }
        }
    }
}
