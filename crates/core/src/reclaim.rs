//! The Reclaim view: one row per unit of developer storage, largest
//! first, drilled one level down, each with what getting it back costs,
//! when it was last used and from what record, what is known about who
//! needs it, and which removal path exists. `swamp report --view
//! reclaim`, its JSON, and the TUI's Reclaim view are all this one
//! value, built from stored facts by [`build`].
//!
//! **A pure function of stored facts.** [`build`] starts no process,
//! lists no directory and stats nothing: everything it says was recorded
//! by an observation (units, drilldowns, last-used facts, evidence, the
//! build adapters' consequence text) or by the scheduled manager pass
//! (`manager_facts`). A test counts work and asserts zero.
//!
//! **Facts, attributed.** No row says a unit is unused, obsolete,
//! orphaned or safe. A package manager's own statement is quoted
//! verbatim beside the manager's name ("Homebrew reports unneeded (brew
//! autoremove)"), and swamp adds nothing to it.
//!
//! **Consumer evidence is scoped and fails closed.** Every listing says
//! what the consumer evidence was checked against ("consumer evidence
//! checked against 40 projects in 2 declared roots"). A declared root
//! that is missing or unreadable, or no declared root at all, makes it
//! incomplete and says so; no row ever reads as needing no consumer,
//! because a tool used only by a project outside those roots would look
//! exactly like that.
//!
//! **Defaults are held out.** A rustup default toolchain, a tool in
//! mise's global configuration and a formula installed on request are
//! marked as such and excluded from the regenerable total. When the
//! manager's own record of that could not be read, the row says
//! `unknown` and is excluded the same way.
//!
//! The JSON shape is documented in `docs/usage.md` ("The Reclaim view");
//! its `totals` object is what the storage headline builds on.

use crate::drilldown::{ChildKind, ChildMeasure, UnitChild};
use crate::evidence::{
    EvidenceSource, FactKind as EvidenceKind, FactStatus, FactSubtype, FactValue,
};
use crate::external::ExternalUnit;
use crate::last_used::LastUsed;
use crate::locations::{ManagerDecl, ManagerProbe, RegenClass, Registry, StorageCategory};
use crate::manager_facts::{self, FactKind, ManagerFact, ManagerFacts};
use crate::report::{UnownedReason, UnownedRow};
use crate::roots::{DeclaredRoot, DeclaredState};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// The kind label of a standalone Cargo target directory.
pub const KIND_STANDALONE_CARGO_TARGET: &str = "standalone-cargo-target";

/// Manager quotes shown on one row before "and N more" (the JSON keeps
/// every one).
const QUOTES_SHOWN: usize = 4;
/// Names listed in a row's consumer sentence before "and N more".
const NAMES_SHOWN: usize = 4;

/// What the consumer evidence was checked against, and whether that is
/// the whole of what a person declared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScopeStatement {
    pub projects: usize,
    pub declared_roots: usize,
    /// False when a declared root is missing, unreadable, partly read or
    /// excluded, or when no root is declared at all.
    pub complete: bool,
    pub incomplete_because: Vec<String>,
    /// The sentence every listing prints.
    pub statement: String,
}

/// Builds the scope statement from the stored project count and the
/// declared roots' states. `explicit` is true when the scope is a root
/// named on the command line, which replaces the declared roots.
pub fn scope_statement(projects: usize, roots: &[DeclaredRoot], explicit: bool) -> ScopeStatement {
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            one.to_string()
        } else {
            many.to_string()
        }
    };
    let projects_word = plural(projects, "project", "projects");
    if explicit {
        return ScopeStatement {
            projects,
            declared_roots: 0,
            complete: false,
            incomplete_because: vec![
                "the root named on the command line replaces the declared roots".to_string(),
            ],
            statement: format!(
                "consumer evidence checked against {projects} {projects_word} under the root named on the command line; incomplete: the declared roots were not used; a tool used only outside what was checked appears here with none listed"
            ),
        };
    }
    let mut because: Vec<String> = Vec::new();
    for r in roots {
        match &r.state {
            DeclaredState::Missing => because.push(format!("{} is missing", r.path.display())),
            DeclaredState::Unreadable { .. } => {
                because.push(format!("{} is unreadable", r.path.display()))
            }
            DeclaredState::Present {
                complete: false, ..
            } => because.push(format!("{} was only partly read", r.path.display())),
            DeclaredState::Excluded { .. } => {
                because.push(format!("{} is excluded from the scan", r.path.display()))
            }
            DeclaredState::Present { .. } | DeclaredState::CoveredBy { .. } => {}
        }
    }
    if roots.is_empty() {
        because.push("no source roots are declared".to_string());
    }
    let complete = because.is_empty();
    let statement = if roots.is_empty() {
        format!(
            "consumer evidence covers the built-in default roots only ({projects} {projects_word}); declare your source roots with `swamp config add-root`"
        )
    } else {
        let base = format!(
            "consumer evidence checked against {projects} {projects_word} in {} declared {}",
            roots.len(),
            plural(roots.len(), "root", "roots"),
        );
        if complete {
            base
        } else {
            format!(
                "{base}; incomplete: {}; a tool used only outside what was checked appears here with none listed",
                because.join("; ")
            )
        }
    };
    ScopeStatement {
        projects,
        declared_roots: roots.len(),
        complete,
        incomplete_because: because,
        statement,
    }
}

/// How a unit's regeneration cost was worded, and from where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Regeneration {
    pub class: RegenClass,
    pub words: String,
    /// `tool consequence text`, `detector recovery hint` or `category
    /// default`: which of the three sources the words came from, in that
    /// order of precedence.
    pub source: String,
}

/// A consumer reference with the record it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConsumerRef {
    pub label: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Who is known to need a unit, in the two tiers the evidence contract
/// keeps apart, plus what could not be established.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Consumers {
    /// Tier one: declared by a project or a person.
    pub declared: Vec<ConsumerRef>,
    /// Tier two: a link the tool itself recorded.
    pub recorded_links: Vec<ConsumerRef>,
    /// Consumer facts a source could not establish, with its reason.
    pub unknown: Vec<String>,
    /// The sentence a row shows, scope included.
    pub summary: String,
}

