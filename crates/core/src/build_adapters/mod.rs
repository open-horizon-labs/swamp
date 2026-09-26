//! Build-artifact adapters: what is *inside* a build container, and
//! what removing a piece of it would cost.
//!
//! An artifact row (`target/`, `node_modules/`, `build/`) is an
//! accounting boundary the folded walk already measured. This module
//! identifies its interior into [`crate::artifact::NestedArtifact`]s the
//! same way [`crate::agents`] identifies the interior of an agent tool's
//! home -- same shape, different domain, and deliberately the same
//! seams:
//!
//! * a trait plus a static registry ([`registry::Registry`]), never a
//!   central `match ecosystem`;
//! * identification that reads directory *names* from the folded walk's
//!   own rows, plus bounded reads of named manifests
//!   ([`bounded_io::read_manifest`]) -- never a second traversal, never
//!   a whole arbitrary file, never a subprocess;
//! * units built through [`NestedUnitBuilder`], whose defaults are the
//!   honest ones (unsupported coverage, unknown accounting basis,
//!   unknown time source, inspection only);
//! * container-level reuse gated on
//!   [`crate::fs_events::EventCoverage`], exactly as stack/13 gated the
//!   agent containers -- never on a directory's own modification stamp,
//!   which does not move when a file inside a subdirectory changes.
//!
//! # What an adapter is allowed to claim
//!
//! Identification is not a verdict. An adapter says what a directory
//! *is* (a build output, a test report, an installed dependency tree, an
//! entry in a shared store), when it was last *modified*, how many bytes
//! it holds on a stated basis, and what it would cost to get it back.
//! It never says a unit is unused, stale, obsolete or removable, and it
//! never infers last execution from a timestamp. Where the evidence runs
//! out the unit carries an explicit unknown -- an unsupported layout is
//! a named limitation, not an empty result.
//!
//! # Cost
//!
//! Ordinary refresh of an unchanged container does zero listings and
//! zero manifest reads: the container's previously identified units are
//! replayed under event coverage. A changed container pays one shallow
//! listing per directory the adapter actually needs to look inside, plus
//! one bounded manifest read per named metadata file. Both are counted
//! (`crate::work_counters`), which is what makes the cost tests in
//! `crates/core/tests/build_adapter_cost.rs` assertions rather than
//! claims.

pub mod android;
pub mod bounded_io;
pub mod cargo;
pub mod docker_buildkit;
pub mod go;
pub mod gradle;
pub mod jvm_common;
pub mod layout;
pub mod matrix;
pub mod maven;
pub mod node;
pub mod python;
pub mod registry;
pub mod xcode_swift;

use crate::artifact::{
    AccountingBasis, ArtifactCoverage, ArtifactEvidence, ArtifactRole, ArtifactVariant, Membership,
    NestedActionCapability, NestedArtifact, RoleFamily, TimeSource, relative_path,
};
use crate::entities::Confidence;
use crate::fs_events::EventCoverage;
use crate::locations::BuildStoreKind;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------
// Containers
// ---------------------------------------------------------------------

/// One build container an adapter has been asked to explain: a
/// `target/`, a `node_modules/`, a Gradle `build/`, a `~/.m2/repository`.
///
/// The path arrives from the report pipeline, resolved from the artifact
/// rows the folded walk produced and (for shared stores) from the
/// location detectors. An adapter never resolves a home, reads an
/// environment variable or guesses a root, for the same reason the agent
/// adapters do not: a fixture must be able to inject one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildContainer {
    pub adapter_id: &'static str,
    pub path: PathBuf,
    /// The checkout this container belongs to, when one is established.
    /// `None` for a shared store that belongs to no single project --
    /// which is a fact about ownership, not a missing value to fill in.
    pub project_root: Option<PathBuf>,
    /// Whether this container is shared across projects. A shared
    /// container's entries are charged once, wherever else they are
    /// linked from (#68: "Account for linked/shared store entries
    /// without duplicated measurement").
    pub shared: bool,
    /// Which kind of machine-wide store this is, when the detector that
    /// resolved it declared one (`locations::Detector::build_stores`).
    /// `None` for a project-local container, and for a shared store a
    /// caller built without a declaration (the adapters' own fixtures).
    pub store_kind: Option<BuildStoreKind>,
}

impl BuildContainer {
    pub fn project(adapter_id: &'static str, path: PathBuf, project_root: PathBuf) -> Self {
        Self {
            adapter_id,
            path,
            project_root: Some(project_root),
            shared: false,
            store_kind: None,
        }
    }

    pub fn shared_store(adapter_id: &'static str, path: PathBuf) -> Self {
        Self {
            adapter_id,
            path,
            project_root: None,
            shared: true,
            store_kind: None,
        }
    }

    /// A machine-wide store the detector declared as `kind`. What the
    /// external observation hands an adapter
    /// (`.oh/guardrails/build-stores-join-by-capability.md`).
    pub fn shared_store_of(adapter_id: &'static str, path: PathBuf, kind: BuildStoreKind) -> Self {
        Self {
            adapter_id,
            path,
            project_root: None,
            shared: true,
            store_kind: Some(kind),
        }
    }

    /// A store a daemon answers for rather than a directory swamp walks:
    /// one BuildKit builder's cache. The path is a stable name, never a
    /// filesystem location -- nothing inside it is statted, listed or
    /// read, and no unit under it carries filesystem provenance.
    pub fn daemon_store(adapter_id: &'static str, name: &str, kind: BuildStoreKind) -> Self {
        Self {
            adapter_id,
            path: PathBuf::from(format!("{DAEMON_STORE_SCHEME}{name}")),
            project_root: None,
            shared: true,
            store_kind: Some(kind),
        }
    }

    /// The container's own storage id, the prefix every unit inside it
    /// hangs off. Identical to what `cargo_artifacts` used before the
    /// port, so a stored report's units keep their ids.
    pub fn scope(&self) -> String {
        NestedArtifact::storage_id(&self.path, "")
    }
}

/// The `producer_evidence` source under which an adapter records a
/// manager's own last-use timestamp for a unit (a BuildKit record's
/// `LastUsedAt`), verbatim, so the decision-evidence layer can normalize
/// it without the adapter ever converting it into a filesystem age.
pub const LAST_USED_EVIDENCE: &str = "manager-last-used";

/// The path prefix of a [`BuildContainer::daemon_store`]. Not a
/// filesystem path on any platform, so nothing can mistake one for a
/// directory to stat.
pub const DAEMON_STORE_SCHEME: &str = "daemon-store://";

// ---------------------------------------------------------------------
// Folded rows: the structure an adapter is allowed to see
// ---------------------------------------------------------------------

/// One measured directory, as the folded walk left it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldedDir {
    pub path: PathBuf,
    /// This directory's own files plus every descendant's, in allocated
    /// bytes.
    pub allocated_total: u64,
    /// Newest modification among its measured entries, in seconds.
    pub mtime_max: u64,
    /// Whether the walk read all of it.
    pub complete: bool,
}

