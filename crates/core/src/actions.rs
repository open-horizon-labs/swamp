//! swamp reports; the human decides. This module turns report rows (a
//! folded artifact, a Cargo purpose group, an agent-storage unit, a
//! worktree/checkout) into [`PlanUnit`]s: the facts a human needs before
//! marking something for the Trash, and the exact member paths that get
//! moved when they press Enter in the TUI (or run the equivalent shell
//! command by hand).
//!
//! There is no plan store, no grant, no confirmation token and no
//! execute-time re-derivation that refuses on drift: swamp's job stops at
//! reporting the facts. A `PlanUnit` is built fresh whenever the caller
//! needs it and lives only in memory; moving it is a direct call to
//! `fs_gate::destroy` with no gate in front of it beyond an ordinary OS
//! error (permission denied, path gone, cross-device).

use crate::filter::Filter;
use crate::fs_gate::{self, Metadata, MetadataExt};
use crate::report::{ArtifactKind, ArtifactRow, ProjectRow, Report, WorktreeRow};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

fn now() -> u64 {
    crate::entities::now()
}

// ---------------------------------------------------------------------
// Plan units: the facts, built fresh, never persisted
// ---------------------------------------------------------------------

/// One unit of action: exactly one folded artifact row (or Cargo group,
/// agent-storage unit, worktree/checkout), with the facts a human needs
/// before marking it and the exact paths that move if they press Enter.
/// Built fresh by `propose*`/`unit_from_*`; never written to disk, never
/// signed, never compared against a later re-derivation.
#[derive(Debug, Clone, Serialize)]
pub struct PlanUnit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cargo_group: Option<crate::cargo_cleanup::CargoGroup>,
    path: PathBuf,
    rel_path: String,
    project: String,
    project_id: String,
    worktree_id: String,
    worktree_path: PathBuf,
    kind: ArtifactKind,
    bytes: u64,
    #[serde(default)]
    dedup_stale: bool,
    growth_bytes: Option<i64>,
    regrowth_count: u32,
    observed_at: u64,
    /// Recovery contract of the kind, stated so the human sees *why* this
    /// verb applies: `local_rebuild` (build outputs, caches),
    /// `network_fetch` (dependency trees).
    recovery: String,
    idle_secs: Option<u64>,
    merge_complete: bool,
    /// Rendered signal values of the owning worktree at proposal time.
    signals: Vec<String>,
    /// `delete` (artifact or Source directory), `remove-worktree` (linked
    /// worktree), `archive` (whole checkout).
    verb: String,
    /// git tracking status of the path, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    track: Option<crate::ignore::TrackState>,
    /// The plain-language consequence of deleting this unit (dirty,
    /// unpushed, untracked content, no remote, git store, unique history
    /// lost…). Shown before Enter; never a verdict ("safe"/"unused") and
    /// never a veto -- the human decides.
    #[serde(default)]
    warnings: Vec<String>,
    /// Set only for a unit built from `agents::AgentUnit` (#101): the
    /// facts a Trash move needs -- a single-path cache/log move, or a
    /// multi-member session removal, where every listed member moves
    /// together into one Trash envelope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_meta: Option<AgentPlanMeta>,
    /// Decision evidence (#61): the exact activity/consumer/reclaimability
    /// facts this unit's report row already carried
    /// (`report::attach_decision_evidence`), plus a fresh current-use
    /// fact taken when this unit was built.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    evidence: Vec<crate::evidence::Evidence>,
}

/// See `PlanUnit::agent_meta`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPlanMeta {
    pub tool_id: String,
    pub tool_home: PathBuf,
    pub category: String,
    /// `None`: a single-path Trash move of the unit's own `path` (a
    /// cache/log category directory). `Some`: a session removal -- every
    /// listed member is moved together into one Trash envelope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_members: Option<Vec<PathBuf>>,
}

/// A row the human/agent asked for that has no supported action, with the
/// fact that names why -- never silently dropped from the response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Refused {
    pub path: PathBuf,
    pub cause: String,
}

impl PlanUnit {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
    pub fn kind(&self) -> &ArtifactKind {
        &self.kind
    }
    pub fn verb(&self) -> &str {
        &self.verb
    }
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
    pub fn evidence(&self) -> &[crate::evidence::Evidence] {
        &self.evidence
    }
    pub fn agent_meta(&self) -> Option<&AgentPlanMeta> {
        self.agent_meta.as_ref()
    }
    pub fn cargo_group(&self) -> Option<&crate::cargo_cleanup::CargoGroup> {
        self.cargo_group.as_ref()
    }
    pub fn project(&self) -> &str {
        &self.project
    }
    pub fn recovery(&self) -> &str {
        &self.recovery
    }
    pub fn growth_bytes(&self) -> Option<i64> {
        self.growth_bytes
    }
    pub fn signals(&self) -> &[String] {
        &self.signals
    }
    pub fn track(&self) -> Option<crate::ignore::TrackState> {
        self.track
    }
    pub fn dedup_stale(&self) -> bool {
        self.dedup_stale
    }
    pub fn worktree_path(&self) -> &Path {
        &self.worktree_path
    }
    pub fn worktree_id(&self) -> &str {
        &self.worktree_id
    }
}

/// What may be planned: anything with a path on this filesystem. The
/// one refusal is Docker build cache, which the daemon exposes no
/// per-entry removal for. Everything else is the human's call, made with
/// the unit's `warnings` in front of them — including a Docker image or
/// volume, whose removal is permanent and says so.
pub fn refusal_for_kind(kind: &ArtifactKind) -> Option<&'static str> {
    match kind {
        // Docker exposes no per-record removal for build cache: only
        // `docker builder prune`, which acts on everything reclaimable at
        // once and so is a different unit of action than a plan unit.
        ArtifactKind::DockerBuildCache => Some(
            "docker has no per-entry build-cache removal; `docker builder prune` acts on all of it",
        ),
        // These two rows report bytes scattered across a checkout and
        // carry the worktree's own path. Acting on that path would take
        // the whole checkout, which is not what the row says.
        ArtifactKind::Ignored | ArtifactKind::Untracked => Some(
            "an aggregate of every such path under the checkout, not one directory; open the worktree and act on what is inside it",
        ),
        _ => None,
    }
}

fn recovery_for(kind: &ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::DependencyTree => "network_fetch",
        ArtifactKind::BuildOutput | ArtifactKind::Cache => "local_rebuild",
        ArtifactKind::Git => "irrecoverable",
        // Docker removals never reach Trash, so the contract is the only
        // thing standing between the human and a permanent loss.
        ArtifactKind::DockerImage => "pull_or_rebuild (permanent: no Trash)",
        ArtifactKind::DockerBuildCache => "local_rebuild (permanent: no Trash)",
        ArtifactKind::DockerVolume => "irrecoverable (permanent: no Trash, no copy anywhere)",
        // Ignored bytes are in no version control at all: nothing to
        // pull, nothing to check out again. Trash is the only copy.
        ArtifactKind::Ignored | ArtifactKind::Untracked => "irrecoverable outside Trash",
        ArtifactKind::Source | ArtifactKind::Loose | ArtifactKind::Unknown => "depends: see track",
    }
}