/// A package manager's own statement about one subject, verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManagerQuote {
    pub manager: String,
    pub subject: String,
    /// The manager's words, unchanged.
    pub quote: String,
    /// Who said it: `Homebrew reports unneeded (brew autoremove)`.
    pub attribution: String,
    /// When the pass that recorded it ran.
    pub observed_at: u64,
    /// Whole days between that pass and the observation this listing
    /// shows (0 when it ran with or after it).
    pub days_before_listing: u64,
    /// The pass is older than the observation beside it: a later
    /// observation did not run one.
    pub older_than_listing: bool,
    /// Seconds between that pass and this listing's observation (0 when
    /// the pass ran with or after it).
    pub seconds_before_listing: u64,
    /// The pass ran after this listing's observation (the usual case: it
    /// runs when an observation finishes).
    pub after_listing: bool,
    /// What the statement covers when it names less than the folder it
    /// sits on (one version of a tool).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub covers: Option<String>,
}

impl ManagerQuote {
    /// The line a row shows: the attribution, the manager's words, and
    /// when it said them.
    pub fn line(&self) -> String {
        let age = if self.older_than_listing {
            format!(
                "quoted {} before this listing; older than this listing",
                age_words(self.seconds_before_listing)
            )
        } else if self.after_listing {
            "quoted from the pass right after this listing".to_string()
        } else {
            "quoted from the pass beside this listing".to_string()
        };
        let covers = self
            .covers
            .as_deref()
            .map(|c| format!("; {c}"))
            .unwrap_or_default();
        format!("{}: \"{}\" ({age}{covers})", self.attribution, self.quote)
    }
}

/// A span as a person reads it: `20 minutes`, `2 hours`, `10 days`.
pub fn age_words(secs: u64) -> String {
    let (n, unit) = if secs < 3_600 {
        ((secs / 60).max(1), "minute")
    } else if secs < 2 * 86_400 {
        (secs / 3_600, "hour")
    } else {
        (secs / 86_400, "day")
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// A hold read from a pass this much older than the observation is not
/// trusted: the standing is unknown, and unknown is held out.
const HOLD_MAX_AGE_SECS: u64 = 86_400;

/// A pass this much older than the observation is not "beside" it.
const SAME_PASS_SLACK_SECS: u64 = 3_600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldKind {
    ActiveDefault,
    /// A toolchain the manager's settings pin for a directory.
    PinnedByOverride,
    InstalledOnRequest,
    /// The manager's own record of what is a default could not be read.
    Unknown,
}

/// Why a row (or a folder of it) is excluded from the regenerable total.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hold {
    pub kind: HoldKind,
    /// `active default (settings.toml default_toolchain)`, `installed on
    /// request (brew list --installed-on-request)` or `unknown (...)`.
    pub label: String,
    pub subjects: Vec<String>,
    /// True when the whole unit is held: it is itself the subject, or a
    /// subject could not be separated from it.
    pub whole_unit: bool,
}

impl Hold {
    /// The two or three words a narrow row carries beside the name.
    pub fn short(&self) -> &'static str {
        match self.kind {
            HoldKind::ActiveDefault => "active default",
            HoldKind::PinnedByOverride => "pinned by override",
            HoldKind::InstalledOnRequest => "installed on request",
            HoldKind::Unknown => "standing unknown",
        }
    }
}

/// Which removal path exists for a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalKind {
    /// The reviewed move to Trash, in the TUI (Space marks, Backspace
    /// opens the confirm).
    TrashReviewed,
    /// A unit whose manager swamp runs removal for (mise, simctl): the
    /// reviewed move to Trash, or the manager's own command.
    TrashOrToolCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Removal {
    pub kind: RemovalKind,
    /// For the CLI report and JSON: keys are named as the TUI's.
    pub text: String,
    /// The TUI's own wording, for its detail pane (not in the JSON).
    #[serde(skip)]
    pub tui_text: String,
}

fn removal(kind: RemovalKind, manager: Option<&str>) -> Removal {
    let (text, tui_text) = match (kind, manager) {
        (RemovalKind::TrashOrToolCommand, Some(m)) => (
            format!(
                "in the TUI: Space then Backspace moves to Trash; Backspace with nothing marked runs {m}'s own command (permanent)"
            ),
            format!(
                "Trash (Space, then Backspace); {m}'s own command, permanent (Backspace, nothing marked)"
            ),
        ),
        _ => (
            "in the TUI: Space then Backspace moves to Trash".to_string(),
            "Trash (Space, then Backspace)".to_string(),
        ),
    };
    Removal {
        kind,
        text,
        tui_text,
    }
}

/// One line of a unit's drilldown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReclaimChild {
    pub kind: ChildKind,
    pub name: String,
    /// Allocated bytes; `None` exactly for a folder that was not
    /// measured (not zero).
    pub bytes: Option<i64>,
    pub measure: ChildMeasure,
    pub last_used: LastUsed,
    /// The last-used fact as text; `None` for the remainder and
    /// adjustment rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_text: Option<String>,
    /// The line a listing shows after the size.
    pub text: String,
    /// What the folder is, when a model store's adapter read it: the
    /// one-line card summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    pub manager: Vec<ManagerQuote>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold: Option<Hold>,
}

/// One unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReclaimRow {
    pub path: String,
    /// The unit's category (`installation`, `cache`, ...) or
    /// `standalone-cargo-target`.
    pub kind: String,
    pub detector: String,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub growth_bytes: Option<i64>,
    pub regeneration: Regeneration,
    pub last_used: LastUsed,
    pub last_used_text: String,
    pub consumers: Consumers,
    pub manager: Vec<ManagerQuote>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold: Option<Hold>,
    pub removal: Removal,
    /// The part of `bytes` in the regenerable total.
    pub regenerable_bytes: u64,
    /// The part of `bytes` held out of it (defaults, requested installs,
    /// unknown standing).
    pub held_bytes: u64,
    /// A coverage note on this unit's measurement (a lower bound).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub children: Vec<ReclaimChild>,
    /// A model store's models, one per repo or model:tag
    /// (`build_adapters::model_stores::model_rows`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<crate::build_adapters::model_stores::ModelRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KindTotal {
    pub kind: String,
    pub count: usize,
    pub bytes: u64,
    pub regenerable_bytes: u64,
}

/// The numbers a storage headline reuses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Totals {
    pub count: usize,
    /// Every row's allocated bytes: `regenerable_bytes + held_bytes +
    /// not_regenerable_bytes + not_established_bytes`.
    pub bytes: u64,
    /// Rows whose kind can be fetched or rebuilt again, less what is
    /// held out below.
    pub regenerable_bytes: u64,
    /// Bytes of regenerable kinds held out: a default, an install made on
    /// request, or a standing that could not be read.
    pub held_bytes: u64,
    pub not_regenerable_bytes: u64,
    pub not_established_bytes: u64,
    /// Bytes of the rows that are the remainder of a location after its
    /// developer units (Homebrew's "other", selected by
    /// `Detector::remainder_of`): listed here, never counted as developer
    /// storage. Part of `bytes`, and of the four classes above.
    pub remainder_bytes: u64,
    pub per_kind: Vec<KindTotal>,
    pub scope_statement: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReclaimView {
    pub observed_at: u64,
    pub scope: ScopeStatement,
    pub totals: Totals,
    /// What was not observed, said plainly.
    pub coverage_notes: Vec<String>,
    pub rows: Vec<ReclaimRow>,
}

