//! What a package manager's own tooling says about the storage it
//! manages, recorded by a scheduled `observe` and only read afterwards.
//!
//! Homebrew's dry-run autoremove lists the formulae it calls unneeded,
//! mise's dry-run prune lists the versions it calls prunable, Homebrew
//! records which formulae were installed on request, mise's global
//! configuration lists the tools a person asked for, and rustup's
//! settings name the default toolchain. Each is the manager's own
//! statement. Swamp stores it verbatim beside the manager's name and
//! shows it attributed ("Homebrew reports unneeded (brew autoremove)"):
//! it never turns one into a statement of its own.
//!
//! **Where it runs.** [`collect`] is called from `swamp observe` after
//! the observation succeeded, and nowhere else. `swamp report` and the
//! TUI read the stored rows (`growth::read_manager_fact_table`) and
//! start no process. Every command is an allow-listed argument shape in
//! `fs_gate::spawn` (a dry run, never the real run), counted as a spawn,
//! killed on a time-out, and run with pagers and colour off.
//!
//! **Failure is a coverage note.** A missing binary, a time-out, a
//! non-zero exit, output that is not the shape this module knows, or
//! output larger than [`MAX_OUTPUT`] becomes one `not-observed` row
//! naming the reason. The view says the report was not observed; it
//! never fails and never shows an older answer as current.

use crate::external::ExternalUnit;
use crate::fs_gate::spawn::{ManagerCommand, RunOutput};
use crate::locations::{
    ConventionRole, GlobalDefaultFormat, ManagerDecl, ManagerProbe, Registry, SubjectShape,
};
use std::time::{Duration, Instant};

/// The most output one manager report may print before swamp treats it
/// as garbage: far above any real answer (a machine's whole formula
/// list is a few kilobytes).
pub const MAX_OUTPUT: usize = 1024 * 1024;

/// The longest one command may run.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// The longest the whole pass may take. Later probes are skipped, with a
/// note, once it is spent.
pub const PASS_BUDGET: Duration = Duration::from_secs(45);

/// What one stored row says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FactKind {
    /// Homebrew's dry-run autoremove lists this formula.
    ReportsUnneeded,
    /// mise's dry-run prune lists this version.
    ReportsPrunable,
    /// The manager's default (rustup's default toolchain, a tool in
    /// mise's global configuration).
    ActiveDefault,
    /// A toolchain the manager's settings pin for a directory.
    PinnedByOverride,
    /// Homebrew records this formula as installed on request.
    InstalledOnRequest,
    /// The probe ran and its answer was read (it may have listed
    /// nothing): the marker that lets a view say "checked".
    Checked,
    /// The probe could not be run or read; `text` says why.
    NotObserved,
    /// One row per pass: the marker that the pass ran at all.
    Pass,
}

impl FactKind {
    pub fn label(self) -> &'static str {
        match self {
            FactKind::ReportsUnneeded => "reports-unneeded",
            FactKind::ReportsPrunable => "reports-prunable",
            FactKind::ActiveDefault => "active-default",
            FactKind::PinnedByOverride => "pinned-by-override",
            FactKind::InstalledOnRequest => "installed-on-request",
            FactKind::Checked => "checked",
            FactKind::NotObserved => "not-observed",
            FactKind::Pass => "pass",
        }
    }

    pub fn from_label(label: &str) -> Option<FactKind> {
        [
            FactKind::ReportsUnneeded,
            FactKind::ReportsPrunable,
            FactKind::ActiveDefault,
            FactKind::PinnedByOverride,
            FactKind::InstalledOnRequest,
            FactKind::Checked,
            FactKind::NotObserved,
            FactKind::Pass,
        ]
        .into_iter()
        .find(|k| k.label() == label)
    }

    /// Whether a unit that has this fact is held out of the reclaimable
    /// total (a default, or installed because a person asked for it).
    pub fn holds(self) -> bool {
        matches!(
            self,
            FactKind::ActiveDefault | FactKind::PinnedByOverride | FactKind::InstalledOnRequest
        )
    }
}

/// One stored fact: what `manager`'s `probe` said about `subject`, in
/// the manager's own words (`text`, verbatim).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerFact {
    pub manager: String,
    pub probe: String,
    pub kind: FactKind,
    pub subject: Option<String>,
    pub text: String,
    pub observed_at: u64,
}

/// The stored facts, and whether any pass has ever stored them (an
/// older store has no table: "not observed yet", not "nothing reported").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManagerFacts {
    pub observed: bool,
    pub facts: Vec<ManagerFact>,
}

