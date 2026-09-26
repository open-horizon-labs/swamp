//! Source-aware registry for developer-storage location detection (#44).
//!
//! Detectors *propose* locations; they never authorize measurement,
//! attribution, or removal (see `.oh/guardrails/extractors-are-pluggable.md`
//! for the sibling discipline this mirrors). This registry is deliberately
//! **not** an `EventBus` consumer (`crate::bus`): the bus's consumers react
//! to walk events for an already-chosen root, while a detector's whole job
//! is figuring out which roots to walk in the first place. Reusing the bus
//! here would mean inventing walk events for a walk that has not started.
//!
//! What *is* reused from the bus's discipline: static registration
//! (`Registry::with_builtins`), no detector knows about another detector,
//! and adding a new detector is "write one small file, register it in
//! `with_builtins`" -- never a change to `scope.rs` or the walker. See
//! `docs/architecture.md`'s "Location detector registry" section for the
//! worked pattern to copy for a new detector.
//!
//! Detectors are read-only. They may inspect environment variables, the
//! home directory's conventional layout, and -- for a handful of detectors
//! -- run one bounded, allow-listed, read-only tool query (`brew --prefix`,
//! never a project script or shell startup file). A failed tool query is
//! reported on the location it would have resolved, never fatal to the
//! rest of detection.

pub mod aider;
pub mod android;
pub mod asdf;
pub mod builtin;
pub mod cargo_home;
pub mod claude_code;
pub mod cline;
pub mod codex;
pub mod codex_desktop;
pub mod conda;
pub mod continue_dev;
pub mod copilot_cli;
pub mod core_simulator;
pub mod cursor;
pub mod docker_desktop;
pub mod gemini_cli;
pub mod go;
pub mod gradle;
pub mod homebrew;
pub mod huggingface;
pub mod maven;
pub mod mise;
pub mod npm;
pub mod nvm;
pub mod oh_my_pi;
pub mod ollama;
pub mod opencode;
pub mod permitted;
pub mod pi;
pub mod pip;
pub mod pnpm;
pub mod pyenv;
pub mod rbenv;
pub mod roo_code;
pub mod ruby_install;
pub mod rustup;
pub mod rvm;
pub mod uv;
pub mod vscode_hosts;
pub mod windsurf;
pub mod xcode;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Bumped whenever the set of detectors or their resolution semantics
/// changes, so a persisted `EffectiveScope` (`crate::scope`) can show it
/// was resolved under an older catalog than the one now running.
/// The one bounded, single-level directory listing the detector and
/// adapter layers are allowed to perform.
///
/// Detectors probe for existence with `metadata`; where a layout
/// genuinely requires enumerating one level (a version manager's
/// `versions/` directory, a toolchain root's installed names), they call
/// this. It is capped, sorted, never recursive, and never follows a
/// symlink into another tree -- so "adapters and detectors do not
/// traverse" stays a structural property rather than a habit
/// (`.oh/guardrails/no-second-traversal-on-report-path.md`,
/// `.oh/guardrails/agent-adapters-do-not-traverse.md`).
pub const SHALLOW_LIST_CAP: usize = 4_096;

/// One entry of a [`shallow_list`]: its name and whether it is a
/// directory, decided from the entry's own file type (never by asking
/// the filesystem about the path again, which would follow a symlink).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShallowEntry {
    pub name: String,
    pub is_dir: bool,
}

/// Whether a bounded listing saw everything.
///
/// Re-review 3 (F1): `shallow_list` stopped at the cap and said nothing,
/// so a caller summing over the entries -- Oh My Pi's shared-blob
/// reference count -- reported a short number as complete. The cap is
/// the bound that earns the listing its exemption from the traversal
/// guardrails; saying when it was hit is what keeps the bound honest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truncation {
    Complete,
    /// The listing stopped at the cap with more entries left; `n` is how
    /// many it kept.
    Truncated {
        n: usize,
    },
    /// The directory exists but could not be listed (permission denied,
    /// not a directory, an I/O error). Not `Complete`: an unreadable
    /// directory is not an empty one that was fully read (re-review 4,
    /// C1). A directory that does not exist at all is `Complete` and
    /// empty.
    Unreadable,
}

impl Truncation {
    /// Whether the listing is *not* the whole directory: cut at the cap,
    /// or not taken at all. What every aggregate over the entries must
    /// check before calling itself complete.
    pub fn is_truncated(self) -> bool {
        !matches!(self, Truncation::Complete)
    }
}

/// A [`shallow_list`]: its entries, and whether they are all of them.
/// Iterates and dereferences as the entries, so a caller that needs only
/// the names reads it as before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShallowListing {
    pub entries: Vec<ShallowEntry>,
    pub truncation: Truncation,
}

impl std::ops::Deref for ShallowListing {
    type Target = [ShallowEntry];
    fn deref(&self) -> &[ShallowEntry] {
        &self.entries
    }
}

impl IntoIterator for ShallowListing {
    type Item = ShallowEntry;
    type IntoIter = std::vec::IntoIter<ShallowEntry>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

pub fn shallow_list(dir: &std::path::Path) -> ShallowListing {
    crate::work_counters::record_dir_listed();
    let entries = match crate::fs_gate::read_dir(dir) {
        Ok(entries) => entries,
        // Nothing there: the whole of an absent directory is nothing.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return ShallowListing {
                entries: Vec::new(),
                truncation: Truncation::Complete,
            };
        }
        // Something there that could not be listed is not "empty".
        Err(_) => {
            return ShallowListing {
                entries: Vec::new(),
                truncation: Truncation::Unreadable,
            };
        }
    };
    let mut out: Vec<ShallowEntry> = Vec::new();
    let mut truncation = Truncation::Complete;
    for entry in entries.flatten() {
        if out.len() >= SHALLOW_LIST_CAP {
            truncation = Truncation::Truncated { n: out.len() };
            break;
        }
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        out.push(ShallowEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            is_dir: ft.is_dir(),
        });
    }
    crate::work_counters::record_files_statted(out.len() as u64);
    out.sort_by(|a, b| a.name.cmp(&b.name));
    ShallowListing {
        entries: out,
        truncation,
    }
}