/// Everything [`build`] reads. All of it comes from the stored
/// observation.
pub struct ReclaimInput<'a> {
    pub units: &'a [ExternalUnit],
    pub interiors: &'a [crate::artifact::NestedArtifact],
    pub unowned: &'a [UnownedRow],
    pub manager_facts: &'a ManagerFacts,
    pub declared_roots: &'a [DeclaredRoot],
    /// True when the scope is a root named on the command line, which
    /// replaces the declared roots.
    pub explicit_scope: bool,
    pub projects: usize,
    pub observed_at: u64,
}

fn source_text(s: &EvidenceSource) -> String {
    match s {
        EvidenceSource::FilesystemMetadata { detail } => format!("file metadata ({detail})"),
        EvidenceSource::ToolReported { tool, detail } => format!("{tool} ({detail})"),
        EvidenceSource::ProcessQuery { tool } => format!("{tool} query"),
        EvidenceSource::ManagerLock { tool, path } => format!("{tool} lock {path}"),
        EvidenceSource::ConfigDeclaration { path } => format!("declared in {path}"),
        EvidenceSource::Lockfile { ecosystem, path } => format!("{ecosystem} lockfile {path}"),
        EvidenceSource::BuildMetadata { path } => format!("recorded in {path}"),
        EvidenceSource::DockerApi { detail } => format!("Docker ({detail})"),
        EvidenceSource::Statvfs => "free-space accounting".to_string(),
        EvidenceSource::Inferred { basis } => basis.clone(),
    }
}

/// Splits a unit's consumer evidence into the two tiers, and lists what a
/// source could not establish.
fn consumer_refs(
    evidence: &[crate::evidence::Evidence],
    associations: &[crate::external::ExternalConsumer],
) -> (Vec<ConsumerRef>, Vec<ConsumerRef>, Vec<String>) {
    let mut declared: Vec<ConsumerRef> = Vec::new();
    let mut recorded: Vec<ConsumerRef> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let push = |list: &mut Vec<ConsumerRef>, r: ConsumerRef| {
        if !list.iter().any(|x| x.label == r.label) {
            list.push(r);
        }
    };
    for e in evidence.iter().filter(|e| e.kind == EvidenceKind::Consumer) {
        match &e.status {
            FactStatus::Known(v) => {
                let labels: Vec<String> = match v {
                    FactValue::Text(t) => vec![t.clone()],
                    FactValue::List(l) => l.clone(),
                    _ => Vec::new(),
                };
                for label in labels {
                    let r = ConsumerRef {
                        label,
                        source: source_text(&e.source),
                        note: e.note.clone(),
                    };
                    if e.subtype == FactSubtype::RecordedLink {
                        push(&mut recorded, r);
                    } else {
                        push(&mut declared, r);
                    }
                }
            }
            FactStatus::Unknown { reason } | FactStatus::Unavailable { reason } => {
                let r = reason.to_string();
                if !unknown.contains(&r) {
                    unknown.push(r);
                }
            }
            FactStatus::Conflicting { reason, .. } => {
                let r = reason.to_string();
                if !unknown.contains(&r) {
                    unknown.push(r);
                }
            }
        }
    }
    for c in associations {
        push(
            &mut declared,
            ConsumerRef {
                label: c.label.clone(),
                source: "consumer association".to_string(),
                note: c.note.clone(),
            },
        );
    }
    (declared, recorded, unknown)
}

fn names(list: &[ConsumerRef]) -> String {
    // A manager's own global default is a declaration too, but not a
    // project's: say which it is.
    let shown: Vec<String> = list
        .iter()
        .take(NAMES_SHOWN)
        .map(|r| match r.note.as_deref() {
            Some(n) if n.starts_with("global default") => format!("{} (global default)", r.label),
            _ => r.label.clone(),
        })
        .collect();
    let rest = list.len().saturating_sub(NAMES_SHOWN);
    if rest > 0 {
        format!("{} and {rest} more", shown.join(", "))
    } else {
        shown.join(", ")
    }
}

fn consumers_for(
    evidence: &[crate::evidence::Evidence],
    associations: &[crate::external::ExternalConsumer],
    scope: &ScopeStatement,
) -> Consumers {
    let (declared, recorded_links, unknown) = consumer_refs(evidence, associations);
    let mut parts: Vec<String> = Vec::new();
    if declared.is_empty() {
        // A statement about what was found and where it looked, never a
        // claim that nothing needs the unit.
        parts.push(format!(
            "declared consumers: none found among {} {} in {} declared {}{}",
            scope.projects,
            if scope.projects == 1 {
                "project"
            } else {
                "projects"
            },
            scope.declared_roots,
            if scope.declared_roots == 1 {
                "root"
            } else {
                "roots"
            },
            if scope.complete { "" } else { " (incomplete)" },
        ));
    } else {
        parts.push(format!("declared by {}", names(&declared)));
    }
    if !recorded_links.is_empty() {
        parts.push(format!("recorded links: {}", names(&recorded_links)));
    }
    if !unknown.is_empty() {
        parts.push(format!("not established: {}", unknown.join("; ")));
    }
    Consumers {
        declared,
        recorded_links,
        unknown,
        summary: parts.join("; "),
    }
}

/// The cost half of a unit's row: the tool's consequence text when a
/// build adapter identified the unit's interior, else the detector's own
/// recovery hint, else the category default.
fn regeneration_of(
    u: &ExternalUnit,
    interiors: &[crate::artifact::NestedArtifact],
) -> Regeneration {
    let (class, default_words) = crate::locations::regeneration_for_category(u.category);
    if let Some((words, adapter)) = tool_consequence(u, interiors) {
        return Regeneration {
            class: class_from_consequence(&words),
            words,
            source: format!("tool consequence text ({adapter} build adapter)"),
        };
    }
    // A detector's hint describes its primary store, so it speaks only for
    // the kinds of storage a re-fetch or a rebuild restores.
    let hint_applies = matches!(
        u.category,
        StorageCategory::Installation
            | StorageCategory::Downloads
            | StorageCategory::Cache
            | StorageCategory::BuildOutput
    );
    if hint_applies && let Some(hint) = crate::locations::recovery_hint_of(&u.detector_id) {
        let (hint_class, words) = crate::locations::recovery_words(u.category, hint);
        return Regeneration {
            class: hint_class,
            words,
            source: "detector recovery hint".to_string(),
        };
    }
    Regeneration {
        class,
        words: default_words.to_string(),
        source: "category default".to_string(),
    }
}

