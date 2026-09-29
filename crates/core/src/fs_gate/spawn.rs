//! The one way swamp starts a subprocess.
//!
//! * **The program is a [`Program`] variant.** There is no `Command`
//!   outside this module (the `std::process` path is rejected everywhere
//!   else by the gate audit and by clippy), so "which binaries can swamp
//!   run" is this enum, and a second scheduler (`crontab`) or an
//!   uncounted probe does not compile.
//! * **Every run is counted** by `work_counters` before the process
//!   starts, so "this observation ran no subprocess" is measured by
//!   swamp's own instrumentation (re-review 3, F2).
//! * **Every run is bounded** by a timeout and returns only a
//!   [`RunOutput`]; no `Child` or `Command` escapes.
//! * **Arguments are allow-listed per program.** [`run`] accepts only
//!   the exact argument shapes swamp's own queries use (verbs, flags and
//!   typed operands: an absolute path, a Docker reference, a pid), so an
//!   option smuggled in as an operand, a different verb or an extra flag
//!   is refused before anything starts. `docker … rm` and `git worktree
//!   prune` are not shapes at all: they are reachable only through
//!   [`super::destroy`], which takes a [`crate::recheck::RecheckProof`]
//!   and an [`crate::authority::Authorized`].

// killpg/signal/atexit for the child-lifetime guard (#156).
#![allow(unsafe_code)]

use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Seek, SeekFrom};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Every program swamp may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Program {
    /// Occupancy probe (`lsof -- <path>`, `lsof +D <dir>`, and the one
    /// machine-wide `lsof -n -P -F n` snapshot a review pass shares).
    Lsof,
    /// Xcode `Info.plist` → XML (`plutil -convert xml1 -o - <plist>`),
    /// read-only, output to stdout.
    Plutil,
    /// `xcrun simctl list devices -j`.
    Xcrun,
    /// `du -skPx <root>` (the `--verify-du` cross-check).
    Du,
    /// Docker daemon queries (`docker system df`, `inspect`, `ps`).
    Docker,
    /// GitHub enrichment (`gh api …`, `gh pr list …`).
    Gh,
    /// Read-only git queries. Mutating git runs through
    /// [`super::destroy::git_worktree_prune`].
    Git,
    /// Free-space measurement (`df -k <path>`).
    Df,
    /// The user's uid for the launchd domain (`id -u`).
    Id,
    /// The scheduled refresh's LaunchAgent (`launchctl bootstrap|bootout|…`).
    Launchctl,
    /// Liveness of the observation lock holder (`kill -0 <pid>`).
    Kill,
    /// `brew --prefix` (detector query).
    Brew,
    /// `defaults read com.apple.dt.Xcode …` (detector query).
    Defaults,
    /// `systemd --user`'s own manager (#83): `crate::systemd_user`'s
    /// timer/service/collector lifecycle. `--user` is part of every
    /// shape below, never a separate prefix a caller could vary.
    Systemctl,
    /// `loginctl show-user <uid> --property=Linger --value`
    /// (`crate::systemd_user::linger`); enabling lingering is never
    /// done by swamp.
    Loginctl,
}

impl Program {
    /// Every variant: the PATH-shim spawn oracle in the cost tests shims
    /// exactly these, so "spawns: []" means all of them.
    pub const ALL: &'static [Program] = &[
        Program::Lsof,
        Program::Plutil,
        Program::Xcrun,
        Program::Du,
        Program::Docker,
        Program::Gh,
        Program::Git,
        Program::Df,
        Program::Id,
        Program::Launchctl,
        Program::Kill,
        Program::Brew,
        Program::Defaults,
        Program::Systemctl,
        Program::Loginctl,
    ];

    /// The executable name looked up on `PATH`.
    pub fn binary(self) -> &'static str {
        match self {
            Program::Lsof => "lsof",
            Program::Plutil => "plutil",
            Program::Xcrun => "xcrun",
            Program::Du => "du",
            Program::Docker => "docker",
            Program::Gh => "gh",
            Program::Git => "git",
            Program::Df => "df",
            Program::Id => "id",
            Program::Launchctl => "launchctl",
            Program::Kill => "kill",
            Program::Brew => "brew",
            Program::Defaults => "defaults",
            Program::Systemctl => "systemctl",
            Program::Loginctl => "loginctl",
        }
    }

    /// The program named `name`, if swamp may run it.
    pub fn named(name: &str) -> Option<Program> {
        Program::ALL.iter().copied().find(|p| p.binary() == name)
    }
}

