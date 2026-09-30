//! Tool-managed removal (#177): an install that Trash would break (a mise
//! tool version, a simulator runtime) is removed by its own manager's
//! command, permanently. There is no Trash to fall back on, so this one
//! action class keeps automated refusals and a review-to-Enter recheck
//! (`.oh/guardrails/tool-removal-refuses-on-manager-facts.md`); every
//! other removal in swamp is the plain human-decides Trash move.
//!
//! The flow, all of it on TUI worker threads (`worker::spawn`), started
//! only by a key press, never on open:
//!
//! 1. [`read_candidates`]: the manager's own listing (`mise ls --json`,
//!    `simctl runtime list -j`) and, for mise, what its prune reports.
//! 2. [`review_target`]: live state again, swamp's refusals (a version a
//!    config requests, a booted simulator, files held open or not
//!    checkable), then the manager's **own dry run** with the exact argv
//!    that would run plus its dry-run flag. The result is a [`Preview`]
//!    (the confirm) or a [`Refusal`] (reason and next step).
//! 3. [`execute`] (the TUI's Enter, and only the TUI: the gate audit):
//!    the whole review runs again; anything that changed since the
//!    preview refuses. Then exactly `preview`'s argv runs through
//!    `fs_gate::destroy::tool_remove`, live state is read back, and one
//!    `tool-remove` ledger record says what was observed, never more.
//!
//! Managers with no dry run (`brew uninstall`, `rustup toolchain
//! uninstall`) are not here at all: those rows keep showing facts only.

mod mise;
mod simctl;

use crate::fs_gate::spawn::{Program, RunOutput, ToolBin, ToolResolver};
use crate::ledger::{ActionRecord, Ledger, LedgerFact, NO_GRANT, Verb};
use crate::occupancy::OccupancyState;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a listing, a version query or a dry run may take.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a removal may take before it is killed and reported
/// `unknown` (never retried: a killed manager can leave a half-removed
/// install, and the next review starts from live state).
const EXEC_TIMEOUT: Duration = Duration::from_secs(600);
/// How many lines of a manager's output a preview keeps for display.
pub const SHOWN_OUTPUT_LINES: usize = 200;

/// A manager swamp can remove through: declared by the detector whose
/// location holds its installs (`Detector::tool_managed`).
pub use crate::locations::ToolManager as Manager;

/// The manager that removes installs under an external unit's `path`,
/// reported by `detector_id`; `None` for everything Trash handles.
pub fn manager_for_unit(detector_id: &str, path: &Path) -> Option<Manager> {
    crate::locations::tool_manager_of(detector_id, path)
}

impl Manager {
    /// The manager's own name, as the confirm says it.
    pub fn name(self) -> &'static str {
        match self {
            Manager::Mise => "mise",
            Manager::Simulator => "simctl",
        }
    }

    fn program(self) -> Program {
        match self {
            Manager::Mise => Program::Mise,
            Manager::Simulator => Program::Xcrun,
        }
    }

    /// The manager version swamp's fixtures and tests were checked
    /// against; a different one is a warning on the confirm.
    pub fn verified_version(self) -> &'static str {
        match self {
            Manager::Mise => mise::VERIFIED_VERSION,
            Manager::Simulator => simctl::VERIFIED_VERSION,
        }
    }
}

/// What a removal acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// One installed mise tool version.
    MiseVersion { tool: String, version: String },
    /// One simulator runtime image, by the UUID `simctl runtime list -j`
    /// keys it by.
    SimRuntime { uuid: String },
}

/// One row of a manager's listing, offered for review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub target: Target,
    /// `go@1.23.5`, `iOS 26.2 (23C54)`.
    pub label: String,
    /// Facts from the listing, attributed (`requested by
    /// ~/.config/mise/config.toml`, `mise reports prunable`, `8.4 GB,
    /// simctl's sizeBytes`).
    pub facts: String,
    /// The install directory, when the listing names one (for swamp's
    /// stored size of it).
    pub path: Option<PathBuf>,
}

/// A manager's listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub manager: Manager,
    pub candidates: Vec<Candidate>,
}

/// Why swamp will not run a removal, and what the human can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub reason: String,
    pub next: String,
    /// The manager's dry-run output, when one ran (shown verbatim, so the
    /// human sees what swamp read).
    pub output: Vec<String>,
}

