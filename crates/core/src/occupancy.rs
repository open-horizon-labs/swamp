//! Live-use/occupancy evidence (#55): bounded, read-only checks for
//! whether something outside this process currently has a path open, a
//! Docker container running against it, a simulator booted on it, or a
//! manager lock held on it. Every check here answers a narrow question
//! ("does `lsof` show an open handle right now") -- never "is this safe
//! to remove". A negative result is evidence, not proof of no consumer
//! (`.oh/guardrails/activity-and-consumer-evidence-have-limits.md`).
//!
//! [`is_active`] is the original boolean gate (test/fixture-only;
//! `.oh/guardrails/occupancy-is-tristate-at-sinks.md` bans a collapsed
//! boolean from production code) and is unchanged. The functions below
//! build the structured [`crate::evidence::Evidence`] the report/action
//! layers attach; they are the same underlying signal, offered with
//! provenance and freshness rather than a bare bool.

use crate::evidence::{Evidence, EvidenceSource, FactKind, FactSubtype, FactValue, Freshness};
use std::{path::Path, time::Duration};

/// Short-lived: a process/lock/container state observed now says nothing
/// about five minutes from now. Existing action boundaries (plan
/// proposal, execute) recheck rather than trust an older observation
/// past this window.
pub const CURRENT_USE_EXPIRY_SECS: u64 = 60;

/// How long an occupancy probe may run before it is killed and reported
/// as [`OccupancyState::Unknown`]. `lsof +D` over a large directory is
/// bounded by this, never by patience.
const OCCUPANCY_TIMEOUT: Duration = Duration::from_secs(10);

/// The answer to "does anything outside this process hold this path (or
/// anything under it) open right now". Deliberately three-valued: a
/// failed, timed-out or permission-denied probe is **not** "nothing is
/// open" (`.oh/guardrails/occupancy-gaps-are-unknown-never-free.md`).
/// A Trash move shows it as a fact and is never gated on it (2026-09-23);
/// tool-managed removal, which has no Trash, refuses on `Occupied` and
/// `Unknown` (`.oh/guardrails/tool-removal-refuses-on-manager-facts.md`).
#[must_use = "an occupancy answer that is not matched on is a recheck that did not happen"]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OccupancyState {
    /// The probe ran to completion and found no open handle.
    Free,
    /// The probe found at least one open handle; the payload names the
    /// member that was open (the anchor itself for a file probe).
    Occupied(std::path::PathBuf),
    /// The probe could not answer (binary missing, timed out, permission
    /// denied, unreadable directory). Treated as a refusal at every sink.
    Unknown(String),
}

/// The one bounded `lsof` probe every occupancy question in this crate
/// goes through. Directories are probed with `+D` so **every descendant**
/// is covered, not just the anchor (the review's open-cache-member
/// counterexample); files are probed directly. `stdout` and `stderr` are
/// captured separately so an ordinary `lsof` warning is not mistaken for
/// an open handle, while a permission error still becomes `Unknown`.
pub fn probe_path(path: &Path) -> OccupancyState {
    probe_paths(&[path])
}

/// [`probe_path`] over several anchors at once. On Linux that is one
/// procfs pass for all of them (a process table scan per anchor would
/// multiply); on macOS it is one bounded `lsof` per anchor, as it
/// always was. The first non-`Free` answer wins.
pub fn probe_paths(paths: &[&Path]) -> OccupancyState {
    match crate::platform::OccupancyProbe::for_os(crate::platform::Os::current()) {
        crate::platform::OccupancyProbe::Lsof => {
            for p in paths {
                match lsof_probe(p) {
                    OccupancyState::Free => {}
                    other => return other,
                }
            }
            OccupancyState::Free
        }
        crate::platform::OccupancyProbe::Procfs => {
            let procfs = crate::fs_gate::procfs::probe(
                Path::new("/proc"),
                paths,
                &crate::fs_gate::procfs::Creds::of_self(),
                crate::fs_gate::procfs::self_pid(),
                OCCUPANCY_TIMEOUT,
            );
            // procfs is the answer; `lsof`, where one is installed, is a
            // second reader of the same kernel state and can only make
            // the answer stricter: Occupied or Unknown from either wins,
            // Free needs both. With no `lsof` on PATH, procfs stands on
            // its own -- a minimal install has none, and needs none.
            if procfs != OccupancyState::Free || !lsof_on_path() {
                return procfs;
            }
            for p in paths {
                match lsof_probe(p) {
                    OccupancyState::Free => {}
                    other => return other,
                }
            }
            OccupancyState::Free
        }
    }
}

/// How long the one machine-wide snapshot may run. It walks the process
/// table, not any directory tree, so seconds to tens of seconds is typical (16s measured on a busy dev Mac); the bound only
/// exists so a wedged `lsof` becomes `Unknown` for every anchor.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(60);

/// The snapshot's `lsof` argv. `-n` and `-P` stop `lsof` resolving host and
/// port names for every socket: the listing is identical but took 16 s
/// instead of 0.2 s on the machine it was measured on.
const SNAPSHOT_ARGS: [&str; 4] = ["-n", "-P", "-F", "n"];

/// One listing of every open file path on the machine, taken with a
/// single `lsof -n -P -F n` (no `+D`, so no directory tree is walked), against
/// which any number of anchors are answered in memory. A review pass over
/// N cleanup groups therefore costs one process-table walk, not N tree
/// walks.
///
/// Tri-state is preserved: a snapshot that failed, timed out, was
/// unparseable or was only partially readable answers `Unknown` for every
/// anchor it cannot positively find open. It never answers `Free` from a
/// listing it could not fully read.
#[derive(Debug)]
pub struct OccupancySnapshot {
    /// Absolute paths reported open (files, cwd, txt, ...), with the
    /// command names holding each when the listing asked for them
    /// (`-F cn`, tool-managed removal); empty otherwise.
    open: std::collections::BTreeMap<std::path::PathBuf, std::collections::BTreeSet<String>>,
    /// Why the listing is not trustworthy for a negative answer.
    gap: Option<String>,
}

impl OccupancySnapshot {
    /// Run the one `lsof -n -P -F n`.
    pub fn capture() -> Self {
        Self::capture_args(&SNAPSHOT_ARGS)
    }

    fn capture_args(args: &[&str]) -> Self {
        match crate::fs_gate::spawn::run(
            crate::fs_gate::spawn::Program::Lsof,
            args,
            SNAPSHOT_TIMEOUT,
        ) {
            Ok(out) => Self::from_lsof_run(
                out.code,
                out.timed_out,
                &out.stdout_lossy(),
                &out.stderr_lossy(),
            ),
            Err(e) => Self::failed(format!("lsof could not be started: {e}")),
        }
    }

    fn failed(why: String) -> Self {
        Self {
            open: Default::default(),
            gap: Some(why),
        }
    }

