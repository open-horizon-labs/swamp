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
//!   [`super::destroy`], which builds their arguments itself.
//! * **A manager that removes things runs as a [`ToolBin`]** (#177): an
//!   absolute binary from a fixed list of directories with an environment
//!   built from nothing, through [`run_tool_read`] (listings and dry runs)
//!   or `destroy::tool_remove` (the removal), never by name on `PATH`.

// killpg/signal/atexit for the child-lifetime guard (#156).
#![allow(unsafe_code)]

use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
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
    /// mise, for tool-managed removal only (#177). It has no shape in
    /// [`shapes`], so [`run`] (a `PATH` lookup with the inherited
    /// environment) refuses it; it runs only as a [`ToolBin`] through
    /// [`run_tool_read`] and `fs_gate::destroy::tool_remove`.
    Mise,
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
        Program::Mise,
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
            Program::Mise => "mise",
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
    /// A tool-removal run ([`run_tool_read`], `destroy::tool_remove`)
    /// printed more than [`TOOL_OUTPUT_LIMIT`] on a stream; what is kept
    /// is the first part. Always false for [`run`].
    pub truncated: bool,
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
/// [`super::destroy`].
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
            // The same listing with each process's command name, for
            // tool-managed removal's open-file check (#177).
            &[Lit("-n"), Lit("-P"), Lit("-F"), Lit("cn")],
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
        // Tool-managed removal's manager: only ever a resolved
        // [`ToolBin`], never a `PATH` name (see [`tool_read_shapes`]).
        Program::Mise => &[],
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

/// [`run`] without the argument allow-list: only [`super::destroy`]
/// calls this, with arguments it builds itself.
pub(super) fn run_unchecked(
    program: Program,
    args: &[OsString],
    timeout: Duration,
) -> io::Result<RunOutput> {
    run_command(
        OsStr::new(program.binary()),
        args,
        timeout,
        &Launch::Inherit,
    )
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
    super::terminal::restore_signal_safe();
    kill_registered(|_| true);
}

extern "C" fn on_fatal_signal(sig: libc::c_int) {
    super::terminal::restore_signal_safe();
    kill_registered(|_| true);
    // SAFETY: restore the default action and re-deliver, so swamp dies
    // of the same signal it would have without the handler.
    unsafe {
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
    }
}