impl Refusal {
    fn new(reason: impl Into<String>, next: impl Into<String>) -> Self {
        Refusal {
            reason: reason.into(),
            next: next.into(),
            output: Vec::new(),
        }
    }

    fn with_output(mut self, output: Vec<String>) -> Self {
        self.output = output;
        self
    }
}

/// A size the confirm shows, with where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Size {
    pub bytes: u64,
    /// `simctl's sizeBytes`, `swamp's stored measurement`.
    pub source: &'static str,
}

/// Everything the confirm shows, and exactly what Enter runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    manager: Manager,
    target: Target,
    bin: ToolBin,
    exec_argv: Vec<OsString>,
    dry_argv: Vec<OsString>,
    dry_output: Vec<String>,
    dry_output_digest: String,
    removes: Vec<String>,
    /// What the manager's list is re-read for afterwards (`t@v`, UUID).
    listed_as: Vec<String>,
    state: Vec<(String, String)>,
    size: Option<Size>,
    regen: String,
    evidence: Vec<String>,
    warnings: Vec<String>,
    open_files: String,
    manager_version: String,
    title: String,
}

impl Preview {
    pub fn manager(&self) -> Manager {
        self.manager
    }

    pub fn target(&self) -> &Target {
        &self.target
    }

    /// What is removed, for the confirm's first line (`go@1.23.5`).
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The exact command Enter runs, one line, shell-quoted from the same
    /// argv that is spawned ([`command_line`]).
    pub fn command_line(&self) -> String {
        command_line(self.manager_command(), &self.exec_argv)
    }

    /// The canonical executable that runs.
    pub fn program_path(&self) -> &Path {
        self.bin.path()
    }

    fn manager_command(&self) -> &'static str {
        self.manager.program().binary()
    }

    /// The manager's dry-run output, control characters stripped, at most
    /// [`SHOWN_OUTPUT_LINES`] lines (the last says how many more).
    pub fn dry_output(&self) -> &[String] {
        &self.dry_output
    }

    /// The paths (mise) or runtime ids (simctl) the dry run named.
    pub fn removes(&self) -> &[String] {
        &self.removes
    }

    pub fn size(&self) -> Option<&Size> {
        self.size.as_ref()
    }

    /// Regeneration cost, in the manager's terms.
    pub fn regen(&self) -> &str {
        &self.regen
    }

    /// Consumers and the manager's own verdicts, quoted and attributed.
    pub fn evidence(&self) -> &[String] {
        &self.evidence
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The open-file check's answer, as a fact (`none held (lsof)`).
    pub fn open_files(&self) -> &str {
        &self.open_files
    }
}

/// How tool removal reaches the machine: the manager resolver and the
/// open-file check. Production is [`Host::system`]; tests use a sandbox
/// of fake managers (`testing` builds only).
#[derive(Debug, Clone)]
pub struct Host {
    resolver: ToolResolver,
    open_files: OpenFiles,
    exec_timeout: Duration,
    /// A test sandbox's own store: its ledger is never the real one.
    sandbox_store: Option<PathBuf>,
}

#[derive(Debug, Clone)]
enum OpenFiles {
    /// The machine-wide `lsof` listing (procfs on Linux).
    System,
    /// A test's fixed answer.
    #[cfg(any(test, feature = "testing"))]
    Fixed(OccupancyState, Option<String>),
}

impl Host {
    /// Managers from the fixed candidate list, open files from `lsof`.
    pub fn system() -> Self {
        let resolver = ToolResolver::system();
        // Only a test build's SWAMP_TEST_TOOL_SANDBOX makes this a
        // sandbox; its ledger is then the sandbox's own.
        let sandbox_store = resolver.sandbox().map(|s| s.join("store"));
        Host {
            resolver,
            open_files: OpenFiles::System,
            exec_timeout: EXEC_TIMEOUT,
            sandbox_store,
        }
    }

    /// Fake managers under `dir/bin`, `dir/home` as their `HOME`, and an
    /// open-file check that finds nothing held. Test builds only.
    #[cfg(any(test, feature = "testing"))]
    pub fn sandboxed(dir: &Path) -> Self {
        Host {
            resolver: ToolResolver::sandboxed(dir),
            open_files: OpenFiles::Fixed(OccupancyState::Free, None),
            exec_timeout: EXEC_TIMEOUT,
            sandbox_store: Some(dir.join("store")),
        }
    }

