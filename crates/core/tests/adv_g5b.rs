//! Independent verification of #177 (G5, PR #204 head 21603ae): attacks
//! round 1 did not try. Harness copied from adv_g5.rs; every fake is a sh
//! script in a temp dir. A test that FAILS on 21603ae is a finding.
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

// ======================= audit attacks =======================

fn review(sb: &Sandbox, t: &Target) -> Result<Preview, Refusal> {
    tool_removal::review_target(&sb.host(), t, &[])
}

// ======================= round 2 attacks =======================

fn uninstalls(sb: &Sandbox) -> usize {
    sb.ran("-C / uninstall go@1.23.5")
}

/// TOCTOU: the install dir is swapped for a symlink to somewhere the
/// human never reviewed between the review and `Y`. Tempting wrong
/// patch: "the symlink check ran at review". The re-review at `Y` must
/// see the link and refuse; the uninstall never runs.
#[test]
fn g5b_install_dir_swapped_for_a_symlink_after_review_refuses_at_y() {
    let sb = Sandbox::new();
    sb.standard_mise();
    sb.go_uninstall_removes(0);
    let host = sb.host();
    let p = review(&sb, &go()).expect("clean review");
    let dir = sb.install_dir("go", "1.23.5");
    fs::remove_dir(&dir).unwrap();
    let elsewhere = sb.root.join("precious");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("keep"), "x").unwrap();
    std::os::unix::fs::symlink(&elsewhere, &dir).unwrap();
    let out = tool_removal::execute(&host, &p, &[], &sb.ledger());
    assert!(matches!(out.status, Status::Refused(_)), "{}", out.line);
    assert_eq!(uninstalls(&sb), 0, "{:?}", sb.calls());
    assert!(elsewhere.join("keep").exists());
}

/// TOCTOU on a parent: `installs/go` itself becomes a symlink after
/// review. Tempting wrong patch: "lstat only the leaf".
#[test]
fn g5b_parent_dir_swapped_for_a_symlink_after_review_refuses_at_y() {
    let sb = Sandbox::new();
    sb.standard_mise();
    let host = sb.host();
    let p = review(&sb, &go()).expect("clean review");
    let parent = sb
        .install_dir("go", "1.23.5")
        .parent()
        .unwrap()
        .to_path_buf();
    let moved = sb.root.join("moved-go");
    fs::rename(&parent, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &parent).unwrap();
    let out = tool_removal::execute(&host, &p, &[], &sb.ledger());
    assert!(matches!(out.status, Status::Refused(_)), "{}", out.line);
    assert_eq!(uninstalls(&sb), 0);
}

/// A manager that exits 0, drops the version from its own list, but
/// leaves the files. Tempting wrong patch: "not listed = removed". The
/// outcome must not be `Removed` while the install dir is still there.
#[test]
fn g5b_exit_0_delisted_but_files_left_is_not_reported_removed() {
    let sb = Sandbox::new();
    sb.standard_mise();
    sb.go_uninstall_removes(0);
    // Same as go_uninstall_removes, minus the rm.
    let ls_key = Sandbox::key("-C / ls --json --installed");
    sb.on_run(
        "-C / uninstall go@1.23.5",
        &format!(
            "cp '{r}/state/ls_after.json' '{r}/state/{ls_key}.out'; exit 0\n",
            r = sb.root.display()
        ),
    );
    let host = sb.host();
    let p = review(&sb, &go()).unwrap();
    let out = tool_removal::execute(&host, &p, &[], &sb.ledger());
    assert!(sb.install_dir("go", "1.23.5").exists());
    assert_ne!(out.status, Status::Removed, "{}", out.line);
    assert!(!out.line.starts_with("Removed"), "{}", out.line);
    let recs = sb.ledger().all().unwrap();
    assert_eq!(
        recs.len(),
        1,
        "the started row is replaced, not left beside"
    );
    assert_ne!(recs[0].outcome, "completed", "{:?}", recs[0]);
}