/// The folded walk's directory rows, indexed by absolute path.
///
/// This is the *only* structural input an adapter gets for free. It is
/// what makes "no second traversal" achievable rather than aspirational:
/// the walk already listed every directory under a container, so an
/// adapter asking "what is under `target/`" is answering from memory.
#[derive(Debug, Default, Clone)]
pub struct FoldedIndex {
    by_path: HashMap<PathBuf, FoldedDir>,
    children: HashMap<PathBuf, Vec<PathBuf>>,
}

impl FoldedIndex {
    pub fn from_dirs(dirs: impl IntoIterator<Item = FoldedDir>) -> Self {
        let mut idx = Self::default();
        for d in dirs {
            if let Some(parent) = d.path.parent() {
                idx.children
                    .entry(parent.to_path_buf())
                    .or_default()
                    .push(d.path.clone());
            }
            idx.by_path.insert(d.path.clone(), d);
        }
        for kids in idx.children.values_mut() {
            kids.sort();
            kids.dedup();
        }
        idx
    }

    pub fn get(&self, path: &Path) -> Option<&FoldedDir> {
        self.by_path.get(path)
    }

    /// Directories directly inside `path`, in name order.
    pub fn children(&self, path: &Path) -> Vec<&FoldedDir> {
        self.children
            .get(path)
            .map(|kids| kids.iter().filter_map(|k| self.by_path.get(k)).collect())
            .unwrap_or_default()
    }

    /// Every measured directory at or under `path`.
    pub fn under(&self, path: &Path) -> Vec<&FoldedDir> {
        let mut out: Vec<&FoldedDir> = self
            .by_path
            .values()
            .filter(|d| d.path.starts_with(path))
            .collect();
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }
}

// ---------------------------------------------------------------------
// Container-level reuse, gated on event coverage
// ---------------------------------------------------------------------

/// Previously identified units, keyed by container scope, with the
/// observation time they were written at.
#[derive(Debug, Default)]
pub struct ContainerCache {
    entries: RefCell<HashMap<String, (u64, Vec<NestedArtifact>)>>,
    enabled: bool,
}

impl ContainerCache {
    /// A cache nothing can be replayed from. What a store-less caller, a
    /// forced full walk and every execution-time recheck get.
    pub fn disabled() -> Self {
        Self {
            entries: RefCell::new(HashMap::new()),
            enabled: false,
        }
    }

    /// Seeds the cache from stored containers, each with the observation
    /// that last verified it -- a machine-wide store's units carry their
    /// own verification time, not one stamp for the whole table.
    pub fn from_containers(containers: Vec<(u64, Vec<NestedArtifact>)>) -> Self {
        let mut entries: HashMap<String, (u64, Vec<NestedArtifact>)> = HashMap::new();
        for (stored_at, units) in containers {
            let Some(key) = units
                .first()
                .map(|u| u.container_id.clone().unwrap_or_else(|| u.id.clone()))
            else {
                continue;
            };
            entries.insert(key, (stored_at, units));
        }
        Self {
            entries: RefCell::new(entries),
            enabled: true,
        }
    }

    /// Seeds the cache from a previous report's nested units, grouped by
    /// the container each one belongs to.
    pub fn from_previous(units: Vec<NestedArtifact>, stored_at: u64) -> Self {
        let mut grouped: HashMap<String, Vec<NestedArtifact>> = HashMap::new();
        for u in units {
            let key = u.container_id.clone().unwrap_or_else(|| u.id.clone());
            grouped.entry(key).or_default().push(u);
        }
        Self {
            entries: RefCell::new(
                grouped
                    .into_iter()
                    .map(|(k, v)| (k, (stored_at, v)))
                    .collect(),
            ),
            enabled: true,
        }
    }
}

/// What an adapter identifies through.
pub struct BuildCtx<'a> {
    pub observed_at: u64,
    folded: &'a FoldedIndex,
    coverage: &'a EventCoverage,
    cache: &'a ContainerCache,
    daemon: Option<&'a crate::docker::DockerFacts>,
}

impl<'a> BuildCtx<'a> {
    pub fn new(
        observed_at: u64,
        folded: &'a FoldedIndex,
        coverage: &'a EventCoverage,
        cache: &'a ContainerCache,
    ) -> Self {
        Self {
            observed_at,
            folded,
            coverage,
            cache,
            daemon: None,
        }
    }

    /// The Docker daemon's answers this pass (already fetched, cached and
    /// bounded by `crate::docker`), for an adapter that identifies a
    /// daemon store. An adapter never asks the daemon itself.
    pub fn with_daemon(mut self, facts: &'a crate::docker::DockerFacts) -> Self {
        self.daemon = Some(facts);
        self
    }

    pub fn daemon(&self) -> Option<&crate::docker::DockerFacts> {
        self.daemon
    }

    pub fn folded(&self) -> &FoldedIndex {
        self.folded
    }

    /// Identifies one container, replaying the stored units when this
    /// pass's event coverage can show that nothing under the container
    /// changed since they were written.
    ///
    /// The gate is [`EventCoverage::unchanged_since`] and nothing else.
    /// A directory-stamp comparison is deliberately absent: a container's
    /// own stamp does not move when a file inside one of its
    /// subdirectories changes, so stamp-only reuse replays a stale
    /// identification forever -- and a `target/` or `node_modules` is
    /// exactly the tree where that happens.
    pub fn container(
        &self,
        container: &BuildContainer,
        identify: &dyn Fn() -> Vec<NestedArtifact>,
    ) -> Vec<NestedArtifact> {
        if let Some(units) = self.replay(container) {
            crate::work_counters::record_container_reused();
            return units;
        }
        crate::work_counters::record_container_identified();
        identify()
    }

    /// The reuse decision on its own, so a test can ask for it without
    /// running an identification.
    pub fn can_reuse(&self, container: &BuildContainer) -> bool {
        self.replay_key(container).is_some()
    }

    fn replay_key(&self, container: &BuildContainer) -> Option<(String, u64)> {
        if !self.cache.enabled {
            return None;
        }
        let key = container.scope();
        let entries = self.cache.entries.borrow();
        let (stored_at, units) = entries.get(&key)?;
        let stored_at = *stored_at;
        // The rows must also have been written by the adapter that
        // claims this container *now*. Which adapter claims a directory
        // is decided by marker files beside it (`settings.gradle`
        // appearing next to a Node project's `build/`), and those live
        // outside the container, so no event under it says the claim
        // changed. Replaying the old adapter's rows would keep calling a
        // Gradle build directory a Node output -- forever, if nothing
        // inside it is ever touched.
        if !units
            .iter()
            .all(|u| u.adapter.as_deref() == Some(container.adapter_id))
        {
            return None;
        }
        drop(entries);
        // The whole gate, in one condition: a trusted window that
        // covers this container, opened no later than the rows were
        // written, reporting no event under it.
        self.coverage
            .unchanged_since(&container.path, stored_at)
            .then_some((key, stored_at))
    }