    /// A shorter removal deadline, for a test's hanging fake. Test builds
    /// only.
    #[cfg(any(test, feature = "testing"))]
    pub fn with_exec_timeout(mut self, timeout: Duration) -> Self {
        self.exec_timeout = timeout;
        self
    }

    /// The environment the children's is built from, for a test that
    /// poisons it. Test builds only.
    #[cfg(any(test, feature = "testing"))]
    pub fn with_parent_env(mut self, vars: &[(&str, &str)]) -> Self {
        self.resolver = self.resolver.with_parent_env(
            vars.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        );
        self
    }

    /// The open-file check could not finish. Test builds only.
    #[cfg(any(test, feature = "testing"))]
    pub fn with_open_files_unknown(mut self, why: &str) -> Self {
        self.open_files = OpenFiles::Fixed(OccupancyState::Unknown(why.to_string()), None);
        self
    }

    /// `command` holds `path` open. Test builds only.
    #[cfg(any(test, feature = "testing"))]
    pub fn with_open_file_held(mut self, path: &Path, command: &str) -> Self {
        self.open_files = OpenFiles::Fixed(
            OccupancyState::Occupied(path.to_path_buf()),
            Some(command.to_string()),
        );
        self
    }

    /// The ledger a removal is recorded in: `store`'s, or a test
    /// sandbox's own (`dir/store`), so a test never writes the real one.
    pub fn ledger_in(&self, store: &crate::fs_gate::StoreDir) -> Ledger {
        if let Some(dir) = &self.sandbox_store {
            // A test build must never write the real ledger.
            return Ledger::open(dir.join("ledger.parquet")).unwrap_or_else(|e| {
                panic!(
                    "test build: the sandbox ledger under {} did not open: {e}",
                    dir.display()
                )
            });
        }
        if self.resolver.sandbox().is_some() {
            panic!("test build: a sandboxed resolver never records in the real ledger");
        }
        Ledger::resolved(store)
    }

    fn home(&self) -> PathBuf {
        self.resolver
            .home()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"))
    }

    fn resolve(&self, manager: Manager) -> Result<ToolBin, Refusal> {
        self.resolver.resolve(manager.program()).map_err(|why| {
            Refusal::new(
                why,
                format!("Remove it with {} yourself if you mean to.", manager.name()),
            )
        })
    }

    fn open_state(
        &self,
        paths: &[PathBuf],
        manager_own: &[&str],
    ) -> (OccupancyState, Option<String>) {
        match &self.open_files {
            OpenFiles::System => {
                crate::occupancy::OccupancySnapshot::tool_removal_state(paths, manager_own)
            }
            #[cfg(any(test, feature = "testing"))]
            OpenFiles::Fixed(state, who) => (state.clone(), who.clone()),
        }
    }
}

/// The open-file answer as a fact for the confirm, or the refusal it is.
/// `Unknown` refuses: there is no Trash to recover from a removal made
/// while something was running from it.
fn open_files_fact(
    host: &Host,
    paths: &[PathBuf],
    manager_own: &[&str],
    manager: Manager,
) -> Result<String, Refusal> {
    let (state, who) = host.open_state(paths, manager_own);
    match state {
        OccupancyState::Free => Ok(if manager_own.is_empty() {
            "none held (lsof, just now)".to_string()
        } else {
            format!(
                "none held apart from {}'s own {} (lsof, just now)",
                manager.name(),
                manager_own.join(", ")
            )
        }),
        OccupancyState::Occupied(path) => Err(Refusal::new(
            format!(
                "{} has {} open (lsof).",
                who.unwrap_or_else(|| "A process".to_string()),
                path.display()
            ),
            "Stop it, then review again.",
        )),
        OccupancyState::Unknown(why) => Err(Refusal::new(
            format!(
                "Open files could not be checked ({why}). Swamp does not remove without that check."
            ),
            format!(
                "Review again; if it keeps failing, remove it with {} yourself.",
                manager.name()
            ),
        )),
    }
}