/// The facts a human weighs before authorizing a unit — the same line the
/// TUI puts on its confirm prompt.
fn warnings_for(wt: &WorktreeRow, a: &ArtifactRow, whole: Option<&ProjectRow>) -> Vec<String> {
    let mut w = Vec::new();
    if a.dedup_stale {
        w.push("unique-byte estimate is out of date; full scan required before budgeted standing-grant cleanup".into());
    }
    match a.track {
        Some(crate::ignore::TrackState::Untracked) => {
            w.push("untracked: in no version control and under no ignore rule".into())
        }
        Some(crate::ignore::TrackState::Tracked) if a.kind == ArtifactKind::Source => {
            w.push("tracked source".into())
        }
        _ => {}
    }
    if a.kind == ArtifactKind::Git {
        w.push("git object store: history goes with it".into());
    }
    if let Some(p) = whole {
        // Whole-worktree/checkout unit: the worktree's own facts apply.
        for sig in &wt.signals {
            match (sig.name.as_str(), sig.value.as_str()) {
                ("dirty", "dirty") => w.push("dirty".into()),
                ("unpushed", v) if v != "0 unpushed" => w.push(v.to_string()),
                ("locked", "locked") => w.push("locked".into()),
                _ => {}
            }
        }
        if wt.kind != crate::report::WorktreeKind::Linked {
            if p.remote.is_none() {
                w.push("no remote to restore from".into());
            }
            for (path, bytes) in crate::ignore::untracked_content(&wt.path, 3, 100_000) {
                let rel = crate::scope::relative_to(&path, &wt.path)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                w.push(format!(
                    "{rel} untracked {}",
                    crate::render::human_bytes_pub(bytes)
                ));
            }
        }
    }
    w
}

fn worktree_merge_complete(wt: &WorktreeRow) -> bool {
    wt.merge_complete
        .as_ref()
        .is_some_and(|m| m.verdict == crate::github::TriState::Yes)
}

fn signal_strings(wt: &WorktreeRow) -> Vec<String> {
    wt.signals.iter().map(|s| s.value.clone()).collect()
}

/// Builds a plan from report rows. `filter` narrows which artifact rows
/// are proposed (same grammar as `--filter`); `paths`, when non-empty,
/// restricts to those exact artifact paths. Rows of non-actionable kinds
/// are recorded under `refused`, never silently dropped. An empty plan is
/// an error, so a proposer cannot mint a plan that says nothing.
pub fn propose(
    report: &Report,
    filter: Option<&Filter>,
    paths: &[PathBuf],
    proposed_by: &str,
) -> Result<Vec<PlanUnit>> {
    let mut units = Vec::new();
    let mut refused = Vec::new();
    for project in &report.projects {
        for wt in &project.worktrees {
            for a in &wt.artifacts {
                // A Source row shares its path with the worktree root; when
                // that exact path is asked for, the human means the whole
                // worktree/checkout (handled below with its own verb).
                let is_worktree_root = a.kind.is_worktree_remainder() && a.path == wt.path;
                let wanted = if paths.is_empty() {
                    filter.is_none_or(|f| f.matches_artifact(project, a))
                } else {
                    !is_worktree_root && paths.iter().any(|p| p == &a.path)
                };
                if !wanted {
                    continue;
                }
                if let Some(cause) = refusal_for_kind(&a.kind) {
                    if !paths.is_empty() {
                        refused.push(Refused {
                            path: a.path.clone(),
                            cause: cause.to_string(),
                        });
                    }
                    continue;
                }
                units.push(unit_from_row(project, wt, a));
            }
        }
    }
    // Paths that name a worktree/checkout root or a Source directory.
    for p in paths {
        if units.iter().any(|u| &u.path == p) || refused.iter().any(|r| &r.path == p) {
            continue;
        }
        if let Some(nested) = report.nested_artifacts.iter().find(|u| &u.path == p) {
            if nested.action == crate::artifact::NestedActionCapability::TrashPath {
                let owner = report
                    .projects
                    .iter()
                    .flat_map(|project| project.worktrees.iter().map(move |wt| (project, wt)))
                    .filter(|(_, wt)| p != &wt.path && crate::scope::under(p, &wt.path))
                    .max_by_key(|(_, wt)| wt.path.components().count());
                if let Some((project, wt)) = owner {
                    let mut unit = unit_from_worktree(project, wt);
                    unit.path = p.clone();
                    unit.rel_path = crate::artifact::relative_path(&wt.path, p);
                    unit.kind = ArtifactKind::BuildOutput;
                    unit.verb = "delete".into();
                    unit.bytes = nested.bytes;
                    unit.growth_bytes = nested.growth_bytes;
                    unit.recovery = nested
                        .consequence
                        .clone()
                        .unwrap_or_else(|| "restore from Trash".into());
                    unit.warnings = vec![unit.recovery.clone(),
                        "moves only this selected path to Trash; stop its build before removing it; allocation is not guaranteed freed space".into()];
                    unit.evidence = nested.decision_evidence.clone();
                    units.push(unit);
                } else {
                    refused.push(Refused {
                        path: p.clone(),
                        cause: "no owning checkout for this project-local action".into(),
                    });
                }
                continue;
            }
            let container = report
                .nested_artifacts
                .iter()
                .find(|u| Some(&u.id) == nested.container_id.as_ref());
            let owner = report
                .projects
                .iter()
                .flat_map(|project| project.worktrees.iter().map(move |wt| (project, wt)))
                .find(|(_, wt)| {
                    container.is_some_and(|c| wt.artifacts.iter().any(|a| a.path == c.path))
                });
            if let (Some(container), Some((project, wt))) = (container, owner) {
                match crate::cargo_cleanup::propose(&report.nested_artifacts, p, &container.path) {
                    Ok(group) => {
                        let row = wt
                            .artifacts
                            .iter()
                            .find(|a| a.path == container.path)
                            .unwrap();
                        let mut unit = unit_from_row(project, wt, row);
                        unit.path = p.clone();
                        unit.rel_path = crate::scope::relative_to(p, &wt.path)
                            .unwrap_or(p)
                            .display()
                            .to_string();
                        unit.bytes = group.members.iter().map(|m| m.bytes).sum();
                        unit.dedup_stale = false; // selected members were freshly measured
                        unit.growth_bytes = nested.growth_bytes;
                        unit.verb = "cargo-group".into();
                        unit.recovery = "Trash envelope with restore.json; rebuilding may require unavailable source/toolchains".into();
                        unit.warnings = vec!["exact selected build, NOT proven obsolete; stop non-Cargo writers; advisory Cargo lock held during move".into()];
                        unit.warnings.push("size is selected allocation, not promised free space; moving to Trash does not free these bytes immediately".into());
                        if group.shared_storage {
                            unit.warnings.push("selected files have hardlinks; any links outside the selection remain intact and reclaimable space is unknown".into());
                        }
                        unit.warnings.extend(
                            group
                                .members
                                .iter()
                                .map(|m| format!("member: {}", m.path.display())),
                        );
                        unit.cargo_group = Some(group);
                        units.push(unit);
                    }
                    Err(e) => refused.push(Refused {
                        path: p.clone(),
                        cause: e.to_string(),
                    }),
                }
            } else {
                refused.push(Refused {
                    path: p.clone(),
                    cause: "nested artifact has no observed owning container".into(),
                });
            }
            continue;
        }
        let mut found = false;
        'outer: for project in &report.projects {
            for wt in &project.worktrees {
                if &wt.path == p {
                    units.push(unit_from_worktree(project, wt));
                    found = true;
                    break 'outer;
                }
                if let Some(dirs) = report
                    .dirs_by_worktree
                    .as_ref()
                    .and_then(|m| m.get(&wt.worktree_id))
                    && let Some(d) = dirs.iter().find(|d| wt.path.join(&d.rel_path) == *p)
                {
                    units.push(unit_from_dir(project, wt, d));
                    found = true;
                    break 'outer;
                }
            }
        }
        if !found {
            refused.push(Refused {
                path: p.clone(),
                cause: "not a path in this report (artifact, Source directory, worktree or checkout); unowned paths and Docker objects are not plannable".into(),
            });
        }
    }
    if units.is_empty() {
        bail!(
            "nothing to propose: no plannable rows matched{}",
            if refused.is_empty() {
                String::new()
            } else {
                format!(
                    " ({} refused: {})",
                    refused.len(),
                    refused
                        .iter()
                        .map(|r| format!("{} — {}", r.path.display(), r.cause))
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            }
        );
    }
    if units.iter().any(|u| u.cargo_group.is_some()) {
        for (i, a) in units.iter().enumerate() {
            for b in units.iter().skip(i + 1) {
                if crate::scope::overlapping(&a.path, &b.path) {
                    bail!(
                        "overlapping cleanup selections; select either parent or child, not both"
                    );
                }
                if let (Some(a), Some(b)) = (&a.cargo_group, &b.cargo_group)
                    && a.members
                        .iter()
                        .any(|x| b.members.iter().any(|y| x.path == y.path))
                {
                    bail!("overlapping Cargo companion selections");
                }
            }
        }
    }
    units.sort_by_key(|a| std::cmp::Reverse(a.bytes));
    let _ = proposed_by;
    Ok(units)
}