pub(super) fn install_cleanup_once() {
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
        binary: &OsStr,
        args: &[OsString],
        out: std::fs::File,
        err: std::fs::File,
        launch: &Launch,
    ) -> io::Result<Self> {
        use std::os::unix::process::CommandExt;
        install_cleanup_once();
        crate::work_counters::record_spawn();
        let mut command = Command::new(binary);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            // Its own group: the deadline, cancel and signal paths kill
            // the child *and* whatever it forked.
            .process_group(0);
        match launch {
            Launch::Inherit => {
                // A pager or a credential prompt can never be why a
                // child blocks (stdin is null too).
                command
                    .env("GIT_PAGER", "cat")
                    .env("PAGER", "cat")
                    .env("GH_PAGER", "cat")
                    .env("SYSTEMD_PAGER", "cat")
                    .env("GIT_TERMINAL_PROMPT", "0")
                    .env("GH_PROMPT_DISABLED", "1")
                    .env("GCM_INTERACTIVE", "never");
            }
            Launch::Scrubbed(env) => {
                // Built from nothing: the parent's environment never
                // reaches a manager that removes things
                // (`tool_child_env`), and its cwd is a fixed neutral one,
                // never swamp's (which may sit inside a project whose
                // config the manager would read).
                command
                    .env_clear()
                    .envs(env.iter().map(|(k, v)| (k, v)))
                    .current_dir("/");
            }
        }
        let child = command.spawn()?;
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

/// How a child's environment is built.
enum Launch {
    /// The parent's environment, with pagers and prompts disabled (every
    /// read-only query).
    Inherit,
    /// Exactly these variables, nothing inherited, cwd `/` (tool-managed
    /// removal, #177).
    Scrubbed(Vec<(OsString, OsString)>),
}

/// Runs `binary args…` under the lifetime rules above.
fn run_command(
    binary: &OsStr,
    args: &[OsString],
    timeout: Duration,
    launch: &Launch,
) -> io::Result<RunOutput> {
    run_command_bounded(binary, args, timeout, launch, None)
}

/// [`run_command`], keeping at most `limit` bytes of each stream.
fn run_command_bounded(
    binary: &OsStr,
    args: &[OsString],
    timeout: Duration,
    launch: &Launch,
    limit: Option<u64>,
) -> io::Result<RunOutput> {
    let mut out_file = tempfile::tempfile()?;
    let mut err_file = tempfile::tempfile()?;
    let mut child = Running::start(
        binary,
        args,
        out_file.try_clone()?,
        err_file.try_clone()?,
        launch,
    )?;
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
    let mut truncated = false;
    let mut read_back = |f: &mut std::fs::File| -> Vec<u8> {
        let mut buf = Vec::new();
        let _ = f.seek(SeekFrom::Start(0));
        match limit {
            None => {
                let _ = f.read_to_end(&mut buf);
            }
            Some(limit) => {
                let _ = Read::by_ref(f).take(limit + 1).read_to_end(&mut buf);
                if buf.len() as u64 > limit {
                    buf.truncate(limit as usize);
                    truncated = true;
                }
            }
        }
        buf
    };
    let stdout = read_back(&mut out_file);
    let stderr = read_back(&mut err_file);
    Ok(RunOutput {
        code,
        stdout,
        stderr,
        timed_out,
        truncated,
    })
}

// ---- tool-managed removal's managers (#177) -------------------------
//
// A removal verb (`mise uninstall`, `simctl runtime delete`) must never
// run a binary found by name on an inherited `PATH`, with an inherited
// environment: a shim earlier on `PATH`, a `MISE_*` or `RUSTUP_TOOLCHAIN`
// the parent happened to carry, or a test that forgot its fake would all
// reach something other than what the human reviewed. So these spawns
// take a [`ToolBin`]: an absolute, canonical executable found in a fixed
// list of directories, owned by the user or root and writable by nobody
// else, run with an environment built from nothing.
// `.oh/guardrails/tool-removal-refuses-on-manager-facts.md`.

/// The most either stream of a tool-removal run keeps. A manager that
/// prints more than this is not printing a dry run swamp can read.
pub const TOOL_OUTPUT_LIMIT: u64 = 1024 * 1024;

/// Variables a child manager keeps from swamp's environment when the
/// user set them on purpose (maintainer decision, #177): which Xcode,
/// which mise global config, which rustup home. Nothing else is
/// inherited.
pub const TOOL_ENV_PASSTHROUGH: &[&str] =
    &["DEVELOPER_DIR", "MISE_GLOBAL_CONFIG_FILE", "RUSTUP_HOME"];

/// Version of the child environment policy below, recorded in the
/// ledger beside every removal.
pub const TOOL_ENV_POLICY: &str = "tool-env-1";

/// A manager binary resolved for tool-managed removal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolBin {
    program: Program,
    /// The canonical executable: what is spawned, every time.
    path: PathBuf,
    /// The fixed directory it was found in (first on the child's `PATH`,
    /// so a manager's own sub-invocations resolve beside it).
    dir: PathBuf,
    /// The child's whole environment, built once at resolution so the
    /// dry run and the removal run with the same one.
    env: Vec<(OsString, OsString)>,
    /// Set only by a test sandbox resolver: every spawn in a test build
    /// panics unless `path` is inside it.
    sandbox: Option<PathBuf>,
}

impl ToolBin {
    pub fn program(&self) -> Program {
        self.program
    }

    /// The canonical executable that runs.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The child's value of `name`, if it gets one.
    pub fn env_value(&self, name: &str) -> Option<&OsStr> {
        self.env
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_os_str())
    }

    /// The names of the variables the child gets (values are not
    /// recorded: `HOME` and `TMPDIR` are the user's).
    pub fn env_names(&self) -> Vec<String> {
        self.env
            .iter()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect()
    }
}

/// Finds manager binaries for tool-managed removal: in a fixed list of
/// directories in production, only under a temp dir in a test sandbox.
#[derive(Debug, Clone)]
pub struct ToolResolver {
    home: Option<PathBuf>,
    sandbox: Option<PathBuf>,
    /// A test's stand-in for swamp's own environment (to prove what is
    /// dropped); `None` reads the real one.
    parent_env: Option<Vec<(OsString, OsString)>>,
}