/// The class a consequence text supports. The category's class is only a
/// default for units with no such text: the tool's own words win. Text that says the bytes are gone or
/// unique is not regenerable; text that says removal breaks something is
/// not established; text that names a download or a rebuild keeps the
/// category's class (rebuild wording picks Rebuild); anything else says
/// nothing about cost, so the class is not established.
pub fn class_from_consequence(words: &str) -> RegenClass {
    // "already-downloaded sources" names what stays on disk, not a fetch.
    let w = words.to_lowercase().replace("already-downloaded", "local");
    let has = |needles: &[&str]| needles.iter().any(|n| w.contains(n));
    if has(&[
        "are gone",
        "is gone",
        "cannot",
        "may be unique",
        "starts empty",
        "is lost",
    ]) {
        return RegenClass::NotRegenerable;
    }
    if has(&["breaks"]) {
        return RegenClass::NotEstablished;
    }
    if has(&[
        "download",
        "reinstall",
        "fetch",
        "unpacks",
        "install again",
        "clones the repository",
        "registry access",
        "index access",
        "network access",
        "proxy access",
    ]) {
        return RegenClass::Download;
    }
    if has(&[
        "rebuild",
        "recompil",
        "regenerat",
        "repackag",
        "cold build",
        "re-deriv",
        "recreat",
        "re-run",
        "re-lint",
        "re-analys",
        "re-optimiz",
        "re-execut",
        "re-resolv",
        "re-index",
        "reconfigur",
        "builds it again",
        "produces it again",
        "stages them again",
        "restage",
        "writes it again",
        "rewrites",
        "relink",
        "runs it again",
        "uploads its context",
        "type-checks every",
        "starts a new",
    ]) {
        return RegenClass::Rebuild;
    }
    RegenClass::NotEstablished
}

/// The consequence a build adapter stated for this unit's own interior:
/// the largest family's text, with a count of the others. Only an
/// interior whose container is this very unit speaks for it.
fn tool_consequence(
    u: &ExternalUnit,
    interiors: &[crate::artifact::NestedArtifact],
) -> Option<(String, String)> {
    let root = interiors.iter().find(|i| {
        i.present && i.path == u.path && Some(i.id.as_str()) == i.container_id.as_deref()
    })?;
    let inside: Vec<crate::artifact::NestedArtifact> = interiors
        .iter()
        .filter(|i| i.present && i.container_id == root.container_id)
        .cloned()
        .collect();
    let summary = crate::build_adapters::summarize_container(&root.path, &inside);
    let family = summary
        .families
        .iter()
        .filter(|f| f.consequence.is_some())
        .max_by_key(|f| f.bytes)?;
    let text = family.consequence.clone()?;
    let words = if family.other_consequences > 0 {
        format!(
            "{text} (and {} other consequences inside)",
            family.other_consequences
        )
    } else {
        text
    };
    Some((
        words,
        root.adapter.clone().unwrap_or_else(|| "unknown".into()),
    ))
}

fn quote(
    decl: &ManagerDecl,
    probe: ManagerProbe,
    f: &ManagerFact,
    listing_at: u64,
) -> ManagerQuote {
    let older = f.observed_at + SAME_PASS_SLACK_SECS < listing_at;
    let subject = f.subject.clone().unwrap_or_default();
    let covers = match (decl.subject, subject.rsplit_once('@')) {
        (crate::locations::SubjectShape::NameBeforeAt, Some((_, version)))
            if !version.is_empty() =>
        {
            Some(format!(
                "names version {version} only; other versions of this tool are not covered"
            ))
        }
        _ => None,
    };
    ManagerQuote {
        manager: decl.manager.to_string(),
        subject,
        quote: f.text.clone(),
        attribution: manager_facts::attribution(decl.display, probe, f.kind),
        observed_at: f.observed_at,
        days_before_listing: if older {
            (listing_at - f.observed_at) / 86_400
        } else {
            0
        },
        older_than_listing: older,
        seconds_before_listing: if older { listing_at - f.observed_at } else { 0 },
        after_listing: f.observed_at > listing_at,
        covers,
    }
}

/// What the managers said about one unit, split among the unit and its
/// listed folders.
#[derive(Default)]
struct ManagerJoin {
    unit_quotes: Vec<ManagerQuote>,
    child_quotes: HashMap<usize, Vec<ManagerQuote>>,
    unit_hold: Option<Hold>,
    child_holds: HashMap<usize, Hold>,
    /// Hold facts that name nothing this unit lists, kept only for a unit
    /// that stands for what the manager did not measure separately.
    unmatched_hold_subjects: Vec<String>,
    unmatched_hold_kind: Option<HoldKind>,
    /// The oldest age, before the listing, of a hold fact read.
    hold_age_secs: u64,
}

fn hold_kind_of(kind: FactKind) -> HoldKind {
    match kind {
        FactKind::InstalledOnRequest => HoldKind::InstalledOnRequest,
        FactKind::PinnedByOverride => HoldKind::PinnedByOverride,
        _ => HoldKind::ActiveDefault,
    }
}

fn merge_hold(slot: &mut Option<Hold>, kind: HoldKind, label: String, subject: &str, whole: bool) {
    match slot {
        Some(h) => {
            if !h.subjects.iter().any(|s| s == subject) {
                h.subjects.push(subject.to_string());
            }
            h.whole_unit |= whole;
        }
        None => {
            *slot = Some(Hold {
                kind,
                label,
                subjects: vec![subject.to_string()],
                whole_unit: whole,
            });
        }
    }
}

/// Whether some unit other than `index` that the same manager owns names
/// `subject` (by its own folder or by one of its listed folders): a
/// statement about it belongs there, not to a unit that only stands for
/// what nothing else claims.
fn claimed_elsewhere(
    units: &[ExternalUnit],
    managed: &HashMap<usize, ManagerDecl>,
    index: usize,
    manager: &str,
    subject: &str,
) -> bool {
    managed.iter().any(|(&j, d)| {
        j != index && d.manager == manager && {
            let other = &units[j];
            let folder = other
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            (!folder.is_empty() && manager_facts::subject_matches(d.subject, subject, folder))
                || other.children.iter().any(|c| {
                    c.kind == ChildKind::Entry
                        && manager_facts::subject_matches(d.subject, subject, &c.name)
                })
        }
    })
}