    fn replay(&self, container: &BuildContainer) -> Option<Vec<NestedArtifact>> {
        let (key, _) = self.replay_key(container)?;
        let mut entries = self.cache.entries.borrow_mut();
        let (observed_at, units) = entries.get_mut(&key)?;
        // Replayed *and re-verified*: the window vouched for these rows
        // just now, so the next pass's window -- which starts where this
        // one ended -- can vouch for them again. Without this, reuse
        // would only ever work on alternate passes.
        *observed_at = self.observed_at;
        Some(units.clone())
    }

    /// A single-level listing of `dir`, through the shared capped
    /// helper. Counted, symlink-refusing, and the only listing an
    /// adapter may do.
    pub fn list(&self, dir: &Path) -> Vec<crate::locations::ShallowEntry> {
        self.list_checked(dir).0
    }

    /// [`BuildCtx::list`], with whether the listing stopped at its cap
    /// or the directory could not be read at all
    /// (`crate::locations::Truncation`). A caller whose output is an
    /// aggregate over the entries -- a count, a "this many packages" --
    /// must read this rather than assume completeness.
    pub fn list_checked(
        &self,
        dir: &Path,
    ) -> (
        Vec<crate::locations::ShallowEntry>,
        crate::locations::Truncation,
    ) {
        let listing = crate::locations::shallow_list(dir);
        let truncation = listing.truncation;
        (listing.into_iter().collect(), truncation)
    }

    /// `symlink_metadata` for one named path. Not a traversal: the
    /// adapter already knew this path's name from the folded rows or
    /// from a capped listing.
    pub fn stat(&self, path: &Path) -> Option<crate::fs_gate::Metadata> {
        crate::work_counters::record_files_statted(1);
        crate::fs_gate::symlink_metadata(path).ok()
    }

    /// A bounded read of one named manifest.
    pub fn manifest(&self, path: &Path) -> Option<bounded_io::Manifest> {
        bounded_io::read_manifest(path, bounded_io::MAX_MANIFEST_BYTES)
    }
}

// ---------------------------------------------------------------------
// The trait
// ---------------------------------------------------------------------

/// What an adapter says it can do, checked against the published matrix.
///
/// Every field defaults to `false`, and that is the point: an adapter
/// that says nothing claims nothing. Project-local Trash support is checked
/// against the adapter's explicit role allowlist and the published matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BuildCapabilities {
    /// Whether this adapter identifies containers shared across
    /// projects (a package store, a local repository) as well as
    /// project-local ones.
    pub identifies_shared_stores: bool,
    /// Whether this adapter can attribute a unit to a package identity
    /// (a name and version) from read-only metadata.
    pub attributes_package_identity: bool,
    /// Whether this adapter exposes exact-path actions on its nested units.
    /// Cargo's separate fingerprint-aware cleanup plan is not this capability.
    pub actions_available: bool,
}

/// One ecosystem family's read-only build-artifact identification.
///
/// An adapter identifies, and only identifies: it reads folded rows,
/// capped listings and bounded manifests, returns
/// [`NestedArtifact`]s, and never acts, never emits and never spawns.
/// Each of those is a separate audited guardrail
/// (`.oh/guardrails/build-adapters-*.md`).
pub trait BuildAdapter: Send + Sync {
    /// Roles this adapter can remove as an exact project-local path.
    /// Shared stores and native manager operations need separate contracts.
    fn trash_roles(&self) -> &'static [ArtifactRole] {
        &[]
    }
    /// Stable id, equal to this adapter's module name with `_` replaced
    /// by `-`, to its [`matrix`] row and to its docs table row.
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> BuildCapabilities {
        BuildCapabilities::default()
    }

    /// Which kinds of machine-wide store this adapter identifies the
    /// interior of. The external observation hands a measured store to
    /// the adapter claiming its declared kind; an adapter claiming none
    /// is only ever given project containers.
    fn store_kinds(&self) -> &'static [BuildStoreKind] {
        &[]
    }

    /// Which containers under `project_root` this adapter claims, given
    /// the artifact directories the walk found there.
    ///
    /// `candidates` are absolute paths of artifact rows beneath the
    /// checkout; the adapter decides which ones are its own, by name and
    /// by the marker files beside them. It never scans for more.
    fn containers(&self, project_root: &Path, candidates: &[PathBuf]) -> Vec<BuildContainer>;

    /// Identify the interior of one container.
    fn identify(&self, container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact>;
}

// ---------------------------------------------------------------------
// NestedUnitBuilder: the honest defaults are a constructor
// ---------------------------------------------------------------------

/// The only way an adapter builds a unit
/// (`.oh/guardrails/build-units-built-through-builder.md`).
///
/// A `NestedArtifact { .. }` literal has to spell out coverage,
/// membership, the accounting basis, the timestamp source, the physical
/// charge and the action capability -- and every one of those has a
/// wrong value that *overstates* what swamp knows: supported coverage
/// for an untested layout, `Exclusive` membership on a hardlinked store
/// entry, a physical charge on an aggregate, an action capability no
/// executor backs. The constructor picks the understating value for each
/// one; lifting it is a named method, visible in the diff.
pub struct NestedUnitBuilder {
    unit: NestedArtifact,
}

impl NestedUnitBuilder {
    /// A new unit inside `container`, at `path`, in `role`.
    ///
    /// Defaults: coverage unsupported and incomplete, membership
    /// unknown, accounting basis unknown, time source unknown, zero
    /// physical charge, [`NestedActionCapability::InspectionOnly`].
    pub fn new(container: &BuildContainer, role: ArtifactRole, path: PathBuf) -> Self {
        let scope = container.scope();
        let rel = relative_path(&container.path, &path);
        let id = NestedArtifact::within(&scope, &rel);
        let parent_id = (path != container.path)
            .then(|| {
                path.parent()
                    .map(|p| NestedArtifact::within(&scope, &relative_path(&container.path, p)))
            })
            .flatten();
        Self {
            unit: NestedArtifact {
                id,
                path,
                relative_path: rel,
                parent_id,
                container_id: Some(scope),
                role,
                membership: Membership::Unknown,
                is_dir: false,
                device: 0,
                inode: 0,
                logical_bytes: 0,
                bytes: 0,
                physical_bytes: 0,
                physical_total: 0,
                mtime_max: 0,
                variant: ArtifactVariant::default(),
                producer_evidence: Vec::new(),
                consumer_evidence: Vec::new(),
                coverage: ArtifactCoverage {
                    supported: false,
                    complete: false,
                    limits: Vec::new(),
                },
                action_group: None,
                present: true,
                growth_bytes: None,
                regrowth_count: 0,
                decision_evidence: Vec::new(),
                adapter: Some(container.adapter_id.to_string()),
                basis: AccountingBasis::Unknown,
                time_source: TimeSource::Unknown,
                action: NestedActionCapability::InspectionOnly,
                consequence: None,
                reported_by: None,
                writer_lock: None,
                guidance: crate::cargo_cleanup::Guidance::default(),
            },
        }
    }