/// Same as [`propose`], additionally refusing any matched unit whose
/// path is human-protected (`swamp protect`, #60): a scanned project
/// file or an agent observation can never add or remove a protection --
/// only this explicit, human-controlled list (`agents::protect_add`/
/// `protect_remove`, itself callable only from the CLI's own `protect`
/// subcommand) does. A protected unit still appears in the plan's
/// `refused` list with a named cause, never silently dropped.
pub fn propose_checking_protection(
    report: &Report,
    filter: Option<&Filter>,
    paths: &[PathBuf],
    proposed_by: &str,
    protected: &[PathBuf],
) -> Result<Vec<PlanUnit>> {
    propose_refusing_protected(
        report,
        filter,
        paths,
        proposed_by,
        report.store_dir.as_deref(),
        protected,
    )
}

fn propose_refusing_protected(
    report: &Report,
    filter: Option<&Filter>,
    paths: &[PathBuf],
    proposed_by: &str,
    store_dir: Option<&Path>,
    protected: &[PathBuf],
) -> Result<Vec<PlanUnit>> {
    let units = propose(report, filter, paths, proposed_by)?;
    // The caller's list is a convenience, never the authority. The PR
    // #123 review's counterexample: an unreadable `agent_protect.json`
    // reached this function as an *empty* list through the CLI's
    // `.unwrap_or_default()`, so a protected unit became plannable
    // exactly when protection state broke. When the report knows which
    // store it came from, protection is reloaded here and an error is a
    // refusal (`.oh/guardrails/protection-fails-closed.md`).
    let live = match store_dir {
        Some(dir) => crate::protection::load_protect(dir)?,
        None => crate::protection::ProtectList::empty(),
    };
    let protected = live.including(protected);
    if protected.is_empty() {
        return Ok(units);
    }
    let mut kept = Vec::new();
    let mut refused = Vec::new();
    for u in units {
        // Both directions, as everywhere else: a unit beneath a
        // protected path, and a unit that *contains* one.
        if protected.conflict(&u.path).is_some() {
            refused.push(Refused {
                path: u.path.clone(),
                cause: "human-protected path (swamp protect); remove protection first if this unit should be actionable".into(),
            });
        } else {
            kept.push(u);
        }
    }
    if kept.is_empty() {
        bail!(
            "nothing to propose: every matched unit is human-protected ({} refused: {})",
            refused.len(),
            refused
                .iter()
                .map(|r| format!("{} — {}", r.path.display(), r.cause))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    Ok(kept)
}

/// A unit's evidence, plus one fresh current-use (open-file) reading
/// taken when this unit is built -- shown on the TUI's confirm banner as
/// a fact, never re-taken or enforced at Enter.
fn plan_unit_evidence(
    existing: &[crate::evidence::Evidence],
    path: &Path,
) -> Vec<crate::evidence::Evidence> {
    let mut evidence = existing.to_vec();
    evidence.push(crate::occupancy::open_file_evidence(path));
    evidence
}

const MANAGER_LOCK_FILENAMES: &[&str] = &[
    ".package-cache",
    ".lock",
    "journal-1.lock",
    "registry.bin.lock",
    "store.lock",
];

/// How many simulator devices one proposal probes. Each probe is one
/// bounded, allow-listed, read-only `xcrun simctl list devices -j`
/// (`locations::ALLOWED_COMMANDS`), run only because a human named this
/// unit; the cap keeps a store with an unusual number of devices from
/// turning one proposal into an unbounded number of subprocesses, and
/// the remainder is stated rather than dropped.
const SIMULATOR_DEVICE_PROBE_CAP: usize = 8;

/// Manager-lock current use (#55) for one external unit. Every existing
/// candidate lock is probed; when none of them exists the single
/// `Unknown` names what was looked for, so "no lock file" never reads as
/// "nothing is using this".
fn manager_lock_facts(unit: &crate::external::ExternalUnit) -> Vec<crate::evidence::Evidence> {
    let mut present = Vec::new();
    let mut absent = None;
    for name in MANAGER_LOCK_FILENAMES {
        let ev =
            crate::occupancy::manager_lock_evidence(&unit.detector_name, &unit.path.join(name));
        if matches!(ev.status, crate::evidence::FactStatus::Unknown { .. }) {
            absent = absent.or(Some(ev));
        } else {
            present.push(ev);
        }
    }
    if !present.is_empty() {
        return present;
    }
    absent
        .map(|ev| {
            vec![ev.with_note(format!(
                "no manager lock file present in this unit's own directory; looked for {}. A \
                 manager that only creates its lock while working leaves none here between \
                 operations, so this is not evidence that nothing is using the store",
                MANAGER_LOCK_FILENAMES.join(", ")
            ))]
        })
        .unwrap_or_default()
}

/// CoreSimulator names each device's data directory by its device UDID
/// (`8-4-4-4-12` hex). Recognizing the *shape* is what keeps an Android
/// AVD directory (`Pixel_5_API_31.avd`) out of a `simctl` query: both
/// are `Environments` units, and asking `simctl` about an AVD name would
/// return an answer about nothing.
fn is_simulator_device_udid(name: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let parts: Vec<&str> = name.split('-').collect();
    parts.len() == GROUPS.len()
        && parts
            .iter()
            .zip(GROUPS)
            .all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Booted-device current use (#55) for a simulator device store: one
/// `simctl` reading per device directory the store holds, capped.
fn simulator_booted_facts(unit: &crate::external::ExternalUnit) -> Vec<crate::evidence::Evidence> {
    if unit.category != crate::locations::StorageCategory::Environments {
        return Vec::new();
    }
    let udids: Vec<String> = crate::locations::shallow_dir_names(&unit.path)
        .into_iter()
        .filter(|n| is_simulator_device_udid(n))
        .collect();
    if udids.is_empty() {
        return Vec::new();
    }
    let env = crate::locations::Environment::from_process();
    let mut out: Vec<crate::evidence::Evidence> = udids
        .iter()
        .take(SIMULATOR_DEVICE_PROBE_CAP)
        .map(|udid| {
            crate::occupancy::simulator_booted_evidence(udid, &env)
                .with_note(format!("device {udid}"))
        })
        .collect();
    if udids.len() > SIMULATOR_DEVICE_PROBE_CAP {
        out.push(crate::evidence::Evidence::unknown(
            crate::evidence::FactKind::CurrentUse,
            crate::evidence::FactSubtype::Booted,
            crate::evidence::EvidenceSource::ProcessQuery {
                tool: "xcrun simctl list devices -j".into(),
            },
            crate::entities::now(),
            crate::reason!(
                "{} device directories in this store; the first {SIMULATOR_DEVICE_PROBE_CAP} were \
                 read this pass and the rest were not asked about",
                udids.len()
            ),
        ));
    }
    out
}

/// Whether this unit's detector declares a Maven-layout dependency
/// store (`locations::StoreEntryLookup::MavenLayout`), read from the
/// registry rather than matched against a detector id -- so a second
/// Maven-layout store (a mirror, a `<localRepository>` override) is
/// recognized without editing anything here.
///
/// The fallback is the layout Maven itself documents and the detector
/// hard-codes (`~/.m2/repository`), used while the detector-declared
/// capability is still being filled in; see the session note.
fn is_maven_layout_store(unit: &crate::external::ExternalUnit) -> bool {
    use crate::locations::{ConventionRole, StoreEntryLookup};
    let registry = crate::locations::Registry::with_builtins();
    if let Some(d) = registry
        .detectors()
        .iter()
        .find(|d| d.id() == unit.detector_id)
        && d.manager_conventions().iter().any(|c| {
            matches!(
                c.role,
                ConventionRole::DependencyStore {
                    lookup: StoreEntryLookup::MavenLayout,
                    ..
                }
            )
        })
    {
        return true;
    }
    // No path-shape fallback. An earlier revision also matched
    // `.m2/repository` directly, because the Maven detector did not yet
    // declare its convention; it does now
    // (`locations::maven::MavenDetector::manager_conventions`), and a
    // path literal here would be exactly the wiring table
    // `.oh/guardrails/detector-ids-only-in-registry.md` exists to
    // prevent -- one that keeps working while the capability it
    // duplicates silently stops being declared.
    false
}

/// Recovery evidence (#58) for one external unit, selected by the unit's
/// own storage category plus the capabilities its detector declares --
/// never by a detector-id match.
fn external_recovery_facts(unit: &crate::external::ExternalUnit) -> Vec<crate::evidence::Evidence> {
    use crate::locations::{ConventionRole, StorageCategory};
    let carry = |r: crate::recovery::RecoveryAssessment| match &r.follow_up_check {
        Some(check) => r.evidence.clone().with_note(format!("check: {check}")),
        None => r.evidence,
    };
    let mut out = Vec::new();

    // Maven's local repository mixes downloaded and locally-`mvn
    // install`ed artifacts in one tree. At store granularity there is no
    // `_remote.repositories` marker to read, so the honest fact is the
    // stated limit, not a blanket `network_fetch` label for the whole
    // repository.
    if is_maven_layout_store(unit) {
        out.push(carry(crate::recovery::maven_artifact_recovery(false)));
    }

    // An installation store holds versions a manager can reinstall by
    // name. The manager is the one whose detector declares the
    // installed-versions convention; the versions are the store's own
    // single-level directory names, never a traversal.
    if unit.category == StorageCategory::Installation {
        let registry = crate::locations::Registry::with_builtins();
        let declares_versions = registry
            .detectors()
            .iter()
            .find(|d| d.id() == unit.detector_id)
            .is_some_and(|d| {
                d.manager_conventions()
                    .iter()
                    .any(|c| matches!(c.role, ConventionRole::DeclaredVersions { .. }))
            });
        if declares_versions {
            let installed = crate::locations::shallow_dir_names(&unit.path);
            if installed.is_empty() {
                out.push(carry(crate::recovery::toolchain_installation_recovery(
                    &unit.detector_name,
                    None,
                )));
            } else {
                for version in installed.iter().take(SIMULATOR_DEVICE_PROBE_CAP) {
                    out.push(carry(crate::recovery::toolchain_installation_recovery(
                        &unit.detector_name,
                        Some(version),
                    )));
                }
            }
        }
    }
    out
}

/// Byte accounting (#59) for one external unit whose storage is sparse:
/// a file's *apparent* length can be far larger than the blocks actually
/// charged on disk (a VM disk image is the canonical case). Removing it
/// only ever frees the allocated blocks -- the gap was never occupying
/// space -- so `sparse_file_accounting` keeps the two numbers separate
/// and never counts the apparent size as reclaimable.
///
/// One `symlink_metadata` for a file-backed unit, or one single-level
/// `locations::shallow_list` plus one `stat` per direct file child for a
/// directory-backed one. Never a traversal, and only for a unit a human
/// named.
fn sparse_byte_accounting_facts(
    unit: &crate::external::ExternalUnit,
) -> Vec<crate::evidence::Evidence> {
    let mut logical = 0u64;
    let mut allocated = 0u64;
    let mut add = |meta: &Metadata| {
        if meta.is_file() {
            logical = logical.saturating_add(meta.len());
            allocated = allocated.saturating_add(meta.blocks().saturating_mul(512));
        }
    };
    match fs_gate::symlink_metadata(&unit.path) {
        Ok(meta) if meta.is_file() => add(&meta),
        Ok(meta) if meta.is_dir() => {
            for entry in crate::locations::shallow_list(&unit.path) {
                if entry.is_dir {
                    continue;
                }
                if let Ok(m) = fs_gate::symlink_metadata(unit.path.join(&entry.name)) {
                    add(&m);
                }
            }
        }
        _ => return Vec::new(),
    }
    if logical <= allocated {
        // Nothing sparse to report: an ordinary dense file's apparent
        // length and its allocated blocks agree (up to block rounding,
        // which rounds allocation *up*).
        return Vec::new();
    }
    let acc = crate::reclaimability::sparse_file_accounting(logical, allocated);
    crate::reclaimability::accounting_evidence(
        &acc,
        crate::evidence::EvidenceSource::FilesystemMetadata {
            detail: "apparent length versus allocated blocks of this unit's own files (sparse \
                     storage; the unallocated gap was never occupying disk)"
                .into(),
        },
    )
}

/// The deep, live per-unit evidence (#55/#58/#59) for one external unit:
/// manager-lock and simulator-booted current-use readings, recovery
/// facts by storage category, and sparse-file byte accounting -- none of
/// it taken during identification (a live probe on every detected unit
/// on every ordinary report would spawn a process per unit); only when
/// something -- a human reviewing `--view external`, or an agent
/// explaining what a unit is -- asks about this exact one.
pub fn unit_from_external(unit: &crate::external::ExternalUnit) -> PlanUnit {
    let category = format!("{:?}", unit.category);
    let mut evidence = unit.evidence.clone();
    evidence.extend(manager_lock_facts(unit));
    evidence.extend(simulator_booted_facts(unit));
    evidence.extend(external_recovery_facts(unit));
    evidence.extend(sparse_byte_accounting_facts(unit));
    PlanUnit {
        cargo_group: None,
        path: unit.path.clone(),
        rel_path: ".".into(),
        project: format!("(external: {})", unit.detector_name),
        project_id: format!("external:{}", unit.detector_id),
        worktree_id: format!("external:{}", unit.detector_id),
        worktree_path: unit.path.clone(),
        kind: ArtifactKind::Unknown,
        bytes: unit.bytes,
        dedup_stale: false,
        growth_bytes: unit.growth_bytes,
        regrowth_count: unit.regrowth_count,
        observed_at: unit.observed_at,
        recovery: format!(
            "shared {category} storage; removing it means the manager that owns it fetches or \
             rebuilds it again the next time it is needed"
        ),
        idle_secs: None,
        merge_complete: false,
        signals: Vec::new(),
        verb: "external".into(),
        track: None,
        warnings: vec![format!(
            "external unit ({category}): shown here for review; act on it with the manager's \
             own tools, not swamp"
        )],
        evidence,
        agent_meta: None,
    }
}

fn unit_from_row(project: &ProjectRow, wt: &WorktreeRow, a: &ArtifactRow) -> PlanUnit {
    let rel = crate::scope::relative_to(&a.path, &wt.path)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| a.path.display().to_string());
    PlanUnit {
        cargo_group: None,
        path: a.path.clone(),
        rel_path: rel,
        project: project.name.clone(),
        project_id: project.project_id.clone(),
        worktree_id: wt.worktree_id.clone(),
        worktree_path: wt.path.clone(),
        kind: a.kind.clone(),
        bytes: a.bytes,
        dedup_stale: a.dedup_stale,
        growth_bytes: a.growth_bytes,
        regrowth_count: a.regrowth_count,
        observed_at: a.observed_at,
        recovery: recovery_for(&a.kind).to_string(),
        idle_secs: wt.idle_secs,
        merge_complete: worktree_merge_complete(wt),
        signals: signal_strings(wt),
        verb: "delete".into(),
        track: a.track,
        warnings: warnings_for(wt, a, None),
        evidence: plan_unit_evidence(&a.evidence, &a.path),
        agent_meta: None,
    }
}

// ---------------------------------------------------------------------
// Agent-storage actions (#101): cache/log Trash moves and explicit
// session removal, both through this same plan/ledger/Trash path.
// A supported unit becomes a real, markable `PlanUnit` only after
// `agent_refusal` clears it.
// ---------------------------------------------------------------------

/// A file whose name suggests a SQLite database or one of its sidecar
/// files. Refused unconditionally (guardrail: "no individual WAL/SHM
/// deletion, no guessed SQLite cleanup") even though no named Claude
/// Code path is currently documented as SQLite -- defense in depth for
/// a future path this adapter has not been told about.
fn is_sqlite_like(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".sqlite")
        || lower.ends_with(".sqlite3")
        || lower.ends_with(".db")
        || lower.ends_with(".db-wal")
        || lower.ends_with(".db-shm")
        || lower.ends_with("-wal")
        || lower.ends_with("-shm")
}

/// Why `unit` cannot be proposed as an actionable agent-storage plan
/// unit, or `None` if it can be listed for the Trash. Occupancy is never
/// checked here: it is shown as a fact at mark/confirm time, never a
/// veto against building the row at all.
fn agent_refusal(u: &crate::agents::AgentUnit) -> Option<String> {
    if u.protected {
        let reason = u
            .protect_reason
            .clone()
            .unwrap_or_else(|| format!("{} is protected by default", u.category.label()));
        return Some(format!("protected: {reason}"));
    }
    if u.action == crate::agents::AgentActionCapability::None {
        return Some(format!(
            "{}: swamp has no Trash move for this category; remove it yourself if you want it gone",
            u.category.label()
        ));
    }
    let touches_db_like = std::iter::once(&u.path)
        .chain(u.members.iter().map(|m| &m.path))
        .any(|p| is_sqlite_like(p));
    if touches_db_like {
        return Some(
            "touches a database-like (SQLite/WAL/SHM) file; never deleted individually".into(),
        );
    }
    None
}

/// Builds a list of Trash-able units from selected `AgentUnit`s (#101).
/// Every unit that reaches the list carries `agent_meta`; anything
/// `agent_refusal` names is refused here, never silently downgraded.
pub fn propose_agents(
    units: &[crate::agents::AgentUnit],
    paths: &[PathBuf],
    proposed_by: &str,
) -> Result<Vec<PlanUnit>> {
    let mut plan_units = Vec::new();
    let mut refused = Vec::new();
    for u in units {
        if !paths.is_empty() && !paths.iter().any(|p| p == &u.path) {
            continue;
        }
        match agent_refusal(u) {
            Some(cause) => refused.push(Refused {
                path: u.path.clone(),
                cause,
            }),
            None => plan_units.push(unit_from_agent(u)),
        }
    }
    for p in paths {
        if !plan_units.iter().any(|u| &u.path == p) && !refused.iter().any(|r| &r.path == p) {
            refused.push(Refused {
                path: p.clone(),
                cause: "no agent-storage unit at this exact path in the current scope".to_string(),
            });
        }
    }
    if plan_units.is_empty() {
        bail!(
            "nothing to propose: no actionable agent-storage unit matched{}",
            if refused.is_empty() {
                String::new()
            } else {
                format!(
                    " ({} refused: {})",
                    refused.len(),
                    refused
                        .iter()
                        .map(|r| format!("{} — {}", r.path.display(), r.cause))
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            }
        );
    }
    // Defense in depth (#101's refusal matrix): today's adapters never
    // produce two agent-storage units whose own anchor paths nest
    // (each unit's identity is one category's own folded
    // directory/file), but nothing *enforces* that invariant across
    // fourteen independent adapters plus whatever a future one adds.
    // Refuse before a plan is minted, the same discipline `propose`'s
    // own Cargo-group overlap check already applies to filesystem
    // units, rather than silently accepting a plan whose execution
    // order could move a parent out from under a child (or vice
    // versa).
    for (i, a) in plan_units.iter().enumerate() {
        for b in plan_units.iter().skip(i + 1) {
            if crate::scope::overlapping(&a.path, &b.path) {
                bail!(
                    "overlapping agent-storage selections: {} and {} are nested (or identical); select either the parent or the child, not both",
                    a.path.display(),
                    b.path.display()
                );
            }
        }
    }
    let _ = proposed_by;
    Ok(plan_units)
}

fn unit_from_agent(u: &crate::agents::AgentUnit) -> PlanUnit {
    use crate::agents::{AgentActionCapability, ProjectLinkState};
    let category = u.category.label().to_string();
    let session_members = if u.action == AgentActionCapability::SessionRemoval {
        Some(u.members.iter().map(|m| m.path.clone()).collect())
    } else {
        None
    };
    let mut warnings = vec![format!(
        "agent-storage unit ({category}, tool {})",
        u.tool_name
    )];
    match u.action {
        AgentActionCapability::SessionRemoval => {
            warnings.push(
                "removes this session's resume/rewind/checkpoint history; the linked project's \
                 own files are untouched"
                    .into(),
            );
            if let ProjectLinkState::Linked { project_name, .. } = &u.project_link {
                warnings.push(format!("linked project: {project_name}"));
            }
            for m in &u.members {
                warnings.push(format!("member: {} ({:?})", m.path.display(), m.kind));
            }
        }
        AgentActionCapability::CacheOrLogTrash => {
            warnings.push(agent_single_path_consequence(u.category).to_string());
        }
        AgentActionCapability::None => {}
    }
    PlanUnit {
        cargo_group: None,
        path: u.path.clone(),
        rel_path: u.relative_path.clone(),
        project: match &u.project_link {
            ProjectLinkState::Linked { project_name, .. } => project_name.clone(),
            _ => format!("(agent: {})", u.tool_name),
        },
        project_id: format!("agent:{}", u.tool_id),
        worktree_id: format!("agent:{}:{}", u.tool_id, u.id),
        worktree_path: u.path.clone(),
        kind: ArtifactKind::Unknown,
        bytes: u.bytes,
        dedup_stale: false,
        growth_bytes: u.growth_bytes,
        regrowth_count: u.regrowth_count,
        observed_at: u.observed_at,
        recovery: match u.action {
            AgentActionCapability::CacheOrLogTrash => {
                agent_single_path_consequence(u.category).to_string()
            }
            AgentActionCapability::SessionRemoval => {
                "irrecoverable outside Trash: unique conversation/checkpoint history".to_string()
            }
            AgentActionCapability::None => "no swamp Trash move for this category".to_string(),
        },
        idle_secs: None,
        merge_complete: false,
        signals: Vec::new(),
        verb: "delete".into(),
        track: None,
        warnings,
        evidence: plan_unit_evidence(&u.evidence, &u.path),
        agent_meta: Some(AgentPlanMeta {
            tool_id: u.tool_id.clone(),
            tool_home: u.tool_home.clone(),
            category,
            session_members,
        }),
    }
}

fn agent_single_path_consequence(category: crate::agents::AgentCategory) -> &'static str {
    use crate::agents::AgentCategory;
    match category {
        AgentCategory::LocalHistory => {
            "loses past command recall or editor undo snapshots; restore from Trash or a backup, not by rerunning the tool"
        }
        AgentCategory::Logs => {
            "loses past diagnostic logs; new logs do not recreate old records; restore from Trash or a backup"
        }
        AgentCategory::Caches => {
            "discards cached data; recreating it may require rerunning the tool or downloading dependencies; restore exact contents from Trash"
        }
        _ => "restore from Trash or a backup; automatic recreation is not established",
    }
}

/// A single-path Trash move of a cache/log category directory. The only
/// refusal is an OS-level error (the directory is gone, or is now a
/// symlink where a directory was reviewed) -- there is no re-derivation
/// gate in front of this: the human was shown this unit's facts before
/// pressing Enter.
pub fn trash_agent_cache(path: &Path, trash: &Path, at: u64) -> Result<(PathBuf, u64)> {
    let meta = fs_gate::symlink_metadata(path).context("path no longer exists")?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        bail!("path is no longer a directory (or is a symlink)");
    }
    let (bytes, _mtime, _truncated) = crate::agents::folded_bytes(path, 2_000_000);
    let basename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("agent-cache");
    let dest = fs_gate::destroy::trash_move(path, trash, &format!("agent-cache-{basename}-{at}"))?;
    Ok((dest.into_path(), bytes))
}

/// One member's fate inside a session-removal Trash envelope's own
/// recovery manifest (`restore.json`, mirroring `cargo_cleanup`'s own
/// precedent of writing a manifest into the envelope it creates).
/// Written *before* any member is moved (every entry `"pending"`) and
/// rewritten after each successful move, so a partial failure (some
/// members moved, then a rename fails) still leaves an accurate,
/// on-disk account of exactly what happened -- never just an error
/// message with no durable record next to the moved content itself.
#[derive(Debug, Clone, Serialize)]
struct RestoreManifestMember {
    /// Absolute original path, so a human/script can restore it.
    original: PathBuf,
    /// Name inside the envelope once moved; `None` while `"pending"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    moved_to: Option<String>,
    bytes: u64,
    /// `"pending"` | `"moved"`.
    status: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct RestoreManifest {
    tool_id: String,
    category: String,
    session_path: PathBuf,
    members: Vec<RestoreManifestMember>,
}

fn write_restore_manifest(
    envelope: &fs_gate::destroy::Envelope,
    manifest: &RestoreManifest,
) -> Result<()> {
    envelope.write_manifest(manifest)
}

/// Returned when a session removal fails *after* the Trash envelope was
/// created and at least the pre-flight pass completed -- i.e. some
/// members may already be physically inside `envelope`. Carries enough
/// for the caller to still record an honest `recovery_location` and
/// partial `trashed_bytes` rather than silently losing track of content
/// that really did move. See `restore.json` inside `envelope` for the
/// exact per-member outcome.
#[derive(Debug)]
pub struct PartialAgentRemoval {
    pub envelope: PathBuf,
    pub moved_bytes: u64,
    pub source: anyhow::Error,
}

impl std::fmt::Display for PartialAgentRemoval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({} already moved into {}; see restore.json there for the exact per-member outcome)",
            self.source,
            crate::render::human_bytes_pub(self.moved_bytes),
            self.envelope.display()
        )
    }
}