/// A hanging manager that also started a grandchild in its process
/// group. Tempting wrong patch: "kill the child pid" (the grandchild
/// keeps running and keeps the pipe open). After the timeout both are
/// gone and execute returned promptly.
#[test]
fn g5b_hang_kills_the_whole_process_group() {
    let sb = Sandbox::new();
    sb.standard_mise();
    let pidf = sb.root.join("grandchild.pid");
    sb.on_run(
        "-C / uninstall go@1.23.5",
        &format!(
            "sh -c 'echo $$ > \"{}\"; exec sleep 600' &\nsleep 600\n",
            pidf.display()
        ),
    );
    let host = sb.host().with_exec_timeout(Duration::from_millis(800));
    let p = tool_removal::review_target(&host, &go(), &[]).unwrap();
    let t = std::time::Instant::now();
    let out = tool_removal::execute(&host, &p, &[], &sb.ledger());
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
    assert_eq!(out.status, Status::TimedOut, "{}", out.line);
    let pid: i32 = fs::read_to_string(&pidf).unwrap().trim().parse().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    // SAFETY: signal 0 only checks for existence.
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    if alive {
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
    assert!(!alive, "grandchild {pid} survived the timeout kill");
}

/// A removal that floods stdout (far past the 1 MiB cap) and exits 0.
/// Tempting wrong patch: "read to end" (unbounded memory) or "stop
/// reading at the cap" (the child blocks on a full pipe and becomes a
/// timeout). It must finish, be observed, and be recorded.
#[test]
fn g5b_a_removal_that_floods_output_finishes_and_is_recorded() {
    let sb = Sandbox::new();
    sb.standard_mise();
    sb.go_uninstall_removes(0);
    let k = Sandbox::key("-C / uninstall go@1.23.5");
    let old = fs::read_to_string(sb.root.join(format!("state/{k}.sh"))).unwrap();
    sb.state(
        &format!("{k}.sh"),
        &format!("yes 'mise go@1.23.5 remove something long enough' | head -c 6000000\n{old}"),
    );
    let host = sb.host().with_exec_timeout(Duration::from_secs(20));
    let p = tool_removal::review_target(&host, &go(), &[]).unwrap();
    let t = std::time::Instant::now();
    let out = tool_removal::execute(&host, &p, &[], &sb.ledger());
    assert!(t.elapsed() < Duration::from_secs(15), "{:?}", t.elapsed());
    assert_ne!(out.status, Status::TimedOut, "{}", out.line);
    assert!(out.recorded.is_ok(), "{:?}", out.recorded);
}

/// Unicode tag characters (U+E0000..U+E007F, invisible "ASCII smuggling")
/// and U+180E in a dry run. Tempting wrong patch: "strip the known bidi
/// list" (round 1 named U+202E/U+200B only). Nothing invisible reaches
/// the confirm.
#[test]
fn g5b_tag_characters_and_mongolian_separator_never_reach_the_confirm() {
    let sb = Sandbox::new();
    sb.standard_mise();
    let tags: String = "ignore"
        .chars()
        .map(|c| char::from_u32(0xE0000 + c as u32).unwrap())
        .collect();
    sb.answer(
        "-C / uninstall --dry-run go@1.23.5",
        "",
        &format!(
            "mise WARN  note{tags}\u{180E}\u{FFF9}x\n{}",
            Sandbox::uninstall_dry("go@1.23.5")
        ),
        0,
    );
    let shown: Vec<String> = match review(&sb, &go()) {
        Ok(p) => p.dry_output().to_vec(),
        Err(r) => r.output,
    };
    for l in &shown {
        for c in l.chars() {
            let cp = c as u32;
            assert!(
                !((0xE0000..=0xE007F).contains(&cp)
                    || cp == 0x180E
                    || (0xFFF9..=0xFFFB).contains(&cp)),
                "invisible U+{cp:04X} reached the confirm in {l:?}"
            );
        }
    }
}

/// The ledger exists but cannot be written (read-only store dir). The
/// guardrail: no record, no removal. Tempting wrong patch (the code at
/// 21603ae): "refuse only when the ledger file does not exist yet".
#[test]
fn g5b_an_existing_but_unwritable_ledger_means_nothing_runs() {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new();
    sb.standard_mise();
    sb.go_uninstall_removes(0);
    let host = sb.host();
    // Put one record in the ledger first (a refused run records too).
    sb.answer("-C / uninstall --dry-run go@1.23.5", "", "garbage\n", 0);
    let bad = review(&sb, &go());
    assert!(bad.is_err());
    sb.answer(
        "-C / uninstall --dry-run go@1.23.5",
        "",
        &Sandbox::uninstall_dry("go@1.23.5"),
        0,
    );
    let p = review(&sb, &go()).unwrap();
    let ledger = sb.ledger();
    let seed = tool_removal::execute(&host, &p, &[], &ledger);
    // The seed run really removed go; put it back for the real attempt.
    let _ = seed;
    let calls_before = uninstalls(&sb);
    fs::create_dir_all(sb.install_dir("go", "1.23.5")).unwrap();
    sb.standard_mise();
    sb.go_uninstall_removes(0);
    let p = review(&sb, &go()).unwrap();
    let store = sb.root.join("store");
    assert!(store.join("ledger.parquet").exists());
    fs::set_permissions(
        store.join("ledger.parquet"),
        fs::Permissions::from_mode(0o444),
    )
    .unwrap();
    fs::set_permissions(&store, fs::Permissions::from_mode(0o555)).unwrap();
    let out = tool_removal::execute(&host, &p, &[], &ledger);
    fs::set_permissions(&store, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        uninstalls(&sb),
        calls_before,
        "a removal ran with a ledger swamp could not write: {} / {:?}",
        out.line,
        out.recorded
    );
}

/// Two swamp instances confirm the same removal at once. Tempting wrong
/// patch: "the re-review makes it safe" (both re-reviews pass before
/// either removal finishes). The manager's removal runs at most once and
/// both outcomes are in the ledger.
#[test]
fn g5b_two_instances_racing_the_same_removal() {
    let sb = Sandbox::new();
    sb.standard_mise();
    sb.go_uninstall_removes(0);
    let k = Sandbox::key("-C / uninstall go@1.23.5");
    let old = fs::read_to_string(sb.root.join(format!("state/{k}.sh"))).unwrap();
    sb.state(&format!("{k}.sh"), &format!("sleep 1\n{old}"));
    let p = review(&sb, &go()).unwrap();
    let root = sb.root.clone();
    let hs: Vec<_> = (0..2)
        .map(|_| {
            let (p, root) = (p.clone(), root.clone());
            std::thread::spawn(move || {
                let host = Host::sandboxed(&root);
                let ledger = Ledger::open(root.join("store/ledger.parquet")).unwrap();
                tool_removal::execute(&host, &p, &[], &ledger)
            })
        })
        .collect();
    let outs: Vec<_> = hs.into_iter().map(|h| h.join().unwrap()).collect();
    let n = uninstalls(&sb);
    let recs = sb.ledger().all().unwrap_or_default();
    assert!(
        n <= 1,
        "both instances ran the removal ({n}): {:?}",
        outs.iter().map(|o| &o.line).collect::<Vec<_>>()
    );
    assert_eq!(recs.len(), 2, "ledger rows lost to the race: {recs:?}");
}

/// Sandbox escape: a test that points the sandbox at `/`, `$HOME`,
/// `/usr/bin`, the temp dir itself, or a temp-dir symlink to `/usr`
/// resolves nothing. Tempting wrong patch: "any directory the test names".
#[test]
fn g5b_sandbox_at_system_or_home_dirs_resolves_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let link = tmp.path().join("usr-link");
    std::os::unix::fs::symlink("/usr", &link).unwrap();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    for dir in [
        PathBuf::from("/"),
        home,
        PathBuf::from("/usr/bin"),
        PathBuf::from("/usr"),
        std::env::temp_dir(),
        link,
    ] {
        for program in [Program::Xcrun, Program::Mise] {
            let r = ToolResolver::sandboxed(&dir).resolve(program);
            assert!(
                r.is_err(),
                "sandbox {} resolved {:?}",
                dir.display(),
                r.map(|b| b.path().to_path_buf())
            );
        }
    }
}