/// Joins the stored manager facts to one managed unit.
fn join_manager(
    units: &[ExternalUnit],
    managed: &HashMap<usize, ManagerDecl>,
    index: usize,
    facts: &ManagerFacts,
    listing_at: u64,
) -> ManagerJoin {
    let u = &units[index];
    let decl = &managed[&index];
    let mut join = ManagerJoin::default();
    let folder = u.path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    for probe in decl.probes {
        for f in facts.facts.iter().filter(|f| {
            f.manager == decl.manager && f.probe == probe.label() && probe.kinds().contains(&f.kind)
        }) {
            let kind = f.kind;
            let Some(subject) = f.subject.as_deref() else {
                continue;
            };
            let age = listing_at.saturating_sub(f.observed_at);
            if kind.holds() {
                join.hold_age_secs = join.hold_age_secs.max(age);
            }
            let mut label = manager_facts::attribution(decl.display, *probe, kind);
            if kind.holds() && age > SAME_PASS_SLACK_SECS {
                label.push_str(&format!(", read {} before this listing", age_words(age)));
            }
            let to_unit =
                !folder.is_empty() && manager_facts::subject_matches(decl.subject, subject, folder);
            let matched: Vec<usize> = u
                .children
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    c.kind == ChildKind::Entry
                        && manager_facts::subject_matches(decl.subject, subject, &c.name)
                })
                .map(|(i, _)| i)
                .collect();
            if to_unit {
                if kind.holds() {
                    merge_hold(
                        &mut join.unit_hold,
                        hold_kind_of(kind),
                        label,
                        subject,
                        true,
                    );
                } else {
                    join.unit_quotes.push(quote(decl, *probe, f, listing_at));
                }
            } else if !matched.is_empty() {
                for i in matched {
                    if kind.holds() {
                        let mut slot = join.child_holds.remove(&i);
                        merge_hold(&mut slot, hold_kind_of(kind), label.clone(), subject, false);
                        if let Some(h) = slot {
                            join.child_holds.insert(i, h);
                        }
                    } else {
                        join.child_quotes
                            .entry(i)
                            .or_default()
                            .push(quote(decl, *probe, f, listing_at));
                    }
                }
            } else if decl.catch_all
                && !claimed_elsewhere(units, managed, index, decl.manager, subject)
            {
                if kind.holds() {
                    // It may be inside what this unit lists only in
                    // aggregate (the remainder row, or a unit with no
                    // drilldown), or under a name swamp cannot map to a
                    // folder. Either way the standing is unknown, and
                    // unknown is held out: the whole unit.
                    join.unmatched_hold_subjects.push(subject.to_string());
                    join.unmatched_hold_kind = Some(hold_kind_of(kind));
                } else {
                    join.unit_quotes.push(quote(decl, *probe, f, listing_at));
                }
            }
        }
    }
    // A hold that could not be separated from the unit holds the unit.
    if !join.unmatched_hold_subjects.is_empty() && join.unit_hold.is_none() {
        let kind = join.unmatched_hold_kind.unwrap_or(HoldKind::ActiveDefault);
        let probe_label = decl
            .probes
            .iter()
            .find(|p| p.kind().holds() && hold_kind_of(p.kind()) == kind)
            .map(|p| manager_facts::attribution(decl.display, *p, p.kind()))
            .unwrap_or_default();
        join.unit_hold = Some(Hold {
            kind,
            label: format!(
                "{probe_label}; not separable from this unit (no listed folder matches)"
            ),
            subjects: join.unmatched_hold_subjects.clone(),
            whole_unit: true,
        });
    }
    // The manager's own record of what is a default was not read: the
    // standing is unknown, and unknown is held out.
    let hold_probes: Vec<ManagerProbe> = decl
        .probes
        .iter()
        .copied()
        .filter(|p| p.kind().holds())
        .collect();
    if !hold_probes.is_empty() {
        let missing = hold_probes
            .iter()
            .find(|p| !facts.checked(decl.manager, **p));
        if let Some(p) = missing {
            let why = if !facts.observed {
                "the manager's records have not been observed yet; `swamp observe` reads them"
                    .to_string()
            } else {
                match facts.not_observed(decl.manager, *p) {
                    Some(r) => format!("{}: {r}", p.command()),
                    None => format!("{} was not observed", p.command()),
                }
            };
            join.unit_hold = Some(Hold {
                kind: HoldKind::Unknown,
                label: format!("unknown ({why})"),
                subjects: Vec::new(),
                whole_unit: true,
            });
        }
    }
    // A hold read from a pass long before this listing may describe a
    // default that has changed since.
    if join.hold_age_secs > HOLD_MAX_AGE_SECS {
        join.unit_hold = Some(Hold {
            kind: HoldKind::Unknown,
            label: format!(
                "unknown (the manager's record was read {} before this listing; `swamp observe` reads it again)",
                age_words(join.hold_age_secs)
            ),
            subjects: Vec::new(),
            whole_unit: true,
        });
    }
    join
}

fn child_of(
    c: &UnitChild,
    now: u64,
    quotes: Vec<ManagerQuote>,
    hold: Option<Hold>,
) -> ReclaimChild {
    let is_entry = c.kind == ChildKind::Entry;
    let text = match (c.kind, c.measure) {
        (ChildKind::Entry, ChildMeasure::NotMeasured) => {
            format!("{} (not measured)", c.name)
        }
        (ChildKind::Entry, ChildMeasure::Partial) => {
            format!("{} (partly measured)", c.name)
        }
        (ChildKind::Entry, ChildMeasure::Complete) => c.name.clone(),
        _ => crate::render::describe_unit_child(c, now),
    };
    ReclaimChild {
        kind: c.kind,
        name: c.name.clone(),
        bytes: c.bytes,
        measure: c.measure,
        last_used: c.last_used.clone(),
        last_used_text: is_entry.then(|| c.last_used.fact(now)),
        text,
        about: None,
        manager: quotes,
        hold,
    }
}

fn clamp(bytes: i64) -> u64 {
    bytes.max(0) as u64
}