    /// Pure interpretation of a finished `lsof -n -P -F n` run.
    pub(crate) fn from_lsof_run(
        code: Option<i32>,
        timed_out: bool,
        stdout: &str,
        stderr: &str,
    ) -> Self {
        if timed_out {
            return Self::failed(format!(
                "lsof did not answer within {}s",
                SNAPSHOT_TIMEOUT.as_secs()
            ));
        }
        // 0 = listed; 1 = listed with some complaint. Anything else
        // (signal, 2, ...) is a run that did not complete.
        if !matches!(code, Some(0) | Some(1)) {
            return Self::failed(format!("lsof exited with {code:?}"));
        }
        let mut open: std::collections::BTreeMap<
            std::path::PathBuf,
            std::collections::BTreeSet<String>,
        > = Default::default();
        let mut saw_process = false;
        // One process at a time: its command name and every path it has
        // open (its own executable among them, the `txt` entry).
        let mut procs: Vec<(Option<String>, Vec<std::path::PathBuf>)> = Vec::new();
        for line in stdout.lines().filter(|l| !l.is_empty()) {
            let mut chars = line.chars();
            let id = chars.next().unwrap_or(' ');
            if !id.is_ascii_alphanumeric() {
                return Self::failed(format!("unparseable lsof output line: {line:.40}"));
            }
            match id {
                'p' => {
                    saw_process = true;
                    procs.push((None, Vec::new()));
                }
                'c' => {
                    if let Some(p) = procs.last_mut() {
                        p.0 = Some(chars.as_str().to_string());
                    }
                }
                'n' => {
                    let name = chars.as_str();
                    if name.starts_with('/') {
                        if procs.is_empty() {
                            procs.push((None, Vec::new()));
                        }
                        if let Some(p) = procs.last_mut() {
                            p.1.push(std::path::PathBuf::from(name));
                        }
                    }
                }
                _ => {}
            }
        }
        for (command, paths) in procs {
            // A command name counts as itself only when the process's own
            // executable is where CoreSimulator or Xcode keeps it; any
            // other process with that name is just its name.
            let label = command.map(|c| {
                let verified = paths.iter().any(|p| {
                    p.file_name().and_then(|n| n.to_str()) == Some(c.as_str())
                        && TRUSTED_HOST_ROOTS.iter().any(|r| p.starts_with(r))
                        && !p.components().any(|x| x.as_os_str() == "..")
                });
                if verified {
                    format!("{VERIFIED}{c}")
                } else {
                    c
                }
            });
            for path in paths {
                let holders = open.entry(path).or_default();
                if let Some(l) = &label {
                    holders.insert(l.clone());
                }
            }
        }
        // A machine always has at least this process's own descriptors
        // open; an empty listing means lsof did not really look.
        if !saw_process {
            return Self::failed("lsof listed no processes".into());
        }
        let gap = stderr
            .to_lowercase()
            .contains("permission denied")
            .then(|| "permission denied listing some open files".to_string());
        Self { open, gap }
    }

    /// Answer one anchor from the snapshot. Occupied if any open path is
    /// the anchor or lies under it (component-wise, so `/a/target2` is
    /// not under `/a/target`). Free only from a complete listing.
    pub fn state_for(&self, path: &Path) -> OccupancyState {
        match crate::fs_gate::symlink_metadata(path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return OccupancyState::Free,
            Err(e) => {
                return OccupancyState::Unknown(format!("cannot stat {}: {e}", path.display()));
            }
        }
        let canonical = crate::fs_gate::canonicalize(path).ok();
        for anchor in std::iter::once(path).chain(canonical.as_deref()) {
            // Path ordering is component-wise, so everything under
            // `anchor` sorts contiguously right after it.
            if let Some((hit, _)) = self.open.range(anchor.to_path_buf()..).next()
                && hit.starts_with(anchor)
            {
                return OccupancyState::Occupied(hit.clone());
            }
        }
        match &self.gap {
            None => OccupancyState::Free,
            Some(why) => OccupancyState::Unknown(format!(
                "open-file snapshot incomplete ({why}) for {}",
                path.display()
            )),
        }
    }

    /// Run `f` with one lazily-taken snapshot serving every
    /// [`open_file_evidence`] call made on this thread inside it. Where
    /// the probe is not `lsof` (Linux's single-pass procfs) this is a
    /// no-op and each call answers as before.
    pub fn scoped<R>(f: impl FnOnce() -> R) -> R {
        Self::scoped_observed(|_| {}, f)
    }

    /// [`scoped`](Self::scoped), telling `on_phase` when the one slow
    /// `lsof` starts and finishes (the snapshot is taken lazily, on the
    /// first evidence call inside `f`), so a UI can say what the wait is.
    pub fn scoped_observed<R>(
        on_phase: impl Fn(SnapshotPhase) + 'static,
        f: impl FnOnce() -> R,
    ) -> R {
        if crate::platform::OccupancyProbe::for_os(crate::platform::Os::current())
            != crate::platform::OccupancyProbe::Lsof
        {
            return f();
        }
        let prev = SCOPED.with(|s| s.replace(Some(std::rc::Rc::new(std::cell::OnceCell::new()))));
        struct Restore(Option<std::rc::Rc<std::cell::OnceCell<OccupancySnapshot>>>);
        impl Drop for Restore {
            fn drop(&mut self) {
                let prev = self.0.take();
                SCOPED.with(|s| *s.borrow_mut() = prev);
            }
        }
        let _restore = Restore(prev);
        let prev_hook = PHASE_HOOK.with(|h| h.replace(Some(std::rc::Rc::new(on_phase))));
        struct RestoreHook(Option<PhaseHook>);
        impl Drop for RestoreHook {
            fn drop(&mut self) {
                let prev = self.0.take();
                PHASE_HOOK.with(|h| *h.borrow_mut() = prev);
            }
        }
        let _restore_hook = RestoreHook(prev_hook);
        f()
    }
    /// Tool-managed removal's open-file answer (#177): every path the
    /// manager's dry run named, from one `lsof -n -P -F cn` listing that
    /// also names each holder's command. A holder whose command is one of
    /// `manager_own` (the manager's own host process, which the manager
    /// stops before it removes: CoreSimulator's `SimLaunchHost` keeps a
    /// library open in every mounted runtime) does not count; any other
    /// is `Occupied`, and a listing that could not be fully read is
    /// `Unknown`, never `Free`. The holder's command name comes back with
    /// an `Occupied` answer.
    pub fn tool_removal_state(
        paths: &[std::path::PathBuf],
        manager_own: &[&str],
    ) -> (OccupancyState, Option<String>) {
        let snapshot = match crate::platform::OccupancyProbe::for_os(crate::platform::Os::current())
        {
            crate::platform::OccupancyProbe::Lsof => Self::capture_args(&TOOL_SNAPSHOT_ARGS),
            crate::platform::OccupancyProbe::Procfs => {
                let refs: Vec<&Path> = paths.iter().map(|p| p.as_path()).collect();
                return (probe_paths(&refs), None);
            }
        };
        snapshot.holders_of(paths, manager_own)
    }