/// The manager's listing, for the human to pick from. Runs the listing
/// commands only (and, for mise, `prune --tools --dry-run`).
pub fn read_candidates(host: &Host, manager: Manager) -> Result<Listing, Refusal> {
    let bin = host.resolve(manager)?;
    let candidates = match manager {
        Manager::Mise => mise::candidates(host, &bin)?,
        Manager::Simulator => simctl::candidates(&bin)?,
    };
    Ok(Listing {
        manager,
        candidates,
    })
}

/// Reviews one target: live state, swamp's refusals, the manager's own
/// dry run, the open-file check. `sizes` is swamp's stored measurement
/// of directories (install path, allocated bytes) for the size line.
pub fn review_target(
    host: &Host,
    target: &Target,
    sizes: &[(PathBuf, u64)],
) -> Result<Preview, Refusal> {
    let manager = match target {
        Target::MiseVersion { .. } => Manager::Mise,
        Target::SimRuntime { .. } => Manager::Simulator,
    };
    let bin = host.resolve(manager)?;
    let version = manager_version(&bin);
    let mut preview = match manager {
        Manager::Mise => mise::review(host, &bin, target, sizes)?,
        Manager::Simulator => simctl::review(host, &bin, target)?,
    };
    match &version {
        Some(v) if v == manager.verified_version() => {}
        Some(v) => preview.warnings.push(format!(
            "{} {v} is not the version swamp's tests were checked against ({}); read the dry run \
             below with that in mind.",
            manager.name(),
            manager.verified_version()
        )),
        None => preview.warnings.push(format!(
            "{}'s version could not be read; swamp's tests were checked against {}.",
            manager.name(),
            manager.verified_version()
        )),
    }
    preview.warnings.extend(bin.notes().iter().cloned());
    preview.manager_version = version.unwrap_or_else(|| "not read".to_string());
    Ok(preview)
}

fn manager_version(bin: &ToolBin) -> Option<String> {
    let out = read(bin, &["--version"]).ok()?;
    if !out.success() {
        return None;
    }
    let text = format!("{}{}", out.stdout_lossy(), out.stderr_lossy());
    match bin.program() {
        Program::Xcrun => simctl::parse_version(&text),
        _ => mise::parse_version(&text),
    }
}

/// What Enter did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Refused at Enter (live state changed, or a refusal that did not
    /// hold at review time holds now). Nothing ran.
    Refused(Refusal),
    /// Exit 0, and the re-read no longer lists the target.
    Removed,
    /// Non-zero exit, and the re-read no longer lists the target.
    RemovedWithError(Option<i32>),
    /// Non-zero exit, and the target is still listed.
    Failed(Option<i32>),
    /// Exit 0, but the target is still listed.
    StillListed,
    /// Killed at the timeout; the re-read says what is left.
    TimedOut,
    /// The removal ran (this exit code) but the manager's list could not
    /// be read back, so what happened was not observed.
    NotReread(Option<i32>),
    /// The removal could not be started (the spawn itself failed).
    NotStarted(String),
}

/// The honest account of one Enter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub status: Status,
    /// One plain line for the result.
    pub line: String,
    /// Whether the ledger record was written (`Err` says why not).
    pub recorded: Result<(), String>,
}