/// What a finished (or timed-out) run produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    /// The exit code; `None` when killed by a signal or by the timeout.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

impl RunOutput {
    pub fn success(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }

    pub fn stdout_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// One argument slot of an allowed invocation.
#[derive(Debug, Clone, Copy)]
enum Slot {
    /// Exactly this word.
    Lit(&'static str),
    /// An absolute path (`/…`): never an option, never relative.
    AbsPath,
    /// A Docker object id or name: non-empty, no leading `-`, only
    /// `[A-Za-z0-9_.:@/-]` (image references carry `:` `@` `/`).
    DockerRef,
    /// One or more [`Slot::DockerRef`]s (a batched `inspect`).
    DockerRefs,
    /// A decimal number (a pid).
    Number,
    /// `gui/<uid>` (a launchd domain).
    LaunchdDomain,
    /// `gui/<uid>/<label>` for swamp's own label.
    LaunchdService,
    /// Swamp's own LaunchAgent plist, exactly
    /// ([`super::store::launch_agent_plist`]).
    SwampPlist,
    /// The `-f key=value` pairs of a read-only GraphQL query: `query=`
    /// must open with `query(` (never `mutation`), every other key is one
    /// of the variables `github.rs` declares.
    GraphqlFields,
    /// Exactly one of swamp's own systemd unit names
    /// (`crate::systemd_user::{SERVICE,TIMER,COLLECTOR}`).
    SwampUnit,
    /// One or two of [`Slot::SwampUnit`] (a batched `disable --now`).
    SwampUnits,
    /// `--property=<comma-separated properties>`, every property one of
    /// the fixed set `systemd_user::show` reads.
    ShowProperties,
    /// This process's own uid, decimal (`systemd_user::linger`).
    OwnUid,
}

/// Every argument shape [`run`] accepts, per program. An allow-list: an
/// invocation that is not one of these shapes -- an extra flag, a
/// different verb, an option smuggled in where an operand belongs
/// (`git -c core.pager=…`, `docker --host …`) -- is refused before
/// anything starts. Programs with no shape here (`git`) run only through
/// [`super::destroy`], behind a recheck proof.
fn shapes(program: Program) -> &'static [&'static [Slot]] {
    use Slot::*;
    match program {
        Program::Lsof => &[
            &[Lit("--"), AbsPath],
            &[Lit("+D"), AbsPath],
            // One machine-wide open-file listing, names only: no tree walk,
            // and no DNS or port-name resolution (`-n -P`; without them the
            // same listing took 16 s instead of 0.2 s).
            &[Lit("-n"), Lit("-P"), Lit("-F"), Lit("n")],
        ],
        Program::Plutil => &[&[Lit("-convert"), Lit("xml1"), Lit("-o"), Lit("-"), AbsPath]],
        Program::Xcrun => &[&[Lit("simctl"), Lit("list"), Lit("devices"), Lit("-j")]],
        Program::Du => &[&[Lit("-skPx"), AbsPath]],
        Program::Docker => &[
            &[
                Lit("system"),
                Lit("df"),
                Lit("-v"),
                Lit("--format"),
                Lit("json"),
            ],
            &[Lit("ps"), Lit("-a"), Lit("--format"), Lit("json")],
            &[
                Lit("image"),
                Lit("inspect"),
                DockerRefs,
                Lit("--format"),
                Lit("json"),
            ],
            &[
                Lit("volume"),
                Lit("inspect"),
                DockerRefs,
                Lit("--format"),
                Lit("json"),
            ],
            &[Lit("inspect"), DockerRefs, Lit("--format"), Lit("json")],
            &[Lit("image"), Lit("inspect"), DockerRef],
            &[Lit("volume"), Lit("inspect"), DockerRef],
            &[Lit("version"), Lit("--format"), Lit("json")],
            &[Lit("buildx"), Lit("ls"), Lit("--format"), Lit("json")],
            &[
                Lit("buildx"),
                Lit("du"),
                Lit("--verbose"),
                Lit("--builder"),
                DockerRef,
            ],
        ],
        Program::Gh => &[
            &[Lit("auth"), Lit("status")],
            &[Lit("api"), Lit("graphql"), GraphqlFields],
        ],
        Program::Git => &[],
        Program::Df => &[&[Lit("-k"), AbsPath]],
        Program::Id => &[&[Lit("-u")]],
        Program::Launchctl => &[
            &[Lit("bootstrap"), LaunchdDomain, SwampPlist],
            &[Lit("load"), Lit("-w"), SwampPlist],
            &[Lit("bootout"), LaunchdService],
            &[Lit("unload"), Lit("-w"), SwampPlist],
        ],
        Program::Kill => &[&[Lit("-0"), Number]],
        Program::Brew => &[&[Lit("--prefix")]],
        Program::Defaults => &[&[
            Lit("read"),
            Lit("com.apple.dt.Xcode"),
            Lit("IDECustomDerivedDataLocation"),
        ]],
        Program::Systemctl => &[
            &[Lit("--user"), Lit("--no-pager"), Lit("show-environment")],
            &[Lit("--user"), Lit("--no-pager"), Lit("daemon-reload")],
            &[
                Lit("--user"),
                Lit("--no-pager"),
                Lit("enable"),
                Lit("--now"),
                SwampUnit,
            ],
            &[Lit("--user"), Lit("--no-pager"), Lit("restart"), SwampUnit],
            &[
                Lit("--user"),
                Lit("--no-pager"),
                Lit("disable"),
                Lit("--now"),
                SwampUnits,
            ],
            &[
                Lit("--user"),
                Lit("--no-pager"),
                Lit("stop"),
                Lit(crate::systemd_user::SERVICE),
            ],
            &[
                Lit("--user"),
                Lit("--no-pager"),
                Lit("show"),
                SwampUnit,
                ShowProperties,
            ],
        ],
        Program::Loginctl => &[&[
            Lit("--no-pager"),
            Lit("show-user"),
            OwnUid,
            Lit("--property=Linger"),
            Lit("--value"),
        ]],
    }
}

/// Whether `a` can be a Docker object reference (see [`Slot::DockerRef`]).
pub(super) fn is_docker_ref(a: &str) -> bool {
    docker_ref(a)
}

fn docker_ref(a: &str) -> bool {
    !a.is_empty()
        && !a.starts_with('-')
        && a.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '@' | '/' | '-'))
}

