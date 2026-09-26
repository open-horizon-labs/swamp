//! Shared nested-artifact facts.
//!
//! An artifact row is an accounting boundary.  The units in this module are
//! identification facts inside that boundary; they are not, by themselves,
//! permission to remove anything.  In particular, ownership is deliberately
//! absent from an identity.  A later attribution pass may attach a unit to a
//! project, but changing that guess must not change the unit's identity or its
//! history.

use crate::entities::{Confidence, id_for};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArtifactRole {
    Container,
    Profile,
    Dependency,
    TestExecutable,
    Example,
    BuildScriptOutput,
    Incremental,
    FinalOutput,
    CompanionMetadata,
    Residual,
    Unknown,
    // ---------------------------------------------------------------
    // The ecosystem-neutral vocabulary (#64). The variants above came
    // out of Cargo and read like Cargo: a "profile" is a Cargo word, an
    // "example" is a Cargo target kind, "incremental" is a Cargo
    // directory. Nothing outside Cargo has any of them, and forcing
    // `node_modules` into `Dependency` (a *compiled* dependency artifact)
    // or `coverage/` into `FinalOutput` would be exactly the false
    // equivalence #64 forbids.
    //
    // So the generic roles are added beside the Cargo ones rather than
    // replacing them, and [`ArtifactRole::family`] maps both sets onto
    // the same small set of families the aggregation and the views
    // group by. Adding variants is also all the compatibility work
    // there is: a stored report deserializes by label, and a label
    // nothing recognizes was never written by this code.
    /// A generated build output: `dist/`, `build/`, `.next/`, a Gradle
    /// `build/libs` jar, a Maven `target/*.jar`.
    Output,
    /// Test or coverage output: `coverage/`, `.nyc_output/`,
    /// `playwright-report/`, Gradle `build/test-results`, Maven
    /// `target/surefire-reports`.
    TestOutput,
    /// A tool's own intermediate or cache directory, kept between builds
    /// to make the next one faster: `.next/cache`, `.turbo`,
    /// `node_modules/.cache/<tool>`, Gradle `build/tmp`, `.gradle/`.
    Intermediate,
    /// An installed dependency tree inside a project: `node_modules`,
    /// `vendor`, `bower_components`. Distinct from [`Self::Dependency`],
    /// which is a *compiled* artifact of one dependency.
    InstalledDependencies,
    /// One entry in a store shared across projects: a pnpm
    /// content-addressed object, an npm `_cacache` entry, a Gradle
    /// `modules-2` module, a Maven repository artifact. Charged once,
    /// wherever else it is linked from.
    SharedStoreEntry,
    /// Metadata a tool wrote about a build, as a first-class unit rather
    /// than a companion of one: a `tsconfig.tsbuildinfo`, a
    /// `maven-metadata-local.xml`, a `.gradle/` task-history file.
    Metadata,
    /// An installed toolchain, SDK, runtime or interpreter: an Android
    /// platform, a simulator runtime, a uv-managed Python, iOS
    /// DeviceSupport symbols. Never build output (#70: "Keep archives,
    /// SDKs, simulator/runtime installations and mutable device state
    /// distinct from rebuildable build output"): getting it back is a
    /// reinstall, usually a download, not a rebuild.
    Installation,
    /// A retained release artifact a person produced on purpose: an Xcode
    /// `.xcarchive`, whose dSYMs may be the only symbols for a shipped
    /// build. Not rebuildable from source in general -- a rebuild makes a
    /// *different* binary.
    Archive,
    /// Mutable state of a device or environment: a simulator's `data/`,
    /// an Android Virtual Device. Holds whatever was installed and
    /// written there; nothing regenerates it.
    DeviceState,
}

/// The small set of families every adapter's roles collapse into, for
/// the collapsed category rows (#64: "collapsed category rows summarize
/// nonempty supported candidates") and for anything that groups units
/// without knowing which ecosystem produced them.
///
/// A family is a *presentation and aggregation* grouping. It is not an
/// identity: the unit's identity is its path inside its container, and
/// reclassifying a unit from one family to another must never look like
/// growth (`reclassification_never_fabricates_growth`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RoleFamily {
    Container,
    Outputs,
    Tests,
    Intermediates,
    Dependencies,
    SharedStore,
    Metadata,
    /// Installed toolchains, SDKs and runtimes.
    Installations,
    /// Retained archives and mutable device state: bytes nothing
    /// regenerates.
    State,
    Residual,
    Unknown,
}