    /// [`Self::tool_removal_state`] over an already-taken listing.
    pub(crate) fn holders_of(
        &self,
        paths: &[std::path::PathBuf],
        manager_own: &[&str],
    ) -> (OccupancyState, Option<String>) {
        for path in paths {
            let canonical = crate::fs_gate::canonicalize(path).ok();
            for anchor in std::iter::once(path.as_path()).chain(canonical.as_deref()) {
                for (hit, holders) in self
                    .open
                    .range(anchor.to_path_buf()..)
                    .take_while(|(hit, _)| hit.starts_with(anchor))
                {
                    let own = !holders.is_empty()
                        && holders.iter().all(|h| {
                            h.strip_prefix(VERIFIED)
                                .is_some_and(|c| manager_own.contains(&c))
                        });
                    if !own {
                        let who = holders
                            .iter()
                            .next()
                            .map(|h| h.trim_start_matches(VERIFIED).to_string());
                        return (OccupancyState::Occupied(hit.clone()), who);
                    }
                }
            }
        }
        match &self.gap {
            None => (OccupancyState::Free, None),
            Some(why) => (
                OccupancyState::Unknown(format!("open-file listing incomplete ({why})")),
                None,
            ),
        }
    }
}

/// Marks a holder whose executable was found under [`TRUSTED_HOST_ROOTS`].
const VERIFIED: &str = "verified-executable:";

/// Where a manager's own host process may live for its name to count
/// (`SimLaunchHost.arm64` is CoreSimulator's XPC service).
const TRUSTED_HOST_ROOTS: &[&str] = &[
    "/Library/Developer/PrivateFrameworks/CoreSimulator.framework/",
    "/Library/Developer/CoreSimulator/",
    "/Applications/Xcode.app/",
];

/// The tool-removal listing's `lsof` argv: the snapshot's, with each
/// process's command name.
const TOOL_SNAPSHOT_ARGS: [&str; 4] = ["-n", "-P", "-F", "cn"];

/// Where a scoped open-file snapshot is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotPhase {
    /// The one `lsof` is running.
    Capturing,
    /// It returned (or failed); per-path answers are now instant.
    Captured,
}

type PhaseHook = std::rc::Rc<dyn Fn(SnapshotPhase)>;

thread_local! {
    static PHASE_HOOK: std::cell::RefCell<Option<PhaseHook>> =
        const { std::cell::RefCell::new(None) };
}

type ScopedSnapshot = std::rc::Rc<std::cell::OnceCell<OccupancySnapshot>>;

thread_local! {
    static SCOPED: std::cell::RefCell<Option<ScopedSnapshot>> =
        const { std::cell::RefCell::new(None) };
}

/// The state for `path`: from this thread's scoped snapshot when a review
/// pass installed one, otherwise a fresh [`probe_path`].
fn probe_for_evidence(path: &Path) -> OccupancyState {
    let cell = SCOPED.with(|s| s.borrow().clone());
    match cell {
        Some(cell) => cell
            .get_or_init(|| {
                let hook = PHASE_HOOK.with(|h| h.borrow().clone());
                if let Some(h) = &hook {
                    h(SnapshotPhase::Capturing);
                }
                let snap = OccupancySnapshot::capture();
                if let Some(h) = &hook {
                    h(SnapshotPhase::Captured);
                }
                snap
            })
            .state_for(path),
        None => probe_path(path),
    }
}

/// Whether an `lsof` is on `PATH` at all -- present, not necessarily
/// runnable. A present `lsof` that cannot be run is a second reader
/// that could not answer, which is `Unknown`, exactly as on macOS.
///
/// A `PATH` entry that cannot be examined counts as "maybe": the second
/// reader is then tried, and if it cannot run the answer is `Unknown`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn lsof_on_path() -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&path) {
        match crate::fs_gate::symlink_metadata(dir.join("lsof")) {
            Ok(_) => return true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return true,
        }
    }
    false
}

/// The name a current-use fact gives for where its answer came from.
fn probe_tool() -> &'static str {
    match crate::platform::OccupancyProbe::for_os(crate::platform::Os::current()) {
        crate::platform::OccupancyProbe::Lsof => "lsof",
        crate::platform::OccupancyProbe::Procfs => "procfs",
    }
}

fn lsof_probe(path: &Path) -> OccupancyState {
    let is_dir = match crate::fs_gate::symlink_metadata(path) {
        Ok(m) => m.is_dir() && !m.file_type().is_symlink(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Nothing there to hold open. The caller's own identity
            // recheck is what refuses a vanished path.
            return OccupancyState::Free;
        }
        Err(e) => return OccupancyState::Unknown(format!("cannot stat {}: {e}", path.display())),
    };
    let flag = if is_dir { "+D" } else { "--" };
    let out = match crate::fs_gate::spawn::run(
        crate::fs_gate::spawn::Program::Lsof,
        [std::ffi::OsStr::new(flag), path.as_os_str()],
        OCCUPANCY_TIMEOUT,
    ) {
        Ok(out) => out,
        Err(e) => return OccupancyState::Unknown(format!("lsof could not be started: {e}")),
    };
    if out.timed_out {
        return OccupancyState::Unknown(format!(
            "lsof did not answer within {}s for {}",
            OCCUPANCY_TIMEOUT.as_secs(),
            path.display()
        ));
    }
    classify_lsof_exit(out.code, &out.stdout_lossy(), &out.stderr_lossy(), path)
}

/// Pure classification of a completed `lsof` run, factored out so the
/// fail-closed rules are unit-testable without spawning a process.
///
/// The distinction that matters: exit 1 with no output means "looked,
/// found nothing"; exit 1 with a permission complaint means "could not
/// look", which is `Unknown` and refuses. Treating the second as the
/// first is how a permission-denied probe came to authorize a removal.
fn classify_lsof_exit(
    code: Option<i32>,
    stdout: &str,
    stderr: &str,
    path: &Path,
) -> OccupancyState {
    let found = !stdout.trim().is_empty();
    let denied = stderr.to_lowercase().contains("permission denied");
    match code {
        // `lsof` exits 0 when it found at least one open handle.
        Some(0) if found => OccupancyState::Occupied(path.to_path_buf()),
        Some(0) => OccupancyState::Free,
        Some(1) if found => OccupancyState::Occupied(path.to_path_buf()),
        Some(1) if denied => OccupancyState::Unknown(format!(
            "permission denied querying open files under {}",
            path.display()
        )),
        Some(1) => OccupancyState::Free,
        other => {
            OccupancyState::Unknown(format!("lsof exited with {other:?} for {}", path.display()))
        }
    }
}