impl ToolResolver {
    /// The production resolver. In a test build (`cfg(test)` or the
    /// `testing` feature, never a shipped binary) the variable
    /// `SWAMP_TEST_TOOL_SANDBOX` names a sandbox of fake managers
    /// instead; without it every spawn from this resolver panics.
    pub fn system() -> Self {
        #[cfg(any(test, feature = "testing"))]
        if let Some(dir) = std::env::var_os("SWAMP_TEST_TOOL_SANDBOX") {
            return Self::sandboxed(Path::new(&dir));
        }
        ToolResolver {
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|h| h.is_absolute()),
            sandbox: None,
            parent_env: None,
        }
    }

    /// Fake managers under `dir/bin/<name>`, with `dir/home` as the
    /// child's `HOME`. Test builds only.
    #[cfg(any(test, feature = "testing"))]
    pub fn sandboxed(dir: &Path) -> Self {
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        ToolResolver {
            home: Some(dir.join("home")),
            sandbox: Some(dir),
            parent_env: None,
        }
    }

    /// Replaces the environment the child's is built from (a test's
    /// poisoned parent). Test builds only.
    #[cfg(any(test, feature = "testing"))]
    pub fn with_parent_env(mut self, vars: Vec<(OsString, OsString)>) -> Self {
        self.parent_env = Some(vars);
        self
    }

    /// The home directory the children run with.
    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }

    /// Where `program` is looked for, in order. Never the inherited
    /// `PATH`.
    pub fn candidates(&self, program: Program) -> Vec<PathBuf> {
        let name = program.binary();
        if let Some(sandbox) = &self.sandbox {
            return vec![sandbox.join("bin").join(name)];
        }
        let mut dirs: Vec<PathBuf> = match program {
            Program::Mise => vec![
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
            ],
            Program::Xcrun => vec![PathBuf::from("/usr/bin")],
            _ => Vec::new(),
        };
        if program == Program::Mise
            && let Some(home) = &self.home
        {
            dirs.push(home.join(".local/bin"));
            dirs.push(home.join(".cargo/bin"));
        }
        dirs.into_iter().map(|d| d.join(name)).collect()
    }

    /// The first candidate that exists, if it passes the ownership and
    /// permission checks. `Err` says what was looked at, or why the one
    /// found was not used; a failing candidate is never skipped for a
    /// later one.
    pub fn resolve(&self, program: Program) -> Result<ToolBin, String> {
        let candidates = self.candidates(program);
        let Some(found) = candidates
            .iter()
            .find(|c| std::fs::symlink_metadata(c).is_ok())
        else {
            let dirs: Vec<String> = candidates
                .iter()
                .filter_map(|c| c.parent().map(|d| d.display().to_string()))
                .collect();
            return Err(format!(
                "{} was not found in {} (swamp does not search PATH for a removal)",
                program.binary(),
                if dirs.is_empty() {
                    "any directory swamp checks".to_string()
                } else {
                    dirs.join(", ")
                }
            ));
        };
        let canonical = std::fs::canonicalize(found)
            .map_err(|e| format!("{} could not be resolved: {e}", found.display()))?;
        trusted_file(&canonical)?;
        if let Some(parent) = canonical.parent() {
            trusted_dir(parent)?;
        }
        let dir = found.parent().map(Path::to_path_buf).unwrap_or_default();
        let parent_env: Vec<(OsString, OsString)> = match &self.parent_env {
            Some(v) => v.clone(),
            None => std::env::vars_os().collect(),
        };
        let home = self.home.clone().unwrap_or_else(|| PathBuf::from("/"));
        Ok(ToolBin {
            program,
            env: tool_child_env(&dir, &home, &parent_env),
            path: canonical,
            dir,
            sandbox: self.sandbox.clone(),
        })
    }
}

fn trusted_owner(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.uid() == 0 || meta.uid() == super::sys::current_uid()
}

fn trusted_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mode = meta.permissions().mode();
    if !meta.is_file() || mode & 0o111 == 0 {
        return Err(format!("{} is not an executable file", path.display()));
    }
    if !trusted_owner(&meta) || mode & 0o022 != 0 {
        return Err(format!(
            "{} is writable by another user or owned by one; swamp runs a removal only through a \
             manager only you or root can change",
            path.display()
        ));
    }
    Ok(())
}