impl ManagerFacts {
    /// Whether `probe` of `manager` ran and was read in the stored pass.
    pub fn checked(&self, manager: &str, probe: ManagerProbe) -> bool {
        self.facts.iter().any(|f| {
            f.manager == manager && f.probe == probe.label() && f.kind == FactKind::Checked
        })
    }

    /// The reason `probe` of `manager` was not observed, when the stored
    /// pass says so.
    pub fn not_observed(&self, manager: &str, probe: ManagerProbe) -> Option<&str> {
        self.facts
            .iter()
            .find(|f| {
                f.manager == manager && f.probe == probe.label() && f.kind == FactKind::NotObserved
            })
            .map(|f| f.text.as_str())
    }
}

impl ManagerProbe {
    /// The stable label stored in the `probe` column.
    pub fn label(self) -> &'static str {
        match self {
            ManagerProbe::BrewAutoremoveDryRun => "autoremove-dry-run",
            ManagerProbe::BrewInstalledOnRequest => "installed-on-request",
            ManagerProbe::MisePruneDryRun => "prune-dry-run",
            ManagerProbe::MiseGlobalTools => "global-tools",
            ManagerProbe::SettingsDefault => "settings-default",
        }
    }

    /// The kind of fact this probe yields.
    pub fn kind(self) -> FactKind {
        match self {
            ManagerProbe::BrewAutoremoveDryRun => FactKind::ReportsUnneeded,
            ManagerProbe::MisePruneDryRun => FactKind::ReportsPrunable,
            ManagerProbe::BrewInstalledOnRequest => FactKind::InstalledOnRequest,
            ManagerProbe::MiseGlobalTools | ManagerProbe::SettingsDefault => {
                FactKind::ActiveDefault
            }
        }
    }

    /// The command as a person would type it, for the attribution.
    pub fn command(self) -> &'static str {
        match self {
            ManagerProbe::BrewAutoremoveDryRun => "brew autoremove",
            ManagerProbe::BrewInstalledOnRequest => "brew list --installed-on-request",
            ManagerProbe::MisePruneDryRun => "mise prune --tools --dry-run",
            ManagerProbe::MiseGlobalTools => "mise global config",
            ManagerProbe::SettingsDefault => "settings.toml default_toolchain",
        }
    }

    /// The kinds of fact this probe yields (its primary one first).
    pub fn kinds(self) -> &'static [FactKind] {
        match self {
            ManagerProbe::BrewAutoremoveDryRun => &[FactKind::ReportsUnneeded],
            ManagerProbe::MisePruneDryRun => &[FactKind::ReportsPrunable],
            ManagerProbe::BrewInstalledOnRequest => &[FactKind::InstalledOnRequest],
            ManagerProbe::MiseGlobalTools => &[FactKind::ActiveDefault],
            ManagerProbe::SettingsDefault => &[FactKind::ActiveDefault, FactKind::PinnedByOverride],
        }
    }

    /// The command this probe runs, when it runs one.
    fn invocation(self) -> Option<ManagerCommand> {
        match self {
            ManagerProbe::BrewAutoremoveDryRun => Some(ManagerCommand::BrewAutoremoveDryRun),
            ManagerProbe::BrewInstalledOnRequest => {
                Some(ManagerCommand::BrewListInstalledOnRequest)
            }
            ManagerProbe::MisePruneDryRun => Some(ManagerCommand::MisePruneToolsDryRun),
            ManagerProbe::MiseGlobalTools => Some(ManagerCommand::MiseListGlobalJson),
            ManagerProbe::SettingsDefault => None,
        }
    }
}

/// The attribution a row shows for a fact: the manager's name, what it
/// said and the command that said it. Never a statement swamp makes.
pub fn attribution(display: &str, probe: ManagerProbe, kind: FactKind) -> String {
    match kind {
        FactKind::ReportsUnneeded => format!("{display} reports unneeded ({})", probe.command()),
        FactKind::ReportsPrunable => format!("{display} reports prunable ({})", probe.command()),
        FactKind::ActiveDefault => format!("active default ({})", probe.command()),
        FactKind::PinnedByOverride => "pinned by override (settings.toml [overrides])".to_string(),
        FactKind::InstalledOnRequest => format!("installed on request ({})", probe.command()),
        FactKind::Checked | FactKind::NotObserved | FactKind::Pass => probe.command().to_string(),
    }
}

/// A rustup toolchain name taken apart: `<channel>[-<date>][-<host triple>]`.
/// Host triples contain dashes, so the name is never split at the first
/// one: the channel is `stable`, `beta`, `nightly` or a version, and a date
/// is exactly `YYYY-MM-DD`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolchain<'a> {
    pub channel: &'a str,
    pub date: Option<&'a str>,
    pub host: Option<&'a str>,
}