fn digits(a: &str) -> bool {
    !a.is_empty() && a.chars().all(|c| c.is_ascii_digit())
}

/// Exactly one of swamp's own systemd unit names.
fn swamp_unit(a: &str) -> bool {
    [
        crate::systemd_user::SERVICE,
        crate::systemd_user::TIMER,
        crate::systemd_user::COLLECTOR,
    ]
    .contains(&a)
}

/// `--property=<comma-separated properties>`, every name one
/// `systemd_user::show` actually reads.
fn show_properties(a: &str) -> bool {
    const ALLOWED: &[&str] = &[
        "ActiveState",
        "UnitFileState",
        "LastTriggerUSec",
        "NextElapseUSecRealtime",
        "SubState",
        "MainPID",
        "NRestarts",
    ];
    let Some(list) = a.strip_prefix("--property=") else {
        return false;
    };
    !list.is_empty() && list.split(',').all(|p| ALLOWED.contains(&p))
}

/// The GraphQL variables `github.rs` binds: `owner`, `name`, and
/// `branch<i>`/`ref<i>` per branch.
fn graphql_variable(key: &str) -> bool {
    matches!(key, "owner" | "name")
        || ["branch", "ref"]
            .iter()
            .any(|p| key.strip_prefix(p).is_some_and(digits))
}