    /// A directory the folded walk measured, identified: `role`, the
    /// layout claim that makes it reviewable, the tool that produced it,
    /// and what removing it costs. The shape nearly every adapter unit
    /// has, in one constructor so the claim and the measurement are
    /// never separated.
    pub fn known_dir(
        container: &BuildContainer,
        role: ArtifactRole,
        dir: &FoldedDir,
        reason: impl Into<String>,
        consequence: impl Into<String>,
    ) -> Self {
        let reason = reason.into();
        let source = format!("{}-layout", container.adapter_id);
        Self::new(container, role, dir.path.clone())
            .folded(dir)
            .supported_with_reason(reason.clone())
            .evidence(&source, reason, Confidence::Medium)
            .consequence(consequence)
    }

    /// A directory the folded walk measured and this adapter does not
    /// identify: a residual, named, never dropped.
    pub fn unknown_dir(
        container: &BuildContainer,
        dir: &FoldedDir,
        limit: impl Into<String>,
    ) -> Self {
        Self::new(container, ArtifactRole::Residual, dir.path.clone())
            .folded(dir)
            .unsupported_layout(limit)
    }

    /// The container's own row, measured from the folded rows when the
    /// walk measured it, and saying so when it did not.
    pub fn container_root(container: &BuildContainer, ctx: &BuildCtx, role: ArtifactRole) -> Self {
        let b = Self::new(container, role, container.path.clone()).is_dir(true);
        match ctx.folded().get(&container.path) {
            Some(d) => b.folded(d),
            None => b.limit("this directory was not measured by the walk this pass"),
        }
    }

    /// Declares this unit's layout tested and understood, with the
    /// reason that makes the claim reviewable. An empty reason is
    /// rejected by `build_units_built_through_builder`.
    pub fn supported_with_reason(mut self, reason: impl Into<String>) -> Self {
        let reason = reason.into();
        debug_assert!(!reason.trim().is_empty(), "a support claim needs a reason");
        self.unit.coverage.supported = true;
        self.unit.producer_evidence.push(ArtifactEvidence {
            source: format!("{}-layout", self.unit.adapter.clone().unwrap_or_default()),
            detail: reason,
            confidence: Confidence::Medium,
        });
        self
    }

    /// Marks the layout as one this adapter does not understand. The
    /// unit is still emitted -- an unsupported layout is a named
    /// limitation, never an empty result.
    pub fn unsupported_layout(mut self, limit: impl Into<String>) -> Self {
        self.unit.coverage.supported = false;
        self.unit.coverage.limits.push(limit.into());
        self.unit.action = NestedActionCapability::Unsupported {
            reason: "the layout is not one this adapter identifies".into(),
        };
        self
    }

    pub fn complete(mut self, complete: bool) -> Self {
        self.unit.coverage.complete = complete;
        self
    }

    pub fn limit(mut self, limit: impl Into<String>) -> Self {
        self.unit.coverage.limits.push(limit.into());
        self
    }

    pub fn is_dir(mut self, is_dir: bool) -> Self {
        self.unit.is_dir = is_dir;
        self
    }

    /// Allocated bytes from a folded directory measurement, with the
    /// basis and the timestamp source recorded together -- the two facts
    /// that make the number interpretable.
    pub fn folded(mut self, dir: &FoldedDir) -> Self {
        self.unit.bytes = dir.allocated_total;
        self.unit.basis = AccountingBasis::Allocated;
        self.unit.mtime_max = dir.mtime_max;
        self.unit.time_source = TimeSource::FoldedDirectoryModification;
        self.unit.is_dir = true;
        self.unit.coverage.complete = dir.complete;
        if !dir.complete {
            self.unit
                .coverage
                .limits
                .push("the walk could not read all of this directory".into());
        }
        self
    }

    /// One file's own metadata.
    pub fn from_file_metadata(mut self, meta: &crate::fs_gate::Metadata) -> Self {
        use crate::fs_gate::MetadataExt;
        self.unit.bytes = meta.blocks() * 512;
        self.unit.basis = AccountingBasis::Allocated;
        self.unit.logical_bytes = meta.len();
        self.unit.mtime_max = meta.mtime().max(0) as u64;
        self.unit.time_source = TimeSource::FileModification;
        self.unit.device = meta.dev();
        self.unit.inode = meta.ino();
        self.unit.is_dir = meta.is_dir();
        self.unit.membership = if meta.nlink() > 1 && meta.is_file() {
            Membership::SharedHardlink
        } else {
            Membership::Exclusive
        };
        self.unit.coverage.complete = true;
        self
    }

    /// Charges this file's allocated bytes to this unit alone, on the
    /// unique-allocated basis. Only for a file with exactly one link:
    /// a hardlinked member is charged to exactly one unit, and nothing
    /// here can tell which, so charging it would inflate the container
    /// by however many links exist. Called after
    /// [`Self::from_file_metadata`]; a no-op otherwise, because an
    /// unmeasured unit has nothing honest to charge.
    pub fn charged_uniquely(mut self) -> Self {
        if self.unit.membership == Membership::Exclusive && !self.unit.is_dir {
            self.unit.physical_total = self.unit.bytes;
            self.unit.physical_bytes = self.unit.bytes;
            self.unit.basis = AccountingBasis::UniqueAllocated;
        }
        self
    }

    pub fn bytes_on_basis(mut self, bytes: u64, basis: AccountingBasis) -> Self {
        self.unit.bytes = bytes;
        self.unit.basis = basis;
        self
    }

    pub fn modified(mut self, mtime: u64, source: TimeSource) -> Self {
        self.unit.mtime_max = mtime;
        self.unit.time_source = source;
        self
    }

    pub fn membership(mut self, m: Membership) -> Self {
        self.unit.membership = m;
        self
    }

    pub fn variant(mut self, v: ArtifactVariant) -> Self {
        self.unit.variant = v;
        self
    }

    pub fn evidence(mut self, source: &str, detail: impl Into<String>, c: Confidence) -> Self {
        self.unit.producer_evidence.push(ArtifactEvidence {
            source: source.into(),
            detail: detail.into(),
            confidence: c,
        });
        self
    }

    /// What it would cost to get these bytes back, in the ecosystem's
    /// own words. A consequence, never a verdict.
    pub fn consequence(mut self, text: impl Into<String>) -> Self {
        self.unit.consequence = Some(text.into());
        self
    }

    /// Declares that no action is available for this unit and why --
    /// for a shared store entry other projects link to, or a unit whose
    /// measurement is incomplete.
    pub fn no_action_because(mut self, reason: impl Into<String>) -> Self {
        self.unit.action = NestedActionCapability::Unsupported {
            reason: reason.into(),
        };
        self
    }