fn trusted_dir(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !trusted_owner(&meta) || meta.permissions().mode() & 0o022 != 0 {
        return Err(format!(
            "{} (the manager's own directory) is writable by another user or owned by one",
            path.display()
        ));
    }
    Ok(())
}

/// The whole environment a tool-removal child gets: fixed values, the
/// user's identity and temp dir, and only the [`TOOL_ENV_PASSTHROUGH`]
/// variables from `parent`. Every other `MISE_*`, `RUSTUP_*`,
/// `HOMEBREW_*` (and `PATH`) of the parent is dropped.
pub fn tool_child_env(
    dir: &Path,
    home: &Path,
    parent: &[(OsString, OsString)],
) -> Vec<(OsString, OsString)> {
    let mut path = dir.as_os_str().to_owned();
    path.push(":/usr/bin:/bin:/usr/sbin:/sbin");
    let mut env: Vec<(OsString, OsString)> = vec![
        ("HOME".into(), home.as_os_str().to_owned()),
        ("PATH".into(), path),
        ("LANG".into(), "C".into()),
        ("LC_ALL".into(), "C".into()),
        ("NO_COLOR".into(), "1".into()),
        ("CLICOLOR".into(), "0".into()),
        ("TERM".into(), "dumb".into()),
        ("PAGER".into(), "cat".into()),
        ("GIT_PAGER".into(), "cat".into()),
    ];
    for (k, v) in parent {
        let Some(name) = k.to_str() else { continue };
        if matches!(name, "USER" | "LOGNAME" | "TMPDIR") || TOOL_ENV_PASSTHROUGH.contains(&name) {
            env.push((k.clone(), v.clone()));
        }
    }
    env
}

/// One argument slot of a tool-removal invocation.
#[derive(Debug, Clone, Copy)]
enum ToolSlot {
    Lit(&'static str),
    /// `<tool>@<version>` as `mise ls --json` names it: no leading `-`,
    /// no whitespace, one `@`.
    MiseToolVersion,
    /// An uppercase 8-4-4-4-12 hex UUID (what `simctl runtime list -j`
    /// keys images by). Never `all`, never a flag.
    SimRuntimeUuid,
}

/// Whether `a` can be a mise `<tool>@<version>` operand.
pub fn is_mise_tool_version(a: &str) -> bool {
    let Some((tool, version)) = a.split_once('@') else {
        return false;
    };
    let tool_ok = tool
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
        && tool.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | ':' | '/' | '.' | '-')
        });
    let version_ok = version
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'));
    tool_ok && version_ok && !tool.contains("..")
}

/// Whether `a` is an uppercase 8-4-4-4-12 hex UUID.
pub fn is_sim_runtime_uuid(a: &str) -> bool {
    let groups: Vec<&str> = a.split('-').collect();
    groups.len() == 5
        && groups.iter().zip([8usize, 4, 4, 4, 12]).all(|(g, n)| {
            g.len() == n
                && g.chars()
                    .all(|c| c.is_ascii_digit() || ('A'..='F').contains(&c))
        })
}

/// The read-only invocations a [`ToolBin`] may run: listings, versions
/// and the managers' own dry runs. A removal shape (a dry run without
/// its flag) is not here: it runs only through
/// `fs_gate::destroy::tool_remove`.
fn tool_read_shapes(program: Program) -> &'static [&'static [ToolSlot]] {
    use ToolSlot::*;
    match program {
        Program::Mise => &[
            &[Lit("--version")],
            &[
                Lit("-C"),
                Lit("/"),
                Lit("ls"),
                Lit("--json"),
                Lit("--installed"),
            ],
            &[
                Lit("-C"),
                Lit("/"),
                Lit("prune"),
                Lit("--tools"),
                Lit("--dry-run"),
            ],
            &[
                Lit("-C"),
                Lit("/"),
                Lit("uninstall"),
                Lit("--dry-run"),
                MiseToolVersion,
            ],
        ],
        Program::Xcrun => &[
            &[Lit("--version")],
            &[Lit("simctl"), Lit("runtime"), Lit("list"), Lit("-j")],
            &[Lit("simctl"), Lit("list"), Lit("devices"), Lit("-j")],
            &[
                Lit("simctl"),
                Lit("runtime"),
                Lit("delete"),
                SimRuntimeUuid,
                Lit("--dry-run"),
            ],
        ],
        _ => &[],
    }
}