/// Enter on the confirm: re-reviews, refuses anything that changed since
/// `preview`, runs exactly `preview`'s argv, re-reads live state and
/// records what it observed. Callable only from the TUI (gate audit:
/// no CLI or agent path can reach a tool removal).
pub fn execute(
    host: &Host,
    preview: &Preview,
    sizes: &[(PathBuf, u64)],
    ledger: &Ledger,
) -> Outcome {
    // One tool removal at a time in this process: a second confirm waits,
    // then its re-review sees what the first did. Two swamp processes are
    // not locked against each other (documented).
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let fresh = review_target(host, &preview.target, sizes);
    let changed = match &fresh {
        Err(r) => Some(Refusal {
            reason: format!("Since review: {}", r.reason),
            next: r.next.clone(),
            output: r.output.clone(),
        }),
        Ok(now) => changed_since(preview, now),
    };
    if let Some(refusal) = changed {
        let line = format!("Nothing ran. {} {}", refusal.reason, refusal.next);
        let recorded = record(
            ledger,
            &crate::entities::new_id(),
            false,
            preview,
            &format!("refused:{}", refusal.reason),
            "not run",
            None,
        );
        return Outcome {
            status: Status::Refused(refusal),
            line,
            recorded,
        };
    }
    // A `started` row before anything runs: a removal cut short (the
    // terminal closed, swamp killed) still leaves a trace. The final row
    // replaces it. A ledger swamp cannot write at all means nothing runs.
    let id = crate::entities::new_id();
    let started = record(
        ledger,
        &id,
        false,
        preview,
        "started",
        "running; not re-read",
        None,
    );
    let mut ledger_note = None;
    if let Err(e) = &started {
        // Only an append that did write the row (into a new ledger, the
        // unreadable one kept aside) lets the removal go on.
        if !e.starts_with(crate::ledger::KEPT_ASIDE) {
            let refusal = Refusal::new(
                format!("swamp could not write its ledger ({e})."),
                "Fix the store directory, then review again.",
            );
            return Outcome {
                line: format!("Nothing ran. {} {}", refusal.reason, refusal.next),
                status: Status::Refused(refusal),
                recorded: started,
            };
        }
        ledger_note = Some(e.clone());
    }
    let run =
        crate::fs_gate::destroy::tool_remove(&preview.bin, &preview.exec_argv, host.exec_timeout);
    let after = still_listed(preview);
    let (status, outcome, observed) = match &run {
        Err(e) => (
            Status::NotStarted(e.to_string()),
            format!("failed:not started: {e}"),
            observed_label(preview, &after),
        ),
        Ok(out) => classify(out, &after, preview),
    };
    let mut line = result_line(
        preview,
        &status,
        run.as_ref().ok(),
        &after,
        host.exec_timeout.as_secs(),
    );
    let mut recorded = record(
        ledger,
        &id,
        true,
        preview,
        &outcome,
        &observed,
        run.as_ref().ok(),
    );
    if let Some(note) = ledger_note {
        line.push_str(&format!(" Note: {note}."));
        recorded = Err(note);
    }
    Outcome {
        status,
        line,
        recorded,
    }
}

/// The first fact that differs between the preview the human confirmed
/// and a fresh review: the argv, the dry run's targets, or a piece of
/// live state (a config source, a device, a size, a set).
fn changed_since(preview: &Preview, now: &Preview) -> Option<Refusal> {
    let again = "Review again: swamp runs only what you confirmed.";
    if now.exec_argv != preview.exec_argv || now.bin.path() != preview.bin.path() {
        return Some(Refusal::new(
            format!(
                "The command changed since review (now `{}`).",
                now.command_line()
            ),
            again,
        ));
    }
    if now.removes != preview.removes {
        return Some(Refusal::new(
            format!(
                "{}'s dry run names {} item(s) now, {} at review.",
                preview.manager.name(),
                now.removes.len(),
                preview.removes.len()
            ),
            again,
        ));
    }
    // The manager's whole dry-run text and its own reason lines, not only
    // what swamp parsed out of them: any change is a different preview.
    if now.dry_output_digest != preview.dry_output_digest {
        return Some(Refusal::new(
            format!(
                "{}'s dry run printed something different since review.",
                preview.manager.name()
            ),
            again,
        ));
    }
    if now.evidence != preview.evidence {
        return Some(Refusal::new(
            format!(
                "{}'s own reasons changed since review.",
                preview.manager.name()
            ),
            again,
        ));
    }
    for (key, was) in &preview.state {
        let is = now
            .state
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str());
        if is != Some(was.as_str()) {
            return Some(Refusal::new(
                format!(
                    "{key} changed since review (was {was}, now {}).",
                    is.unwrap_or("absent")
                ),
                again,
            ));
        }
    }
    if now.state.len() != preview.state.len() {
        return Some(Refusal::new(
            "The manager's list changed since review.",
            again,
        ));
    }
    None
}

/// Re-reads whether each target is still listed after the removal ran.
/// `Err` when the re-read itself failed (recorded as `not re-read`).
fn still_listed(preview: &Preview) -> Result<Vec<String>, String> {
    match preview.manager {
        Manager::Mise => mise::still_listed(&preview.bin, preview),
        Manager::Simulator => simctl::still_listed(&preview.bin, preview),
    }
}