/// Structured current-use evidence for whether some process holds this
/// unit open right now.
///
/// A unit *is* the path and everything under it -- that is what an action
/// renames away -- so this asks about the whole unit, through the same
/// bounded [`probe_path`] the sinks use (`lsof +D` for a directory). The
/// PR #123 review's counterexample: `lsof -- <dir>` reports only handles
/// on the directory inode, and the old implementation turned that into a
/// published `Known(false)` fact reading "no open-file match this pass"
/// for a unit whose contents were demonstrably open. A negative answer
/// to a question that was never asked is worse than no answer.
pub fn open_file_evidence(path: &Path) -> Evidence {
    let observed_at = crate::entities::now();
    let source = EvidenceSource::ProcessQuery {
        tool: probe_tool().into(),
    };
    match probe_for_evidence(path) {
        OccupancyState::Occupied(open_at) => Evidence::known(
            FactKind::CurrentUse,
            FactSubtype::OpenFile,
            FactValue::Bool(true),
            source,
            observed_at,
        )
        .with_freshness(Freshness::expires_after(CURRENT_USE_EXPIRY_SECS))
        .with_note(format!("open handle found on {}", open_at.display())),
        OccupancyState::Free => Evidence::known(
            FactKind::CurrentUse,
            FactSubtype::OpenFile,
            FactValue::Bool(false),
            source,
            observed_at,
        )
        .with_freshness(Freshness::expires_after_with_coverage(
            CURRENT_USE_EXPIRY_SECS,
            "only processes this user can inspect: those running with this user's credentials \
             and no more. Another user's process, a more privileged one (a root daemon, a \
             container runtime, a setuid program) and one the kernel marks non-dumpable (a PAM \
             session holder, an agent that hides its memory) are not visible without privileges",
        ))
        .with_note(
            "no open-file match for this path or anything under it this pass; not proof that no \
             process or consumer needs it",
        ),
        OccupancyState::Unknown(reason) => Evidence::unavailable(
            FactKind::CurrentUse,
            FactSubtype::OpenFile,
            source,
            observed_at,
            crate::evidence::Reason::carried(reason),
        ),
    }
}

/// Current-use evidence from a Docker object's already-collected
/// container references (`docker::ContainerRef`, `docker::DockerFacts`):
/// a `running` container is a live consumer; anything else (`exited`,
/// none at all) is not proof of no consumer, just no *running* one seen
/// this pass.
pub fn docker_running_container_evidence(containers: &[crate::docker::ContainerRef]) -> Evidence {
    let observed_at = crate::entities::now();
    if containers.is_empty() {
        return Evidence::unknown(
            FactKind::CurrentUse,
            FactSubtype::RunningContainer,
            EvidenceSource::DockerApi {
                detail: "no container references collected for this object".into(),
            },
            observed_at,
            crate::reason!("no container reference recorded this pass"),
        );
    }
    let running: Vec<String> = containers
        .iter()
        .filter(|c| c.state == "running")
        .map(|c| c.name.clone())
        .collect();
    let has_running = !running.is_empty();
    Evidence::known(
        FactKind::CurrentUse,
        FactSubtype::RunningContainer,
        FactValue::Bool(has_running),
        EvidenceSource::DockerApi {
            detail: format!("{} container reference(s) inspected", containers.len()),
        },
        observed_at,
    )
    // Coverage, not just expiry: the daemon reports the containers *it*
    // knows about. A consumer that is not a container (a `docker build`
    // in flight, another daemon, a `docker cp` reading a volume) is
    // outside what this question can see, so the fact carries that limit
    // alongside its recheck window rather than reading as "nothing at
    // all uses this".
    .with_freshness(Freshness::expires_after_with_coverage(
        CURRENT_USE_EXPIRY_SECS,
        "only container references this daemon reported this pass; a non-container consumer \
         (an in-flight build, another daemon, a running `docker cp`) would not appear here",
    ))
    .with_note(if has_running {
        format!("running: {}", running.join(", "))
    } else {
        "no container currently running against this object".into()
    })
}

/// Advisory-lock probe: attempts (and immediately releases) a
/// non-blocking exclusive `flock` on `lock_path`. `Ok(true)` means the
/// lock was free (this call briefly held and released it); `Ok(false)`
/// means another process currently holds it. Never writes to the file;
/// never blocks.
fn probe_flock(lock_path: &Path) -> std::io::Result<bool> {
    crate::fs_gate::sys::flock_is_free(lock_path)
}

/// Manager lock-file evidence (Cargo's `.package-cache`, a Gradle daemon
/// registry lock, ...): read-only, bounded, never held past this check.
/// Absence of the lock file is `Unknown` (most managers only create the
/// file while doing something), not "not in use".
pub fn manager_lock_evidence(tool: impl Into<String>, lock_path: &Path) -> Evidence {
    let observed_at = crate::entities::now();
    let tool = tool.into();
    let source = EvidenceSource::ManagerLock {
        tool: tool.clone(),
        path: lock_path.display().to_string(),
    };
    if !crate::fs_gate::exists(lock_path) {
        return Evidence::unknown(
            FactKind::CurrentUse,
            FactSubtype::Lock,
            source,
            observed_at,
            crate::reason!("no lock file present"),
        );
    }
    match probe_flock(lock_path) {
        Ok(free) => Evidence::known(
            FactKind::CurrentUse,
            FactSubtype::Lock,
            FactValue::Bool(!free),
            source,
            observed_at,
        )
        .with_freshness(Freshness::expires_after(CURRENT_USE_EXPIRY_SECS)),
        Err(e) => Evidence::unavailable(
            FactKind::CurrentUse,
            FactSubtype::Lock,
            source,
            observed_at,
            crate::reason!("could not probe lock: {e}"),
        ),
    }
}

/// Simulator "booted" state (#55) via the bounded, allow-listed,
/// read-only `xcrun simctl list devices -j` query
/// (`locations::ALLOWED_COMMANDS`). `udid` identifies the target
/// simulator; a query failure is `Unavailable`, an unmatched udid is
/// `Unknown` (never assumed "not booted").
pub fn simulator_booted_evidence(udid: &str, env: &crate::locations::Environment) -> Evidence {
    let observed_at = crate::entities::now();
    let source = EvidenceSource::ProcessQuery {
        tool: "xcrun simctl list devices -j".into(),
    };
    match env.run_command("xcrun", &["simctl", "list", "devices", "-j"]) {
        Ok(outcome) if outcome.success => match find_simulator_state(&outcome.stdout, udid) {
            Some(state) => Evidence::known(
                FactKind::CurrentUse,
                FactSubtype::Booted,
                FactValue::Bool(state.eq_ignore_ascii_case("booted")),
                source,
                observed_at,
            )
            .with_note(format!("simctl state: {state}"))
            // `simctl list devices` answers for the devices *this user's*
            // CoreSimulator store holds. A device in another user's store,
            // or one backed by a system-wide runtime volume this user
            // cannot read, is outside the query's reach -- a coverage
            // limit, separate from the recheck window.
            .with_freshness(Freshness::expires_after_with_coverage(
                CURRENT_USE_EXPIRY_SECS,
                "only devices `simctl list devices` reports for this user; a device in another \
                 user's CoreSimulator store or on an unreadable runtime volume would not appear",
            )),
            None => Evidence::unknown(
                FactKind::CurrentUse,
                FactSubtype::Booted,
                source,
                observed_at,
                crate::reason!("udid not found in simctl device list"),
            ),
        },
        Ok(_) => Evidence::unavailable(
            FactKind::CurrentUse,
            FactSubtype::Booted,
            source,
            observed_at,
            crate::reason!("simctl query did not succeed"),
        ),
        Err(reason) => Evidence::unavailable(
            FactKind::CurrentUse,
            FactSubtype::Booted,
            source,
            observed_at,
            crate::evidence::Reason::carried(reason),
        ),
    }
}