/// The removal invocations (`destroy::tool_remove` only). `mise prune`
/// always carries `--tools`: bare `mise prune` also prunes tracked
/// config links.
fn tool_exec_shapes(program: Program) -> &'static [&'static [ToolSlot]] {
    use ToolSlot::*;
    match program {
        Program::Mise => &[
            &[Lit("-C"), Lit("/"), Lit("uninstall"), MiseToolVersion],
            &[Lit("-C"), Lit("/"), Lit("prune"), Lit("--tools")],
        ],
        Program::Xcrun => &[&[Lit("simctl"), Lit("runtime"), Lit("delete"), SimRuntimeUuid]],
        _ => &[],
    }
}

fn tool_shape_matches(shapes: &[&[ToolSlot]], args: &[OsString]) -> bool {
    let Some(words) = args
        .iter()
        .map(|a| a.to_str())
        .collect::<Option<Vec<&str>>>()
    else {
        return false;
    };
    shapes.iter().any(|shape| {
        shape.len() == words.len()
            && shape.iter().zip(&words).all(|(slot, w)| match slot {
                ToolSlot::Lit(l) => l == w,
                ToolSlot::MiseToolVersion => is_mise_tool_version(w),
                ToolSlot::SimRuntimeUuid => is_sim_runtime_uuid(w),
            })
    })
}

/// Whether `args` is a read-only tool invocation (see [`tool_read_shapes`]).
pub fn is_tool_read(program: Program, args: &[OsString]) -> bool {
    tool_shape_matches(tool_read_shapes(program), args)
}

/// Whether `args` is a removal invocation (see [`tool_exec_shapes`]).
pub fn is_tool_exec(program: Program, args: &[OsString]) -> bool {
    tool_shape_matches(tool_exec_shapes(program), args)
}

/// In any test build, a tool spawn whose binary is not inside the
/// resolver's sandbox panics before anything starts: no test can reach
/// the maintainer's real mise or xcrun, whatever it forgot to set up.
fn guard_test_sandbox(bin: &ToolBin) {
    #[cfg(any(test, feature = "testing"))]
    {
        let inside = bin
            .sandbox
            .as_ref()
            .is_some_and(|s| bin.path.starts_with(s));
        if !inside {
            panic!(
                "test build: {} is not inside a test sandbox; tests never run a real manager",
                bin.path.display()
            );
        }
    }
    #[cfg(not(any(test, feature = "testing")))]
    let _ = bin;
}

fn run_tool(bin: &ToolBin, args: &[OsString], timeout: Duration) -> io::Result<RunOutput> {
    guard_test_sandbox(bin);
    run_command_bounded(
        bin.path.as_os_str(),
        args,
        timeout,
        &Launch::Scrubbed(bin.env.clone()),
        Some(TOOL_OUTPUT_LIMIT),
    )
}

/// Runs one read-only tool invocation (a listing, a version, a dry run)
/// with the [`ToolBin`]'s scrubbed environment. Anything that is not a
/// [`tool_read_shapes`] shape is refused before anything starts.
pub fn run_tool_read(bin: &ToolBin, args: &[OsString], timeout: Duration) -> io::Result<RunOutput> {
    if !is_tool_read(bin.program, args) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} {} is not a read-only invocation swamp runs for a tool removal",
                bin.program.binary(),
                args.iter()
                    .map(|a| a.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        ));
    }
    run_tool(bin, args, timeout)
}