impl std::error::Error for PartialAgentRemoval {}

/// Moves a session's exact member set (exactly what was reviewed) into
/// one Trash envelope. Nothing here is re-derived or compared against
/// live state first: a member that vanished in the meantime is just an
/// ordinary rename failure, reported like any other OS error, and the
/// members already moved stay recorded in `restore.json`.
pub fn trash_agent_session(
    meta: &AgentPlanMeta,
    session_path: &Path,
    members: &[PathBuf],
    trash: &Path,
    at: u64,
) -> Result<(PathBuf, u64)> {
    let current_members: Vec<PathBuf> = members.to_vec();

    // Pre-flight: stat every member *before* moving any of them, so the
    // common failure (a member vanished since it was reviewed) is caught
    // before this session is left half-moved. This does not make the
    // multi-file move fully atomic (a concurrent deletion or a
    // cross-device rename can still fail mid-loop), but it removes the
    // most likely partial-failure cause outright.
    let mut sized_members: Vec<(PathBuf, u64, bool)> = Vec::with_capacity(current_members.len());
    for member in &current_members {
        let member_meta = fs_gate::symlink_metadata(member)
            .with_context(|| format!("member no longer exists: {}", member.display()))?;
        let is_dir = member_meta.is_dir();
        let bytes = if is_dir {
            crate::agents::folded_bytes(member, 2_000_000).0
        } else {
            member_meta.len()
        };
        sized_members.push((member.clone(), bytes, is_dir));
    }

    let slug = session_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("session");
    let mut envelope = fs_gate::destroy::Envelope::open(
        session_path,
        trash,
        &format!("agent-session-{slug}-{at}"),
        None,
    )?;

    let mut manifest = RestoreManifest {
        tool_id: meta.tool_id.clone(),
        category: meta.category.clone(),
        session_path: session_path.to_path_buf(),
        members: sized_members
            .iter()
            .map(|(path, bytes, _)| RestoreManifestMember {
                original: path.clone(),
                moved_to: None,
                bytes: *bytes,
                status: "pending",
            })
            .collect(),
    };
    // Written before any move, so even a failure on the very first
    // member leaves an accurate (all-pending) manifest next to whatever
    // Trash envelope directory was created.
    write_restore_manifest(&envelope, &manifest)?;

    let mut moved_bytes = 0u64;
    for (i, (member, bytes, _is_dir)) in sized_members.iter().enumerate() {
        let name = member
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("member");
        let dest_name = format!("{i}-{name}");
        if let Err(e) = envelope.move_member(member, &dest_name).with_context(|| {
            format!(
                "rename to Trash failed for {} ({} of {} members already moved into {})",
                member.display(),
                i,
                sized_members.len(),
                envelope.path().display()
            )
        }) {
            // Best-effort: leave the manifest reflecting exactly what
            // moved before this failure, never silently stale.
            let _ = write_restore_manifest(&envelope, &manifest);
            return Err(PartialAgentRemoval {
                envelope: envelope.path().to_path_buf(),
                moved_bytes,
                source: e,
            }
            .into());
        }
        moved_bytes += bytes;
        manifest.members[i].status = "moved";
        manifest.members[i].moved_to = Some(dest_name);
        // Rewritten after every successful move (not only at the end),
        // so a failure on member i+1 still leaves an accurate record of
        // members 0..=i having actually moved.
        write_restore_manifest(&envelope, &manifest)?;
    }
    Ok((envelope.path().to_path_buf(), moved_bytes))
}