/// Just the subdirectory names, the shape most callers want.
pub fn shallow_dir_names(dir: &std::path::Path) -> Vec<String> {
    shallow_list(dir)
        .into_iter()
        .filter(|e| e.is_dir)
        .map(|e| e.name)
        .collect()
}

pub const CATALOG_VERSION: &str = "2026-09-21.4";

/// Detection platform. Data, not a compile-time cfg: tests inject any
/// value so a Linux-configured `Environment` can be asserted to produce
/// zero macOS-specific paths without needing a second build target.
/// [`Platform::current`] is the only place this crate lets the *real*
/// process's OS pick a value, and it is `#[cfg]`-gated so a non-macOS
/// build of this crate never even compiles the macOS defaults path as
/// "current" -- Linux's own defaults are #84's job; this only guarantees
/// today's macOS table cannot silently leak into that later build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Platform {
    MacOS,
    Linux,
}

impl Platform {
    #[cfg(target_os = "macos")]
    pub fn current() -> Self {
        Platform::MacOS
    }

    #[cfg(target_os = "linux")]
    pub fn current() -> Self {
        Platform::Linux
    }

    /// No convention table exists for any other target, and answering
    /// `Linux` here -- what this used to do -- handed a BSD build Linux's
    /// default roots and paths while `platform::Os::current` refused to
    /// compile at all. The two must agree, so this refuses too.
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    pub fn current() -> Self {
        compile_error!(
            "swamp's location conventions exist for macOS and Linux only; \
             see docs/platform.md before adding a target"
        )
    }
}

/// What kind of developer storage a proposed location holds. Deliberately
/// coarse -- fine-grained role identification (tests vs. deps vs. output)
/// is the build-artifact-identification epic's job (#74), not this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StorageCategory {
    Installation,
    Downloads,
    Cache,
    LocalState,
    Environments,
    BuildOutput,
    Models,
    Unclassified,
}

/// How a detector arrived at a candidate path. Kept even after the path
/// is folded into scope, so a human can ask "why does swamp think this is
/// here" without re-running detection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provenance {
    /// A hard-coded convention this detector always knows about
    /// (`~/.cargo`, `/opt/homebrew`), independent of any tool being
    /// installed -- this is what lets a leftover cache be found after
    /// its manager was uninstalled.
    BuiltinConvention,
    /// Resolved from this named environment variable's value.
    EnvVar(String),
    /// Resolved from a field in a config file the detector read.
    ConfigField(String),
    /// Resolved by running this bounded, read-only, allow-listed command.
    ToolQuery(String),
}

/// How a location was resolved, as text output says it (never the
/// variant's Debug form).
pub fn provenance_label(p: &Provenance) -> String {
    match p {
        Provenance::BuiltinConvention => "built-in convention".to_string(),
        Provenance::EnvVar(v) => format!("env {v}"),
        Provenance::ConfigField(v) => format!("config {v}"),
        Provenance::ToolQuery(v) => format!("queried {v}"),
    }
}

/// Whether a detector actually produced a usable path for this candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum LocationStatus {
    /// The detector determined this path; scope resolution still checks
    /// whether it exists on disk (see `crate::scope::RootStatus`).
    Resolved,
    /// The detector positively determined this candidate does not apply
    /// here (e.g. no override and no known convention path for this
    /// platform).
    NotPresent,
    /// Excluded from scope by `disabled_detectors` (or `defaults = false`
    /// for the built-in-defaults detector); still reported for
    /// transparency, never silently dropped.
    Disabled,
    /// A tool query or other resolution step failed; the reason is
    /// surfaced rather than treated as "not present".
    UnresolvedWithReason { reason: String },
}

/// One candidate developer-storage location a detector proposes.
/// Discovery only: this never authorizes measurement, attribution, or
/// removal of anything at `path`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposedLocation {
    pub detector_id: String,
    /// Present exactly when `status == Resolved`.
    pub path: Option<PathBuf>,
    pub category: StorageCategory,
    pub provenance: Provenance,
    pub status: LocationStatus,
    /// Free-text note a detector can attach (e.g. which of two
    /// conventional prefixes this is), never used for control flow.
    pub note: Option<String>,
}

/// Outcome of a read-only command a detector wants to run.
pub struct CommandOutcome {
    pub stdout: String,
    pub success: bool,
}

/// Runs a detector's tool queries. The only implementation shipped for
/// real use is [`SystemCommandRunner`], which enforces a fixed
/// allow-list of `(program, args)` pairs *in addition to* each detector
/// only ever constructing an allow-listed call -- belt and suspenders,
/// so a future detector cannot introduce a side-effecting command by
/// mistake. Tests use [`FakeCommandRunner`], which records every call it
/// receives so a test can assert a detector never asked for anything
/// beyond the documented allow-list, even when the fixture's canned
/// answer would happily satisfy a broader request.
pub trait CommandRunner: Send + Sync {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<CommandOutcome, String>;
}

/// Every command any detector in this catalog is allowed to run. A
/// detector that needs a new query adds one line here, in the open,
/// reviewable alongside the detector itself -- never a free-form string
/// built at call time.
pub const ALLOWED_COMMANDS: &[(&str, &[&str])] = &[
    ("brew", &["--prefix"]),
    (
        "defaults",
        &["read", "com.apple.dt.Xcode", "IDECustomDerivedDataLocation"],
    ),
    // Current-use evidence (#55): a read-only device-state listing, never
    // a boot/shutdown/erase command. Used by `crate::occupancy` to answer
    // "is this simulator's data directory currently mounted by a booted
    // instance", not by the detector registry itself.
    ("xcrun", &["simctl", "list", "devices", "-j"]),
];