/// Whether `args` match `shape` exactly.
fn matches_shape(shape: &[Slot], args: &[String]) -> bool {
    let mut i = 0;
    for slot in shape {
        match slot {
            Slot::DockerRefs => {
                let start = i;
                while i < args.len() && docker_ref(&args[i]) {
                    i += 1;
                }
                if i == start {
                    return false;
                }
                continue;
            }
            Slot::SwampUnits => {
                let start = i;
                while i < args.len() && swamp_unit(&args[i]) {
                    i += 1;
                }
                if i == start || i - start > 2 {
                    return false;
                }
                continue;
            }
            Slot::GraphqlFields => {
                let mut saw_query = false;
                while i < args.len() {
                    if args[i] != "-f" {
                        return false;
                    }
                    let Some((key, value)) = args.get(i + 1).and_then(|kv| kv.split_once('='))
                    else {
                        return false;
                    };
                    if key == "query" {
                        if saw_query || !value.trim_start().starts_with("query(") {
                            return false;
                        }
                        saw_query = true;
                    } else if !graphql_variable(key) {
                        return false;
                    }
                    i += 2;
                }
                if !saw_query {
                    return false;
                }
                continue;
            }
            _ => {}
        }
        let Some(a) = args.get(i) else {
            return false;
        };
        let ok = match slot {
            Slot::Lit(w) => a == w,
            Slot::AbsPath => a.starts_with('/'),
            Slot::DockerRef => docker_ref(a),
            Slot::Number => digits(a),
            Slot::LaunchdDomain => a.strip_prefix("gui/").is_some_and(digits),
            Slot::LaunchdService => a
                .strip_prefix("gui/")
                .and_then(|r| r.split_once('/'))
                .is_some_and(|(uid, label)| digits(uid) && label == crate::schedule::LABEL),
            Slot::SwampPlist => super::store::launch_agent_plist()
                .is_ok_and(|p| p.as_os_str() == std::ffi::OsStr::new(a)),
            Slot::SwampUnit => swamp_unit(a),
            Slot::ShowProperties => show_properties(a),
            Slot::OwnUid => digits(a) && a.parse::<u32>().ok() == Some(super::sys::current_uid()),
            Slot::DockerRefs | Slot::GraphqlFields | Slot::SwampUnits => {
                unreachable!("handled above")
            }
        };
        if !ok {
            return false;
        }
        i += 1;
    }
    i == args.len()
}

/// `Ok` when `program args…` is one of the shapes [`shapes`] allows.
fn permitted(program: Program, args: &[OsString]) -> Result<(), String> {
    let words: Option<Vec<String>> = args
        .iter()
        .map(|a| a.to_str().map(str::to_string))
        .collect();
    let refuse = |words: &str| {
        format!(
            "{} {words} is not an invocation swamp runs: every program has an allow-list of \
             argument shapes (fs_gate::spawn), and mutating ones run only through \
             fs_gate::destroy",
            program.binary()
        )
    };
    let Some(words) = words else {
        return Err(refuse("<non-UTF-8 argument>"));
    };
    if shapes(program).iter().any(|s| matches_shape(s, &words)) {
        Ok(())
    } else {
        Err(refuse(&words.join(" ")))
    }
}

/// Runs `program args…` with stdin closed, stdout and stderr captured to
/// anonymous temp files (so a chatty program can never deadlock a full
/// pipe), and kills it after `timeout`. Counted as one spawn.
pub fn run<I, S>(program: Program, args: I, timeout: Duration) -> io::Result<RunOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
    if let Err(why) = permitted(program, &args) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, why));
    }
    run_unchecked(program, &args, timeout)
}

/// [`run`] without the argument allow-list: only [`super::destroy`],
/// behind a recheck proof and an authorization, calls this, with
/// arguments it builds itself from the proof.
pub(super) fn run_unchecked(
    program: Program,
    args: &[OsString],
    timeout: Duration,
) -> io::Result<RunOutput> {
    run_command(program.binary(), args, timeout)
}

// ---- child lifetime (#156) ------------------------------------------
//
// Every child is its own process group leader, is registered in a
// fixed-size table of live group ids, and is owned by a [`Running`]
// guard, so it is killed (whole group, SIGKILL) and reaped on: the
// deadline, an error, a panic unwinding through `run`, `Drop`,
// SIGINT/SIGTERM/SIGHUP to swamp (signal handler walks the table), and
// normal process exit (`atexit`, which also covers `process::exit`).
// A child cannot outlive swamp except when swamp itself is SIGKILLed.