/// A whole worktree (linked → `remove-worktree`) or checkout (→ `archive`)
/// as one unit, when a proposer asks for the worktree path itself.
fn unit_from_worktree(project: &ProjectRow, wt: &WorktreeRow) -> PlanUnit {
    let bytes: u64 = wt.artifacts.iter().map(|a| a.bytes).sum();
    let linked = wt.kind == crate::report::WorktreeKind::Linked;
    let pseudo = ArtifactRow {
        kind: ArtifactKind::Source,
        path: wt.path.clone(),
        bytes,
        mtime_max: 0,
        ecosystem: None,
        hardlinked: false,
        dedup_stale: wt.artifacts.iter().any(|a| a.dedup_stale),
        allocated_bytes: None,
        allocated_growth_bytes: None,
        local_bytes: bytes,
        track: None,
        growth_bytes: wt
            .artifacts
            .iter()
            .filter_map(|a| a.growth_bytes)
            .reduce(|x, y| x + y),
        regrowth_count: 0,
        observed_at: wt
            .artifacts
            .first()
            .map(|a| a.observed_at)
            .unwrap_or_else(now),
        confidence: crate::entities::Confidence::High,
        source: crate::report::Source::new("filesystem.walk"),
        note: None,
        created_at: None,
        containers: Vec::new(),
        shared_with: Vec::new(),
        dangling: false,
        evidence: Vec::new(),
    };
    let mut u = unit_from_row(project, wt, &pseudo);
    u.rel_path = ".".into();
    u.verb = if linked { "remove-worktree" } else { "archive" }.into();
    u.recovery = if linked {
        "git worktree add again from the same repo".into()
    } else {
        project
            .remote
            .as_ref()
            .map(|r| format!("git clone {r}"))
            .unwrap_or_else(|| "none: no remote".into())
    };
    u.warnings = warnings_for(wt, &pseudo, Some(project));
    u
}