/// Runs allow-listed commands for real, bounded by `timeout`. Refuses
/// (without spawning a process) anything not on [`ALLOWED_COMMANDS`].
pub struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<CommandOutcome, String> {
        if !ALLOWED_COMMANDS
            .iter()
            .any(|(p, a)| *p == program && *a == args)
        {
            return Err(format!(
                "refusing to run non-allow-listed command: {program} {args:?}"
            ));
        }
        let Some(bin) = crate::fs_gate::spawn::Program::named(program) else {
            return Err(format!(
                "refusing to run {program}: not a program swamp may run"
            ));
        };
        let out = crate::fs_gate::spawn::run(bin, args, timeout)
            .map_err(|e| format!("{program} {args:?}: spawn failed: {e}"))?;
        if out.timed_out {
            return Err(format!("{program} {args:?}: timed out after {timeout:?}"));
        }
        Ok(CommandOutcome {
            stdout: out.stdout_lossy().trim().to_string(),
            success: out.success(),
        })
    }
}

/// A command runner that never runs anything and reports every call as
/// unavailable -- the default for fixture environments that do not
/// explicitly opt into tool queries, so a test that forgets to inject a
/// [`FakeCommandRunner`] fails loudly (`UnresolvedWithReason`) instead of
/// quietly reaching the real `brew` on the machine running the test.
pub struct NullCommandRunner;

impl CommandRunner for NullCommandRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        _timeout: Duration,
    ) -> Result<CommandOutcome, String> {
        Err(format!(
            "no command runner configured for {program} {args:?}"
        ))
    }
}

/// Records every call it receives (even a refused one) so a test can
/// assert a detector only ever asked for allow-listed, read-only
/// commands -- the adversarial check #44 requires: a detector whose tool
/// query *would* have a side effect if it ran for real must never
/// actually run it.
pub struct FakeCommandRunner {
    pub calls: std::sync::Mutex<Vec<(String, Vec<String>)>>,
    pub answers: HashMap<(&'static str, &'static [&'static str]), Result<String, String>>,
}

impl FakeCommandRunner {
    pub fn new() -> Self {
        Self {
            calls: std::sync::Mutex::new(Vec::new()),
            answers: HashMap::new(),
        }
    }

    pub fn with_answer(
        mut self,
        program: &'static str,
        args: &'static [&'static str],
        out: &str,
    ) -> Self {
        self.answers.insert((program, args), Ok(out.to_string()));
        self
    }

    pub fn with_failure(
        mut self,
        program: &'static str,
        args: &'static [&'static str],
        reason: &str,
    ) -> Self {
        self.answers
            .insert((program, args), Err(reason.to_string()));
        self
    }

    pub fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.calls.lock().unwrap().clone()
    }
}

impl Default for FakeCommandRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandRunner for FakeCommandRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        _timeout: Duration,
    ) -> Result<CommandOutcome, String> {
        self.calls.lock().unwrap().push((
            program.to_string(),
            args.iter().map(|s| s.to_string()).collect(),
        ));
        if !ALLOWED_COMMANDS
            .iter()
            .any(|(p, a)| *p == program && *a == args)
        {
            return Err(format!(
                "refusing to run non-allow-listed command: {program} {args:?}"
            ));
        }
        match self
            .answers
            .iter()
            .find(|((p, a), _)| *p == program && *a == args)
        {
            Some((_, Ok(out))) => Ok(CommandOutcome {
                stdout: out.clone(),
                success: true,
            }),
            Some((_, Err(reason))) => Err(reason.clone()),
            None => Err(format!("no fixture answer for {program} {args:?}")),
        }
    }
}

/// Everything a detector is allowed to see, injected so tests never scan
/// the developer's real home (#44's fixture-injection requirement).
pub struct Environment {
    pub home: PathBuf,
    pub env: HashMap<String, String>,
    pub platform: Platform,
    pub runner: Arc<dyn CommandRunner>,
    /// Bound applied to every tool query a detector runs through this
    /// environment.
    pub command_timeout: Duration,
}

impl Environment {
    /// The real process's home/env/platform, with the real (allow-listed)
    /// command runner. Only ever constructed at the CLI's entry point.
    pub fn from_process() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            home,
            env: std::env::vars().collect(),
            platform: Platform::current(),
            runner: Arc::new(SystemCommandRunner),
            command_timeout: Duration::from_secs(2),
        }
    }

    /// A fixture environment: explicit home/env/platform, no live tool
    /// queries unless a [`FakeCommandRunner`] is supplied with
    /// [`Environment::with_runner`].
    pub fn fixture(home: PathBuf, env: HashMap<String, String>, platform: Platform) -> Self {
        Self {
            home,
            env,
            platform,
            runner: Arc::new(NullCommandRunner),
            command_timeout: Duration::from_millis(50),
        }
    }

    pub fn with_runner(mut self, runner: Arc<dyn CommandRunner>) -> Self {
        self.runner = runner;
        self
    }

    pub fn env_var(&self, key: &str) -> Option<&str> {
        self.env.get(key).map(|s| s.as_str())
    }

    pub fn run_command(&self, program: &str, args: &[&str]) -> Result<CommandOutcome, String> {
        self.runner.run(program, args, self.command_timeout)
    }
}

// ---------------------------------------------------------------------
// Declared capabilities: what a detector's storage *does*, so consumers
// never hold a table mapping tool names to detector ids
// (`.oh/guardrails/detector-ids-only-in-registry.md`).
// ---------------------------------------------------------------------