const SLOTS: usize = 256;
static LIVE: [std::sync::atomic::AtomicI32; SLOTS] =
    [const { std::sync::atomic::AtomicI32::new(0) }; SLOTS];

/// SIGKILLs the process group of every registered child `keep` selects.
/// Async-signal-safe (atomics and `killpg` only): the signal handler
/// calls it.
fn kill_registered(keep: impl Fn(i32) -> bool) {
    for slot in &LIVE {
        let pgid = slot.load(std::sync::atomic::Ordering::SeqCst);
        if pgid > 0 && keep(pgid) {
            // SAFETY: killpg on a group id this process created.
            unsafe { libc::killpg(pgid, libc::SIGKILL) };
        }
    }
}

/// Kills every child swamp currently has running (whole process
/// groups). The TUI calls it when the user cancels or quits; the run
/// that owned each child sees it die and returns.
pub fn kill_all_children() {
    kill_registered(|_| true);
}

extern "C" fn on_exit() {
    kill_registered(|_| true);
}

extern "C" fn on_fatal_signal(sig: libc::c_int) {
    kill_registered(|_| true);
    // SAFETY: restore the default action and re-deliver, so swamp dies
    // of the same signal it would have without the handler.
    unsafe {
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
    }
}

fn install_cleanup_once() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: registering plain `extern "C"` functions.
        unsafe {
            libc::atexit(on_exit);
            for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
                libc::signal(sig, on_fatal_signal as extern "C" fn(libc::c_int) as usize);
            }
        }
    });
}

/// A started child, killed (group) and reaped when dropped unless it
/// was already reaped.
struct Running {
    child: std::process::Child,
    slot: Option<usize>,
    reaped: bool,
}