/// A Source directory (from the dir rollups) as one deletable unit.
fn unit_from_dir(project: &ProjectRow, wt: &WorktreeRow, d: &crate::report::DirRollup) -> PlanUnit {
    let pseudo = ArtifactRow {
        kind: ArtifactKind::Unknown,
        path: wt.path.join(&d.rel_path),
        bytes: d.allocated_total,
        mtime_max: 0,
        ecosystem: None,
        hardlinked: false,
        dedup_stale: false,
        allocated_bytes: None,
        allocated_growth_bytes: None,
        local_bytes: d.allocated_total,
        track: d.track,
        growth_bytes: None,
        regrowth_count: 0,
        observed_at: wt
            .artifacts
            .first()
            .map(|a| a.observed_at)
            .unwrap_or_else(now),
        confidence: crate::entities::Confidence::High,
        source: crate::report::Source::new("filesystem.walk"),
        note: None,
        created_at: None,
        containers: Vec::new(),
        shared_with: Vec::new(),
        dangling: false,
        evidence: Vec::new(),
    };
    unit_from_row(project, wt, &pseudo)
}

pub use crate::preserve::{Preserved, preserve_executables};

/// Moves a Cargo purpose group's exact member list to the Trash.
pub fn trash_cargo_group(
    group: &crate::cargo_cleanup::CargoGroup,
    trash: &Path,
) -> Result<PathBuf> {
    crate::cargo_cleanup::move_group(group, trash)
}