impl RoleFamily {
    pub fn label(self) -> &'static str {
        match self {
            Self::Container => "container",
            Self::Outputs => "outputs",
            Self::Tests => "tests",
            Self::Intermediates => "intermediates",
            Self::Dependencies => "dependencies",
            Self::SharedStore => "shared-store",
            Self::Metadata => "metadata",
            Self::Installations => "installations",
            Self::State => "state",
            Self::Residual => "residual",
            Self::Unknown => "unknown",
        }
    }

    /// The heading a view shows for a collapsed family row.
    pub fn title(self) -> &'static str {
        match self {
            Self::Container => "Containers",
            Self::Outputs => "Build outputs",
            Self::Tests => "Test & coverage output",
            Self::Intermediates => "Caches & intermediates",
            Self::Dependencies => "Installed dependencies",
            Self::SharedStore => "Shared store entries",
            Self::Metadata => "Tool metadata",
            Self::Installations => "Installed SDKs & runtimes",
            Self::State => "Archives & device state",
            Self::Residual => "Not identified",
            Self::Unknown => "Unknown role",
        }
    }

    /// Every family, in the order the views show them: what a build
    /// produced, then what it produced for tests, then what it kept to
    /// go faster, then what it downloaded, then what is shared, then the
    /// bookkeeping, then what is not understood. Unknown last, because
    /// an unknown family is not a small one -- it is an unmeasured one.
    pub const ALL: &'static [Self] = &[
        Self::Outputs,
        Self::Tests,
        Self::Intermediates,
        Self::Dependencies,
        Self::SharedStore,
        Self::Installations,
        Self::State,
        Self::Metadata,
        Self::Residual,
        Self::Unknown,
    ];
}

impl ArtifactRole {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Container => "container",
            Self::Profile => "profile",
            Self::Dependency => "dependency",
            Self::TestExecutable => "test-executable",
            Self::Example => "example",
            Self::BuildScriptOutput => "build-script-output",
            Self::Incremental => "incremental",
            Self::FinalOutput => "final-output",
            Self::CompanionMetadata => "companion-metadata",
            Self::Residual => "residual",
            Self::Unknown => "unknown",
            Self::Output => "output",
            Self::TestOutput => "test-output",
            Self::Intermediate => "intermediate",
            Self::InstalledDependencies => "installed-dependencies",
            Self::SharedStoreEntry => "shared-store-entry",
            Self::Metadata => "metadata",
            Self::Installation => "installation",
            Self::Archive => "archive",
            Self::DeviceState => "device-state",
        }
    }

    /// The inverse of [`Self::label`]. Exhaustive by construction, so a
    /// new variant is a compile error here rather than a value that
    /// round-trips into `Unknown`.
    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL.iter().find(|r| r.label() == label).cloned()
    }

    pub const ALL: &'static [Self] = &[
        Self::Container,
        Self::Profile,
        Self::Dependency,
        Self::TestExecutable,
        Self::Example,
        Self::BuildScriptOutput,
        Self::Incremental,
        Self::FinalOutput,
        Self::CompanionMetadata,
        Self::Residual,
        Self::Unknown,
        Self::Output,
        Self::TestOutput,
        Self::Intermediate,
        Self::InstalledDependencies,
        Self::SharedStoreEntry,
        Self::Metadata,
        Self::Installation,
        Self::Archive,
        Self::DeviceState,
    ];

    /// Which family this role aggregates into.
    ///
    /// The Cargo roles map onto the same families as the generic ones,
    /// which is the whole point: a `--view builds` grouping must not
    /// have a Rust column and an everything-else column.
    /// `Profile` maps to `Container` because a profile directory is an
    /// intermediate *container* of other units -- counting it as an
    /// output would double-count everything under it.
    pub fn family(&self) -> RoleFamily {
        match self {
            Self::Container | Self::Profile => RoleFamily::Container,
            Self::FinalOutput | Self::Example | Self::Output => RoleFamily::Outputs,
            Self::TestExecutable | Self::TestOutput => RoleFamily::Tests,
            Self::Incremental | Self::BuildScriptOutput | Self::Intermediate => {
                RoleFamily::Intermediates
            }
            Self::Dependency | Self::InstalledDependencies => RoleFamily::Dependencies,
            Self::SharedStoreEntry => RoleFamily::SharedStore,
            Self::CompanionMetadata | Self::Metadata => RoleFamily::Metadata,
            Self::Installation => RoleFamily::Installations,
            Self::Archive | Self::DeviceState => RoleFamily::State,
            Self::Residual => RoleFamily::Residual,
            Self::Unknown => RoleFamily::Unknown,
        }
    }
}