fn observed_label(preview: &Preview, after: &Result<Vec<String>, String>) -> String {
    match after {
        Err(_) => "not re-read".to_string(),
        Ok(left) if left.is_empty() => {
            format!("removed via {}, no longer listed", preview.manager.name())
        }
        Ok(left) => format!("still listed: {}", left.join(", ")),
    }
}

fn classify(
    out: &RunOutput,
    after: &Result<Vec<String>, String>,
    preview: &Preview,
) -> (Status, String, String) {
    let observed = observed_label(preview, after);
    let gone = matches!(after, Ok(left) if left.is_empty());
    let present = matches!(after, Ok(left) if !left.is_empty());
    if out.timed_out {
        return (Status::TimedOut, "unknown:timed_out".to_string(), observed);
    }
    let code = out.code;
    let label = |c: Option<i32>| c.map_or_else(|| "signal".to_string(), |c| c.to_string());
    match (code == Some(0), gone, present) {
        (true, true, _) => (Status::Removed, "completed".to_string(), observed),
        (true, _, true) => (
            Status::StillListed,
            "failed:exit 0, still listed".to_string(),
            observed,
        ),
        (false, true, _) => (
            Status::RemovedWithError(code),
            format!("completed_with_error:{}", label(code)),
            observed,
        ),
        (false, _, true) => (
            Status::Failed(code),
            format!("failed:{}", label(code)),
            observed,
        ),
        // The re-read failed: say what the exit code was and that the
        // state was not observed, never a success or a failure.
        _ => (
            Status::NotReread(code),
            format!("unknown:exit {}, not re-read", label(code)),
            observed,
        ),
    }
}

fn first_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(clean_line)
        .find(|l| !l.trim().is_empty())
}

fn result_line(
    preview: &Preview,
    status: &Status,
    run: Option<&RunOutput>,
    after: &Result<Vec<String>, String>,
    host_timeout_secs: u64,
) -> String {
    let who = preview.manager.name();
    let what = &preview.title;
    let reread = match after {
        Err(e) => format!("Swamp could not read {who}'s list afterwards ({e})."),
        Ok(left) if left.is_empty() => format!("{who} no longer lists it."),
        Ok(left) => format!("{who} still lists {}.", left.join(", ")),
    };
    let said = run
        .and_then(|o| first_line(&o.stderr).or_else(|| first_line(&o.stdout)))
        .map(|l| format!(" {who} said: {l}"))
        .unwrap_or_default();
    match status {
        Status::Removed => format!("Removed {what} with {who}, permanently. {reread}"),
        Status::RemovedWithError(code) => format!(
            "{who} exited {} while removing {what}. {reread}{said}",
            code.map_or_else(|| "on a signal".to_string(), |c| c.to_string())
        ),
        Status::Failed(code) => format!(
            "{who} exited {} and did not remove {what}. {reread}{said}",
            code.map_or_else(|| "on a signal".to_string(), |c| c.to_string())
        ),
        Status::StillListed => format!("{who} exited 0 but {reread}{said}"),
        Status::TimedOut => format!(
            "{who} did not finish within {} s and was stopped. {reread} Review again before \
             trying anything else.",
            host_timeout_secs
        ),
        Status::NotReread(code) => format!(
            "{who} exited {} for {what}. {reread} Review again to see what is left.",
            code.map_or_else(|| "on a signal".to_string(), |c| c.to_string())
        ),
        Status::NotStarted(e) => format!("{who} could not be started ({e}). Nothing ran."),
        Status::Refused(r) => format!("Nothing ran. {} {}", r.reason, r.next),
    }
}