/// Runs one removal invocation: only `fs_gate::destroy::tool_remove`
/// calls this.
pub(super) fn run_tool_exec(
    bin: &ToolBin,
    args: &[OsString],
    timeout: Duration,
) -> io::Result<RunOutput> {
    if !is_tool_exec(bin.program, args) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "not a removal invocation swamp runs",
        ));
    }
    run_tool(bin, args, timeout)
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
        let out = run_command(
            OsStr::new("sh"),
            &hung_tree(),
            Duration::from_millis(600),
            &Launch::Inherit,
        )
        .unwrap();
        assert!(out.timed_out && t.elapsed() < Duration::from_secs(10));
        let grand: i32 = out.stdout_lossy().trim().parse().unwrap();
        assert!(wait_gone(grand), "grandchild {grand} survived the deadline");
    }

    #[test]
    fn dropping_the_guard_kills_the_group() {
        let out = tempfile::tempfile().unwrap();
        let err = tempfile::tempfile().unwrap();
        let mut r = Running::start(
            OsStr::new("sh"),
            &hung_tree(),
            out.try_clone().unwrap(),
            err,
            &Launch::Inherit,
        )
        .unwrap();
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
                OsStr::new("sh"),
                &["-c".into(), script.into()],
                Duration::from_secs(600),
                &Launch::Inherit,
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
        let out = run_command(
            OsStr::new("sh"),
            &["-c".into(), script.into()],
            Duration::from_secs(10),
            &Launch::Inherit,
        )
        .unwrap();
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

    /// Tempting wrong patch: "any executable named mise will do". One that
    /// another user could have replaced is refused, and the next candidate
    /// is not tried in its place.
    #[test]
    fn a_manager_writable_by_others_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();
        let fake = dir.path().join("bin/mise");
        std::fs::write(&fake, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o777)).unwrap();
        let r = ToolResolver::sandboxed(dir.path()).resolve(Program::Mise);
        assert!(r.unwrap_err().contains("writable by another user"));
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let bin = ToolResolver::sandboxed(dir.path())
            .resolve(Program::Mise)
            .unwrap();
        assert!(bin.path().is_absolute());
        assert!(
            ToolResolver::sandboxed(dir.path())
                .resolve(Program::Docker)
                .is_err()
        );
    }

    #[test]
    fn the_child_environment_keeps_only_the_named_variables() {
        let parent: Vec<(OsString, OsString)> = [
            ("RUSTUP_TOOLCHAIN", "x"),
            ("RUSTUP_HOME", "/r"),
            ("MISE_GLOBAL_CONFIG_FILE", "/g.toml"),
            ("MISE_ENV", "x"),
            ("HOMEBREW_NO_INSTALL_FROM_API", "1"),
            ("TMPDIR", "/t/"),
            ("PATH", "/evil"),
        ]
        .iter()
        .map(|(k, v)| (OsString::from(k), OsString::from(v)))
        .collect();
        let env = tool_child_env(
            Path::new("/opt/homebrew/bin"),
            Path::new("/Users/dev"),
            &parent,
        );
        let get = |k: &str| {
            env.iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.to_string_lossy().into_owned())
        };
        assert_eq!(
            get("PATH").unwrap(),
            "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"
        );
        assert_eq!(get("RUSTUP_HOME").as_deref(), Some("/r"));
        assert_eq!(get("MISE_GLOBAL_CONFIG_FILE").as_deref(), Some("/g.toml"));
        assert_eq!(get("TMPDIR").as_deref(), Some("/t/"));
        for dropped in [
            "RUSTUP_TOOLCHAIN",
            "MISE_ENV",
            "HOMEBREW_NO_INSTALL_FROM_API",
        ] {
            assert!(get(dropped).is_none(), "{dropped}");
        }
    }

    #[test]
    fn tool_operands_are_typed() {
        for ok in [
            "go@1.23.5",
            "java@temurin-17.0.20+101",
            "aqua:ouch-org/ouch@0.6.1",
        ] {
            assert!(is_mise_tool_version(ok), "{ok}");
        }
        for bad in [
            "-a@1", "go", "go@", "@1", "go@1 2", "go@-1", "../x@1", "go@1;rm",
        ] {
            assert!(!is_mise_tool_version(bad), "{bad}");
        }
        assert!(is_sim_runtime_uuid("5FF350CD-0800-4015-B796-BE66B16D154E"));
        for bad in [
            "all",
            "5ff350cd-0800-4015-b796-be66b16d154e",
            "--outdated",
            "",
        ] {
            assert!(!is_sim_runtime_uuid(bad), "{bad}");
        }
    }

    #[test]
    fn run_refuses_mise_by_path_name() {
        let (r, counted) = crate::work_counters::measured(|| {
            run(Program::Mise, ["--version"], Duration::from_secs(5))
        });
        assert!(r.is_err());
        assert_eq!(counted.subprocess_spawns, 0);
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