pub fn parse_toolchain(name: &str) -> Option<Toolchain<'_>> {
    let is_version = |c: &str| {
        let mut parts = c.split('.');
        let (Some(a), Some(b)) = (parts.next(), parts.next()) else {
            return false;
        };
        let c3 = parts.next();
        parts.next().is_none()
            && [Some(a), Some(b), c3]
                .into_iter()
                .flatten()
                .all(|p| !p.is_empty() && p.bytes().all(|x| x.is_ascii_digit()))
    };
    let (channel, mut rest) = match name.split_once('-') {
        Some((c, r)) => (c, Some(r)),
        None => (name, None),
    };
    if !(matches!(channel, "stable" | "beta" | "nightly") || is_version(channel)) {
        return None;
    }
    let mut date = None;
    if let Some(r) = rest {
        // Bytes only: a name is user data and may hold multi-byte text, so
        // nothing here slices at a fixed byte offset without `get`.
        let b = r.as_bytes();
        let digit = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
        let looks_dated = b.len() >= 10
            && (0..4).all(digit)
            && b[4] == b'-'
            && (5..7).all(digit)
            && b[7] == b'-'
            && (8..10).all(digit)
            && (b.len() == 10 || b[10] == b'-');
        if looks_dated {
            date = r.get(..10);
            rest = if b.len() > 11 { r.get(11..) } else { None };
        }
    }
    let host = rest.filter(|h| !h.is_empty());
    Some(Toolchain {
        channel,
        date,
        host,
    })
}

/// mise names a tool's install folder by kebab-casing the whole backend
/// argument (checked against mise 2026.9.15 in a sandboxed data directory):
/// `[options]` are dropped, an upper-case letter starts a new word, and every
/// other character that is not a letter or digit (`:`, `/`, `@`, `_`, `.`)
/// is one dash. `npm:@scope/pkg` is `npm-scope-pkg`, `ubi:BurntSushi/ripgrep`
/// is `ubi-burnt-sushi-ripgrep`, `cargo:foo_bar` is `cargo-foo-bar`.
fn mise_folder_name(tool: &str) -> String {
    let mut tool = tool;
    let stripped;
    if let Some(open) = tool.find('[') {
        stripped = tool[..open].to_string();
        tool = &stripped;
    }
    let mut out = String::new();
    for c in tool.chars() {
        if c.is_uppercase() {
            if !out.is_empty() && !out.ends_with('-') {
                out.push('-');
            }
            out.extend(c.to_lowercase());
        } else if c.is_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Whether a manager's `subject` names the folder `folder` of its store,
/// by the shape its detector declared. A channel that matches more than
/// one folder matches each of them: a default is never narrowed by a
/// guess.
pub fn subject_matches(shape: SubjectShape, subject: &str, folder: &str) -> bool {
    match shape {
        // A tap formula is named `user/tap/foo` by Homebrew and lives in
        // the Cellar as `foo`.
        SubjectShape::FolderName => subject.rsplit('/').next().unwrap_or(subject) == folder,
        SubjectShape::NameBeforeAt => {
            // `tool@version`, or a backend tool whose own name holds a
            // `@` (`npm:@scope/pkg`): try the whole subject and each cut.
            let mut names = vec![subject];
            if let Some((n, _)) = subject.split_once('@') {
                names.push(n);
            }
            if let Some((n, _)) = subject.rsplit_once('@') {
                names.push(n);
            }
            names
                .into_iter()
                .any(|n| n == folder || mise_folder_name(n) == folder)
        }
        SubjectShape::ChannelWithHostTriple => {
            if folder == subject {
                return true;
            }
            match (parse_toolchain(subject), parse_toolchain(folder)) {
                (Some(s), Some(f)) => {
                    s.channel == f.channel
                        && s.date == f.date
                        && s.host.is_none_or(|h| Some(h) == f.host)
                }
                _ => false,
            }
        }
    }
}

// ---------------------------------------------------------------------
// Parsing: defensive, pure.
// ---------------------------------------------------------------------

/// Why an answer was not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreadable(pub String);

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && !s.starts_with('-')
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '+' | '/' | '-' | ':')
        })
}

fn text_of(bytes: &[u8]) -> Result<&str, Unreadable> {
    if bytes.len() > MAX_OUTPUT {
        return Err(Unreadable(format!(
            "the output was larger than {MAX_OUTPUT} bytes"
        )));
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| Unreadable("the output was not text".into()))?;
    if text.contains('\0') {
        return Err(Unreadable("the output was not text".into()));
    }
    Ok(text)
}