/// One `tool-remove` ledger record: what the human saw and what was
/// observed afterwards.
fn record(
    ledger: &Ledger,
    id: &str,
    replace: bool,
    preview: &Preview,
    outcome: &str,
    observed: &str,
    run: Option<&RunOutput>,
) -> Result<(), String> {
    let mut evidence = vec![
        LedgerFact::new("manager", preview.manager.name()),
        LedgerFact::new("manager_version", &preview.manager_version),
        LedgerFact::new("program", preview.bin.path().display()),
        LedgerFact::new(
            "developer_dir",
            preview
                .bin
                .developer_dir()
                .unwrap_or("not set (xcrun's default)"),
        ),
        LedgerFact::new("argv_exec", preview.command_line()),
        LedgerFact::new(
            "argv_dry",
            command_line(preview.manager_command(), &preview.dry_argv),
        ),
        LedgerFact::new("dry_output_blake3", &preview.dry_output_digest),
        LedgerFact::new(
            "dry_output_head",
            head_bytes(&preview.dry_output.join("\n"), 4096),
        ),
        LedgerFact::new("targets", preview.removes.join("; ")),
        LedgerFact::new(
            "bytes",
            preview.size.as_ref().map_or_else(
                || "not measured".to_string(),
                |s| format!("{} ({})", s.bytes, s.source),
            ),
        ),
        LedgerFact::new("regen_cost", &preview.regen),
        LedgerFact::new("open_files", &preview.open_files),
        LedgerFact::new("warnings_shown", preview.warnings.join("; ")),
        LedgerFact::new(
            "env_policy",
            format!(
                "{} ({})",
                crate::fs_gate::spawn::TOOL_ENV_POLICY,
                preview.bin.env_names().join(",")
            ),
        ),
        LedgerFact::new("permanent", true),
    ];
    if let Some(out) = run {
        evidence.push(LedgerFact::new(
            "exit_code",
            out.code
                .map_or_else(|| "none".to_string(), |c| c.to_string()),
        ));
        evidence.push(LedgerFact::new("timed_out", out.timed_out));
        evidence.push(LedgerFact::new(
            "stderr_head",
            head_bytes(&clean_block(&out.stderr).join("\n"), 2048),
        ));
    }
    let rec = ActionRecord {
        id: id.to_string(),
        verb: Verb::ToolRemove,
        entity_id: crate::entities::id_for(&preview.command_line()),
        evidence,
        grant_id: NO_GRANT.to_string(),
        actor: "human:tui".to_string(),
        outcome: outcome.to_string(),
        // A manager has no Trash: there is nowhere to point at.
        recovery_location: None,
        measured_free_space_delta: None,
        observed_path_state: Some(observed.to_string()),
        recorded_at: crate::entities::now(),
    };
    if replace {
        ledger.replace(&rec)
    } else {
        ledger.append(&rec)
    }
    .map_err(|e| e.to_string())
}

fn head_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// One shell-quoting formatter for every command swamp shows and
/// records: `program` then each argument, single-quoted when it holds
/// anything outside a plain-word set. The argv that is spawned is the
/// argv this renders; `parse_command_line` reads it back.
pub fn command_line(program: &str, argv: &[OsString]) -> String {
    let mut out = String::from(program);
    for a in argv {
        out.push(' ');
        let a = a.to_string_lossy();
        let plain = !a.is_empty()
            && a.chars().all(|c| {
                c.is_ascii_alphanumeric()
                    || matches!(c, '_' | '@' | '%' | '+' | '=' | ':' | ',' | '.' | '/' | '-')
            });
        if plain {
            out.push_str(&a);
        } else {
            out.push('\'');
            out.push_str(&a.replace('\'', "'\\''"));
            out.push('\'');
        }
    }
    out
}

/// Reads a [`command_line`] back into its words (program first).
pub fn parse_command_line(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    cur.push(q);
                }
            }
            '\\' => {
                in_word = true;
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            ' ' => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    words
}

/// One line of manager output made safe to draw: escape sequences and
/// control characters removed (a manager's text must not be able to
/// move the cursor or hide a line of the confirm), tabs as spaces,
/// capped at 300 characters.
pub(crate) fn clean_line(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => {
                // CSI (`ESC [ ... final`) or a two-character escape.
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for n in chars.by_ref() {
                        if ('@'..='~').contains(&n) {
                            break;
                        }
                    }
                } else {
                    chars.next();
                }
            }
            '\t' => out.push(' '),
            c if c.is_control() || invisible(c) => {}
            c => out.push(c),
        }
    }
    if out.chars().count() > 300 {
        out = out.chars().take(300).collect::<String>() + "...";
    }
    out
}