/// Pure JSON scan for `udid`'s `"state"` field in `simctl list devices
/// -j`'s output shape (`{"devices": {"<runtime>": [{"udid":..,
/// "state":..}, ...]}}`), factored out for testability without a real
/// simulator.
fn find_simulator_state(json_text: &str, udid: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json_text).ok()?;
    let devices = value.get("devices")?.as_object()?;
    for runtime_list in devices.values() {
        if let Some(list) = runtime_list.as_array() {
            for dev in list {
                if dev.get("udid").and_then(|v| v.as_str()) == Some(udid) {
                    return dev
                        .get("state")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
            }
        }
    }
    None
}

/// Fail-closed boolean view (anything but `Free` is active), for test
/// fixtures only: production code has no boolean occupancy
/// (`.oh/guardrails/occupancy-is-tristate-at-sinks.md`).
#[cfg(any(test, feature = "testing"))]
pub fn is_active(path: &Path) -> bool {
    !matches!(probe_path(path), OccupancyState::Free)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::FactStatus;
    use crate::fs_gate::procfs::{Creds, parse_status, probe as procfs_probe, withheld_owner};
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// Reverting to a plain `lsof -F n` costs 16 s instead of 0.2 s (name
    /// resolution per socket), so the argv is pinned, and the allow-list
    /// must accept exactly this shape (a refusal would make every anchor
    /// `Unknown`).
    #[test]
    fn snapshot_argv_disables_name_resolution_and_is_allow_listed() {
        assert!(SNAPSHOT_ARGS.contains(&"-n"), "{SNAPSHOT_ARGS:?}");
        assert!(SNAPSHOT_ARGS.contains(&"-P"), "{SNAPSHOT_ARGS:?}");
        let (r, counted) = crate::work_counters::measured(|| {
            crate::fs_gate::spawn::run(
                crate::fs_gate::spawn::Program::Lsof,
                ["-F", "n"],
                Duration::from_secs(5),
            )
        });
        assert!(r.is_err(), "plain -F n is no longer an allowed shape");
        assert_eq!(counted.subprocess_spawns, 0);
    }

    fn probe(code: Option<i32>, stdout: &str, stderr: &str) -> OccupancyState {
        classify_lsof_exit(code, stdout, stderr, Path::new("/x"))
    }

    #[test]
    fn lsof_positive_match_is_occupied() {
        assert!(matches!(
            probe(Some(0), "COMMAND PID\nswamp 1 /x", ""),
            OccupancyState::Occupied(_)
        ));
    }

    #[test]
    fn lsof_no_match_is_free() {
        assert_eq!(probe(Some(1), "", ""), OccupancyState::Free);
    }

    #[test]
    fn lsof_permission_denied_is_unknown_not_free() {
        // The tempting shortcut this rejects: treating a query that
        // could not run the same as "checked, found nothing". A sink
        // refuses on Unknown, so this distinction is the difference
        // between refusing and deleting.
        let state = probe(Some(1), "", "lsof: WARNING: Permission denied");
        assert!(matches!(state, OccupancyState::Unknown(_)), "{state:?}");
        assert!(!matches!(state, OccupancyState::Free));
    }

    #[test]
    fn an_unexpected_lsof_exit_code_is_unknown() {
        assert!(matches!(probe(Some(9), "", ""), OccupancyState::Unknown(_)));
        assert!(matches!(probe(None, "", ""), OccupancyState::Unknown(_)));
    }

    #[test]
    fn a_free_probe_is_the_only_state_that_permits_an_action() {
        assert!(matches!(OccupancyState::Free, OccupancyState::Free));
        assert!(!matches!(
            OccupancyState::Occupied(PathBuf::from("/x")),
            OccupancyState::Free
        ));
        assert!(!matches!(
            OccupancyState::Unknown("nope".into()),
            OccupancyState::Free
        ));
    }

    #[test]
    fn docker_running_container_detected() {
        let containers = vec![crate::docker::ContainerRef {
            name: "web-1".into(),
            state: "running".into(),
            finished_at: None,
        }];
        let ev = docker_running_container_evidence(&containers);
        assert!(matches!(
            ev.status,
            FactStatus::Known(FactValue::Bool(true))
        ));
    }

    #[test]
    fn docker_exited_container_is_known_false_not_no_consumer() {
        let containers = vec![crate::docker::ContainerRef {
            name: "web-1".into(),
            state: "exited".into(),
            finished_at: Some("2026-01-01T00:00:00Z".into()),
        }];
        let ev = docker_running_container_evidence(&containers);
        assert!(matches!(
            ev.status,
            FactStatus::Known(FactValue::Bool(false))
        ));
    }

    #[test]
    fn docker_no_containers_recorded_is_unknown_not_known_false() {
        let ev = docker_running_container_evidence(&[]);
        assert!(matches!(ev.status, FactStatus::Unknown { .. }));
    }

    #[test]
    fn manager_lock_absent_file_is_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = tmp.path().join("does-not-exist.lock");
        let ev = manager_lock_evidence("cargo", &lock);
        assert!(matches!(ev.status, FactStatus::Unknown { .. }));
    }

    #[test]
    fn manager_lock_free_is_known_false() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = tmp.path().join("package-cache");
        std::fs::write(&lock, b"").unwrap();
        let ev = manager_lock_evidence("cargo", &lock);
        assert!(matches!(
            ev.status,
            FactStatus::Known(FactValue::Bool(false))
        ));
    }

    #[test]
    fn manager_lock_held_is_known_true() {
        use std::os::unix::io::AsRawFd;
        let tmp = tempfile::tempdir().unwrap();
        let lock = tmp.path().join("daemon.lock");
        std::fs::write(&lock, b"").unwrap();
        let held = std::fs::File::open(&lock).unwrap();
        let rc = unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(rc, 0, "test setup must acquire the lock first");
        let ev = manager_lock_evidence("gradle", &lock);
        assert!(matches!(
            ev.status,
            FactStatus::Known(FactValue::Bool(true))
        ));
        drop(held);
    }

    #[test]
    fn find_simulator_state_matches_udid() {
        let json = r#"{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-17-0":[
            {"udid":"AAAA","state":"Booted","name":"iPhone 15"},
            {"udid":"BBBB","state":"Shutdown","name":"iPhone 14"}
        ]}}"#;
        assert_eq!(
            find_simulator_state(json, "AAAA").as_deref(),
            Some("Booted")
        );
        assert_eq!(
            find_simulator_state(json, "BBBB").as_deref(),
            Some("Shutdown")
        );
        assert_eq!(find_simulator_state(json, "ZZZZ"), None);
    }

    #[test]
    fn simulator_booted_evidence_positive_match() {
        let json = r#"{"devices":{"iOS-17":[{"udid":"AAAA","state":"Booted"}]}}"#;
        let runner = Arc::new(crate::locations::FakeCommandRunner::new().with_answer(
            "xcrun",
            &["simctl", "list", "devices", "-j"],
            json,
        ));
        let env = crate::locations::Environment::fixture(
            std::path::PathBuf::from("/home/dev"),
            std::collections::HashMap::new(),
            crate::locations::Platform::MacOS,
        )
        .with_runner(runner);
        let ev = simulator_booted_evidence("AAAA", &env);
        assert!(matches!(
            ev.status,
            FactStatus::Known(FactValue::Bool(true))
        ));
    }

    #[test]
    fn simulator_booted_evidence_unavailable_on_query_failure() {
        let runner = Arc::new(crate::locations::FakeCommandRunner::new().with_failure(
            "xcrun",
            &["simctl", "list", "devices", "-j"],
            "xcrun not found",
        ));
        let env = crate::locations::Environment::fixture(
            std::path::PathBuf::from("/home/dev"),
            std::collections::HashMap::new(),
            crate::locations::Platform::MacOS,
        )
        .with_runner(runner);
        let ev = simulator_booted_evidence("AAAA", &env);
        assert!(matches!(ev.status, FactStatus::Unavailable { .. }));
    }

    #[test]
    fn simulator_booted_evidence_unknown_udid_is_unknown_never_assumed_shutdown() {
        let json = r#"{"devices":{"iOS-17":[{"udid":"OTHER","state":"Booted"}]}}"#;
        let runner = Arc::new(crate::locations::FakeCommandRunner::new().with_answer(
            "xcrun",
            &["simctl", "list", "devices", "-j"],
            json,
        ));
        let env = crate::locations::Environment::fixture(
            std::path::PathBuf::from("/home/dev"),
            std::collections::HashMap::new(),
            crate::locations::Platform::MacOS,
        )
        .with_runner(runner);
        let ev = simulator_booted_evidence("AAAA", &env);
        assert!(matches!(ev.status, FactStatus::Unknown { .. }));
    }

    #[test]
    fn all_current_use_facts_expire_and_are_not_trusted_forever() {
        let ev = manager_lock_evidence("cargo", Path::new("/nowhere/does-not-exist"));
        let ev = Evidence::known(
            FactKind::CurrentUse,
            FactSubtype::OpenFile,
            FactValue::Bool(true),
            EvidenceSource::ProcessQuery {
                tool: "lsof".into(),
            },
            ev.observed_at,
        )
        .with_freshness(Freshness::expires_after(CURRENT_USE_EXPIRY_SECS));
        assert!(!ev.is_stale(ev.observed_at + CURRENT_USE_EXPIRY_SECS));
        assert!(ev.is_stale(ev.observed_at + CURRENT_USE_EXPIRY_SECS + 1));
    }

    // ---- procfs (#86): the fail-closed rules against fixture trees ----

    struct FakeProc {
        _tmp: tempfile::TempDir,
        root: std::path::PathBuf,
        work: std::path::PathBuf,
    }

    /// "This user" in the fixtures is the real one: whether the kernel
    /// withholds a process is read from who owns its fixture entries,
    /// and the test's files are owned by whoever runs it.
    fn uid() -> u32 {
        unsafe { libc::getuid() }
    }
    const MY_PID: u32 = 4242;

    fn me() -> Creds {
        Creds {
            uids: [uid(); 4],
            gids: [uid(); 4],
            cap_prm: 0,
        }
    }

    fn fake_proc() -> FakeProc {
        let tmp = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(tmp.path()).unwrap();
        let root = base.join("proc");
        let work = base.join("work");
        std::fs::create_dir_all(work.join("target/debug")).unwrap();
        std::fs::write(work.join("target/debug/app"), b"x").unwrap();
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(MY_PID.to_string(), root.join("self")).unwrap();
        FakeProc {
            _tmp: tmp,
            root,
            work,
        }
    }

    fn process(
        fp: &FakeProc,
        pid: u32,
        owner: u32,
        cwd: &Path,
        fds: &[&Path],
    ) -> std::path::PathBuf {
        let (uid, me) = (owner, uid());
        let d = fp.root.join(pid.to_string());
        std::fs::create_dir_all(d.join("fd")).unwrap();
        std::fs::write(
            d.join("status"),
            format!(
                "Name:\tx\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\nGid:\t{me}\t{me}\t{me}\t{me}\nCapPrm:\t0000000000000000\n"
            ),
        )
        .unwrap();
        std::fs::write(d.join("comm"), "fixture\n").unwrap();
        std::os::unix::fs::symlink(cwd, d.join("cwd")).unwrap();
        std::os::unix::fs::symlink("/", d.join("root")).unwrap();
        std::os::unix::fs::symlink("/usr/bin/fixture", d.join("exe")).unwrap();
        for (i, f) in fds.iter().enumerate() {
            std::os::unix::fs::symlink(f, d.join("fd").join(i.to_string())).unwrap();
        }
        std::fs::write(d.join("maps"), "").unwrap();
        d
    }

    fn proc_probe(fp: &FakeProc) -> OccupancyState {
        let target = fp.work.join("target");
        procfs_probe(&fp.root, &[&target], &me(), MY_PID, Duration::from_secs(10))
    }

    #[test]
    fn procfs_a_process_holding_a_descendant_file_or_cwd_is_occupied() {
        let fp = fake_proc();
        process(&fp, 10, uid(), Path::new("/"), &[Path::new("/dev/null")]);
        assert_eq!(proc_probe(&fp), OccupancyState::Free);

        let app = fp.work.join("target/debug/app");
        process(&fp, 11, uid(), Path::new("/"), &[&app]);
        assert_eq!(proc_probe(&fp), OccupancyState::Occupied(app));

        let fp = fake_proc();
        let cwd = fp.work.join("target/debug");
        process(&fp, 12, uid(), &cwd, &[]);
        assert_eq!(proc_probe(&fp), OccupancyState::Occupied(cwd));

        let fp = fake_proc();
        let d = process(&fp, 13, uid(), Path::new("/"), &[]);
        let app = fp.work.join("target/debug/app");
        std::fs::write(
            d.join("maps"),
            format!("7f00-7f01 r-xp 00000000 08:01 1234  {}\n", app.display()),
        )
        .unwrap();
        assert_eq!(
            proc_probe(&fp),
            OccupancyState::Occupied(app),
            "a mapped file counts"
        );
    }

    /// A deleted-but-open file under the anchor still holds its space.
    #[test]
    fn procfs_a_deleted_open_file_under_the_anchor_still_counts() {
        let fp = fake_proc();
        let gone = format!("{}/target/debug/old.o (deleted)", fp.work.display());
        process(&fp, 20, uid(), Path::new("/"), &[Path::new(&gone)]);
        assert!(matches!(proc_probe(&fp), OccupancyState::Occupied(_)));
    }

    /// Another user's process is outside the question; one of *ours*
    /// whose fd table cannot be read is `Unknown`, never `Free`.
    #[test]
    fn procfs_an_unreadable_process_of_this_user_is_unknown_not_free() {
        if unsafe { libc::getuid() } == 0 {
            eprintln!("SKIP procfs_an_unreadable_process: root ignores the mode bits");
            return;
        }
        let fp = fake_proc();
        let d = process(&fp, 30, uid() + 1, Path::new("/"), &[]);
        std::fs::set_permissions(d.join("fd"), std::fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(
            proc_probe(&fp),
            OccupancyState::Free,
            "another user's process is not read"
        );
        std::fs::set_permissions(d.join("fd"), std::fs::Permissions::from_mode(0o700)).unwrap();

        let fp = fake_proc();
        let d = process(&fp, 31, uid(), Path::new("/"), &[]);
        std::fs::set_permissions(d.join("fd"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let got = proc_probe(&fp);
        std::fs::set_permissions(d.join("fd"), std::fs::Permissions::from_mode(0o700)).unwrap();
        match got {
            OccupancyState::Unknown(why) => {
                assert!(why.contains("31") && why.contains("open files"), "{why}")
            }
            other => panic!("a same-user process we cannot read must be Unknown: {other:?}"),
        }
    }

    /// The kernel's own read rule decides who is in the question: a
    /// process running with more privilege than this user -- a saved uid
    /// of root (a setuid program), a permitted capability -- is not
    /// readable by this user and is outside the question, like another
    /// user's; one with exactly this user's credentials that still
    /// cannot be read is `Unknown`.
    #[test]
    fn procfs_privilege_not_the_uid_alone_decides_who_is_in_scope() {
        if unsafe { libc::getuid() } == 0 {
            eprintln!("SKIP procfs_privilege: root ignores the mode bits");
            return;
        }
        for status in [
            format!(
                "Uid:\t{u}\t{u}\t0\t{u}\nGid:\t{u}\t{u}\t{u}\t{u}\nCapPrm:\t0\n",
                u = uid()
            ),
            format!(
                "Uid:\t{u}\t{u}\t{u}\t{u}\nGid:\t{u}\t{u}\t{u}\t{u}\nCapPrm:\t0000000000200000\n",
                u = uid()
            ),
        ] {
            let fp = fake_proc();
            let d = process(&fp, 60, uid(), Path::new("/"), &[]);
            std::fs::write(d.join("status"), &status).unwrap();
            std::fs::set_permissions(d.join("fd"), std::fs::Permissions::from_mode(0o000)).unwrap();
            let got = proc_probe(&fp);
            std::fs::set_permissions(d.join("fd"), std::fs::Permissions::from_mode(0o700)).unwrap();
            assert_eq!(
                got,
                OccupancyState::Free,
                "a more-privileged process is not read: {status}"
            );
        }
        assert!(
            parse_status("Uid:\t1\t2\t3\t4\nGid:\t5\t6\t7\t8\nCapPrm:\t00000000000000ff\n")
                .is_some_and(|c| c.uids == [1, 2, 3, 4]
                    && c.gids == [5, 6, 7, 8]
                    && c.cap_prm == 0xff)
        );
    }

    /// A same-credential process whose procfs entries turned root-owned
    /// (non-dumpable) is withheld by the kernel and outside the question;
    /// one whose entries are still ours and unreadable is not.
    #[test]
    fn procfs_the_kernels_non_dumpable_boundary_is_read_from_ownership() {
        assert!(
            withheld_owner(0, 1000),
            "root-owned entries: the kernel withholds it"
        );
        assert!(
            !withheld_owner(1000, 1000),
            "our own entries, unreadable: Unknown, not withheld"
        );
    }

    #[test]
    fn procfs_a_foreign_pid_namespace_or_missing_proc_is_unknown() {
        let fp = fake_proc();
        std::fs::remove_file(fp.root.join("self")).unwrap();
        std::os::unix::fs::symlink("1", fp.root.join("self")).unwrap();
        match proc_probe(&fp) {
            OccupancyState::Unknown(why) => assert!(why.contains("namespace"), "{why}"),
            other => panic!("{other:?}"),
        }
        let fp = fake_proc();
        let target = fp.work.join("target");
        let got = procfs_probe(
            &fp.root.join("missing"),
            &[&target],
            &me(),
            MY_PID,
            Duration::from_secs(10),
        );
        assert!(matches!(got, OccupancyState::Unknown(_)), "{got:?}");
    }

    /// A process that exits mid-scan holds nothing; its vanished entries
    /// are not an error and not a reason to refuse.
    #[test]
    fn procfs_a_process_that_exited_mid_scan_is_skipped() {
        let fp = fake_proc();
        let d = fp.root.join("40");
        std::fs::create_dir_all(&d).unwrap();
        // status present, everything else gone: exited after listing.
        std::fs::write(
            d.join("status"),
            format!(
                "Uid:\t{u}\t{u}\t{u}\t{u}\nGid:\t{u}\t{u}\t{u}\t{u}\n",
                u = uid()
            ),
        )
        .unwrap();
        assert_eq!(proc_probe(&fp), OccupancyState::Free);
    }

    #[test]
    fn procfs_past_its_time_bound_is_unknown() {
        let fp = fake_proc();
        process(&fp, 50, uid(), Path::new("/"), &[]);
        let target = fp.work.join("target");
        let got = procfs_probe(&fp.root, &[&target], &me(), MY_PID, Duration::ZERO);
        assert!(
            matches!(got, OccupancyState::Unknown(ref w) if w.contains("did not finish")),
            "{got:?}"
        );
    }

    // ---- OccupancySnapshot -------------------------------------------

    fn snap(stdout: &str) -> OccupancySnapshot {
        OccupancySnapshot::from_lsof_run(Some(0), false, stdout, "")
    }

    fn anchor_dir(root: &Path, name: &str) -> PathBuf {
        let d = root.join(name);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn snapshot_matches_anchor_and_descendants_but_not_shared_prefix_siblings() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let target = anchor_dir(&root, "target");
        let target2 = anchor_dir(&root, "target2");
        let other = anchor_dir(&root, "other");
        let open_file = target.join("debug/x.o");
        let listing = format!(
            "p10\nfcwd\nn{}\nf3\nn{}\np11\nf4\nn/dev/null\nn*:8080\n",
            other.join("gone-not-under-target").display(),
            open_file.display()
        );
        let s = snap(&listing);
        // descendant of the anchor
        assert_eq!(s.state_for(&target), OccupancyState::Occupied(open_file));
        // sibling sharing a string prefix must not match
        assert_eq!(s.state_for(&target2), OccupancyState::Free);
        assert!(matches!(s.state_for(&other), OccupancyState::Occupied(_)));
    }

    #[test]
    fn snapshot_matches_the_anchor_itself_and_cwd_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let dir = anchor_dir(&root, "proj");
        let s = snap(&format!("p1\nfcwd\nn{}\n", dir.display()));
        assert_eq!(s.state_for(&dir), OccupancyState::Occupied(dir.clone()));
    }

    #[test]
    fn snapshot_of_a_vanished_anchor_is_free_like_the_per_path_probe() {
        let s = snap("p1\nn/somewhere\n");
        assert_eq!(
            s.state_for(Path::new("/definitely/not/here")),
            OccupancyState::Free
        );
    }

    #[test]
    fn snapshot_free_only_from_a_complete_listing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let dir = anchor_dir(&root, "d");
        assert_eq!(
            snap("p1\nn/elsewhere\n").state_for(&dir),
            OccupancyState::Free
        );
    }

    #[test]
    fn snapshot_fails_closed_on_timeout_signal_garbage_and_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let dir = anchor_dir(&root, "d");
        let cases = [
            OccupancySnapshot::from_lsof_run(None, true, "p1\nn/x\n", ""),
            OccupancySnapshot::from_lsof_run(None, false, "p1\nn/x\n", ""),
            OccupancySnapshot::from_lsof_run(Some(2), false, "p1\nn/x\n", ""),
            OccupancySnapshot::from_lsof_run(Some(0), false, "\u{1}garbage\n", ""),
            OccupancySnapshot::from_lsof_run(Some(0), false, "not lsof output at all\n", ""),
            OccupancySnapshot::from_lsof_run(Some(0), false, "", ""),
            OccupancySnapshot::from_lsof_run(Some(1), false, "", ""),
        ];
        for (i, s) in cases.iter().enumerate() {
            assert!(
                matches!(s.state_for(&dir), OccupancyState::Unknown(_)),
                "case {i} must be Unknown, got {:?}",
                s.state_for(&dir)
            );
        }
    }

    #[test]
    fn snapshot_permission_denied_partial_listing_is_never_free() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let dir = anchor_dir(&root, "d");
        let busy = anchor_dir(&root, "busy");
        let listing = format!("p1\nn{}\n", busy.join("f").display());
        for code in [0, 1] {
            let s = OccupancySnapshot::from_lsof_run(
                Some(code),
                false,
                &listing,
                "lsof: WARNING: can't stat() x\nlsof: Permission denied\n",
            );
            assert!(matches!(s.state_for(&dir), OccupancyState::Unknown(_)));
            // A positive find is still a positive find.
            assert!(matches!(s.state_for(&busy), OccupancyState::Occupied(_)));
        }
    }

    /// N anchors, one `lsof`: the review-pass cost is O(open files), not
    /// O(groups x tree). Counted through the gate's own spawn counter, so
    /// there is no process-wide PATH fake to race sibling tests.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn n_anchors_in_a_scope_cost_exactly_one_lsof_spawn() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let anchors: Vec<PathBuf> = (0..40)
            .map(|i| anchor_dir(&root, &format!("g{i}")))
            .collect();
        let (_, counted) = crate::work_counters::measured(|| {
            OccupancySnapshot::scoped(|| {
                for a in &anchors {
                    let _ = open_file_evidence(a);
                }
            })
        });
        assert_eq!(counted.subprocess_spawns, 1);
        // Outside a scope the old per-path probe is unchanged: one each.
        let (_, counted) = crate::work_counters::measured(|| {
            for a in anchors.iter().take(3) {
                let _ = open_file_evidence(a);
            }
        });
        assert_eq!(counted.subprocess_spawns, 3);
    }
    /// Tool removal counts every holder but the manager's own host
    /// process (exact name, executable where CoreSimulator keeps it; the
    /// same name elsewhere blocks), and an incomplete listing is `Unknown`, never `Free`.
    #[test]
    fn tool_removal_holders_skip_only_the_managers_own_process() {
        let listing = "p1\ncSimLaunchHost.arm64\nn/Library/Developer/PrivateFrameworks/CoreSimulator.framework/Versions/A/XPCServices/SimLaunchHost.arm64.xpc/Contents/MacOS/SimLaunchHost.arm64\nn/vol/iOS/lib.dylib\np2\ncnode\nn/m/node/24/bin/node\n";
        let snap = OccupancySnapshot::from_lsof_run(Some(0), false, listing, "");
        let (s, who) = snap.holders_of(&[PathBuf::from("/vol/iOS")], &["SimLaunchHost.arm64"]);
        assert_eq!((s, who), (OccupancyState::Free, None));
        let impostor = "p3\ncSimLaunchHost.arm64\nn/tmp/SimLaunchHost.arm64\nn/vol/iOS/lib.dylib\n";
        let fake = OccupancySnapshot::from_lsof_run(Some(0), false, impostor, "");
        assert!(matches!(
            fake.holders_of(&[PathBuf::from("/vol/iOS")], &["SimLaunchHost.arm64"])
                .0,
            OccupancyState::Occupied(_)
        ));
        let (s, who) = snap.holders_of(&[PathBuf::from("/m/node/24")], &["SimLaunchHost"]);
        assert_eq!(
            s,
            OccupancyState::Occupied(PathBuf::from("/m/node/24/bin/node"))
        );
        assert_eq!(who.as_deref(), Some("node"));
        let partial =
            OccupancySnapshot::from_lsof_run(Some(1), false, listing, "lsof: Permission denied");
        let (s, _) = partial.holders_of(&[PathBuf::from("/vol/iOS")], &["SimLaunchHost.arm64"]);
        assert!(matches!(s, OccupancyState::Unknown(_)), "{s:?}");
        let failed = OccupancySnapshot::from_lsof_run(None, true, "", "");
        assert!(matches!(
            failed.holders_of(&[PathBuf::from("/x")], &[]).0,
            OccupancyState::Unknown(_)
        ));
    }
}