/// The names `brew autoremove --dry-run` lists, with the header line
/// that introduced them (kept verbatim as the quote). Empty output is a
/// list of nothing.
pub fn parse_brew_autoremove(stdout: &[u8]) -> Result<(String, Vec<String>), Unreadable> {
    let text = text_of(stdout)?;
    let mut lines = text.lines();
    let mut header: Option<String> = None;
    for line in lines.by_ref() {
        let cleaned = strip_ansi(line);
        let line = cleaned.trim();
        if line.is_empty() {
            continue;
        }
        // Homebrew prints the header through its `oh1` formatter, which
        // puts an arrow in front (`==> Would autoremove 2 unneeded
        // formulae:`, cleanup.rb).
        let line = line.strip_prefix("==>").map_or(line, str::trim_start);
        if line.starts_with("Would autoremove ") && line.contains("unneeded formula") {
            header = Some(line.to_string());
            break;
        }
        return Err(Unreadable("the output was not the list this reads".into()));
    }
    let Some(header) = header else {
        return Ok((String::new(), Vec::new()));
    };
    let mut names = Vec::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        if !valid_name(line) {
            return Err(Unreadable("a listed name was not a formula name".into()));
        }
        names.push(line.to_string());
    }
    if let Some(n) = header
        .strip_prefix("Would autoremove ")
        .and_then(|r| r.split_whitespace().next())
        .and_then(|n| n.parse::<usize>().ok())
        && n != names.len()
    {
        return Err(Unreadable(
            "the count in the header did not match the names listed".into(),
        ));
    }
    Ok((header, names))
}

/// `text` without ANSI colour sequences.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// The formulae `brew list --formula --installed-on-request` prints, one
/// per line.
pub fn parse_name_lines(stdout: &[u8]) -> Result<Vec<String>, Unreadable> {
    let text = text_of(stdout)?;
    let mut names = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !valid_name(line) {
            return Err(Unreadable("a listed name was not a formula name".into()));
        }
        names.push(line.to_string());
    }
    Ok(names)
}

/// The `(subject, line)` pairs of mise's `... is prunable: ...` lines.
/// The line is the manager's own sentence, kept verbatim; any other line
/// (the action lines of the dry run) is not a report and is skipped.
pub fn parse_mise_prune(output: &[u8]) -> Result<Vec<(String, String)>, Unreadable> {
    let text = text_of(output)?;
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("mise ") else {
            continue;
        };
        let Some((subject, reason)) = rest.split_once(" is prunable: ") else {
            continue;
        };
        if !valid_name(subject) || !subject.contains('@') || reason.trim().is_empty() {
            continue;
        }
        out.push((subject.to_string(), line.to_string()));
    }
    Ok(out)
}

/// The tools of `mise ls --global --json`: `(tool, source file,
/// requested version)`.
pub fn parse_mise_global(stdout: &[u8]) -> Result<Vec<(String, String, String)>, Unreadable> {
    let text = text_of(stdout)?;
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| Unreadable("the output was not JSON".into()))?;
    let Some(map) = value.as_object() else {
        return Err(Unreadable("the JSON was not a table of tools".into()));
    };
    let mut out = Vec::new();
    for (tool, entries) in map {
        let Some(entries) = entries.as_array() else {
            return Err(Unreadable("a tool's entry was not a list".into()));
        };
        if tool.is_empty() || tool.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(Unreadable("a tool name was not a name".into()));
        }
        let source = entries
            .iter()
            .find_map(|e| e.pointer("/source/path").and_then(|p| p.as_str()))
            .unwrap_or("")
            .to_string();
        let requested = entries
            .iter()
            .find_map(|e| e.get("requested_version").and_then(|p| p.as_str()))
            .unwrap_or("")
            .to_string();
        out.push((tool.clone(), source, requested));
    }
    Ok(out)
}

// ---------------------------------------------------------------------
// The pass.
// ---------------------------------------------------------------------

/// How a probe's command is run. The real one is the gate's spawn; a
/// test hands in a fake that answers, times out, or fails.
pub trait ProbeRunner {
    fn run(&self, command: ManagerCommand, timeout: Duration) -> std::io::Result<RunOutput>;
    /// The text of a settings file, or why it could not be read.
    fn read_settings(&self, path: &std::path::Path) -> Result<String, String>;
}

/// The gate's spawn and bounded file read.
pub struct SystemProbeRunner;

impl ProbeRunner for SystemProbeRunner {
    fn run(&self, command: ManagerCommand, timeout: Duration) -> std::io::Result<RunOutput> {
        crate::fs_gate::spawn::run_manager(command, timeout)
    }