/// Bidi controls, zero-width characters and line/paragraph separators: a
/// manager line holding one could reorder or hide text on the confirm.
fn invisible(c: char) -> bool {
    // Every Unicode format (Cf) character: bidi controls, zero-width
    // characters, tag characters (U+E0000..U+E007F), the Mongolian vowel
    // separator, interlinear annotation marks; plus the line and
    // paragraph separators.
    matches!(c,
        '\u{00AD}' | '\u{0600}'..='\u{0605}' | '\u{061C}' | '\u{06DD}' | '\u{070F}'
            | '\u{0890}'..='\u{0891}' | '\u{08E2}' | '\u{180E}' | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}' | '\u{110BD}' | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}' | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}' | '\u{E0001}' | '\u{E0020}'..='\u{E007F}')
}

/// A whole stream as cleaned lines, bounded to [`SHOWN_OUTPUT_LINES`].
pub(crate) fn clean_block(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<String> = text.lines().map(clean_line).collect();
    bound_lines(lines)
}

fn bound_lines(mut lines: Vec<String>) -> Vec<String> {
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if lines.len() > SHOWN_OUTPUT_LINES {
        let more = lines.len() - (SHOWN_OUTPUT_LINES - 1);
        lines.truncate(SHOWN_OUTPUT_LINES - 1);
        lines.push(format!("+{more} more lines"));
    }
    lines
}

/// Runs one read-only invocation of `bin` (a listing, a dry run).
fn read(bin: &ToolBin, args: &[&str]) -> std::io::Result<RunOutput> {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    crate::fs_gate::spawn::run_tool_read(bin, &args, READ_TIMEOUT)
}

/// A read that must succeed and fit, as a refusal when it does not.
fn read_ok(
    bin: &ToolBin,
    args: &[&str],
    manager: Manager,
    what: &str,
) -> Result<RunOutput, Refusal> {
    let out = read(bin, args).map_err(|e| {
        Refusal::new(
            format!("{} {what} could not be run: {e}", manager.name()),
            "Review again later.",
        )
    })?;
    if out.timed_out {
        return Err(Refusal::new(
            format!(
                "{} {what} did not finish within {} s and was stopped.",
                manager.name(),
                READ_TIMEOUT.as_secs()
            ),
            "Review again later.",
        ));
    }
    if out.truncated {
        return Err(Refusal::new(
            format!(
                "{} {what} printed more than swamp reads ({}); swamp shows it and runs nothing.",
                manager.name(),
                crate::render::human_bytes_pub(crate::fs_gate::spawn::TOOL_OUTPUT_LIMIT)
            ),
            "Look at it yourself in a shell.",
        )
        .with_output(clean_block(&out.stdout)));
    }
    Ok(out)
}

fn digest(lines: &[String]) -> String {
    blake3::hash(lines.join("\n").as_bytes())
        .to_hex()
        .to_string()
}

/// `~/x` against `home`.
fn expand_home(p: &str, home: &Path) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if p == "~" => home.to_path_buf(),
        None => PathBuf::from(p),
    }
}

/// `home/x` as `~/x`, for display.
fn tilde(p: &Path, home: &Path) -> String {
    match p.strip_prefix(home) {
        Ok(rest) if home != Path::new("/") => format!("~/{}", rest.display()),
        _ => p.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_round_trips_through_the_parser() {
        let argv: Vec<OsString> = [
            "-C",
            "/",
            "uninstall",
            "java@temurin-17.0.20+101",
            "a b",
            "it's",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        let line = command_line("mise", &argv);
        let words = parse_command_line(&line);
        assert_eq!(words[0], "mise");
        let back: Vec<OsString> = words[1..].iter().map(OsString::from).collect();
        assert_eq!(back, argv, "{line}");
    }

    #[test]
    fn escapes_and_control_characters_never_reach_the_screen() {
        let l = clean_line("mise go@1 \u{1b}[2K\u{1b}[1Aremove /x\r\u{7}\tdone");
        assert_eq!(l, "mise go@1 remove /x done");
        assert!(!l.chars().any(|c| c.is_control()));
    }

    #[test]
    fn a_huge_output_is_bounded_with_a_count() {
        let big: Vec<u8> = (0..5000)
            .flat_map(|i| format!("line {i}\n").into_bytes())
            .collect();
        let lines = clean_block(&big);
        assert_eq!(lines.len(), SHOWN_OUTPUT_LINES);
        assert_eq!(lines.last().unwrap(), "+4801 more lines");
    }
}