/// How installed version identifiers are arranged underneath one of a
/// manager's `Installation` locations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstalledVersionLayout {
    /// `<installation>/<version>` -- pyenv's `versions/`, rbenv's
    /// `versions/`, RVM's `rubies/`, nvm's `versions/node/`, rustup's
    /// `toolchains/`.
    VersionPerEntry,
    /// `<installation>/<tool>/<version>` -- asdf's and mise's
    /// `installs/`, where one root serves every tool.
    ToolThenVersion,
}

/// How an installed directory's *name* relates to the version a project
/// declares, which decides whether a declaration can be compared to it
/// directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstalledVersionNaming {
    /// The directory name is the identifier the declaration names
    /// (`3.12.1`, `v20.11.0`).
    AsDeclared,
    /// The directory name qualifies a declared *channel* with the host
    /// triple (`stable` -> `stable-aarch64-apple-darwin`), so a
    /// declaration has to be widened to the installed name before it can
    /// match.
    ChannelWithHostTriple,
}

/// The shape of a file under a manager's own `LocalState` location that
/// records a *machine-wide* default version -- a real role of its own,
/// distinct from any project's declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalDefaultFormat {
    /// TOML whose top-level `field` is the version identifier
    /// (`default_toolchain = "stable"`). The *shape* is named, not the
    /// manager, so a second manager that writes the same shape needs no
    /// consumer change.
    TomlTopLevelString,
}

/// Where a manager records its global default, if it records one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalDefaultFile {
    /// File name relative to the manager's own `LocalState` location.
    pub file_name: &'static str,
    /// The field in that file holding the default version identifier.
    pub field: &'static str,
    pub format: GlobalDefaultFormat,
}

/// Which of a detector's own proposed locations is the joinable store.
/// Expressed against what the detector already publishes (category and
/// path shape), never as a path literal a consumer has to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreAnchor {
    /// The detector proposes exactly one store and nothing that could be
    /// confused with it, so every location it proposes is the store
    /// (Maven's local repository, pnpm's store, npm's cache).
    SoleLocation,
    /// The detector's location in `category` whose path ends with
    /// `suffix` (an empty suffix means "its only location in that
    /// category").
    Categorized {
        category: StorageCategory,
        suffix: &'static [&'static str],
    },
    /// The store's own path is a free-form override with no fixed shape
    /// (`GOMODCACHE`), but the detector derives a sibling location from
    /// it at a fixed depth: the store is that sibling's ancestor `up`
    /// components above, carrying `category`. Derived, never guessed
    /// from a path shape.
    AncestorOfSibling {
        sibling: StorageCategory,
        up: usize,
        category: StorageCategory,
    },
    /// The detector's locations in `category` whose path ends with none
    /// of `except`: for a category that holds one fixed-shape location
    /// beside free-form ones (Xcode's `Archives` beside the default *and*
    /// a custom `IDECustomDerivedDataLocation`).
    CategorizedExcept {
        category: StorageCategory,
        except: &'static [&'static [&'static str]],
    },
}

impl StoreAnchor {
    /// Which of one detector's locations -- given as `(category, path)`,
    /// in the detector's own order -- this anchor selects. The one
    /// evaluation of the anchor vocabulary, so a consumer never holds a
    /// path shape of its own.
    pub fn select(&self, locations: &[(StorageCategory, &Path)]) -> Vec<usize> {
        fn ends_with(path: &Path, suffix: &[&str]) -> bool {
            let comps: Vec<String> = path
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            suffix.len() <= comps.len()
                && comps[comps.len() - suffix.len()..]
                    .iter()
                    .zip(suffix.iter())
                    .all(|(a, b)| a == b)
        }
        let of = |category: StorageCategory, pred: &dyn Fn(&Path) -> bool| -> Vec<usize> {
            locations
                .iter()
                .enumerate()
                .filter(|(_, (c, p))| *c == category && pred(p))
                .map(|(i, _)| i)
                .collect()
        };
        match *self {
            Self::SoleLocation => (0..locations.len()).collect(),
            Self::Categorized { category, suffix } => of(category, &|p| ends_with(p, suffix)),
            Self::CategorizedExcept { category, except } => {
                of(category, &|p| !except.iter().any(|s| ends_with(p, s)))
            }
            Self::AncestorOfSibling {
                sibling,
                up,
                category,
            } => {
                let Some((_, sib)) = locations.iter().find(|(c, _)| *c == sibling) else {
                    return Vec::new();
                };
                let mut path: &Path = sib;
                for _ in 0..up {
                    let Some(parent) = path.parent() else {
                        return Vec::new();
                    };
                    path = parent;
                }
                of(category, &|p| p == path)
            }
        }
    }
}