    fn read_settings(&self, path: &std::path::Path) -> Result<String, String> {
        crate::fs_gate::read::bounded_string(path, crate::fs_gate::read::BoundedCap::LOCKFILE)
            .map_err(|e| e.to_string())
    }
}

fn row(
    manager: &str,
    probe: &str,
    kind: FactKind,
    subject: Option<String>,
    text: String,
    now: u64,
) -> ManagerFact {
    ManagerFact {
        manager: manager.to_string(),
        probe: probe.to_string(),
        kind,
        subject,
        text,
        observed_at: now,
    }
}

/// Runs one command probe and turns its answer into rows: the facts and
/// one `checked` row, or one `not-observed` row saying why.
fn run_probe(
    runner: &dyn ProbeRunner,
    decl: &ManagerDecl,
    probe: ManagerProbe,
    command: ManagerCommand,
    timeout: Duration,
    now: u64,
) -> Vec<ManagerFact> {
    let program = command.program();
    let manager = decl.manager;
    let label = probe.label();
    let not_observed =
        |why: String| vec![row(manager, label, FactKind::NotObserved, None, why, now)];
    let out = match runner.run(command, timeout) {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return not_observed(format!(
                "{} is not installed or not on PATH",
                program.binary()
            ));
        }
        Err(e) => {
            return not_observed(format!(
                "{} could not be run ({})",
                program.binary(),
                e.kind()
            ));
        }
    };
    if out.timed_out {
        return not_observed(format!(
            "{} did not answer within {} seconds and was stopped",
            program.binary(),
            timeout.as_secs()
        ));
    }
    if out.code != Some(0) {
        return not_observed(match out.code {
            Some(c) => format!("{} exited with status {c}", program.binary()),
            None => format!("{} was ended by a signal", program.binary()),
        });
    }
    let mut facts: Vec<ManagerFact> = Vec::new();
    let parsed: Result<(), Unreadable> = match probe {
        ManagerProbe::BrewAutoremoveDryRun => {
            parse_brew_autoremove(&out.stdout).map(|(header, names)| {
                for n in names {
                    facts.push(row(
                        manager,
                        label,
                        probe.kind(),
                        Some(n),
                        header.clone(),
                        now,
                    ));
                }
            })
        }
        ManagerProbe::BrewInstalledOnRequest => parse_name_lines(&out.stdout).map(|names| {
            for n in names {
                facts.push(row(
                    manager,
                    label,
                    probe.kind(),
                    Some(n),
                    "installed on request".to_string(),
                    now,
                ));
            }
        }),
        ManagerProbe::MisePruneDryRun => {
            // mise prints its dry run on standard error.
            let mut all = out.stderr.clone();
            all.extend_from_slice(&out.stdout);
            parse_mise_prune(&all).map(|pairs| {
                for (subject, line) in pairs {
                    facts.push(row(manager, label, probe.kind(), Some(subject), line, now));
                }
            })
        }
        ManagerProbe::MiseGlobalTools => parse_mise_global(&out.stdout).map(|tools| {
            for (tool, source, requested) in tools {
                let text = match (source.is_empty(), requested.is_empty()) {
                    (false, false) => format!(
                        "listed in the global configuration {source} (requested {requested})"
                    ),
                    (false, true) => format!("listed in the global configuration {source}"),
                    _ => "listed in the global configuration".to_string(),
                };
                facts.push(row(manager, label, probe.kind(), Some(tool), text, now));
            }
        }),
        ManagerProbe::SettingsDefault => Ok(()),
    };
    match parsed {
        Ok(()) => {
            facts.push(row(
                manager,
                label,
                FactKind::Checked,
                None,
                String::new(),
                now,
            ));
            facts
        }
        Err(Unreadable(why)) => not_observed(format!(
            "{} answered, but it could not be read: {why}",
            program.binary()
        )),
    }
}