/// Which bytes a unit's `bytes` field counts. #65 forbids mixing these
/// in one total, so the basis travels with the number instead of being
/// remembered by whoever reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AccountingBasis {
    /// Disk blocks this path occupies (`blocks * 512`), summed over a
    /// folded directory. What every other swamp total uses.
    #[default]
    Allocated,
    /// Apparent size (`st_size`), which is larger than allocated for a
    /// sparse file and smaller for a small one.
    Logical,
    /// Allocated bytes with hardlinked members charged to exactly one
    /// unit -- so two units' `bytes` can be added without inflating.
    UniqueAllocated,
    /// The size could not be established for this unit at all. Not zero:
    /// a zero that means "unknown" is the bug this variant exists to
    /// stop.
    Unknown,
}

impl AccountingBasis {
    pub fn label(self) -> &'static str {
        match self {
            Self::Allocated => "allocated",
            Self::Logical => "logical",
            Self::UniqueAllocated => "unique-allocated",
            Self::Unknown => "unknown",
        }
    }

    /// The inverse of [`Self::label`], for `nested_artifacts.parquet`
    /// (R16). Exhaustive by construction, like every other `from_label`
    /// in this module -- a new variant is a compile error here rather
    /// than a value that round-trips into a default.
    pub(crate) fn from_label(label: &str) -> Option<Self> {
        [
            Self::Allocated,
            Self::Logical,
            Self::UniqueAllocated,
            Self::Unknown,
        ]
        .into_iter()
        .find(|b| b.label() == label)
    }
}

/// Where a unit's `mtime_max` came from. #64's cleanup-guidance contract
/// requires the timestamp be labelled "modified", and requires the
/// *source* to be explicit -- a folded directory's rolled-up minute
/// resolution is a different fact from one file's `st_mtime`, and
/// neither is a last-use time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TimeSource {
    /// One path's own `st_mtime`.
    FileModification,
    /// The newest modification among a folded directory's measured
    /// entries, at the walk's minute resolution.
    FoldedDirectoryModification,
    /// A timestamp a tool itself recorded in metadata swamp read.
    ToolRecorded,
    /// No timestamp could be established. Ranks *last*, never as
    /// "ancient" (#64: "unknown/future times must not rank as ancient").
    #[default]
    Unknown,
}

impl TimeSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::FileModification => "file-modification",
            Self::FoldedDirectoryModification => "folded-directory-modification",
            Self::ToolRecorded => "tool-recorded",
            Self::Unknown => "unknown",
        }
    }

    /// The inverse of [`Self::label`]. Exhaustive by construction (R18a
    /// nested-artifact typed columns): an unrecognized label is
    /// [`Self::Unknown`], never a silently dropped fact, since
    /// `Unknown` is already this type's own "no timestamp could be
    /// established" state.
    pub fn from_label(label: &str) -> Self {
        match label {
            "file-modification" => Self::FileModification,
            "folded-directory-modification" => Self::FoldedDirectoryModification,
            "tool-recorded" => Self::ToolRecorded,
            _ => Self::Unknown,
        }
    }
}

/// What, if anything, swamp can do to a unit -- carried separately from
/// its identity (#64: "Identification works without a cleanup adapter").
///
/// Exact project-local paths can be selected for TUI Trash. Shared stores,
/// installations and unknown layouts retain explicit inspection-only limits.
/// Cargo's fingerprint-aware cleanup groups use their existing separate plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "capability", rename_all = "kebab-case")]
pub enum NestedActionCapability {
    /// Exact filesystem path moved to Trash by the human in the TUI.
    /// This never authorizes a manager-wide prune or a parent fallback.
    TrashPath,
    /// Identified and explainable; no action is offered.
    #[default]
    InspectionOnly,
    /// Identified, and an action exists but is not available for this
    /// unit, for the stated reason (a shared store entry other projects
    /// link to, an unsupported layout, an incomplete measurement).
    Unsupported { reason: String },
}