    /// The exact filesystem members that would have to be considered
    /// together. Naming the group does not make it actionable; the
    /// action layer still authorizes and re-checks it (#64: units are
    /// distinct from action groups).
    pub fn action_group(mut self, group: impl Into<String>) -> Self {
        self.unit.action_group = Some(NestedArtifact::action_group(&group.into()));
        self
    }

    /// Re-opens a unit an adapter already built, so a later enrichment
    /// pass (Cargo's fingerprints, a budget limit counted after the
    /// members) goes through the same named methods as construction.
    ///
    /// The alternative -- assigning `u.role = ..` or pushing onto
    /// `u.coverage.limits` after `build()` -- is the builder bypass
    /// re-review 3 found on the agent side (`u.protected = false`), and
    /// `build_units_built_through_builder` rejects it. Every change made
    /// here is a method call visible in the diff.
    pub fn amend(unit: NestedArtifact) -> Self {
        Self { unit }
    }

    /// Replaces the role. For an enrichment that has *read* evidence the
    /// original classification did not (a fingerprint naming a test
    /// target); a role is identity-neutral, so this never changes the
    /// unit's id or history key.
    pub fn role(mut self, role: ArtifactRole) -> Self {
        self.unit.role = role;
        self
    }

    /// Raises the recorded modification time to at least `mtime`, from
    /// the same source. For a folded group whose deeper directories were
    /// measured separately: the group's last change includes theirs.
    pub fn modified_at_least(mut self, mtime: u64) -> Self {
        self.unit.mtime_max = self.unit.mtime_max.max(mtime);
        self
    }

    /// Narrows completeness: a unit is complete only if every measured
    /// part of it was. There is deliberately no method that widens it.
    pub fn complete_only_if(mut self, complete: bool) -> Self {
        self.unit.coverage.complete &= complete;
        self
    }

    /// Bytes and a time a manager reported rather than swamp measured:
    /// the daemon's logical size, on the logical basis, and the record's
    /// own creation time as a tool-recorded fact. The two things that
    /// make a reported number interpretable travel with it, and nothing
    /// here can be mistaken for a filesystem measurement.
    pub fn reported_by_manager(
        mut self,
        manager: impl Into<String>,
        logical_bytes: u64,
        recorded_at: Option<u64>,
    ) -> Self {
        self.unit.reported_by = Some(manager.into());
        self.unit.bytes = logical_bytes;
        self.unit.logical_bytes = logical_bytes;
        self.unit.basis = AccountingBasis::Logical;
        match recorded_at {
            Some(t) => {
                self.unit.mtime_max = t;
                self.unit.time_source = TimeSource::ToolRecorded;
            }
            None => {
                self.unit.mtime_max = 0;
                self.unit.time_source = TimeSource::Unknown;
            }
        }
        self.unit.coverage.complete = true;
        self
    }

    /// Records a writer's lock file found present this pass, and
    /// withholds any action while it is there.
    pub fn writer_lock(mut self, lock: PathBuf, tool: &str) -> Self {
        self.unit.coverage.limits.push(format!(
            "`{}` is present: {tool} may be writing here now (a crashed build also leaves one)",
            lock.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        ));
        self.unit.writer_lock = Some(lock);
        self.unit.action = NestedActionCapability::Unsupported {
            reason: format!("{tool} may be writing here now"),
        };
        self
    }

    /// This pass's history for the unit, from the current + reverse-delta
    /// store. History is annotation, never identity.
    pub fn history(mut self, growth_bytes: Option<i64>, regrowth_count: u32) -> Self {
        self.unit.growth_bytes = growth_bytes;
        self.unit.regrowth_count = regrowth_count;
        self
    }

    /// An action group by its already-computed storage id.
    pub fn action_group_id(mut self, id: String) -> Self {
        self.unit.action_group = Some(id);
        self
    }

    pub fn build(self) -> NestedArtifact {
        self.unit
    }

    pub fn trash_path(mut self) -> Self {
        self.unit.action = NestedActionCapability::TrashPath;
        self
    }
}

// ---------------------------------------------------------------------
// Aggregation (#64's collapsed category rows)
// ---------------------------------------------------------------------

/// A collapsed family row: what a container holds in one role family,
/// on one accounting basis, with one oldest *modification* time.
///
/// Counted over **nonempty supported candidates** only (#64/#65): a unit
/// whose layout the adapter does not understand is not a candidate of
/// any family -- it is reported once, in
/// [`ContainerSummary::unsupported_bytes`] -- and a unit with nothing in
/// it is not a candidate of anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FamilySummary {
    pub family: RoleFamily,
    pub count: usize,
    pub bytes: u64,
    pub basis: AccountingBasis,
    /// The oldest known modification among this family's units, or
    /// `None` when no unit in it has a known time. A category's oldest
    /// candidate is not its last use.
    pub oldest_modified: Option<u64>,
    /// Units whose modification time is unknown. Reported rather than
    /// folded into the oldest, so "unknown ages last" is a fact the
    /// caller has, not an ordering trick.
    pub unknown_age: usize,
    /// Whether every unit in this family had complete coverage.
    pub complete: bool,
    /// What removing this family's bytes costs, in the adapter's own
    /// words: the largest member's consequence.
    pub consequence: Option<String>,
    /// How many *other* distinct consequences the family's members
    /// state. Non-zero means the one above is not the whole story, and a
    /// view says so rather than presenting one member's cost as the
    /// family's.
    pub other_consequences: usize,
    /// Review guidance derived from the family alone (see
    /// [`family_guidance`]). Guidance, never a verdict.
    pub recommendation: &'static str,
}

/// One container's interior, collapsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSummary {
    /// One row per family with at least one nonempty supported
    /// candidate, in [`RoleFamily::ALL`] order.
    pub families: Vec<FamilySummary>,
    /// Outermost units the adapter emitted but could not identify
    /// (unsupported layout), with their bytes when they share one basis.
    pub unsupported_count: usize,
    pub unsupported_bytes: Option<u64>,
    /// Bytes of the container that **no** outermost unit accounts for:
    /// the container's own loose files and whatever no unit claims
    /// (#65: "retain explicit unclassified residuals and reconcile leaf
    /// totals to container totals"). `None` when the container was not
    /// measured, when the members are on a different basis from it, or
    /// when the members add up to *more* than the container -- which
    /// can only mean a double count upstream, and is not papered over
    /// with a zero.
    pub unaccounted_bytes: Option<u64>,
    /// Outermost units with nothing in them. Not candidates; counted so
    /// "absent" and "empty" stay different facts.
    pub empty: usize,
}