/// The Trash root `fs_gate::destroy::trash_move`/`Envelope::open` write
/// into: `~/.Trash` on macOS (a rename target directly); on Linux, the
/// freedesktop home trash (`$XDG_DATA_HOME/Trash`, defaulting to
/// `~/.local/share/Trash`), inside which the gate itself lays out
/// `files/` and `info/` (see `fs_gate::destroy::items_dir`). Always
/// overridable via `SWAMP_TRASH_DIR` (tests; never process-global
/// state).
pub fn trash_root() -> PathBuf {
    if let Ok(dir) = std::env::var("SWAMP_TRASH_DIR") {
        return PathBuf::from(dir);
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
            && !xdg.is_empty()
        {
            return PathBuf::from(xdg).join("Trash");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    #[cfg(target_os = "linux")]
    {
        PathBuf::from(home).join(".local/share/Trash")
    }
    #[cfg(not(target_os = "linux"))]
    {
        PathBuf::from(home).join(".Trash")
    }
}

/// Free space on the volume containing `path`, in bytes, via `df -k`.
/// Returns `None` if `df` cannot be read (advisory only: a missing
/// measurement never blocks or fakes the reported result).
pub fn free_space_bytes(path: &Path) -> Option<u64> {
    let out = crate::fs_gate::spawn::run(
        crate::fs_gate::spawn::Program::Df,
        [std::ffi::OsStr::new("-k"), path.as_os_str()],
        std::time::Duration::from_secs(10),
    )
    .ok()?;
    if !out.success() {
        return None;
    }
    let text = out.stdout_lossy();
    let fields: Vec<&str> = text.lines().nth(1)?.split_whitespace().collect();
    let available_kb: u64 = fields.get(3)?.parse().ok()?;
    Some(available_kb * 1024)
}

#[cfg(test)]
mod agent_partial_removal_tests {
    //! #101's "account for partial failure (some members moved, then
    //! failure) with explicit outcome and recovery manifest": these
    //! tests call the private `execute_agent_session_removal` directly
    //! (same crate, same file) because forcing a *specific* member's
    //! `rename` to fail deterministically needs to reach in past the
    //! public `execute` surface. No real user data anywhere -- every
    //! path here is a synthetic fixture under a `tempfile::tempdir`.
    use super::*;
    use crate::agents::claude_code::{CLAUDE_CODE_TOOL_ID, identify};
    use std::fs;

    fn touch(path: &Path, content: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// A Claude Code session with several members (transcript,
    /// subagents-companion dir, file-history dir, todos file) -- the
    /// same fixture shape `claude_code`'s own tests use, reused here so
    /// a real, multi-member session removal is exercised, not a
    /// hand-built `AgentUnit`.
    fn fixture_session(home: &Path) -> (PathBuf, String) {
        let repo = home.join("fixture-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let session_id = "22222222-2222-4222-8222-222222222222".to_string();
        let proj_dir = home.join("projects").join("-fixture-repo-encoded");
        let jsonl = proj_dir.join(format!("{session_id}.jsonl"));
        let line = format!(
            "{{\"type\":\"user\",\"sessionId\":\"s\",\"cwd\":\"{}\",\"gitBranch\":\"main\"}}\n",
            repo.display()
        );
        touch(&jsonl, line.as_bytes());
        touch(
            &proj_dir.join(&session_id).join("subagents").join("a.jsonl"),
            line.as_bytes(),
        );
        touch(
            &home.join("file-history").join(&session_id).join("snap.txt"),
            b"recoverable-fixture-content",
        );
        touch(
            &home
                .join("todos")
                .join(format!("{session_id}-agent-1.json")),
            b"[]",
        );
        (jsonl, session_id)
    }

    /// Forces the *last* member (in the same sorted order
    /// `execute_agent_session_removal` itself uses) to fail its rename
    /// by pre-occupying its exact destination inside the envelope with
    /// an incompatible entry (a plain file where a directory needs to
    /// land, or vice versa -- both are reliable, portable `rename`
    /// failures). Returns the envelope path and the members in the
    /// order the function will process them, so the test can assert
    /// precisely which ones must have moved and which must not have.
    fn force_last_member_rename_to_fail(
        home: &Path,
        trash: &Path,
        session_path: &Path,
        session_id: &str,
        at: u64,
    ) -> (PathBuf, Vec<PathBuf>) {
        let cache = crate::agents::IdentificationCache::disabled();
        let candidates = identify(home, &crate::agents::IdentifyCtx::new(at, &cache));
        let current = candidates
            .iter()
            .find(|c| c.path == session_path)
            .expect("session identified");
        let mut members: Vec<PathBuf> = current.members().iter().map(|m| m.path.clone()).collect();
        members.sort();
        assert!(
            members.len() >= 2,
            "need at least two members to prove a *partial* failure, got {members:?}"
        );

        let slug = session_path.file_stem().and_then(|s| s.to_str()).unwrap();
        assert_eq!(slug, session_id);
        // On Linux, `Envelope::open` nests items under `trash/files/`
        // (the freedesktop Trash layout); on other platforms the item
        // sits directly under `trash`.
        #[cfg(target_os = "linux")]
        let items_root = trash.join("files");
        #[cfg(not(target_os = "linux"))]
        let items_root = trash.to_path_buf();
        let envelope = items_root.join(format!("agent-session-{slug}-{at}"));
        fs::create_dir_all(&envelope).unwrap();

        let last_idx = members.len() - 1;
        let last = &members[last_idx];
        let name = last.file_name().and_then(|n| n.to_str()).unwrap();
        let dest = envelope.join(format!("{last_idx}-{name}"));
        let last_is_dir = fs::symlink_metadata(last).unwrap().is_dir();
        if last_is_dir {
            // Renaming a directory onto an existing non-directory path
            // fails (ENOTDIR) on every platform this project targets.
            fs::write(&dest, b"blocking").unwrap();
        } else {
            // Renaming a file onto an existing non-empty directory
            // fails (EISDIR/ENOTEMPTY) on every platform this project
            // targets.
            fs::create_dir_all(dest.join("blocking-child")).unwrap();
        }
        (envelope, members)
    }

    #[test]
    fn partial_session_removal_reports_the_envelope_and_writes_a_restore_manifest() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let trash = tempfile::tempdir().unwrap();
        let (session_path, session_id) = fixture_session(home);
        let at = 5_000_000u64;
        let (envelope, members) =
            force_last_member_rename_to_fail(home, trash.path(), &session_path, &session_id, at);

        let meta = AgentPlanMeta {
            tool_id: CLAUDE_CODE_TOOL_ID.to_string(),
            tool_home: home.to_path_buf(),
            category: "sessions".to_string(),
            session_members: Some(members.clone()),
        };
        let err = trash_agent_session(&meta, &session_path, &members, trash.path(), at)
            .expect_err("the last member's rename was deliberately blocked");
        let partial = err
            .downcast_ref::<PartialAgentRemoval>()
            .unwrap_or_else(|| panic!("expected PartialAgentRemoval, got: {err:#}"));
        assert_eq!(partial.envelope, envelope);
        assert!(
            partial.moved_bytes > 0 || members.len() == 1,
            "at least the members before the blocked one must have moved"
        );

        // The recovery manifest exists and reflects exactly what
        // happened: every member but the last is "moved" (and no
        // longer at its original path); the last is "pending" (and
        // untouched at its original path).
        let manifest_bytes = fs::read(envelope.join("restore.json")).expect("restore.json exists");
        let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        assert_eq!(manifest["tool_id"], CLAUDE_CODE_TOOL_ID);
        let manifest_members = manifest["members"].as_array().unwrap();
        assert_eq!(manifest_members.len(), members.len());
        let last_idx = members.len() - 1;
        for (i, m) in manifest_members.iter().enumerate() {
            let original = std::path::PathBuf::from(m["original"].as_str().unwrap());
            assert_eq!(&original, &members[i]);
            if i == last_idx {
                assert_eq!(m["status"], "pending", "{manifest}");
                assert!(
                    original.exists(),
                    "the blocked member must remain at its original location"
                );
            } else {
                assert_eq!(m["status"], "moved", "{manifest}");
                assert!(
                    !original.exists(),
                    "a member reported \"moved\" must no longer be at its original location"
                );
                assert!(m["moved_to"].as_str().is_some());
            }
        }

        // Nothing here ever wrote the fixture's own content into the
        // manifest (metadata only: original path, byte count, status).
        assert!(
            !manifest_bytes
                .windows(b"recoverable-fixture-content".len())
                .any(|w| w == b"recoverable-fixture-content")
        );
    }
}