/// A sandbox inside the temp dir whose fake is a symlink to a real
/// system binary (`/usr/bin/true`, harmless if it ran). Tempting wrong
/// patch: "the path is under the sandbox, so it is a fake". Either the
/// resolver refuses or the spawn guard panics; it never yields a preview.
#[test]
fn g5b_a_fake_that_links_to_a_system_binary_never_runs() {
    let sb = Sandbox::new();
    let x = sb.root.join("bin/xcrun");
    fs::remove_file(&x).unwrap();
    std::os::unix::fs::symlink("/usr/bin/true", &x).unwrap();
    let root = sb.root.clone();
    let r = std::panic::catch_unwind(move || {
        tool_removal::review_target(
            &Host::sandboxed(&root),
            &Target::SimRuntime { uuid: UUID.into() },
            &[],
        )
        .is_ok()
    });
    assert!(
        !matches!(r, Ok(true)),
        "a symlink to /usr/bin/true was used as a fake"
    );
}

/// DEVELOPER_DIR with a real-looking layout (usr/bin/simctl present) but
/// world-writable. Round 1's test used a dir with no simctl, so it
/// passes even if the permission check is deleted. Tempting wrong patch:
/// "a DEVELOPER_DIR that holds simctl is Xcode".
#[test]
fn g5b_world_writable_developer_dir_with_simctl_is_dropped() {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new();
    let d = sb.root.join("Xcode-evil/Contents/Developer");
    fs::create_dir_all(d.join("usr/bin")).unwrap();
    let simctl = d.join("usr/bin/simctl");
    fs::write(&simctl, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&simctl, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&d, fs::Permissions::from_mode(0o777)).unwrap();
    let bin = ToolResolver::sandboxed(&sb.root)
        .with_parent_env(vec![("DEVELOPER_DIR".into(), d.display().to_string())])
        .resolve(Program::Xcrun)
        .unwrap();
    fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        bin.env_value("DEVELOPER_DIR").is_none(),
        "world-writable DEVELOPER_DIR passed to xcrun"
    );
}