/// Review guidance for a family, from the family alone.
///
/// Each phrase is at most 32 characters: it is what the 80-column
/// frame's advice column shows whole, and the count and age that follow
/// are what a narrow terminal gives up first.
///
/// #64's contract: age + size + removal consequence is enough to suggest
/// a review, and advice is derived from facts rather than persisted as a
/// verdict. This is the family half of that; the view adds the age and
/// the size. Nothing here says a family is unused or removable.
pub fn family_guidance(family: RoleFamily) -> &'static str {
    match family {
        RoleFamily::Intermediates => "Start here: slower next build",
        RoleFamily::Outputs => "Review: a build regenerates",
        RoleFamily::Tests => "Review: a test run regenerates",
        RoleFamily::Dependencies => "Review: reinstall from registry",
        RoleFamily::SharedStore => "Shared: other projects may link",
        RoleFamily::Metadata => "Lower priority: tool bookkeeping",
        RoleFamily::Installations => "Review: reinstall is a download",
        RoleFamily::State => "Keep in mind: may be unique",
        RoleFamily::Container => "Container: see groups inside",
        RoleFamily::Residual | RoleFamily::Unknown => "Inspect: not identified",
    }
}

/// The units a family row stands for, outermost only, in the order a
/// person should review them: oldest known modification first, unknown
/// ages **last** (never ranked as ancient), then larger first.
pub fn family_members<'a>(
    container: &Path,
    units: &'a [NestedArtifact],
    family: RoleFamily,
) -> Vec<&'a NestedArtifact> {
    let mut members: Vec<&NestedArtifact> = outermost(container, units)
        .into_iter()
        .filter(|u| is_candidate(u) && u.role.family() == family)
        .collect();
    members.sort_by(|a, b| {
        known_time(a)
            .is_none()
            .cmp(&known_time(b).is_none())
            .then_with(|| known_time(a).cmp(&known_time(b)))
            .then_with(|| b.bytes.cmp(&a.bytes))
            .then_with(|| a.path.cmp(&b.path))
    });
    members
}

fn known_time(u: &NestedArtifact) -> Option<u64> {
    (u.time_source != TimeSource::Unknown && u.mtime_max > 0).then_some(u.mtime_max)
}

/// A nonempty supported candidate.
fn is_candidate(u: &NestedArtifact) -> bool {
    u.coverage.supported && u.bytes > 0 && u.basis != AccountingBasis::Unknown
}

/// The units inside `container` that no other *directory* unit in the
/// list contains: the ones whose bytes are not already inside another
/// unit's total.
fn outermost<'a>(container: &Path, units: &'a [NestedArtifact]) -> Vec<&'a NestedArtifact> {
    let inside: Vec<&NestedArtifact> = units
        .iter()
        .filter(|u| u.present)
        .filter(|u| u.path != container && u.path.starts_with(container))
        .filter(|u| u.role != ArtifactRole::Container)
        .collect();
    let dirs: std::collections::HashSet<&Path> = inside
        .iter()
        .filter(|u| u.is_dir)
        .map(|u| u.path.as_path())
        .collect();
    inside
        .into_iter()
        .filter(|u| {
            !u.path
                .ancestors()
                .skip(1)
                .take_while(|a| *a != container)
                .any(|a| dirs.contains(a))
        })
        .collect()
}

/// Collapses the units *inside* `container` into one row per role
/// family. See [`summarize_container`]; this is its family rows.
pub fn summarize_families(container: &Path, units: &[NestedArtifact]) -> Vec<FamilySummary> {
    summarize_container(container, units).families
}

/// Collapses the units *inside* `container`.
///
/// `container` is taken explicitly and excluded, rather than inferred.
/// A container's own row usually does not carry the `Container` role --
/// a `node_modules` is `InstalledDependencies`, a Gradle `build/` is
/// `Output` -- so inferring it from the role meant the container looked
/// like an ordinary unit, every real member looked like its descendant,
/// and the summary collapsed to one row holding the container itself.
///
/// The rules, all from #64/#65:
///
/// * **only nonempty supported candidates are counted.** An unsupported
///   unit is reported once, as unidentified; an empty one is counted as
///   empty. Neither inflates a family.
/// * **stop at the outermost included unit.** A unit whose ancestor is
///   also in the list is not counted again -- otherwise a `dist/` and
///   the `dist/assets/` inside it would both be charged and the family
///   total would exceed the container.
/// * **never mix bases.** Units are summed only with others on the same
///   accounting basis; a family holding both allocated and logical
///   numbers reports [`AccountingBasis::Unknown`] and no total, because
///   a mixed number is worse than no number.
/// * **reconcile to the container.** What the outermost units do not
///   account for is reported as unaccounted bytes, never dropped.
pub fn summarize_container(container: &Path, units: &[NestedArtifact]) -> ContainerSummary {
    let outer = outermost(container, units);
    let mut by_family: HashMap<RoleFamily, Vec<&NestedArtifact>> = HashMap::new();
    let mut unsupported: Vec<&NestedArtifact> = Vec::new();
    let mut empty = 0usize;
    for u in outer.iter().copied() {
        if !u.coverage.supported {
            unsupported.push(u);
        } else if u.bytes == 0 || u.basis == AccountingBasis::Unknown {
            empty += 1;
        } else {
            by_family.entry(u.role.family()).or_default().push(u);
        }
    }
    let one_basis = |members: &[&NestedArtifact]| -> Option<(u64, AccountingBasis)> {
        let first = members.first()?;
        members
            .iter()
            .all(|u| u.basis == first.basis)
            .then(|| (members.iter().map(|u| u.bytes).sum(), first.basis))
    };
    let mut families = Vec::new();
    for family in RoleFamily::ALL {
        let Some(members) = by_family.get(family) else {
            continue;
        };
        let (bytes, basis) = one_basis(members).unwrap_or((0, AccountingBasis::Unknown));
        let known: Vec<u64> = members.iter().filter_map(|u| known_time(u)).collect();
        let mut by_size: Vec<&&NestedArtifact> = members.iter().collect();
        by_size.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
        let consequence = by_size.iter().find_map(|u| u.consequence.clone());
        let distinct: std::collections::BTreeSet<&str> = members
            .iter()
            .filter_map(|u| u.consequence.as_deref())
            .collect();
        families.push(FamilySummary {
            family: *family,
            count: members.len(),
            bytes,
            basis,
            oldest_modified: known.iter().copied().min(),
            unknown_age: members.len() - known.len(),
            complete: members.iter().all(|u| u.coverage.complete),
            other_consequences: distinct.len().saturating_sub(1),
            consequence,
            recommendation: family_guidance(*family),
        });
    }
    // Reconciliation against the container's own measured row.
    let root = units.iter().find(|u| u.path == container);
    let measured: Vec<&NestedArtifact> = outer
        .iter()
        .copied()
        .filter(|u| u.basis != AccountingBasis::Unknown)
        .collect();
    let unaccounted_bytes = root
        .filter(|r| r.basis != AccountingBasis::Unknown && r.bytes > 0)
        .and_then(|r| {
            let compatible = measured.iter().all(|u| u.basis == r.basis);
            let sum: u64 = measured.iter().map(|u| u.bytes).sum();
            (compatible && sum <= r.bytes).then(|| r.bytes - sum)
        });
    ContainerSummary {
        families,
        unsupported_count: unsupported.len(),
        unsupported_bytes: if unsupported.is_empty() {
            Some(0)
        } else {
            one_basis(&unsupported).map(|(b, _)| b)
        },
        unaccounted_bytes,
        empty,
    }
}