/// The settings-file probe: the default the manager's own settings name.
fn settings_probe(
    runner: &dyn ProbeRunner,
    registry: &Registry,
    decl: &ManagerDecl,
    units: &[ExternalUnit],
    now: u64,
) -> Vec<ManagerFact> {
    let manager = decl.manager;
    let label = ManagerProbe::SettingsDefault.label();
    let not_observed =
        |why: String| vec![row(manager, label, FactKind::NotObserved, None, why, now)];
    // The file and its shape are what the manager's detector declared for
    // the same purpose (`DeclaredVersions::global_default`).
    let Some((detector_id, file)) = registry.detectors().iter().find_map(|d| {
        d.manager().filter(|m| m.manager == manager)?;
        d.manager_conventions().iter().find_map(|c| match c.role {
            ConventionRole::DeclaredVersions {
                global_default: Some(g),
                ..
            } => Some((d.id(), g)),
            _ => None,
        })
    }) else {
        return not_observed("no settings file is declared for this manager".into());
    };
    let Some(state_unit) = units.iter().find(|u| {
        u.detector_id == detector_id && u.category == crate::locations::StorageCategory::LocalState
    }) else {
        return not_observed("the manager's home was not measured".into());
    };
    let path = state_unit.path.join(file.file_name);
    let text = match runner.read_settings(&path) {
        Ok(t) => t,
        Err(e) => return not_observed(format!("{} could not be read ({e})", file.file_name)),
    };
    let declared = match file.format {
        GlobalDefaultFormat::TomlTopLevelString => match toml::from_str::<toml::Table>(&text) {
            Ok(table) => table
                .get(file.field)
                .and_then(|v| v.as_str())
                .map(str::to_string),
            Err(_) => {
                return not_observed(format!("{} is not valid TOML", file.file_name));
            }
        },
    };
    let mut out = Vec::new();
    // Toolchains the settings pin for a directory are held out too.
    if let Ok(table) = toml::from_str::<toml::Table>(&text)
        && let Some(overrides) = table.get("overrides").and_then(|v| v.as_table())
    {
        let mut pins: Vec<(&String, &str)> = overrides
            .iter()
            .filter_map(|(dir, v)| Some((dir, v.as_str()?)))
            .collect();
        pins.sort();
        for (dir, name) in pins {
            out.push(row(
                manager,
                label,
                FactKind::PinnedByOverride,
                Some(name.to_string()),
                format!("pinned for {dir} in {} [overrides]", file.file_name),
                now,
            ));
        }
    }
    if let Some(name) = declared {
        out.push(row(
            manager,
            label,
            FactKind::ActiveDefault,
            Some(name.clone()),
            format!("{} \"{name}\" in {}", file.field, file.file_name),
            now,
        ));
    }
    out.push(row(
        manager,
        label,
        FactKind::Checked,
        None,
        String::new(),
        now,
    ));
    out
}

/// One scheduled pass: every manager that owns a measured unit is asked
/// its own read-only questions, each bounded by [`PROBE_TIMEOUT`] and the
/// pass by [`PASS_BUDGET`]. The result replaces the stored table whole.
pub fn collect(units: &[ExternalUnit], runner: &dyn ProbeRunner, now: u64) -> Vec<ManagerFact> {
    collect_within(units, runner, now, PASS_BUDGET, PROBE_TIMEOUT)
}