/// Which kind of machine-wide build store a detector location is: the
/// capability a detector declares (`Detector::build_stores`) and a build
/// adapter claims (`build_adapters::BuildAdapter::store_kinds`), so the
/// external observation can hand a measured store to the adapter that
/// identifies its interior without either side naming the other
/// (`.oh/guardrails/build-stores-join-by-capability.md`).
///
/// A kind names a *layout*, not a tool: a second tool that writes the
/// same layout declares the same kind and needs no adapter change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildStoreKind {
    /// npm's content-addressed `_cacache`.
    NpmCache,
    /// pnpm's content-addressable store.
    PnpmStore,
    /// `<gradle-user-home>/caches`.
    GradleCaches,
    /// `<gradle-user-home>/wrapper/dists`.
    GradleWrapperDists,
    /// `<gradle-user-home>/daemon`.
    GradleDaemon,
    /// `<gradle-user-home>/native`.
    GradleNative,
    /// A Maven-layout local repository.
    MavenRepository,
    /// Go's extracted module cache (`GOMODCACHE`, minus `cache/download`).
    GoModuleCache,
    /// Go's raw module downloads (`GOMODCACHE/cache/download`).
    GoModuleDownloads,
    /// Go's build cache (`GOCACHE`).
    GoBuildCache,
    /// pip's HTTP and wheel cache.
    PipCache,
    /// uv's cache directory.
    UvCache,
    /// uv-managed Python installations.
    UvPythonInstallations,
    /// uv-installed tool environments.
    UvToolEnvironments,
    /// Xcode's DerivedData directory (default or custom location).
    XcodeDerivedData,
    /// Xcode's `Archives` directory.
    XcodeArchives,
    /// `iOS DeviceSupport` / `watchOS DeviceSupport` symbol caches.
    XcodeDeviceSupport,
    /// CoreSimulator's `Devices` directory.
    SimulatorDevices,
    /// CoreSimulator runtime installations.
    SimulatorRuntimes,
    /// CoreSimulator's own cache.
    SimulatorCaches,
    /// One Android SDK package directory (`platforms`, `build-tools`,
    /// `system-images`, `emulator`, `ndk`, ...).
    AndroidSdkPackages,
    /// Android Virtual Device data (`~/.android/avd`).
    AndroidVirtualDevices,
    /// A BuildKit build cache, answered by the Docker daemon rather than
    /// measured on disk.
    BuildKitCache,
}

impl BuildStoreKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::NpmCache => "npm-cache",
            Self::PnpmStore => "pnpm-store",
            Self::GradleCaches => "gradle-caches",
            Self::GradleWrapperDists => "gradle-wrapper-dists",
            Self::GradleDaemon => "gradle-daemon",
            Self::GradleNative => "gradle-native",
            Self::MavenRepository => "maven-repository",
            Self::GoModuleCache => "go-module-cache",
            Self::GoModuleDownloads => "go-module-downloads",
            Self::GoBuildCache => "go-build-cache",
            Self::PipCache => "pip-cache",
            Self::UvCache => "uv-cache",
            Self::UvPythonInstallations => "uv-python-installations",
            Self::UvToolEnvironments => "uv-tool-environments",
            Self::XcodeDerivedData => "xcode-derived-data",
            Self::XcodeArchives => "xcode-archives",
            Self::XcodeDeviceSupport => "xcode-device-support",
            Self::SimulatorDevices => "simulator-devices",
            Self::SimulatorRuntimes => "simulator-runtimes",
            Self::SimulatorCaches => "simulator-caches",
            Self::AndroidSdkPackages => "android-sdk-packages",
            Self::AndroidVirtualDevices => "android-virtual-devices",
            Self::BuildKitCache => "buildkit-cache",
        }
    }

    /// Whether a daemon answers for this store's contents rather than a
    /// directory swamp walks. Such a store is joined from the daemon's
    /// already-fetched facts, never measured on disk.
    pub fn answered_by_daemon(self) -> bool {
        matches!(self, Self::BuildKitCache)
    }

    pub const ALL: &'static [Self] = &[
        Self::NpmCache,
        Self::PnpmStore,
        Self::GradleCaches,
        Self::GradleWrapperDists,
        Self::GradleDaemon,
        Self::GradleNative,
        Self::MavenRepository,
        Self::GoModuleCache,
        Self::GoModuleDownloads,
        Self::GoBuildCache,
        Self::PipCache,
        Self::UvCache,
        Self::UvPythonInstallations,
        Self::UvToolEnvironments,
        Self::XcodeDerivedData,
        Self::XcodeArchives,
        Self::XcodeDeviceSupport,
        Self::SimulatorDevices,
        Self::SimulatorRuntimes,
        Self::SimulatorCaches,
        Self::AndroidSdkPackages,
        Self::AndroidVirtualDevices,
        Self::BuildKitCache,
    ];
}

/// One declaration: which of a detector's locations is a store of
/// `kind`, expressed with the same [`StoreAnchor`] vocabulary
/// `manager_conventions` uses -- against what the detector already
/// publishes, never a path literal the joining code has to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildStoreDecl {
    pub kind: BuildStoreKind,
    pub anchor: StoreAnchor,
}

/// How a shared package store answers "do you hold this exact package
/// identity?". Each variant names an on-disk *layout*, so a new detector
/// for a tool that reuses an existing layout needs no consumer change;
/// only a genuinely new layout needs a new bounded lookup, which has to
/// be written anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreEntryLookup {
    /// `<store>/<registry-host>/<name>-<version>/`: Cargo's extracted
    /// registry sources.
    RegistrySourceTree,
    /// `<store>/<escaped-module-path>@<version>/`: Go's module cache.
    GoModulePath,
    /// `<store>/modules-N/<group>/<name>/<version>/`: Gradle's
    /// dependency cache.
    GradleModules,
    /// `<store>/<group-path>/<name>/<version>/`: a Maven-layout local
    /// repository.
    MavenLayout,
    /// Content-addressed by hash: a declared name+version can never be
    /// mapped to a specific entry, so the join is reported as an honest
    /// `Unknown` carrying this basis and reason rather than guessed.
    ContentAddressed {
        basis: &'static str,
        reason: &'static str,
    },
    /// The store holds the ecosystem's packages but exposes no
    /// per-identity path a bounded lookup could test; a project naming
    /// any identity in this ecosystem consumes the store as a whole
    /// (pnpm).
    WholeStore,
}