/// The entry point the report pipeline uses: every registered adapter,
/// over every container it claims under the given projects.
///
/// One pass, one ownership, no per-adapter traversal. Ordering between
/// adapters is fixed by the registry, so two adapters that both claim a
/// `build/` directory (a Gradle project that is also a Node workspace)
/// resolve the same way every pass.
pub fn identify_all(
    registry: &registry::Registry,
    projects: &[(PathBuf, Vec<PathBuf>)],
    shared: &[BuildContainer],
    ctx: &BuildCtx,
) -> Vec<NestedArtifact> {
    let mut out = Vec::new();
    let mut claimed: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for adapter in registry.adapters() {
        for (root, candidates) in projects {
            for container in adapter.containers(root, candidates) {
                if !claimed.insert(container.path.clone()) {
                    continue;
                }
                let units = ctx.container(&container, &|| adapter.identify(&container, ctx));
                out.extend(units.into_iter().map(|unit| {
                    let selected_role = adapter.trash_roles().contains(&unit.role);
                    let within_project = container
                        .project_root
                        .as_ref()
                        .is_some_and(|root| unit.path != *root && unit.path.starts_with(root));
                    if selected_role
                        && within_project
                        && !container.shared
                        && unit.coverage.supported
                        && unit.consequence.is_some()
                        && matches!(unit.action, NestedActionCapability::InspectionOnly)
                    {
                        NestedUnitBuilder::amend(unit).trash_path().build()
                    } else {
                        unit
                    }
                }));
            }
        }
        for container in shared.iter().filter(|c| c.adapter_id == adapter.id()) {
            if !claimed.insert(container.path.clone()) {
                continue;
            }
            out.extend(ctx.container(container, &|| adapter.identify(container, ctx)));
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn container(path: &Path) -> BuildContainer {
        BuildContainer::project("cargo", path.to_path_buf(), path.to_path_buf())
    }

    #[test]
    fn builder_defaults_understate_rather_than_overstate() {
        let tmp = tempfile::tempdir().unwrap();
        let c = container(tmp.path());
        let u = NestedUnitBuilder::new(&c, ArtifactRole::Output, tmp.path().join("dist")).build();
        assert!(!u.coverage.supported, "support is claimed, never assumed");
        assert!(!u.coverage.complete);
        assert_eq!(u.basis, AccountingBasis::Unknown);
        assert_eq!(u.time_source, TimeSource::Unknown);
        assert_eq!(u.membership, Membership::Unknown);
        assert_eq!(u.physical_total, 0, "an aggregate carries no charge");
        assert_eq!(u.action, NestedActionCapability::InspectionOnly);
    }

    #[test]
    fn an_unchanged_container_is_replayed_only_under_trusted_coverage() {
        let tmp = tempfile::tempdir().unwrap();
        let c = container(tmp.path());
        let folded = FoldedIndex::default();
        let prior =
            vec![NestedUnitBuilder::new(&c, ArtifactRole::Output, tmp.path().join("dist")).build()];
        let cache = ContainerCache::from_previous(prior, 100);

        let untrusted = EventCoverage::untrusted();
        let ctx = BuildCtx::new(200, &folded, &untrusted, &cache);
        assert!(
            !ctx.can_reuse(&c),
            "no window, no reuse -- a stamp is not a substitute"
        );

        let trusted = EventCoverage::trusted(tmp.path().to_path_buf(), Vec::new(), 100);
        let ctx = BuildCtx::new(200, &folded, &trusted, &cache);
        assert!(ctx.can_reuse(&c));

        let noisy = EventCoverage::trusted(
            tmp.path().to_path_buf(),
            vec![tmp.path().join("dist/app.js")],
            100,
        );
        let ctx = BuildCtx::new(200, &folded, &noisy, &cache);
        assert!(
            !ctx.can_reuse(&c),
            "an event *inside* the container is a change to it"
        );
    }

    fn supported(c: &BuildContainer, role: ArtifactRole, path: PathBuf) -> NestedUnitBuilder {
        NestedUnitBuilder::new(c, role, path).supported_with_reason("fixture layout")
    }

    #[test]
    fn rows_written_by_another_adapter_are_never_replayed() {
        let tmp = tempfile::tempdir().unwrap();
        let node = BuildContainer::project("node", tmp.path().join("build"), tmp.path().into());
        let gradle = BuildContainer::project("gradle", tmp.path().join("build"), tmp.path().into());
        let prior = vec![
            NestedUnitBuilder::new(&node, ArtifactRole::Output, tmp.path().join("build")).build(),
        ];
        let cache = ContainerCache::from_previous(prior, 100);
        let folded = FoldedIndex::default();
        let trusted = EventCoverage::trusted(tmp.path().to_path_buf(), Vec::new(), 100);
        let ctx = BuildCtx::new(200, &folded, &trusted, &cache);
        assert!(ctx.can_reuse(&node));
        assert!(
            !ctx.can_reuse(&gradle),
            "the claim moved to another adapter through a marker outside the container; no event \
             under the container can say so, so the old adapter's rows are not evidence"
        );
    }

    #[test]
    fn a_family_summary_stops_at_the_outermost_unit() {
        let tmp = tempfile::tempdir().unwrap();
        let c = container(tmp.path());
        let outer = supported(&c, ArtifactRole::Output, tmp.path().join("dist"))
            .is_dir(true)
            .bytes_on_basis(1000, AccountingBasis::Allocated)
            .build();
        let inner = supported(&c, ArtifactRole::Output, tmp.path().join("dist/assets"))
            .is_dir(true)
            .bytes_on_basis(400, AccountingBasis::Allocated)
            .build();
        let s = summarize_families(tmp.path(), &[outer, inner]);
        let outputs = s.iter().find(|f| f.family == RoleFamily::Outputs).unwrap();
        assert_eq!(outputs.count, 1);
        assert_eq!(
            outputs.bytes, 1000,
            "a descendant's bytes are already inside its ancestor's total"
        );
    }

    #[test]
    fn a_family_mixing_accounting_bases_reports_no_total() {
        let tmp = tempfile::tempdir().unwrap();
        let c = container(tmp.path());
        let a = supported(&c, ArtifactRole::Output, tmp.path().join("a"))
            .bytes_on_basis(100, AccountingBasis::Allocated)
            .build();
        let b = supported(&c, ArtifactRole::Output, tmp.path().join("b"))
            .bytes_on_basis(100, AccountingBasis::Logical)
            .build();
        let s = summarize_families(tmp.path(), &[a, b]);
        let outputs = s.iter().find(|f| f.family == RoleFamily::Outputs).unwrap();
        assert_eq!(outputs.basis, AccountingBasis::Unknown);
        assert_eq!(
            outputs.bytes, 0,
            "a mixed-basis number is worse than no number"
        );
    }

    #[test]
    fn unknown_ages_are_counted_not_folded_into_the_oldest() {
        let tmp = tempfile::tempdir().unwrap();
        let c = container(tmp.path());
        let dated = supported(&c, ArtifactRole::Output, tmp.path().join("a"))
            .bytes_on_basis(1, AccountingBasis::Allocated)
            .modified(5_000, TimeSource::FoldedDirectoryModification)
            .build();
        let undated = supported(&c, ArtifactRole::Output, tmp.path().join("b"))
            .bytes_on_basis(1, AccountingBasis::Allocated)
            .build();
        let units = [dated, undated];
        let s = summarize_families(tmp.path(), &units);
        let outputs = s.iter().find(|f| f.family == RoleFamily::Outputs).unwrap();
        assert_eq!(outputs.oldest_modified, Some(5_000));
        assert_eq!(
            outputs.unknown_age, 1,
            "an unknown age is reported, never treated as epoch and ranked ancient"
        );
        let order: Vec<&Path> = family_members(tmp.path(), &units, RoleFamily::Outputs)
            .iter()
            .map(|u| u.path.as_path())
            .collect();
        assert_eq!(
            order,
            vec![
                tmp.path().join("a").as_path(),
                tmp.path().join("b").as_path()
            ],
            "the undated unit sorts after the dated one, not before it as if it were ancient"
        );
    }

    #[test]
    fn only_nonempty_supported_candidates_are_counted() {
        // The tempting summary counts every unit an adapter emitted. An
        // unsupported layout is not a candidate of any family (it is
        // unidentified), and an empty directory is not a candidate of
        // anything.
        let tmp = tempfile::tempdir().unwrap();
        let c = container(tmp.path());
        let real = supported(&c, ArtifactRole::Output, tmp.path().join("dist"))
            .is_dir(true)
            .bytes_on_basis(700, AccountingBasis::Allocated)
            .build();
        let empty = supported(&c, ArtifactRole::Output, tmp.path().join("out"))
            .is_dir(true)
            .bytes_on_basis(0, AccountingBasis::Allocated)
            .build();
        let unknown = NestedUnitBuilder::new(&c, ArtifactRole::Output, tmp.path().join("weird"))
            .is_dir(true)
            .bytes_on_basis(200, AccountingBasis::Allocated)
            .unsupported_layout("fixture: not a layout this adapter knows")
            .build();
        let s = summarize_container(tmp.path(), &[real, empty, unknown]);
        let outputs = s
            .families
            .iter()
            .find(|f| f.family == RoleFamily::Outputs)
            .unwrap();
        assert_eq!(outputs.count, 1, "{s:?}");
        assert_eq!(outputs.bytes, 700);
        assert_eq!(s.unsupported_count, 1);
        assert_eq!(s.unsupported_bytes, Some(200));
        assert_eq!(s.empty, 1);
    }

    #[test]
    fn leaf_totals_reconcile_to_the_container_through_an_explicit_residual() {
        let tmp = tempfile::tempdir().unwrap();
        let c = container(tmp.path());
        let root = supported(&c, ArtifactRole::Output, tmp.path().to_path_buf())
            .is_dir(true)
            .bytes_on_basis(1_000, AccountingBasis::Allocated)
            .build();
        let a = supported(&c, ArtifactRole::Output, tmp.path().join("a"))
            .is_dir(true)
            .bytes_on_basis(600, AccountingBasis::Allocated)
            .build();
        let a_inner = supported(&c, ArtifactRole::Intermediate, tmp.path().join("a/cache"))
            .is_dir(true)
            .bytes_on_basis(500, AccountingBasis::Allocated)
            .build();
        let b = NestedUnitBuilder::new(&c, ArtifactRole::Residual, tmp.path().join("b"))
            .is_dir(true)
            .bytes_on_basis(150, AccountingBasis::Allocated)
            .unsupported_layout("fixture")
            .build();
        let s = summarize_container(tmp.path(), &[root.clone(), a, a_inner, b]);
        assert_eq!(
            s.unaccounted_bytes,
            Some(250),
            "1000 in the container, 600 + 150 in its outermost units: 250 are loose files no unit \
             claims, and saying so is what makes the leaves reconcile"
        );
        let over = supported(&c, ArtifactRole::Output, tmp.path().join("big"))
            .is_dir(true)
            .bytes_on_basis(5_000, AccountingBasis::Allocated)
            .build();
        let s = summarize_container(tmp.path(), &[root, over]);
        assert_eq!(
            s.unaccounted_bytes, None,
            "members adding up to more than their container are a double count upstream, never a \
             residual of zero"
        );
    }

    #[test]
    fn a_family_with_several_consequences_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let c = container(tmp.path());
        let big = supported(&c, ArtifactRole::Intermediate, tmp.path().join(".turbo"))
            .bytes_on_basis(900, AccountingBasis::Allocated)
            .consequence("the next task run re-executes")
            .build();
        let small = supported(&c, ArtifactRole::Intermediate, tmp.path().join(".vite"))
            .bytes_on_basis(100, AccountingBasis::Allocated)
            .consequence("the next dev server start re-optimizes")
            .build();
        let s = summarize_families(tmp.path(), &[small, big]);
        let f = &s[0];
        assert_eq!(
            f.consequence.as_deref(),
            Some("the next task run re-executes"),
            "the largest member's consequence leads"
        );
        assert_eq!(
            f.other_consequences, 1,
            "and the family says it has another, rather than presenting one member's cost as all \
             of them"
        );
        assert_eq!(f.recommendation, family_guidance(RoleFamily::Intermediates));
    }

    #[test]
    fn family_guidance_fits_the_narrow_advice_column() {
        for f in RoleFamily::ALL.iter().chain([&RoleFamily::Container]) {
            assert!(
                family_guidance(*f).chars().count() <= 32,
                "{:?}: `{}` is truncated at 80 columns",
                f,
                family_guidance(*f)
            );
        }
    }

    #[test]
    fn amending_a_unit_keeps_its_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let c = container(tmp.path());
        let u = supported(&c, ArtifactRole::Dependency, tmp.path().join("deps/x"))
            .modified(100, TimeSource::FoldedDirectoryModification)
            .complete(true)
            .build();
        let id = u.id.clone();
        let amended = NestedUnitBuilder::amend(u)
            .role(ArtifactRole::TestExecutable)
            .modified_at_least(50)
            .complete_only_if(false)
            .build();
        assert_eq!(amended.id, id, "a role is not identity");
        assert_eq!(amended.mtime_max, 100, "at least, never earlier");
        assert!(!amended.coverage.complete);
        assert_eq!(amended.role, ArtifactRole::TestExecutable);
    }
}