/// Adversarial audit G5 (#177): the manager's-own exemption is exact.
#[cfg(test)]
mod adv_g5_tests {
    use super::*;
    use std::path::PathBuf;

    /// Tempting wrong patch: "`starts_with` because lsof prints
    /// `SimLaunchHost.arm64`". A process merely named like it
    /// (`SimLaunchHostX`, `SimLaunchHost-evil`) must still block.
    #[test]
    fn adv_simlaunchhost_exemption_is_not_a_prefix_match() {
        for name in ["SimLaunchHostX", "SimLaunchHost-evil", "SimLaunchHostile"] {
            let listing = format!("p1\nc{name}\nn/vol/iOS/lib.dylib\n");
            let snap = OccupancySnapshot::from_lsof_run(Some(0), false, &listing, "");
            let (s, _) = snap.holders_of(&[PathBuf::from("/vol/iOS")], &["SimLaunchHost"]);
            assert!(
                matches!(s, OccupancyState::Occupied(_)),
                "{name} was treated as simctl's own host process: {s:?}"
            );
        }
    }

    /// A holder with no `c` line (command unknown) must block.
    #[test]
    fn adv_holder_without_command_name_blocks() {
        let snap =
            OccupancySnapshot::from_lsof_run(Some(0), false, "p1\nn/vol/iOS/lib.dylib\n", "");
        let (s, _) = snap.holders_of(&[PathBuf::from("/vol/iOS")], &["SimLaunchHost"]);
        assert!(matches!(s, OccupancyState::Occupied(_)), "{s:?}");
    }
}