/// [`collect`] with the pass budget and the per-command limit named, so
/// a test can spend the budget without waiting for it.
pub fn collect_within(
    units: &[ExternalUnit],
    runner: &dyn ProbeRunner,
    now: u64,
    budget: Duration,
    probe_timeout: Duration,
) -> Vec<ManagerFact> {
    let registry = Registry::with_builtins();
    let started = Instant::now();
    let mut rows = vec![row("", "", FactKind::Pass, None, String::new(), now)];
    // One entry per manager, with the union of its detectors' probes, for
    // the managers that own at least one measured unit.
    let mut managers: Vec<(ManagerDecl, Vec<ManagerProbe>)> = Vec::new();
    for detector in registry.detectors() {
        let Some(decl) = detector.manager() else {
            continue;
        };
        let published: Vec<(crate::locations::StorageCategory, &std::path::Path)> = units
            .iter()
            .filter(|u| u.detector_id == detector.id())
            .map(|u| (u.category, u.path.as_path()))
            .collect();
        if decl.anchor.select(&published).is_empty() {
            continue;
        }
        match managers.iter_mut().find(|(d, _)| d.manager == decl.manager) {
            Some((_, probes)) => {
                for p in decl.probes {
                    if !probes.contains(p) {
                        probes.push(*p);
                    }
                }
            }
            None => managers.push((decl, decl.probes.to_vec())),
        }
    }
    for (decl, probes) in &managers {
        for probe in probes {
            let remaining = budget.saturating_sub(started.elapsed());
            let Some(command) = probe.invocation() else {
                rows.extend(settings_probe(runner, &registry, decl, units, now));
                continue;
            };
            if remaining < Duration::from_millis(1) {
                rows.push(row(
                    decl.manager,
                    probe.label(),
                    FactKind::NotObserved,
                    None,
                    "the time budget for this pass was spent before it ran".into(),
                    now,
                ));
                continue;
            }
            rows.extend(run_probe(
                runner,
                decl,
                *probe,
                command,
                remaining.min(probe_timeout),
                now,
            ));
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brew_autoremove_lists_names_under_the_managers_own_header() {
        let out = b"Would autoremove 4 unneeded formulae:\nlibevent\nlibnghttp2\nunbound\nusage\n";
        let (header, names) = parse_brew_autoremove(out).unwrap();
        assert_eq!(header, "Would autoremove 4 unneeded formulae:");
        assert_eq!(names, vec!["libevent", "libnghttp2", "unbound", "usage"]);
    }

    #[test]
    fn brew_autoremove_with_nothing_to_list_is_a_list_of_nothing() {
        assert_eq!(parse_brew_autoremove(b"").unwrap(), (String::new(), vec![]));
        assert_eq!(
            parse_brew_autoremove(b"\n\n").unwrap(),
            (String::new(), vec![])
        );
    }

    #[test]
    fn brew_autoremove_refuses_garbage_and_a_count_that_does_not_match() {
        assert!(parse_brew_autoremove(b"Error: something else entirely\n").is_err());
        assert!(parse_brew_autoremove(b"\x00\x01\x02").is_err());
        assert!(parse_brew_autoremove(&[0xff, 0xfe, 0xfd]).is_err());
        assert!(
            parse_brew_autoremove(b"Would autoremove 3 unneeded formulae:\na\nb\n").is_err(),
            "a header that promises three names over two is not read"
        );
        assert!(
            parse_brew_autoremove(b"Would autoremove 1 unneeded formulae:\nnot a name; x\n")
                .is_err()
        );
        let huge = vec![b'a'; MAX_OUTPUT + 1];
        assert!(parse_brew_autoremove(&huge).is_err());
    }

    #[test]
    fn mise_prune_keeps_the_managers_own_sentence_and_skips_action_lines() {
        let out = "mise pruned configuration links [dryrun]\n\
mise poetry@2.1.3 is prunable: no tracked config or tool stub requires poetry\n\
mise poetry@2.1.3 [dryrun]  uninstall\n\
mise poetry@2.1.3 [dryrun]  remove ~/.local/share/mise/installs/poetry/2.1.3\n";
        let got = parse_mise_prune(out.as_bytes()).unwrap();
        assert_eq!(
            got,
            vec![(
                "poetry@2.1.3".to_string(),
                "mise poetry@2.1.3 is prunable: no tracked config or tool stub requires poetry"
                    .to_string()
            )]
        );
    }

    #[test]
    fn mise_global_reads_the_tools_and_their_source() {
        let json = br#"{"node":[{"version":"24.1","requested_version":"latest","source":{"type":"mise.toml","path":"/h/.config/mise/config.toml"}}]}"#;
        assert_eq!(
            parse_mise_global(json).unwrap(),
            vec![(
                "node".to_string(),
                "/h/.config/mise/config.toml".to_string(),
                "latest".to_string()
            )]
        );
        assert!(parse_mise_global(b"not json").is_err());
        assert!(parse_mise_global(b"[1,2]").is_err());
    }

    #[test]
    fn subjects_join_folders_by_the_declared_shape_only() {
        use SubjectShape::*;
        assert!(subject_matches(FolderName, "libevent", "libevent"));
        assert!(!subject_matches(FolderName, "libevent", "libevent2"));
        assert!(subject_matches(NameBeforeAt, "poetry@2.1.3", "poetry"));
        assert!(!subject_matches(NameBeforeAt, "poetry@2.1.3", "poet"));
        assert!(subject_matches(
            ChannelWithHostTriple,
            "stable",
            "stable-aarch64-apple-darwin"
        ));
        assert!(subject_matches(
            ChannelWithHostTriple,
            "stable-aarch64-apple-darwin",
            "stable-aarch64-apple-darwin"
        ));
        assert!(!subject_matches(
            ChannelWithHostTriple,
            "stable",
            "nightly-aarch64-apple-darwin"
        ));
    }

    /// The tempting wrong patch: split the name at the first dash. Host
    /// triples and dates contain dashes.
    #[test]
    fn toolchain_names_are_parsed_not_split_at_the_first_dash() {
        let t = |n| parse_toolchain(n).unwrap();
        assert_eq!(t("stable").host, None);
        let x = t("nightly-2024-01-01-aarch64-apple-darwin");
        assert_eq!(
            (x.channel, x.date, x.host),
            ("nightly", Some("2024-01-01"), Some("aarch64-apple-darwin"))
        );
        let x = t("1.90.0-x86_64-unknown-linux-gnu");
        assert_eq!(
            (x.channel, x.date, x.host),
            ("1.90.0", None, Some("x86_64-unknown-linux-gnu"))
        );
        let x = t("1.90-aarch64-apple-darwin");
        assert_eq!((x.channel, x.host), ("1.90", Some("aarch64-apple-darwin")));
        assert_eq!(t("nightly-2024-01-01").host, None);
        assert!(parse_toolchain("my-linked").is_none());
        use SubjectShape::ChannelWithHostTriple as C;
        assert!(subject_matches(
            C,
            "nightly-2024-01-01",
            "nightly-2024-01-01-aarch64-apple-darwin"
        ));
        assert!(!subject_matches(
            C,
            "nightly-2024-01-01",
            "nightly-2024-01-02-aarch64-apple-darwin"
        ));
        assert!(!subject_matches(
            C,
            "nightly",
            "nightly-2024-01-01-aarch64-apple-darwin"
        ));
        assert!(subject_matches(
            C,
            "nightly",
            "nightly-aarch64-apple-darwin"
        ));
        assert!(subject_matches(
            C,
            "stable-aarch64-apple-darwin",
            "stable-aarch64-apple-darwin"
        ));
        assert!(!subject_matches(
            C,
            "stable-x86_64-apple-darwin",
            "stable-aarch64-apple-darwin"
        ));
        assert!(subject_matches(C, "1.90", "1.90-aarch64-apple-darwin"));
        assert!(!subject_matches(C, "1.9", "1.90-aarch64-apple-darwin"));
    }

    /// Each mise backend prefix maps to the folder mise itself makes.
    #[test]
    fn mise_backend_tools_join_their_install_folders() {
        use SubjectShape::NameBeforeAt as N;
        for (tool, folder) in [
            ("npm:prettier", "npm-prettier"),
            ("npm:@scope/pkg", "npm-scope-pkg"),
            ("ubi:BurntSushi/ripgrep", "ubi-burnt-sushi-ripgrep"),
            ("cargo:foo_bar", "cargo-foo-bar"),
            ("github:owner/repo[bin=x]", "github-owner-repo"),
            ("cargo:ripgrep", "cargo-ripgrep"),
            ("aqua:cli/cli", "aqua-cli-cli"),
            ("github:owner/repo", "github-owner-repo"),
            ("ubi:owner/repo", "ubi-owner-repo"),
            ("pipx:black", "pipx-black"),
            ("go:golang.org/x/tools/gopls", "go-golang-org-x-tools-gopls"),
            ("gem:rubocop", "gem-rubocop"),
            ("asdf:plugin", "asdf-plugin"),
            ("node", "node"),
        ] {
            assert!(subject_matches(N, tool, folder), "{tool}");
            assert!(
                subject_matches(N, &format!("{tool}@1.2.3"), folder),
                "{tool}@version"
            );
        }
        assert!(!subject_matches(N, "npm:prettier", "npm-eslint"));
    }

    /// A tap formula is named in full and lives under its short name.
    #[test]
    fn tap_and_versioned_formulae_join_their_cellar_folder() {
        use SubjectShape::FolderName as F;
        assert!(subject_matches(F, "user/tap/foo", "foo"));
        assert!(subject_matches(F, "llvm@20", "llvm@20"));
        assert!(!subject_matches(F, "llvm@20", "llvm"));
        assert!(!subject_matches(F, "user/tap/foo", "bar"));
    }

    /// The header is printed through Homebrew's `oh1` formatter, arrow
    /// first, singular or plural, with or without colour.
    #[test]
    fn brew_autoremove_real_shapes() {
        let (h, n) = parse_brew_autoremove(
            b"==> Would autoremove 1 unneeded formula:
libevent
",
        )
        .unwrap();
        assert_eq!(h, "Would autoremove 1 unneeded formula:");
        assert_eq!(n, vec!["libevent"]);
        let (_, n) = parse_brew_autoremove(
            b"\x1b[34m==>\x1b[0m \x1b[1mWould autoremove 2 unneeded formulae:\x1b[0m\nuser/tap/foo\nunbound\n",
        )
        .unwrap();
        assert_eq!(n, vec!["user/tap/foo", "unbound"]);
        assert_eq!(parse_brew_autoremove(b"").unwrap().1, Vec::<String>::new());
    }

    #[test]
    fn every_kind_label_round_trips() {
        for k in [
            FactKind::ReportsUnneeded,
            FactKind::ReportsPrunable,
            FactKind::ActiveDefault,
            FactKind::InstalledOnRequest,
            FactKind::Checked,
            FactKind::NotObserved,
            FactKind::Pass,
        ] {
            assert_eq!(FactKind::from_label(k.label()), Some(k));
        }
        assert_eq!(FactKind::from_label("something-else"), None);
    }
}