impl NestedActionCapability {
    pub fn label(&self) -> &'static str {
        match self {
            Self::TrashPath => "trash-path",
            Self::InspectionOnly => "inspection-only",
            Self::Unsupported { .. } => "unsupported",
        }
    }

    /// The inverse of [`Self::label`], taking the `Unsupported` reason
    /// separately since a column stores the tag and the reason in two
    /// cells (R18a nested-artifact typed columns). An unrecognized tag
    /// is [`Self::InspectionOnly`] ("no action offered"), the safer
    /// reading -- never a claimed capability nothing behind it granted.
    pub fn from_label(label: &str, reason: Option<String>) -> Self {
        match label {
            "trash-path" => Self::TrashPath,
            "unsupported" => Self::Unsupported {
                reason: reason.unwrap_or_default(),
            },
            _ => Self::InspectionOnly,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Membership {
    Exclusive,
    SharedHardlink,
    Residual,
    Unknown,
}

impl Membership {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Exclusive => "exclusive",
            Self::SharedHardlink => "shared-hardlink",
            Self::Residual => "residual",
            Self::Unknown => "unknown",
        }
    }

    /// The inverse of [`Self::label`]. Exhaustive by construction (R18a):
    /// an unrecognized label is [`Self::Unknown`], never a silently
    /// dropped fact, since `Unknown` is already this type's own
    /// "membership could not be established" state.
    pub fn from_label(label: &str) -> Self {
        match label {
            "exclusive" => Self::Exclusive,
            "shared-hardlink" => Self::SharedHardlink,
            "residual" => Self::Residual,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactEvidence {
    pub source: String,
    pub detail: String,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactVariant {
    pub package: Option<String>,
    /// The package's own declared version, where a manifest states one.
    /// Distinct from `generation`: a version is what a package calls
    /// itself, a generation would be a claim that one build supersedes
    /// another -- which no ecosystem here records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub target: Option<String>,
    pub profile: Option<String>,
    pub architecture: Option<String>,
    pub toolchain: Option<String>,
    pub features: Option<String>,
    pub configuration: Option<String>,
    pub generation: Option<String>,
    #[serde(default)]
    pub unknowns: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactCoverage {
    pub supported: bool,
    #[serde(default)]
    pub complete: bool,
    #[serde(default)]
    pub limits: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NestedArtifact {
    /// Stable identity based on canonical container and relative path, not
    /// role or an inferred project/package owner.
    pub id: String,
    pub path: PathBuf,
    pub relative_path: String,
    pub parent_id: Option<String>,
    pub container_id: Option<String>,
    pub role: ArtifactRole,
    pub membership: Membership,
    #[serde(default)]
    pub is_dir: bool,
    #[serde(default)]
    pub device: u64,
    #[serde(default)]
    pub inode: u64,
    #[serde(default)]
    pub logical_bytes: u64,
    /// Aggregate allocated bytes for this node. Container values include descendants;
    /// only leaf `physical_bytes` are charged to the containing report row.
    pub bytes: u64,
    /// Physical bytes charged by this node after hardlink de-duplication.
    /// This is zero for aggregate/container nodes.
    pub physical_bytes: u64,
    /// Aggregate charge, for display only; do not sum with descendants.
    /// Folded groups with Unknown membership have no subgroup charge estimate;
    /// their zero placeholder must be displayed as unknown, not measured zero.
    #[serde(default)]
    pub physical_total: u64,
    pub mtime_max: u64,
    pub variant: ArtifactVariant,
    #[serde(default)]
    pub producer_evidence: Vec<ArtifactEvidence>,
    #[serde(default)]
    pub consumer_evidence: Vec<ArtifactEvidence>,
    pub coverage: ArtifactCoverage,
    /// Exact filesystem members that would have to be considered together.
    /// This does not make the group actionable; the action layer must still
    /// authorize and re-check it.
    pub action_group: Option<String>,
    pub present: bool,
    #[serde(default)]
    pub growth_bytes: Option<i64>,
    #[serde(default)]
    pub regrowth_count: u32,
    /// Decision evidence (#53), distinct from `producer_evidence`/
    /// `consumer_evidence` above (the #64-#71 build-artifact-
    /// identification epic's own narrower evidence shape): activity/
    /// recovery/reclaimability facts using the shared `crate::evidence`
    /// contract, so a nested unit's decision evidence renders the same
    /// way as an artifact row's or external unit's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decision_evidence: Vec<crate::evidence::Evidence>,
    /// The adapter that identified this unit (`cargo`, `node`,
    /// `gradle`, `maven`), or `None` for a unit produced before the
    /// adapter registry existed.
    ///
    /// Carried so a view can say *which* support claim a row rests on,
    /// and so the capability matrix can be joined to real rows rather
    /// than asserted in prose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    /// Which bytes `bytes`/`physical_total` count. Never mix bases in
    /// one total (#65).
    #[serde(default)]
    pub basis: AccountingBasis,
    /// Where `mtime_max` came from. Always a *modification* fact, never
    /// a use or access fact.
    #[serde(default)]
    pub time_source: TimeSource,
    /// What swamp can do to this unit, separately from what it knows
    /// about it (#64).
    #[serde(default)]
    pub action: NestedActionCapability,
    /// What removing this unit would cost, in the ecosystem's own terms
    /// ("rebuild with `npm run build`"; "reinstall with `npm ci` --
    /// needs registry access"). A consequence, not a verdict: it says
    /// what happens if the bytes go, never whether they should.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consequence: Option<String>,
    /// The manager that reported this unit's bytes and times, when swamp
    /// did not measure them itself: `Some("docker")` for a BuildKit cache
    /// record the daemon described. `None` is swamp's own folded walk or
    /// `stat`. A reported unit's size is the manager's logical figure and
    /// its times are the manager's records, never a filesystem age
    /// (#71: "do not manufacture filesystem-equivalent build
    /// generations").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_by: Option<String>,
    /// A lock file a build tool holds while it writes here, found
    /// present this pass (SwiftPM's `.build/.lock`, Gradle's
    /// `*.lock`). Evidence that a writer may be active now, never proof
    /// either way: a crashed build leaves one behind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer_lock: Option<PathBuf>,
    /// Cargo-cleanup guidance (recommendation, age, consequence, exact-
    /// selection check status), computed exactly once per observe pass
    /// from this unit's own facts and the report's fixed `observed_at`
    /// (`report::attach_cargo_guidance`) -- R18a. Constructed as
    /// `Guidance::default()` at identification time everywhere a unit
    /// is built; the observe-pass finalizer overwrites every unit's
    /// value before the report is ever read or serialized, so a
    /// `Default` value here is never what a caller actually sees.
    /// Serialized under `"cleanup"`, matching the pre-R18a
    /// `serialize_with` hook's JSON shape exactly.
    #[serde(default, skip_deserializing, rename = "cleanup")]
    pub guidance: crate::cargo_cleanup::Guidance,
}

impl NestedArtifact {
    pub fn storage_id(root: &Path, relative: &str) -> String {
        use std::os::unix::ffi::OsStrExt;
        let root = crate::fs_gate::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let mut hash = blake3::Hasher::new();
        hash.update(b"nested-storage:v2\0");
        hash.update(root.as_os_str().as_bytes());
        hash.update(b"\0");
        Self::within(hash.finalize().to_hex().as_ref(), relative)
    }
    pub fn within(container_id: &str, relative: &str) -> String {
        if relative.is_empty() {
            return container_id.to_string();
        }
        id_for(&format!("nested-entry:v2:{container_id}:{relative}"))
    }
    pub fn stable_id(relative_path: &str, _role: &ArtifactRole) -> String {
        // Role is evidence and may be reclassified when a build message
        // arrives. The physical boundary is the identity.
        id_for(&format!("nested-artifact:v1:{relative_path}"))
    }

    pub fn action_group(relative_path: &str) -> String {
        id_for(&format!("nested-action:v1:{relative_path}"))
    }
}

pub fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

pub fn architecture_from_target(target: &str) -> Option<String> {
    let arch = target.split('-').next()?.trim();
    (!arch.is_empty()).then(|| arch.to_string())
}