/// What a detector's storage does for one named tool or package
/// ecosystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConventionRole {
    /// The detector's `Installation` locations hold the installed
    /// versions that satisfy a project's *declared* version of this
    /// tool. `declaration_files` are the filenames a project uses to pin
    /// it (`.nvmrc`, `.node-version`); the rest says how to read and
    /// compare what is installed.
    DeclaredVersions {
        declaration_files: &'static [&'static str],
        layout: InstalledVersionLayout,
        naming: InstalledVersionNaming,
        global_default: Option<GlobalDefaultFile>,
    },
    /// The detector proposes a shared package store a project's lockfile
    /// identities can be joined against.
    DependencyStore {
        anchor: StoreAnchor,
        lookup: StoreEntryLookup,
    },
    /// The detector proposes a build-output store whose immediate
    /// subfolders each record which workspace produced them, so a folder
    /// can be joined back to a project root.
    BuildOutputWorkspaceIndex { anchor: StoreAnchor },
}

/// One thing a detector's storage does for a tool a project can name.
///
/// This is the capability consumers match on. A detector may declare
/// several (mise satisfies both `.tool-versions` and `mise.toml`;
/// nothing stops a detector from being both an installation store and a
/// dependency store), and three detectors may declare the same one
/// (rbenv, RVM and ruby-install all satisfy `.ruby-version`) -- which is
/// exactly the "which of several managers actually holds it" question
/// the wiring used to answer with a hand-written id list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManagerConvention {
    /// The tool or package ecosystem this convention speaks for, spelled
    /// the way `crate::toolchain_declarations` and
    /// `crate::external_associations` spell it: `"python"`, `"nodejs"`,
    /// `"rust"`, `"cargo"`, `"go"`. `None` when the declaration file
    /// names the tool itself (asdf's `.tool-versions`, mise's
    /// `mise.toml`), so the convention answers for whichever tool is
    /// named there.
    pub tool: Option<&'static str>,
    pub role: ConventionRole,
}

impl ManagerConvention {
    /// Whether this convention answers for `tool`. A convention with no
    /// tool of its own answers for any.
    pub fn answers_for(&self, tool: &str) -> bool {
        self.tool.is_none_or(|t| t == tool)
    }
}

/// How expensive re-obtaining a store's contents is once it is gone --
/// the thing a human actually weighs before deleting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryCost {
    /// Re-materialized locally from something still on disk (Cargo's
    /// `registry/src` from `registry/cache`).
    LocalRematerialization,
    /// Re-downloaded from the network.
    NetworkRefetch,
    /// Rebuilt from the project's own sources.
    LocalRebuild,
}

/// What a human can do to re-obtain a detector's store if it is removed.
/// Facts, never a verdict: this says what re-obtaining costs, not
/// whether removing it is a good idea.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryHint {
    /// The command that re-obtains the contents, exactly as a human
    /// would type it.
    pub command: &'static str,
    pub cost: RecoveryCost,
}

/// One fact source under the registry. Copy `builtin.rs` or
/// `cargo_home.rs` for the pattern; see `docs/architecture.md`.
pub trait Detector: Send + Sync {
    /// Stable, never-reused identifier (used in `disabled_detectors`
    /// config and in `EffectiveScope` provenance).
    fn id(&self) -> &'static str;
    /// Human-readable name for `swamp scope` output.
    fn name(&self) -> &'static str;
    /// Platforms this detector applies to. Checked by the registry, not
    /// by `#[cfg]`, so a fixture can exercise a Linux-only detector on a
    /// macOS test run.
    fn platforms(&self) -> &'static [Platform];
    /// One or more supported-version/format notes for `swamp scope`
    /// output and docs generation; not machine-checked.
    fn version_note(&self) -> &'static str {
        "current documented layout"
    }
    /// Which declaration files / package managers an installation store
    /// under this detector satisfies -- e.g. pyenv <-> `.python-version`.
    ///
    /// Declaring this here is what lets `crate::consumer_wiring` wire a
    /// new detector into association evidence without editing a table
    /// somewhere else
    /// (`.oh/guardrails/detector-ids-only-in-registry.md`).
    fn manager_conventions(&self) -> &'static [ManagerConvention] {
        &[]
    }
    /// What a human can do to re-obtain this store's contents if it is
    /// removed. Describes the detector's primary store; a detector whose
    /// locations have materially different recovery stories declares
    /// none rather than picking one to stand for the rest.
    fn recovery_hint(&self) -> Option<RecoveryHint> {
        None
    }
    /// Which of this detector's locations are machine-wide build stores,
    /// and of which kind. The external observation hands each such
    /// measured location to the build adapter that claims the kind
    /// (`.oh/guardrails/build-stores-join-by-capability.md`); a detector
    /// that declares nothing contributes no interior, only its whole
    /// external units.
    fn build_stores(&self) -> &'static [BuildStoreDecl] {
        &[]
    }
    /// Whether this detector runs under ordinary `defaults = true` scope
    /// without the config naming it.
    ///
    /// `true` for every detector except a **system-wide install tree**:
    /// one whose store is not per-user (`$HOME`-rooted) but shared by
    /// every account on the machine, installed once regardless of which
    /// developer runs swamp, and often holding bytes with nothing to do
    /// with any one project (Homebrew's Cellar/Caskroom can contain GUI
    /// applications installed via `brew install --cask`). Overriding this
    /// to `false` still lets the detector run: `swamp scope` reports it
    /// `disabled (default off)` rather than merely `disabled`, and
    /// `[scan] enabled_detectors = ["<id>"]` turns it on -- the same
    /// config key `defaults = false` already uses for the opposite
    /// direction, so it means "on" in both scan modes
    /// (`crate::locations::permitted::PermittedDetectors::from_config`).
    fn default_enabled(&self) -> bool {
        true
    }
    fn detect(&self, env: &Environment) -> Vec<ProposedLocation>;
}