/// The bytes of a managed unit held out of the regenerable total.
fn held_bytes_of(u: &ExternalUnit, join: &ManagerJoin) -> u64 {
    if join.unit_hold.as_ref().is_some_and(|h| h.whole_unit) {
        return u.bytes;
    }
    let mut held: u64 = u
        .children
        .iter()
        .enumerate()
        .filter(|(i, _)| join.child_holds.contains_key(i))
        .map(|(_, c)| clamp(c.bytes.unwrap_or(0)))
        .sum();
    if !join.unmatched_hold_subjects.is_empty() {
        held += u
            .children
            .iter()
            .filter(|c| c.kind == ChildKind::Remainder)
            .map(|c| clamp(c.bytes.unwrap_or(0)))
            .sum::<u64>();
    }
    held.min(u.bytes)
}

fn managed_units(units: &[ExternalUnit]) -> HashMap<usize, ManagerDecl> {
    let registry = Registry::with_builtins();
    let mut out = HashMap::new();
    for d in registry.detectors() {
        let Some(decl) = d.manager() else {
            continue;
        };
        let mine: Vec<usize> = units
            .iter()
            .enumerate()
            .filter(|(_, u)| u.detector_id == d.id())
            .map(|(i, _)| i)
            .collect();
        let published: Vec<(StorageCategory, &Path)> = mine
            .iter()
            .map(|&i| (units[i].category, units[i].path.as_path()))
            .collect();
        for j in decl.anchor.select(&published) {
            out.insert(mine[j], decl);
        }
    }
    out
}

fn unit_row(
    index: usize,
    input: &ReclaimInput<'_>,
    scope: &ScopeStatement,
    managed: &HashMap<usize, ManagerDecl>,
) -> ReclaimRow {
    let u = &input.units[index];
    let now = input.observed_at;
    let regeneration = regeneration_of(u, input.interiors);
    let join = managed.contains_key(&index).then(|| {
        join_manager(
            input.units,
            managed,
            index,
            input.manager_facts,
            input.observed_at,
        )
    });
    let (held, unit_quotes, unit_hold) = match &join {
        Some(j) => (
            held_bytes_of(u, j),
            j.unit_quotes.clone(),
            j.unit_hold.clone(),
        ),
        None => (0, Vec::new(), None),
    };
    let children: Vec<ReclaimChild> = u
        .children
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let (quotes, hold) = match &join {
                Some(j) => (
                    j.child_quotes.get(&i).cloned().unwrap_or_default(),
                    j.child_holds.get(&i).cloned(),
                ),
                None => (Vec::new(), None),
            };
            child_of(c, now, quotes, hold)
        })
        .collect();
    let models = crate::build_adapters::model_stores::model_rows(&u.path, input.interiors, now);
    let children: Vec<ReclaimChild> = children
        .into_iter()
        .map(|mut c| {
            let path = crate::build_adapters::model_stores::shown_path(&u.path.join(&c.name));
            if c.kind == ChildKind::Entry
                && let Some(m) = models.iter().find(|m| m.path == path)
            {
                c.about = m.about.clone();
                if let Some(a) = &m.about {
                    c.text = format!("{} · {a}", c.text);
                }
            }
            c
        })
        .collect();
    let regenerable = matches!(
        regeneration.class,
        RegenClass::Download | RegenClass::Rebuild
    );
    let (regenerable_bytes, held_bytes) = if regenerable {
        (u.bytes - held.min(u.bytes), held.min(u.bytes))
    } else {
        (0, 0)
    };
    ReclaimRow {
        path: u.path.display().to_string(),
        kind: crate::external::category_label(u.category).to_string(),
        detector: u.detector_name.clone(),
        bytes: u.bytes,
        growth_bytes: u.growth_bytes,
        regeneration,
        last_used: u.last_used.clone(),
        last_used_text: u.last_used.fact(now),
        consumers: consumers_for(&u.evidence, &u.consumers, scope),
        manager: unit_quotes,
        hold: unit_hold,
        removal: match crate::tool_removal::manager_for_unit(&u.detector_id, &u.path) {
            Some(m) => removal(RemovalKind::TrashOrToolCommand, Some(m.name())),
            None => removal(RemovalKind::TrashReviewed, None),
        },
        regenerable_bytes,
        held_bytes,
        note: u.display_note(),
        children,
        models,
    }
}

fn standalone_row(
    row: &UnownedRow,
    input: &ReclaimInput<'_>,
    scope: &ScopeStatement,
) -> ReclaimRow {
    let now = input.observed_at;
    ReclaimRow {
        path: row.path_or_object.clone(),
        kind: KIND_STANDALONE_CARGO_TARGET.to_string(),
        detector: "Cargo".to_string(),
        bytes: row.bytes,
        growth_bytes: None,
        regeneration: Regeneration {
            class: RegenClass::Rebuild,
            words: crate::attribution::STANDALONE_CARGO_TARGET_COST.to_string(),
            source: "standalone Cargo target consequence text".to_string(),
        },
        last_used: LastUsed::default(),
        last_used_text: LastUsed::default().fact(now),
        consumers: consumers_for(&row.evidence, &[], scope),
        manager: Vec::new(),
        hold: None,
        removal: removal(RemovalKind::TrashReviewed, None),
        regenerable_bytes: row.bytes,
        held_bytes: 0,
        note: None,
        children: Vec::new(),
        models: Vec::new(),
    }
}

fn totals_of(rows: &[ReclaimRow], scope: &ScopeStatement) -> Totals {
    let mut t = Totals {
        count: rows.len(),
        bytes: 0,
        regenerable_bytes: 0,
        held_bytes: 0,
        not_regenerable_bytes: 0,
        not_established_bytes: 0,
        remainder_bytes: 0,
        per_kind: Vec::new(),
        scope_statement: scope.statement.clone(),
    };
    let mut kinds: BTreeMap<String, KindTotal> = BTreeMap::new();
    for r in rows {
        t.bytes = t.bytes.saturating_add(r.bytes);
        t.regenerable_bytes = t.regenerable_bytes.saturating_add(r.regenerable_bytes);
        t.held_bytes = t.held_bytes.saturating_add(r.held_bytes);
        match r.regeneration.class {
            RegenClass::NotRegenerable => {
                t.not_regenerable_bytes = t.not_regenerable_bytes.saturating_add(r.bytes)
            }
            RegenClass::NotEstablished => {
                t.not_established_bytes = t.not_established_bytes.saturating_add(r.bytes)
            }
            RegenClass::Download | RegenClass::Rebuild => {}
        }
        let k = kinds.entry(r.kind.clone()).or_insert_with(|| KindTotal {
            kind: r.kind.clone(),
            count: 0,
            bytes: 0,
            regenerable_bytes: 0,
        });
        k.count += 1;
        k.bytes = k.bytes.saturating_add(r.bytes);
        k.regenerable_bytes = k.regenerable_bytes.saturating_add(r.regenerable_bytes);
    }
    let mut per_kind: Vec<KindTotal> = kinds.into_values().collect();
    per_kind.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.kind.cmp(&b.kind)));
    t.per_kind = per_kind;
    t
}