impl Running {
    fn start(
        binary: &str,
        args: &[OsString],
        out: std::fs::File,
        err: std::fs::File,
    ) -> io::Result<Self> {
        use std::os::unix::process::CommandExt;
        install_cleanup_once();
        crate::work_counters::record_spawn();
        let child = Command::new(binary)
            .args(args)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            // Its own group: the deadline, cancel and signal paths kill
            // the child *and* whatever it forked.
            .process_group(0)
            // A pager or a credential prompt can never be why a child
            // blocks (stdin is null too).
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat")
            .env("GH_PAGER", "cat")
            .env("SYSTEMD_PAGER", "cat")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GH_PROMPT_DISABLED", "1")
            .env("GCM_INTERACTIVE", "never")
            .spawn()?;
        let pgid = child.id() as i32;
        let slot = LIVE.iter().position(|s| {
            s.compare_exchange(
                0,
                pgid,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_ok()
        });
        Ok(Running {
            child,
            slot,
            reaped: false,
        })
    }

    fn unregister(&mut self) {
        if let Some(i) = self.slot.take() {
            LIVE[i].store(0, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        let st = self.child.try_wait()?;
        if st.is_some() {
            self.reaped = true;
            self.unregister();
        }
        Ok(st)
    }

    /// Kills the whole group and reaps the leader.
    fn kill_and_reap(&mut self) {
        if !self.reaped {
            // SAFETY: the leader is unreaped, so the group id is ours.
            unsafe { libc::killpg(self.child.id() as i32, libc::SIGKILL) };
            let _ = self.child.kill();
            let _ = self.child.wait();
            self.reaped = true;
        }
        self.unregister();
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.kill_and_reap();
    }
}

/// Runs `binary args…` under the lifetime rules above.
fn run_command(binary: &str, args: &[OsString], timeout: Duration) -> io::Result<RunOutput> {
    let mut out_file = tempfile::tempfile()?;
    let mut err_file = tempfile::tempfile()?;
    let mut child = Running::start(binary, args, out_file.try_clone()?, err_file.try_clone()?)?;
    let started = Instant::now();
    let (code, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (status.code(), false),
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                child.kill_and_reap();
                break (None, true);
            }
            Err(e) => return Err(e), // `Drop` kills and reaps
        }
    };
    let read_back = |f: &mut std::fs::File| -> Vec<u8> {
        let mut buf = Vec::new();
        let _ = f.seek(SeekFrom::Start(0));
        let _ = f.read_to_end(&mut buf);
        buf
    };
    let stdout = read_back(&mut out_file);
    let stderr = read_back(&mut err_file);
    Ok(RunOutput {
        code,
        stdout,
        stderr,
        timed_out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alive(pid: i32) -> bool {
        // SAFETY: signal 0 only probes.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    /// `sh` that backgrounds a `sleep 600` grandchild, prints its pid,
    /// then hangs itself.
    fn hung_tree() -> Vec<OsString> {
        vec!["-c".into(), "sleep 600 & echo $!; sleep 600".into()]
    }

    fn wait_gone(pid: i32) -> bool {
        for _ in 0..200 {
            if !alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn a_hung_child_and_its_grandchild_are_gone_after_the_deadline() {
        let t = Instant::now();
        let out = run_command("sh", &hung_tree(), Duration::from_millis(600)).unwrap();
        assert!(out.timed_out && t.elapsed() < Duration::from_secs(10));
        let grand: i32 = out.stdout_lossy().trim().parse().unwrap();
        assert!(wait_gone(grand), "grandchild {grand} survived the deadline");
    }

    #[test]
    fn dropping_the_guard_kills_the_group() {
        let out = tempfile::tempfile().unwrap();
        let err = tempfile::tempfile().unwrap();
        let mut r = Running::start("sh", &hung_tree(), out.try_clone().unwrap(), err).unwrap();
        let leader = r.child.id() as i32;
        std::thread::sleep(Duration::from_millis(300));
        let mut buf = String::new();
        let mut f = out;
        f.seek(SeekFrom::Start(0)).unwrap();
        f.read_to_string(&mut buf).unwrap();
        let grand: i32 = buf.trim().parse().unwrap();
        assert!(alive(leader) && alive(grand));
        // A panic unwinding through `run` drops the guard the same way.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _keep = &mut r;
            panic!("worker panic");
        }));
        assert!(wait_gone(leader), "leader survived the drop");
        assert!(wait_gone(grand), "grandchild survived the drop");
    }

    #[test]
    fn cancel_and_signal_paths_kill_the_registered_group() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("grand.pid");
        let script = format!("sleep 600 & echo $! > {}; sleep 600", pidfile.display());
        let (tx, rx) = std::sync::mpsc::channel();
        let h = std::thread::spawn(move || {
            let r = run_command(
                "sh",
                &["-c".into(), script.into()],
                Duration::from_secs(600),
            );
            tx.send(r.unwrap()).unwrap();
        });
        // The grandchild shares the leader's group; its pid names this
        // test's group (other tests run in parallel, so only it is hit).
        let mut grand = 0;
        for _ in 0..300 {
            std::thread::sleep(Duration::from_millis(10));
            if let Ok(t) = std::fs::read_to_string(&pidfile)
                && let Ok(p) = t.trim().parse::<i32>()
            {
                grand = p;
                break;
            }
        }
        assert!(grand > 0, "grandchild never started");
        // SAFETY: getpgid only reads.
        let pgid = unsafe { libc::getpgid(grand) };
        assert!(
            LIVE.iter()
                .any(|s| s.load(std::sync::atomic::Ordering::SeqCst) == pgid)
        );
        // What the TUI cancel, the exit hook and the signal handler run.
        kill_registered(|p| p == pgid);
        let out = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("run returns");
        assert!(!out.timed_out && out.code.is_none());
        assert!(wait_gone(grand) && wait_gone(pgid));
        h.join().unwrap();
    }

    #[test]
    fn stdin_is_null_and_pagers_and_prompts_are_disabled() {
        let script = "cat; printf '%s|%s|%s' \"$GIT_PAGER\" \"$GIT_TERMINAL_PROMPT\" \"$GH_PROMPT_DISABLED\"";
        let out =
            run_command("sh", &["-c".into(), script.into()], Duration::from_secs(10)).unwrap();
        assert!(!out.timed_out);
        assert_eq!(out.stdout_lossy(), "cat|0|1");
    }

    #[test]
    fn a_run_counts_one_spawn() {
        let (_, counted) =
            crate::work_counters::measured(|| run(Program::Id, ["-u"], Duration::from_secs(5)));
        assert_eq!(counted.subprocess_spawns, 1);
    }

    #[test]
    fn mutating_verbs_are_refused_without_spawning() {
        for (program, args) in [
            (Program::Docker, vec!["image", "rm", "x"]),
            (Program::Docker, vec!["rmi", "x"]),
            (Program::Git, vec!["-C", "/tmp/x", "worktree", "prune"]),
            (Program::Git, vec!["-C", "/nonexistent", "worktree", "list"]),
            // Re-review 5: an option parser that skipped `-C <dir>` read
            // `-c k=v` as a flag and the next word as the subcommand.
            (
                Program::Git,
                vec!["-c", "core.pager=sh", "worktree", "list"],
            ),
            // Programs the old deny-list never looked at.
            (Program::Kill, vec!["-9", "1"]),
            (Program::Kill, vec!["1"]),
            (Program::Launchctl, vec!["remove", "com.apple.something"]),
            (Program::Brew, vec!["uninstall", "x"]),
            (
                Program::Defaults,
                vec!["write", "com.apple.dt.Xcode", "x", "y"],
            ),
            (Program::Plutil, vec!["-convert", "xml1", "/tmp/x.plist"]),
            (Program::Xcrun, vec!["simctl", "erase", "all"]),
            // Operands that are options, and relative paths.
            (Program::Lsof, vec!["+D", "-t"]),
            (Program::Du, vec!["-skPx", "relative"]),
            (
                Program::Docker,
                vec!["--host", "tcp://x", "ps", "-a", "--format", "json"],
            ),
            (Program::Docker, vec!["image", "inspect", "--help"]),
            (Program::Docker, vec!["builder", "prune", "-f"]),
            (Program::Docker, vec!["buildx", "prune", "--filter", "id=x"]),
            (
                Program::Docker,
                vec!["buildx", "du", "--verbose", "--builder", "-ci"],
            ),
            // A GraphQL mutation through the read-only query shape.
            (
                Program::Gh,
                vec!["api", "graphql", "-f", "query=mutation { x }"],
            ),
            (Program::Gh, vec!["api", "repos/x/y", "-X", "DELETE"]),
        ] {
            let (r, counted) = crate::work_counters::measured(|| {
                run(program, args.clone(), Duration::from_secs(5))
            });
            assert!(r.is_err(), "{program:?} {args:?} must be refused");
            assert_eq!(counted.subprocess_spawns, 0, "{program:?} {args:?} spawned");
        }
    }

    #[test]
    fn swamps_own_invocations_are_shapes() {
        for (program, args) in [
            (Program::Lsof, vec!["+D", "/tmp"]),
            (Program::Lsof, vec!["--", "/tmp/x"]),
            (Program::Lsof, vec!["-n", "-P", "-F", "n"]),
            (
                Program::Docker,
                vec![
                    "image",
                    "inspect",
                    "sha256:ab",
                    "repo/x:1",
                    "--format",
                    "json",
                ],
            ),
            (Program::Docker, vec!["volume", "inspect", "v1"]),
            (
                Program::Docker,
                vec!["system", "df", "-v", "--format", "json"],
            ),
            (Program::Docker, vec!["version", "--format", "json"]),
            (Program::Docker, vec!["buildx", "ls", "--format", "json"]),
            (
                Program::Docker,
                vec!["buildx", "du", "--verbose", "--builder", "ci"],
            ),
            (Program::Gh, vec!["auth", "status"]),
            (
                Program::Gh,
                vec![
                    "api",
                    "graphql",
                    "-f",
                    "query=query($owner: String!) { x }",
                    "-f",
                    "owner=o",
                    "-f",
                    "branch0=main",
                    "-f",
                    "ref0=refs/heads/main",
                ],
            ),
            (Program::Kill, vec!["-0", "123"]),
            (Program::Id, vec!["-u"]),
            (Program::Df, vec!["-k", "/"]),
        ] {
            let words: Vec<OsString> = args.iter().map(OsString::from).collect();
            assert!(permitted(program, &words).is_ok(), "{program:?} {args:?}");
        }
    }

    #[test]
    fn every_program_has_a_distinct_binary() {
        let mut names: Vec<&str> = Program::ALL.iter().map(|p| p.binary()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), Program::ALL.len());
        for p in Program::ALL {
            assert_eq!(Program::named(p.binary()), Some(*p));
        }
    }
}