/// Static registration, mirroring `EventBus::with_builtins`: every
/// detector is registered here before the first call to
/// [`Registry::resolve`], and nothing registers a detector at runtime.
pub struct Registry {
    detectors: Vec<Box<dyn Detector>>,
}

impl Registry {
    pub fn with_builtins() -> Self {
        Self {
            detectors: vec![
                Box::new(builtin::BuiltinDefaultsDetector),
                Box::new(cargo_home::CargoHomeDetector),
                Box::new(rustup::RustupDetector),
                Box::new(homebrew::HomebrewDetector),
                Box::new(claude_code::ClaudeCodeDetector),
                Box::new(codex::CodexDetector),
                Box::new(codex_desktop::CodexDesktopDetector),
                Box::new(oh_my_pi::OhMyPiDetector),
                Box::new(opencode::OpenCodeDetector),
                Box::new(gemini_cli::GeminiCliDetector),
                Box::new(pi::PiDetector),
                Box::new(aider::AiderDetector),
                Box::new(copilot_cli::CopilotCliDetector),
                Box::new(cursor::CursorDetector),
                Box::new(windsurf::WindsurfDetector),
                Box::new(cline::ClineDetector),
                Box::new(roo_code::RooCodeDetector),
                Box::new(continue_dev::ContinueDetector),
                Box::new(mise::MiseDetector),
                Box::new(asdf::AsdfDetector),
                Box::new(pyenv::PyenvDetector),
                Box::new(uv::UvDetector),
                Box::new(conda::CondaDetector),
                Box::new(rbenv::RbenvDetector),
                Box::new(rvm::RvmDetector),
                Box::new(ruby_install::RubyInstallDetector),
                Box::new(nvm::NvmDetector),
                Box::new(npm::NpmDetector),
                Box::new(pnpm::PnpmDetector),
                Box::new(gradle::GradleDetector),
                Box::new(maven::MavenDetector),
                Box::new(go::GoDetector),
                Box::new(pip::PipDetector),
                Box::new(xcode::XcodeDetector),
                Box::new(core_simulator::CoreSimulatorDetector),
                Box::new(android::AndroidDetector),
                Box::new(huggingface::HuggingFaceDetector),
                Box::new(ollama::OllamaDetector),
                Box::new(docker_desktop::DockerDesktopDetector),
            ],
        }
    }

    pub fn detectors(&self) -> &[Box<dyn Detector>] {
        &self.detectors
    }

    /// Runs every detector applicable to `env.platform`, marking any
    /// whose `id()` is in `disabled` as `Disabled` without calling
    /// `detect` at all (a disabled detector never even touches the
    /// environment or runs a tool query). Results are grouped by
    /// detector in registration order; callers needing a flat,
    /// deduplicated candidate list should go through
    /// `crate::scope::resolve_effective_scope`, which also handles
    /// cross-detector deduplication (two detectors resolving the same
    /// canonical path).
    pub fn resolve(
        &self,
        env: &Environment,
        permitted: &permitted::PermittedDetectors,
    ) -> Vec<(String, Vec<ProposedLocation>)> {
        self.detectors
            .iter()
            .filter(|d| d.platforms().contains(&env.platform))
            .map(|d| {
                let id = d.id().to_string();
                if !permitted.permits(d.id()) {
                    (
                        id.clone(),
                        vec![ProposedLocation {
                            detector_id: id,
                            path: None,
                            category: StorageCategory::Unclassified,
                            provenance: Provenance::BuiltinConvention,
                            status: LocationStatus::Disabled,
                            note: Some(format!("{} disabled by configuration", d.name())),
                        }],
                    )
                } else {
                    (id, d.detect(env))
                }
            })
            .collect()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_listing_past_its_cap_says_it_was_truncated() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..(super::SHALLOW_LIST_CAP + 7) {
            std::fs::write(dir.path().join(format!("e{i:05}")), b"").unwrap();
        }
        let listing = super::shallow_list(dir.path());
        assert_eq!(listing.len(), super::SHALLOW_LIST_CAP);
        assert_eq!(
            listing.truncation,
            super::Truncation::Truncated {
                n: super::SHALLOW_LIST_CAP
            },
            "a capped listing that stays silent turns every aggregate over it into a short \
             number presented as complete"
        );
        std::fs::remove_file(dir.path().join("e00000")).unwrap();
        for i in 1..8 {
            std::fs::remove_file(dir.path().join(format!("e{i:05}"))).unwrap();
        }
        assert_eq!(
            super::shallow_list(dir.path()).truncation,
            super::Truncation::Complete
        );
    }

    use super::*;

    #[test]
    fn a_store_anchor_selects_what_the_detector_published_including_custom_paths() {
        let derived = Path::new("/h/Library/Developer/Xcode/DerivedData");
        let archives = Path::new("/h/Library/Developer/Xcode/Archives");
        let custom = Path::new("/Volumes/fast/builds");
        let locs = [
            (StorageCategory::BuildOutput, derived),
            (StorageCategory::BuildOutput, archives),
            (StorageCategory::BuildOutput, custom),
        ];
        let not_archives = StoreAnchor::CategorizedExcept {
            category: StorageCategory::BuildOutput,
            except: &[&["Archives"]],
        };
        assert_eq!(
            not_archives.select(&locs),
            vec![0, 2],
            "the default and a custom DerivedData, never Archives"
        );
        let only_archives = StoreAnchor::Categorized {
            category: StorageCategory::BuildOutput,
            suffix: &["Archives"],
        };
        assert_eq!(only_archives.select(&locs), vec![1]);
        // Go: the module cache is derived from its download sibling, so a
        // free-form GOMODCACHE is still found; the build cache has its
        // own category.
        let go = [
            (
                StorageCategory::Downloads,
                Path::new("/x/mods/cache/download"),
            ),
            (StorageCategory::Cache, Path::new("/x/mods")),
            (StorageCategory::BuildOutput, Path::new("/y/compiled")),
        ];
        let modcache = StoreAnchor::AncestorOfSibling {
            sibling: StorageCategory::Downloads,
            up: 2,
            category: StorageCategory::Cache,
        };
        assert_eq!(modcache.select(&go), vec![1]);
        assert_eq!(
            StoreAnchor::Categorized {
                category: StorageCategory::BuildOutput,
                suffix: &[]
            }
            .select(&go),
            vec![2]
        );
    }