/// The bytes of the units that are the remainder of a location (selected
/// by the detector's own declaration, never by its id).
fn remainder_bytes(units: &[ExternalUnit]) -> u64 {
    let registry = Registry::with_builtins();
    units
        .iter()
        .filter(|u| {
            registry
                .detectors()
                .iter()
                .find(|d| d.id() == u.detector_id)
                .is_some_and(|d| d.remainder_of().is_some())
        })
        .fold(0u64, |a, u| a.saturating_add(u.bytes))
}

fn coverage_notes(input: &ReclaimInput<'_>, managed: &HashMap<usize, ManagerDecl>) -> Vec<String> {
    let mut notes: Vec<String> = Vec::new();
    let mut seen: Vec<&'static str> = Vec::new();
    let mut display: Vec<&'static str> = Vec::new();
    for decl in managed.values() {
        if !seen.contains(&decl.manager) {
            seen.push(decl.manager);
            display.push(decl.display);
        }
    }
    display.sort_unstable();
    seen.sort_unstable();
    if !seen.is_empty() && !input.manager_facts.observed {
        notes.push(format!(
            "manager reports ({}) not observed yet; `swamp observe` records them, and until then their rows have unknown standing",
            display.join(", ")
        ));
    } else {
        let mut probes: Vec<(&'static str, &'static str, ManagerProbe)> = Vec::new();
        for decl in managed.values() {
            for p in decl.probes {
                if !probes.iter().any(|(m, _, q)| *m == decl.manager && q == p) {
                    probes.push((decl.manager, decl.display, *p));
                }
            }
        }
        probes.sort_by(|a, b| (a.0, a.2.label()).cmp(&(b.0, b.2.label())));
        for (manager, shown, probe) in probes {
            if let Some(why) = input.manager_facts.not_observed(manager, probe) {
                notes.push(format!("{shown}: {} not observed: {why}", probe.command()));
            }
        }
    }
    notes
}

/// Builds the Reclaim view from stored facts.
pub fn build(input: &ReclaimInput<'_>) -> ReclaimView {
    let scope = scope_statement(input.projects, input.declared_roots, input.explicit_scope);
    let managed = managed_units(input.units);
    let mut rows: Vec<ReclaimRow> = (0..input.units.len())
        .map(|i| unit_row(i, input, &scope, &managed))
        .collect();
    rows.extend(
        input
            .unowned
            .iter()
            .filter(|u| u.reason == UnownedReason::StandaloneCargoTarget)
            .map(|u| standalone_row(u, input, &scope)),
    );
    // Largest first; ties by path, so two runs over the same facts print
    // the same order.
    rows.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
    let mut totals = totals_of(&rows, &scope);
    totals.remainder_bytes = remainder_bytes(input.units);
    let mut notes = coverage_notes(input, &managed);
    notes.extend(unjoined_notes(input, &managed, &rows));
    ReclaimView {
        observed_at: input.observed_at,
        scope,
        totals,
        coverage_notes: notes,
        rows,
    }
}