/// Same, but the dir is fine and its `simctl` is world-writable.
#[test]
fn g5b_developer_dir_with_a_writable_simctl_is_dropped() {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new();
    let d = sb.root.join("Xcode-ok/Contents/Developer");
    fs::create_dir_all(d.join("usr/bin")).unwrap();
    let simctl = d.join("usr/bin/simctl");
    fs::write(&simctl, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&simctl, fs::Permissions::from_mode(0o777)).unwrap();
    let bin = ToolResolver::sandboxed(&sb.root)
        .with_parent_env(vec![("DEVELOPER_DIR".into(), d.display().to_string())])
        .resolve(Program::Xcrun)
        .unwrap();
    assert!(
        bin.env_value("DEVELOPER_DIR").is_none(),
        "DEVELOPER_DIR with a world-writable simctl passed to xcrun"
    );
    // Control: the same dir with a sane simctl is passed.
    fs::set_permissions(&simctl, fs::Permissions::from_mode(0o755)).unwrap();
    let ok = ToolResolver::sandboxed(&sb.root)
        .with_parent_env(vec![("DEVELOPER_DIR".into(), d.display().to_string())])
        .resolve(Program::Xcrun)
        .unwrap();
    assert_eq!(ok.env_value("DEVELOPER_DIR"), Some(d.to_str().unwrap()));
}