    /// The guardrail this capability exists for
    /// (`.oh/guardrails/detector-ids-only-in-registry.md`) only holds if
    /// every declaration file `crate::toolchain_declarations` can parse
    /// is claimed by *some* detector. A detector added without its
    /// convention would otherwise produce no associations silently,
    /// which is the failure mode the hand-written wiring table had.
    #[test]
    fn every_supported_declaration_file_is_claimed_by_a_detector() {
        let registry = Registry::with_builtins();
        let claimed: Vec<&'static str> = registry
            .detectors()
            .iter()
            .flat_map(|d| d.manager_conventions())
            .filter_map(|c| match c.role {
                ConventionRole::DeclaredVersions {
                    declaration_files, ..
                } => Some(declaration_files),
                _ => None,
            })
            .flatten()
            .copied()
            .collect();
        // `.java-version` is deliberately absent: no detector in this
        // catalog measures a jenv/SDKMAN installation, and an empty
        // installed list is the honest answer (see
        // `consumer_wiring::installed_versions_for`).
        for file in [
            ".tool-versions",
            ".mise.toml",
            ".python-version",
            ".ruby-version",
            ".nvmrc",
            ".node-version",
            "rust-toolchain",
            "rust-toolchain.toml",
        ] {
            assert!(
                claimed.contains(&file),
                "no detector declares a convention for `{file}`, so a project pinning a \
                 version there can never be joined to an installation"
            );
        }
        assert!(
            !claimed.contains(&".java-version"),
            "a detector now claims `.java-version`; drop it from this exclusion and check \
             `toolchain_declarations`'s `jenv-or-sdkman` note still reads true"
        );
    }

    /// Three managers install into `.ruby-version`'s tool, and the
    /// wiring has to consult all three -- which is exactly what a single
    /// hard-coded detector id could not express.
    #[test]
    fn several_detectors_may_satisfy_the_same_declaration_file() {
        let registry = Registry::with_builtins();
        let ruby: Vec<&'static str> = registry
            .detectors()
            .iter()
            .filter(|d| {
                d.manager_conventions().iter().any(|c| match c.role {
                    ConventionRole::DeclaredVersions {
                        declaration_files, ..
                    } => declaration_files.contains(&".ruby-version") && c.answers_for("ruby"),
                    _ => false,
                })
            })
            .map(|d| d.id())
            .collect();
        assert!(
            ruby.len() >= 3,
            "rbenv, RVM and ruby-install all satisfy `.ruby-version`; got {ruby:?}"
        );
    }

    /// A convention with no tool of its own (asdf's `.tool-versions`)
    /// answers for whatever the declaration names; a convention that
    /// names one answers only for that tool.
    #[test]
    fn a_convention_answers_only_for_the_tool_it_names() {
        let any = ManagerConvention {
            tool: None,
            role: ConventionRole::DeclaredVersions {
                declaration_files: &[".tool-versions"],
                layout: InstalledVersionLayout::ToolThenVersion,
                naming: InstalledVersionNaming::AsDeclared,
                global_default: None,
            },
        };
        assert!(any.answers_for("nodejs"));
        assert!(any.answers_for("anything-at-all"));
        let python = ManagerConvention {
            tool: Some("python"),
            ..any
        };
        assert!(python.answers_for("python"));
        assert!(!python.answers_for("nodejs"));
    }

    /// Only rustup records a machine-wide default in this catalog, and
    /// its installed directories are the channel-qualified ones -- both
    /// facts the wiring used to hard-code against the `"rustup"` id.
    #[test]
    fn a_global_default_and_channel_qualified_naming_are_declared_not_assumed() {
        let registry = Registry::with_builtins();
        let mut with_default = Vec::new();
        for d in registry.detectors() {
            for c in d.manager_conventions() {
                if let ConventionRole::DeclaredVersions {
                    naming,
                    global_default: Some(file),
                    ..
                } = c.role
                {
                    assert_eq!(
                        naming,
                        InstalledVersionNaming::ChannelWithHostTriple,
                        "{}: a manager recording a global default channel must also say its \
                         installed directories are channel-qualified, or the default can never \
                         be matched",
                        d.id()
                    );
                    assert_eq!(file.file_name, "settings.toml");
                    assert_eq!(file.field, "default_toolchain");
                    with_default.push(d.id());
                }
            }
        }
        assert_eq!(with_default, vec!["rustup"]);
    }

    /// Every dependency store names the ecosystem it serves: a store
    /// convention with no tool would silently claim every lockfile
    /// identity in `consumer_wiring::dependency_stores_for`.
    #[test]
    fn every_dependency_store_names_its_ecosystem() {
        let registry = Registry::with_builtins();
        let mut ecosystems = Vec::new();
        for d in registry.detectors() {
            for c in d.manager_conventions() {
                if matches!(c.role, ConventionRole::DependencyStore { .. }) {
                    assert!(
                        c.tool.is_some(),
                        "{}: a dependency store must name its ecosystem",
                        d.id()
                    );
                    ecosystems.push(c.tool.unwrap());
                }
            }
        }
        ecosystems.sort_unstable();
        assert_eq!(
            ecosystems,
            vec!["cargo", "go", "gradle", "maven", "npm", "pnpm"]
        );
    }
}