/// Manager statements that reached no measured unit, said instead of
/// dropped: Homebrew reporting a formula whose unit is not measured is
/// still a fact the person may want.
fn unjoined_notes(
    input: &ReclaimInput<'_>,
    managed: &HashMap<usize, ManagerDecl>,
    rows: &[ReclaimRow],
) -> Vec<String> {
    let mut out = Vec::new();
    let mut decls: Vec<&ManagerDecl> = Vec::new();
    for d in managed.values() {
        if !decls.iter().any(|x| x.manager == d.manager) {
            decls.push(d);
        }
    }
    decls.sort_by_key(|d| d.manager);
    // A default or pin the manager names that matches no measured folder
    // is said, never dropped.
    for decl in &decls {
        for f in &input.manager_facts.facts {
            if f.manager != decl.manager
                || !matches!(f.kind, FactKind::ActiveDefault | FactKind::PinnedByOverride)
            {
                continue;
            }
            let Some(subject) = f.subject.as_deref() else {
                continue;
            };
            if !claimed_elsewhere(input.units, managed, usize::MAX, decl.manager, subject) {
                let what = if f.kind == FactKind::PinnedByOverride {
                    "pinned by override"
                } else {
                    "default"
                };
                let note = format!(
                    "{}: {what}: {subject}, no matching folder found in the measured units",
                    decl.display
                );
                if !out.contains(&note) {
                    out.push(note);
                }
            }
        }
    }
    for decl in decls {
        for probe in decl.probes.iter().filter(|p| !p.kind().holds()) {
            let subjects: Vec<&str> = input
                .manager_facts
                .facts
                .iter()
                .filter(|f| {
                    f.manager == decl.manager && f.probe == probe.label() && f.kind == probe.kind()
                })
                .filter_map(|f| f.subject.as_deref())
                .collect();
            let reached = |s: &str| {
                rows.iter().any(|r| {
                    r.manager
                        .iter()
                        .any(|q| q.subject == s && q.manager == decl.manager)
                        || r.children.iter().any(|c| {
                            c.manager
                                .iter()
                                .any(|q| q.subject == s && q.manager == decl.manager)
                        })
                })
            };
            let missed: Vec<&str> = subjects.iter().copied().filter(|s| !reached(s)).collect();
            if !missed.is_empty() {
                out.push(format!(
                    "{}: {} names {} outside every measured unit: {}",
                    decl.display,
                    probe.command(),
                    missed.len(),
                    missed.join(", ")
                ));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------

fn quotes_lines(quotes: &[ManagerQuote], indent: &str) -> Vec<String> {
    let mut lines: Vec<String> = quotes
        .iter()
        .take(QUOTES_SHOWN)
        .map(|q| format!("{indent}{}", q.line()))
        .collect();
    if quotes.len() > QUOTES_SHOWN {
        lines.push(format!(
            "{indent}and {} more in --json",
            quotes.len() - QUOTES_SHOWN
        ));
    }
    lines
}

/// The label of a hold, as a listing words it.
pub fn hold_line(h: &Hold) -> String {
    match h.kind {
        HoldKind::Unknown => format!("{}; held out of the regenerable total", h.label),
        _ if h.subjects.is_empty() => format!("{}; held out of the regenerable total", h.label),
        _ => {
            let rest = h.subjects.len().saturating_sub(NAMES_SHOWN);
            let shown = h
                .subjects
                .iter()
                .take(NAMES_SHOWN)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            let more = if rest > 0 {
                format!(" and {rest} more")
            } else {
                String::new()
            };
            format!(
                "{}: {shown}{more}; held out of the regenerable total",
                h.label
            )
        }
    }
}

/// The text of `swamp report --view reclaim`.
pub fn render_text(view: &ReclaimView) -> String {
    use crate::render::{human_bytes_pub, human_bytes_signed};
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Reclaim view: developer storage by unit, largest first. Facts with their sources; swamp removes nothing here."
    );
    let _ = writeln!(out, "{}", view.scope.statement);
    for n in &view.coverage_notes {
        let _ = writeln!(out, "coverage: {n}");
    }
    if view.rows.is_empty() {
        let _ = writeln!(out, "\nno storage units detected");
    }
    for r in &view.rows {
        let growth = r
            .growth_bytes
            .map(human_bytes_signed)
            .unwrap_or_else(|| "n/a".to_string());
        let note = r
            .note
            .as_deref()
            .map(|n| format!("  [{n}]"))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "\n{:<10} {:>10}  {}  {}{note}",
            human_bytes_pub(r.bytes),
            growth,
            r.kind,
            r.path
        );
        let _ = writeln!(
            out,
            "    regeneration: {} [{}]",
            r.regeneration.words, r.regeneration.source
        );
        let _ = writeln!(out, "    last used: {}", r.last_used_text);
        let _ = writeln!(out, "    consumers: {}", r.consumers.summary);
        for line in quotes_lines(&r.manager, "    manager: ") {
            let _ = writeln!(out, "{line}");
        }
        if let Some(h) = &r.hold {
            let _ = writeln!(out, "    standing: {}", hold_line(h));
        }
        let _ = writeln!(out, "    removal: {}", r.removal.text);
        if !r.children.is_empty() {
            let _ = writeln!(
                out,
                "    inside, largest first (rows add up to {}):",
                human_bytes_pub(r.bytes)
            );
            for c in &r.children {
                let size = match c.bytes {
                    Some(b) if c.kind == ChildKind::Adjustment || b < 0 => human_bytes_signed(b),
                    Some(b) => human_bytes_pub(b.max(0) as u64),
                    None => "not measured".to_string(),
                };
                let used = c
                    .last_used_text
                    .as_deref()
                    .map(|t| format!("  last used: {t}"))
                    .unwrap_or_default();
                let _ = writeln!(out, "      {size:>12}  {}{used}", c.text);
                if let Some(h) = &c.hold {
                    let _ = writeln!(out, "                    standing: {}", hold_line(h));
                }
                for line in quotes_lines(&c.manager, "                    manager: ") {
                    let _ = writeln!(out, "{line}");
                }
            }
        }
        for line in crate::render::model_lines(&r.models) {
            let _ = writeln!(out, "    {line}");
        }
    }
    let t = &view.totals;
    let _ = writeln!(
        out,
        "\n{} unit{}, {} allocated. Regenerable kinds: {} ({} more held out as defaults, installed on request, or unknown standing). Not regenerable: {}. Cost not established: {}.",
        t.count,
        if t.count == 1 { "" } else { "s" },
        human_bytes_pub(t.bytes),
        human_bytes_pub(t.regenerable_bytes),
        human_bytes_pub(t.held_bytes),
        human_bytes_pub(t.not_regenerable_bytes),
        human_bytes_pub(t.not_established_bytes),
    );
    for k in &t.per_kind {
        let _ = writeln!(
            out,
            "  {:<24} {:>3} unit{}  {:>10}",
            k.kind,
            k.count,
            if k.count == 1 { " " } else { "s" },
            human_bytes_pub(k.bytes)
        );
    }
    let _ = writeln!(out, "{}", t.scope_statement);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn root(path: &str, state: DeclaredState) -> DeclaredRoot {
        DeclaredRoot {
            path: PathBuf::from(path),
            state,
        }
    }

    #[test]
    fn the_scope_statement_names_what_was_checked() {
        let s = scope_statement(
            40,
            &[root(
                "/h/src",
                DeclaredState::Present {
                    bytes: Some(1),
                    complete: true,
                },
            )],
            false,
        );
        assert_eq!(
            s.statement,
            "consumer evidence checked against 40 projects in 1 declared root"
        );
        assert!(s.complete);
    }

    #[test]
    fn a_missing_unreadable_or_partly_read_root_makes_the_evidence_incomplete() {
        for roots in [
            vec![root("/h/src", DeclaredState::Missing)],
            vec![root(
                "/h/src",
                DeclaredState::Unreadable {
                    reason: "denied".into(),
                },
            )],
            vec![root(
                "/h/src",
                DeclaredState::Present {
                    bytes: Some(1),
                    complete: false,
                },
            )],
        ] {
            let s = scope_statement(3, &roots, false);
            assert!(!s.complete, "{roots:?}");
            assert!(s.statement.contains("incomplete"), "{}", s.statement);
        }
    }

    /// The tempting wrong patch: the category's class is used whatever
    /// the tool's own text says, so a session scratch directory whose
    /// removal "breaks that session" counts as regenerable by download.
    #[test]
    fn the_tools_own_words_decide_the_class() {
        use RegenClass::*;
        let c = class_from_consequence;
        assert_eq!(
            c("session scratch; removing it during a session breaks that session"),
            NotEstablished
        );
        assert_eq!(
            c("this emulator's apps and data are gone; a recreated AVD starts empty"),
            NotRegenerable
        );
        assert_eq!(
            c("iOS_23F77 is downloaded again when a simulator needs it"),
            Download
        );
        assert_eq!(c("the next simulator boot rebuilds it"), Rebuild);
        assert_eq!(c("something with no cost in it"), NotEstablished);
    }

    #[test]
    fn no_declared_root_is_incomplete_and_says_how_to_fix_it() {
        let s = scope_statement(40, &[], false);
        assert!(!s.complete);
        assert!(
            s.statement.starts_with(
                "consumer evidence covers the built-in default roots only (40 projects)"
            )
        );
        assert!(s.statement.contains("swamp config add-root"));
        assert!(!s.statement.contains("0 declared roots"));
    }

    #[test]
    fn standalone_cost_is_the_note_it_is_pinned_to() {
        assert!(
            crate::attribution::STANDALONE_CARGO_TARGET_NOTE
                .contains(crate::attribution::STANDALONE_CARGO_TARGET_COST)
        );
    }
}
