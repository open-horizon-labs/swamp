//! Live wiring of `toolchain_declarations.rs` (#56) and
//! `external_associations.rs` (#57) into the production report /
//! external-unit pipeline.
//!
//! Both modules were already complete, real and unit-tested (see their
//! own module docs); what was missing (named in
//! `.oh/sessions/2026-09-21-decision-evidence.md`'s follow-ups) was a
//! caller that actually runs them against real worktrees on every
//! report, with a real per-worktree cache so unchanged worktrees never
//! re-parse declaration/lockfile text on every call. This module is
//! that caller.
//!
//! Cost shape (#56/#57's own bounded-cost requirement): one bounded
//! `fs::metadata` per known declaration/lockfile filename per worktree
//! to build a cheap fingerprint; a full re-parse only when that
//! fingerprint changes (a declaration/lockfile file was added, removed,
//! or modified). Joining a parsed identity against a shared store is a
//! single `Path::exists()` hash lookup (see
//! `external_associations::cargo_registry_entry_exists` and its
//! siblings), never an enumeration of the store's contents. Nothing
//! here is written to the byte-history Parquet store; the cache is a
//! small JSON sidecar under the current-state `${SWAMP_DIR}`, exactly
//! like `external.rs`'s existing `external_consumers.json`.
//!
//! Entry point: [`attach_associations`], called once by every caller
//! that has *both* a computed [`Report`] and a computed
//! `Vec<ExternalUnit>` in hand at the same time (the CLI's `report
//! --view external`/`propose` routes and the TUI's startup) --
//! mirroring how `agents::discover_and_measure` already takes
//! `project_worktrees` from that same already-computed report rather
//! than re-walking anything.
//!
//! **Which detector answers which question is never decided here.**
//! This module used to hold a hand-written table mapping manager names
//! (`"pyenv"`, `"rbenv-or-rvm"`, `"asdf-or-mise"`) and ecosystems
//! (`"cargo"`, `"go"`) onto detector id constants, so adding a detector
//! meant editing a table in a file the detector's author never opened --
//! and forgetting to meant the new detector silently produced no
//! associations. Every such question is now asked of
//! `locations::Registry::with_builtins()` and answered by what the
//! detector itself declares in `Detector::manager_conventions()`
//! (`.oh/guardrails/detector-ids-only-in-registry.md`):
//!
//! - "whose installed versions satisfy this project's declared version"
//!   -> the detectors whose [`ConventionRole::DeclaredVersions`] names
//!   the declaration file the project actually used (`.nvmrc`,
//!   `.tool-versions`) and answers for the tool it named;
//! - "which unit is the Cargo registry / Go module cache / Maven
//!   repository" -> the detectors whose
//!   [`ConventionRole::DependencyStore`] claims that ecosystem, anchored
//!   to one of their own proposed locations by category and path shape;
//! - "which unit is a per-project build-output store" -> the detectors
//!   declaring [`ConventionRole::BuildOutputWorkspaceIndex`].
//!
//! Detector *ids* still appear, but only as `Detector::id()` read back
//! out of the registry to match `ExternalUnit::detector_id` -- identity
//! flows from the registry to the consumer, never the other way.

use crate::external::ExternalUnit;
use crate::external_associations;
use crate::fs_gate as fs;
use crate::locations::{
    self, ConventionRole, GlobalDefaultFile, GlobalDefaultFormat, InstalledVersionLayout,
    InstalledVersionNaming, StorageCategory, StoreAnchor, StoreEntryLookup,
};
use crate::report::{ArtifactKind, Report};
use crate::toolchain_declarations::{self, ProjectDeclarationSources, ToolVersionDeclaration};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn now() -> u64 {
    crate::entities::now()
}

// ---------------------------------------------------------------------
// Per-worktree mtime fingerprint (shared shape for both caches).
// ---------------------------------------------------------------------

const DECLARATION_FILENAMES: &[&str] = &[
    ".tool-versions",
    ".mise.toml",
    "mise.toml",
    ".python-version",
    ".ruby-version",
    ".nvmrc",
    ".node-version",
    "rust-toolchain",
    "rust-toolchain.toml",
];

const LOCKFILE_FILENAMES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
    "go.sum",
    "gradle.lockfile",
    "pom.xml",
];

#[cfg(unix)]
fn mtime_secs(meta: &fs::Metadata) -> u64 {
    use crate::fs_gate::MetadataExt;
    meta.mtime().max(0) as u64
}
#[cfg(not(unix))]
fn mtime_secs(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Every present file's own mtime, from the fixed candidate list --
/// never a directory walk. Two calls with the same result mean "nothing
/// this module reads has changed" (added/removed/modified files all
/// change this).
fn fingerprint(root: &Path, names: &[&str]) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = names
        .iter()
        .filter_map(|name| {
            crate::fs_gate::metadata_following(root.join(name))
                .ok()
                .map(|m| (name.to_string(), mtime_secs(&m)))
        })
        .collect();
    out.sort();
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CachedIdentity {
    ecosystem: String,
    name: String,
    version: String,
}

/// The three caches now live in the store's own columnar tables
/// (`crate::assoc_store`), not in JSON sidecars: they are per-worktree,
/// per-unit data, and a JSON file rewritten whole on every change is a
/// parallel database with a JSON syntax
/// (`.oh/guardrails/store-data-is-parquet-not-json-sidecars.md`).
type CacheMap = HashMap<String, crate::assoc_store::CachedRows>;

// ---------------------------------------------------------------------
// Toolchain declarations (#56): real file reads, cached per worktree.
// ---------------------------------------------------------------------

fn read_declarations(root: &Path) -> Vec<ToolVersionDeclaration> {
    let read = |name: &str| {
        crate::fs_gate::read::bounded_string(
            root.join(name),
            crate::fs_gate::read::BoundedCap::LOCKFILE,
        )
        .ok()
    };
    let tool_versions = read(".tool-versions");
    let mise_toml = read(".mise.toml").or_else(|| read("mise.toml"));
    let python_version = read(".python-version");
    let ruby_version = read(".ruby-version");
    let nvmrc = read(".nvmrc");
    let node_version = read(".node-version");
    let rust_toolchain = read("rust-toolchain");
    let rust_toolchain_toml = read("rust-toolchain.toml");
    let sources = ProjectDeclarationSources {
        tool_versions: tool_versions.as_deref(),
        mise_toml: mise_toml.as_deref(),
        python_version: python_version.as_deref(),
        ruby_version: ruby_version.as_deref(),
        nvmrc: nvmrc.as_deref(),
        node_version: node_version.as_deref(),
        rust_toolchain: rust_toolchain.as_deref(),
        rust_toolchain_toml: rust_toolchain_toml.as_deref(),
        java_version: None,
    };
    toolchain_declarations::project_declarations(root, &sources)
}

/// Declarations for one worktree, reusing the cache when the worktree's
/// declaration files have not changed since the last call.
fn cached_declarations(worktree: &Path, cache: &mut CacheMap) -> Vec<ToolVersionDeclaration> {
    let key = worktree.display().to_string();
    let fp = crate::assoc_store::fingerprint_string(&fingerprint(worktree, DECLARATION_FILENAMES));
    if let Some(entry) = cache.get(&key)
        && entry.fingerprint == fp
    {
        crate::work_counters::record_cache_hit();
        return entry
            .rows
            .iter()
            .map(Vec::as_slice)
            .filter_map(declaration_from_row)
            .collect();
    }
    crate::work_counters::record_cache_miss();
    let declarations = read_declarations(worktree);
    cache.insert(
        key,
        crate::assoc_store::CachedRows {
            fingerprint: fp,
            observed_at: 0,
            rows: declarations.iter().map(declaration_to_row).collect(),
        },
    );
    declarations
}

fn declaration_to_row(d: &ToolVersionDeclaration) -> Vec<String> {
    let (role, scope_path) = match &d.scope {
        toolchain_declarations::DeclarationScope::Project(p) => {
            ("project", p.display().to_string())
        }
        toolchain_declarations::DeclarationScope::GlobalDefault => ("global", String::new()),
    };
    vec![
        format!("{}\u{2}{}", d.manager, d.tool),
        d.version_spec.clone(),
        d.source_path.display().to_string(),
        format!("{role}\u{2}{scope_path}"),
    ]
}

fn declaration_from_row(row: &[String]) -> Option<ToolVersionDeclaration> {
    let (manager, tool) = row.first()?.split_once('\u{2}')?;
    let (role, scope_path) = row.get(3)?.split_once('\u{2}')?;
    Some(ToolVersionDeclaration {
        manager: manager.to_string(),
        tool: tool.to_string(),
        version_spec: row.get(1)?.clone(),
        source_path: PathBuf::from(row.get(2)?),
        scope: if role == "global" {
            toolchain_declarations::DeclarationScope::GlobalDefault
        } else {
            toolchain_declarations::DeclarationScope::Project(PathBuf::from(scope_path))
        },
    })
}

// ---------------------------------------------------------------------
// Dependency identities (#57): real lockfile reads, cached per worktree.
// ---------------------------------------------------------------------

fn read_identities(root: &Path) -> (Vec<CachedIdentity>, Vec<(String, String)>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    if let Ok(text) = crate::fs_gate::read::bounded_string(
        root.join("Cargo.lock"),
        crate::fs_gate::read::BoundedCap::LOCKFILE,
    ) {
        match external_associations::parse_cargo_lock(&text) {
            Ok(ids) => out.extend(ids.into_iter().map(|i| CachedIdentity {
                ecosystem: i.ecosystem.to_string(),
                name: i.name,
                version: i.version,
            })),
            Err(e) => errors.push(("cargo".to_string(), e)),
        }
    }
    if let Ok(text) = crate::fs_gate::read::bounded_string(
        root.join("package-lock.json"),
        crate::fs_gate::read::BoundedCap::LOCKFILE,
    ) {
        match external_associations::parse_package_lock_json(&text) {
            Ok(ids) => out.extend(ids.into_iter().map(|i| CachedIdentity {
                ecosystem: i.ecosystem.to_string(),
                name: i.name,
                version: i.version,
            })),
            Err(e) => errors.push(("npm".to_string(), e)),
        }
    }
    if let Ok(text) = crate::fs_gate::read::bounded_string(
        root.join("pnpm-lock.yaml"),
        crate::fs_gate::read::BoundedCap::LOCKFILE,
    ) {
        out.extend(
            external_associations::parse_pnpm_lock_yaml(&text)
                .into_iter()
                .map(|i| CachedIdentity {
                    ecosystem: i.ecosystem.to_string(),
                    name: i.name,
                    version: i.version,
                }),
        );
    }
    if let Ok(text) = crate::fs_gate::read::bounded_string(
        root.join("go.sum"),
        crate::fs_gate::read::BoundedCap::LOCKFILE,
    ) {
        out.extend(
            external_associations::parse_go_sum(&text)
                .into_iter()
                .map(|i| CachedIdentity {
                    ecosystem: i.ecosystem.to_string(),
                    name: i.name,
                    version: i.version,
                }),
        );
    }
    if let Ok(text) = crate::fs_gate::read::bounded_string(
        root.join("gradle.lockfile"),
        crate::fs_gate::read::BoundedCap::LOCKFILE,
    ) {
        out.extend(
            external_associations::parse_gradle_lockfile(&text)
                .into_iter()
                .map(|i| CachedIdentity {
                    ecosystem: i.ecosystem.to_string(),
                    name: i.name,
                    version: i.version,
                }),
        );
    }
    if let Ok(text) = crate::fs_gate::read::bounded_string(
        root.join("pom.xml"),
        crate::fs_gate::read::BoundedCap::LOCKFILE,
    ) {
        match external_associations::parse_pom_xml_with_gaps(&text) {
            Ok((ids, gaps)) => {
                out.extend(ids.into_iter().map(|i| CachedIdentity {
                    ecosystem: i.ecosystem.to_string(),
                    name: i.name,
                    version: i.version,
                }));
                // A dependency whose version is a property or inherited
                // from a parent POM is a *stated gap*, not a silent
                // absence.
                errors.extend(
                    gaps.into_iter()
                        .map(|g| ("maven-unresolved".to_string(), g)),
                );
            }
            Err(e) => errors.push(("maven".to_string(), e)),
        }
    }
    (out, errors)
}

fn cached_identities(
    worktree: &Path,
    cache: &mut CacheMap,
) -> (Vec<CachedIdentity>, Vec<(String, String)>) {
    let key = worktree.display().to_string();
    let fp = crate::assoc_store::fingerprint_string(&fingerprint(worktree, LOCKFILE_FILENAMES));
    if let Some(entry) = cache.get(&key)
        && entry.fingerprint == fp
    {
        crate::work_counters::record_cache_hit();
        let mut identities = Vec::new();
        let mut errors = Vec::new();
        for row in &entry.rows {
            match row.first().map(String::as_str) {
                Some("identity") => identities.push(CachedIdentity {
                    ecosystem: row.get(1).cloned().unwrap_or_default(),
                    name: row.get(2).cloned().unwrap_or_default(),
                    version: row.get(3).cloned().unwrap_or_default(),
                }),
                Some("gap") => errors.push((
                    row.get(1).cloned().unwrap_or_default(),
                    row.get(2).cloned().unwrap_or_default(),
                )),
                _ => {}
            }
        }
        return (identities, errors);
    }
    crate::work_counters::record_cache_miss();
    let (identities, errors) = read_identities(worktree);
    let mut rows: Vec<Vec<String>> = identities
        .iter()
        .map(|i| {
            vec![
                "identity".to_string(),
                i.ecosystem.clone(),
                i.name.clone(),
                i.version.clone(),
            ]
        })
        .collect();
    rows.extend(
        errors
            .iter()
            .map(|(eco, msg)| vec!["gap".to_string(), eco.clone(), msg.clone(), String::new()]),
    );
    cache.insert(
        key,
        crate::assoc_store::CachedRows {
            fingerprint: fp,
            observed_at: 0,
            rows,
        },
    );
    (identities, errors)
}

// ---------------------------------------------------------------------
// Matching declarations against measured installations (#56).
// ---------------------------------------------------------------------

/// Installed version names under a manager's root: one bounded,
/// single-level listing through the shared helper, never a traversal.
fn readdir_names(dir: &Path) -> Vec<String> {
    crate::locations::shallow_dir_names(dir)
}

/// Installed version identifiers for one declaration's `(manager,
/// tool)`, paired with the index into `units` each one came from -- so
/// a real match can be attributed back to the specific external unit(s)
/// that measured it, without guessing which of possibly several
/// (asdf+mise, rbenv+rvm+ruby-install) actually holds it.
fn add_flat(out: &mut Vec<(String, usize)>, units: &[ExternalUnit], detector_id: &str) {
    for (i, u) in units.iter().enumerate() {
        if u.detector_id == detector_id && u.category == StorageCategory::Installation {
            out.extend(readdir_names(&u.path).into_iter().map(|n| (n, i)));
        }
    }
}

fn add_per_tool(
    out: &mut Vec<(String, usize)>,
    units: &[ExternalUnit],
    detector_id: &str,
    tool: &str,
) {
    for (i, u) in units.iter().enumerate() {
        if u.detector_id == detector_id && u.category == StorageCategory::Installation {
            let tool_dir = u.path.join(tool);
            if crate::fs_gate::is_dir(&tool_dir) {
                out.extend(readdir_names(&tool_dir).into_iter().map(|n| (n, i)));
            }
        }
    }
}

/// What the measured installation stores in this catalog actually offer
/// for one declaration.
#[derive(Debug, Default)]
struct InstalledVersions {
    /// `(installed version identifier, index into `units`)`.
    named: Vec<(String, usize)>,
    /// At least one contributing store names its installed directories
    /// `<channel>-<host-triple>` rather than the identifier a project
    /// declares, so the declared spec has to be widened before it can be
    /// compared (declared by the detector as
    /// [`InstalledVersionNaming::ChannelWithHostTriple`]).
    channel_qualified: bool,
}

/// Installed version identifiers for one declaration, paired with the
/// index into `units` each one came from -- so a real match can be
/// attributed back to the specific external unit(s) that measured it,
/// without guessing which of possibly several (asdf+mise,
/// rbenv+RVM+ruby-install) actually holds it.
///
/// The declaration is matched to detectors purely by what they declare:
/// a detector satisfies it when one of its `manager_conventions()` names
/// the declaration's *own source file* and answers for the tool the
/// declaration names. A tool nothing in the catalog measures (`java`
/// from `.java-version`) yields an empty list, which is exactly the
/// honest answer `match_version`'s own docs give it --
/// `NoMatchingInstallation`/`UnresolvedRange`, "the installation may be
/// outside scanned scope" -- never a fabricated source.
fn installed_versions_for(
    decl: &ToolVersionDeclaration,
    units: &[ExternalUnit],
    registry: &locations::Registry,
) -> InstalledVersions {
    let mut out = InstalledVersions::default();
    let Some(file) = decl.source_path.file_name().and_then(|n| n.to_str()) else {
        return out;
    };
    for detector in registry.detectors() {
        for convention in detector.manager_conventions() {
            let ConventionRole::DeclaredVersions {
                declaration_files,
                layout,
                naming,
                ..
            } = convention.role
            else {
                continue;
            };
            if !declaration_files.contains(&file) || !convention.answers_for(&decl.tool) {
                continue;
            }
            match layout {
                InstalledVersionLayout::VersionPerEntry => {
                    add_flat(&mut out.named, units, detector.id())
                }
                InstalledVersionLayout::ToolThenVersion => {
                    add_per_tool(&mut out.named, units, detector.id(), &decl.tool)
                }
            }
            if naming == InstalledVersionNaming::ChannelWithHostTriple {
                out.channel_qualified = true;
            }
        }
    }
    out
}

/// Resolves one declaration, applying channel-to-host-triple widening
/// ([`toolchain_declarations::resolve_rustup_channel_to_dir`]) only when
/// a contributing store declared that naming, where the installed
/// directory convention (`<channel>-<host-triple>`) is not the
/// dotted-version convention `match_version`'s own alias/range logic
/// models.
fn resolve_declaration(
    decl: ToolVersionDeclaration,
    installed: &InstalledVersions,
) -> (toolchain_declarations::ToolVersionAssociation, Vec<usize>) {
    let names: Vec<String> = installed.named.iter().map(|(n, _)| n.clone()).collect();
    let assoc = if installed.channel_qualified && !names.contains(&decl.version_spec) {
        match toolchain_declarations::resolve_rustup_channel_to_dir(&decl.version_spec, &names) {
            Some(dir) => toolchain_declarations::resolve_explicit(
                decl,
                toolchain_declarations::VersionMatch::Exact { installed: dir },
            ),
            None => toolchain_declarations::resolve(decl, &names),
        }
    } else {
        toolchain_declarations::resolve(decl, &names)
    };
    let contributing_units: Vec<usize> = match &assoc.match_result {
        toolchain_declarations::VersionMatch::Exact { installed: name } => installed
            .named
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, i)| *i)
            .collect(),
        _ => Vec::new(),
    };
    (assoc, contributing_units)
}

// ---------------------------------------------------------------------
// Shared-store identity joins (#57): hash-lookup existence checks, not
// enumeration -- see `external_associations`'s own module doc for why.
// ---------------------------------------------------------------------

fn path_ends_with(path: &Path, suffix: &[&str]) -> bool {
    let comps: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    if suffix.len() > comps.len() {
        return false;
    }
    comps[comps.len() - suffix.len()..] == suffix.iter().map(|s| s.to_string()).collect::<Vec<_>>()
}

/// Which of `units` measure the store a detector anchored a convention
/// to. The anchor is expressed against what the detector already
/// publishes -- a category, a path shape, or a location derived from a
/// sibling -- so no consumer ever holds a path literal for a store.
fn anchored_units(units: &[ExternalUnit], detector_id: &str, anchor: StoreAnchor) -> Vec<usize> {
    let of_detector = |category: StorageCategory, pred: &dyn Fn(&ExternalUnit) -> bool| {
        units
            .iter()
            .enumerate()
            .filter(|(_, u)| u.detector_id == detector_id && u.category == category && pred(u))
            .map(|(i, _)| i)
            .collect::<Vec<usize>>()
    };
    match anchor {
        StoreAnchor::SoleLocation => units
            .iter()
            .enumerate()
            .filter(|(_, u)| u.detector_id == detector_id)
            .map(|(i, _)| i)
            .collect(),
        StoreAnchor::Categorized { category, suffix } => {
            of_detector(category, &|u| path_ends_with(&u.path, suffix))
        }
        StoreAnchor::CategorizedExcept { category, except } => of_detector(category, &|u| {
            !except.iter().any(|s| path_ends_with(&u.path, s))
        }),
        StoreAnchor::AncestorOfSibling {
            sibling,
            up,
            category,
        } => {
            let Some(mut path) = units
                .iter()
                .find(|u| u.detector_id == detector_id && u.category == sibling)
                .map(|u| u.path.clone())
            else {
                return Vec::new();
            };
            for _ in 0..up {
                let Some(parent) = path.parent() else {
                    return Vec::new();
                };
                path = parent.to_path_buf();
            }
            of_detector(category, &|u| u.path == path)
        }
    }
}

/// Whether the store at `path`, laid out the way `lookup` describes,
/// holds an entry for one package identity: exactly one bounded,
/// deterministic probe through `external_associations`, never a store
/// enumeration. `None` for the two layouts that cannot answer per
/// identity at all (see [`StoreEntryLookup::ContentAddressed`] and
/// [`StoreEntryLookup::WholeStore`], both handled by the caller).
fn store_holds_entry(
    lookup: StoreEntryLookup,
    path: &Path,
    identity: &CachedIdentity,
) -> Option<bool> {
    let name = identity.name.as_str();
    let version = identity.version.as_str();
    match lookup {
        StoreEntryLookup::RegistrySourceTree => Some(
            external_associations::cargo_registry_entry_exists(path, name, version),
        ),
        StoreEntryLookup::GoModulePath => Some(
            external_associations::go_module_cache_entry_exists(path, name, version),
        ),
        StoreEntryLookup::GradleModules => Some(external_associations::gradle_cache_entry_exists(
            path, name, version,
        )),
        StoreEntryLookup::MavenLayout => Some(external_associations::maven_repo_entry_exists(
            path, name, version,
        )),
        StoreEntryLookup::ContentAddressed { .. } | StoreEntryLookup::WholeStore => None,
    }
}

/// Every `(detector id, anchor, lookup)` in the catalog whose declared
/// dependency store claims `ecosystem`.
fn dependency_stores_for<'a>(
    ecosystem: &str,
    registry: &'a locations::Registry,
) -> Vec<(&'a str, StoreAnchor, StoreEntryLookup)> {
    let mut out = Vec::new();
    for detector in registry.detectors() {
        for convention in detector.manager_conventions() {
            if convention.tool != Some(ecosystem) {
                continue;
            }
            if let ConventionRole::DependencyStore { anchor, lookup } = convention.role {
                out.push((detector.id(), anchor, lookup));
            }
        }
    }
    out
}

/// Which shared-store unit indices actually hold an entry for
/// `identity`, via one deterministic `Path::exists()` lookup per
/// candidate unit -- never a store enumeration. A content-addressed
/// store (npm cacache) and a whole-store-only one (pnpm) are not
/// resolved here: neither can be mapped to a specific entry, and both
/// are handled by the caller.
fn match_identity(
    identity: &CachedIdentity,
    units: &[ExternalUnit],
    registry: &locations::Registry,
) -> Vec<usize> {
    let mut out = Vec::new();
    for (detector_id, anchor, lookup) in dependency_stores_for(&identity.ecosystem, registry) {
        if let Some(i) = anchored_units(units, detector_id, anchor)
            .into_iter()
            .next()
            && store_holds_entry(lookup, &units[i].path, identity) == Some(true)
        {
            out.push(i);
        }
    }
    out
}

/// The measured unit of a store that can only ever answer "yes,
/// something from this ecosystem is here" (pnpm's per-file
/// content-addressed store): a project naming any identity in the
/// ecosystem consumes the store as a whole, which is all that can
/// honestly be said about it.
fn whole_store_unit(
    ecosystem: &str,
    units: &[ExternalUnit],
    registry: &locations::Registry,
) -> Option<usize> {
    dependency_stores_for(ecosystem, registry)
        .into_iter()
        .filter(|(_, _, lookup)| matches!(lookup, StoreEntryLookup::WholeStore))
        .find_map(|(detector_id, anchor, _)| {
            anchored_units(units, detector_id, anchor)
                .into_iter()
                .next()
        })
}

// ---------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------

/// Attaches consumer evidence both ways -- installation/shared-store
/// unit <- projects declaring/depending on it; project's own `Source`
/// row -> installations/dependencies it declares -- from real per-
/// worktree declaration and lockfile reads, cached by file mtime.
/// Called once per report by every caller that has both `report` and
/// `external_units` computed (CLI `report --view external`/`propose`,
/// TUI startup); never invoked from inside the bus (external units are
/// not part of `Report` -- see `external.rs`'s own module doc).
pub fn attach_associations(
    report: &mut Report,
    external_units: &mut [ExternalUnit],
    swamp_dir: Option<&Path>,
) {
    let mut decl_cache: CacheMap = swamp_dir
        .map(|d| crate::assoc_store::DeclarationTable::open(d).load())
        .unwrap_or_default();
    let mut dep_cache: CacheMap = swamp_dir
        .map(|d| crate::assoc_store::IdentityTable::open(d).load())
        .unwrap_or_default();
    let mut xcode_cache: CacheMap = swamp_dir
        .map(|d| crate::assoc_store::XcodeJoinTable::open(d).load())
        .unwrap_or_default();

    // The catalog itself, asked instead of a wiring table. Constructing
    // it boxes the detector list and touches nothing on disk (no
    // `detect` call happens here); `detector_id` on an already-measured
    // `ExternalUnit` is matched against `Detector::id()` read back out
    // of it.
    let registry = locations::Registry::with_builtins();

    let (project_roots, mut unit_evidence, unit_consumers) = attach_project_side(
        report,
        external_units,
        &registry,
        &mut decl_cache,
        &mut dep_cache,
    );
    for detector in registry.detectors() {
        for convention in detector.manager_conventions() {
            let ConventionRole::DependencyStore {
                anchor,
                lookup: StoreEntryLookup::ContentAddressed { basis, reason },
            } = convention.role
            else {
                continue;
            };
            for i in anchored_units(external_units, detector.id(), anchor) {
                unit_evidence
                    .entry(i)
                    .or_default()
                    .push(crate::evidence::Evidence::unknown(
                        crate::evidence::FactKind::Consumer,
                        crate::evidence::FactSubtype::DeclaredConsumer,
                        crate::evidence::EvidenceSource::Inferred {
                            basis: basis.into(),
                        },
                        now(),
                        crate::evidence::Reason::carried(reason),
                    ));
            }
        }
    }

    // Global defaults (#56's "global defaults as their own role"): read
    // from the manager's own `LocalState` unit, using the file the
    // detector declares (rustup's `settings.toml`) -- a real
    // ExternalUnit already resolved by this same call, never a fresh
    // Environment/env-var lookup.
    attach_global_defaults(external_units, &registry);

    // Build-output store -> project (#57's own listed shared-store
    // join): for Xcode DerivedData, one `plutil` read per subfolder,
    // bounded by how many Xcode projects have ever built locally.
    attach_build_output_associations(
        external_units,
        &project_roots,
        &mut unit_evidence,
        &mut xcode_cache,
        &registry,
    );

    for (i, labels) in unit_consumers {
        let mut labels = labels;
        labels.sort();
        labels.dedup();
        let value = if labels.len() == 1 {
            crate::evidence::FactValue::Text(labels[0].clone())
        } else {
            crate::evidence::FactValue::List(labels.clone())
        };
        let mut ev = crate::evidence::Evidence::known(
            crate::evidence::FactKind::Consumer,
            crate::evidence::FactSubtype::DeclaredConsumer,
            value,
            crate::evidence::EvidenceSource::Inferred {
                basis: "toolchain declaration or dependency lockfile match".into(),
            },
            now(),
        );
        if labels.len() > 1 {
            ev =
                ev.with_note("declared by more than one project; a shared reference, not an error");
        }
        unit_evidence.entry(i).or_default().push(ev);
    }
    for (i, evs) in unit_evidence {
        if let Some(u) = external_units.get_mut(i) {
            u.evidence.extend(evs);
        }
    }

    if let Some(dir) = swamp_dir {
        let at = now();
        let _ = crate::assoc_store::DeclarationTable::open(dir).save(&decl_cache, at);
        let _ = crate::assoc_store::IdentityTable::open(dir).save(&dep_cache, at);
        let _ = crate::assoc_store::XcodeJoinTable::open(dir).save(&xcode_cache, at);
    }
}

/// The project side alone: each worktree's own `Source` row gets the
/// installations and dependencies it declares, from the cached
/// declaration/lockfile tables. For a report with no external units to
/// join against -- `report::merge_reports`, which the TUI calls on its
/// event thread whenever one root's report arrives -- this is all of
/// the association work there is, and it never reaches the unit side's
/// Xcode `plutil` read (`attach_build_output_associations`), which the
/// source audit `tui_actions_off_event_thread` now sees as a subprocess.
pub fn attach_project_associations(report: &mut Report, swamp_dir: Option<&Path>) {
    let mut decl_cache: CacheMap = swamp_dir
        .map(|d| crate::assoc_store::DeclarationTable::open(d).load())
        .unwrap_or_default();
    let mut dep_cache: CacheMap = swamp_dir
        .map(|d| crate::assoc_store::IdentityTable::open(d).load())
        .unwrap_or_default();
    let registry = locations::Registry::with_builtins();
    let _ = attach_project_side(report, &[], &registry, &mut decl_cache, &mut dep_cache);
    if let Some(dir) = swamp_dir {
        let at = now();
        let _ = crate::assoc_store::DeclarationTable::open(dir).save(&decl_cache, at);
        let _ = crate::assoc_store::IdentityTable::open(dir).save(&dep_cache, at);
    }
}

type UnitEvidence = HashMap<usize, Vec<crate::evidence::Evidence>>;
type UnitConsumers = HashMap<usize, Vec<String>>;

/// Declarations and lockfile identities per worktree: evidence on each
/// project's `Source` row, and which unit indices each project consumes.
fn attach_project_side(
    report: &mut Report,
    external_units: &[ExternalUnit],
    registry: &locations::Registry,
    decl_cache: &mut CacheMap,
    dep_cache: &mut CacheMap,
) -> (Vec<PathBuf>, UnitEvidence, UnitConsumers) {
    let project_roots: Vec<PathBuf> = report
        .projects
        .iter()
        .flat_map(|p| p.worktrees.iter())
        .map(|wt| wt.path.clone())
        .collect();

    // unit index -> evidence to append, collected during the read-only
    // matching pass below and applied once at the end (avoids mixing
    // mutable/immutable borrows of `external_units` mid-pass).
    let unit_evidence: HashMap<usize, Vec<crate::evidence::Evidence>> = HashMap::new();
    // unit index -> confirmed consuming project labels, deduplicated
    // and combined into one Evidence per unit at the end.
    let mut unit_consumers: HashMap<usize, Vec<String>> = HashMap::new();

    for project in &mut report.projects {
        let project_label = project.name.clone();
        for wt in &mut project.worktrees {
            let wt_path = wt.path.clone();

            // --- #56: toolchain declarations -> installations ---
            let declarations = cached_declarations(&wt_path, decl_cache);
            let mut project_side_evidence = Vec::new();
            for decl in declarations {
                let installed = installed_versions_for(&decl, external_units, registry);
                let (assoc, contributing_units) = resolve_declaration(decl, &installed);
                project_side_evidence.push(assoc.evidence.clone());
                if assoc.evidence.is_known() {
                    for u in contributing_units {
                        unit_consumers
                            .entry(u)
                            .or_default()
                            .push(project_label.clone());
                    }
                }
            }

            // --- #57: dependency lockfiles -> shared store entries ---
            let (identities, parse_errors) = cached_identities(&wt_path, dep_cache);
            for (ecosystem, message) in &parse_errors {
                project_side_evidence.push(match ecosystem.strip_suffix("-unresolved") {
                    Some(base) => external_associations::unresolved_dependency_evidence(
                        base, &wt_path, message,
                    ),
                    None => external_associations::invalid_lockfile_evidence(
                        ecosystem, &wt_path, message,
                    ),
                });
            }
            // Stores that can only be joined as a whole are attributed
            // once per worktree, not once per identity.
            let mut whole_stores: Vec<usize> = Vec::new();
            for identity in &identities {
                whole_stores.extend(whole_store_unit(
                    &identity.ecosystem,
                    external_units,
                    registry,
                ));
                for unit_index in match_identity(identity, external_units, registry) {
                    unit_consumers
                        .entry(unit_index)
                        .or_default()
                        .push(project_label.clone());
                }
            }
            whole_stores.sort_unstable();
            whole_stores.dedup();
            for i in whole_stores {
                unit_consumers
                    .entry(i)
                    .or_default()
                    .push(project_label.clone());
            }

            // Attach every project-side fact to this worktree's own
            // `Source` row -- "at most one Source row per worktree" is
            // this pass's single canonical attachment point.
            if let Some(source_row) = wt
                .artifacts
                .iter_mut()
                .find(|a| a.kind == ArtifactKind::Source)
            {
                source_row.evidence.extend(project_side_evidence);
            }
        }
    }

    // Content-addressed stores (npm cacache): the honest "cannot be
    // mapped to an entry" note, once per measured unit and never per
    // project -- there is no per-entry attribution to make. Both the
    // basis and the reason come from the detector's own declaration.
    (project_roots, unit_evidence, unit_consumers)
}

/// Every manager that declares where it records a *machine-wide*
/// default version gets that default read and joined against its own
/// installation store.
fn attach_global_defaults(units: &mut [ExternalUnit], registry: &locations::Registry) {
    for detector in registry.detectors() {
        for convention in detector.manager_conventions() {
            let ConventionRole::DeclaredVersions {
                naming,
                global_default: Some(global_default),
                ..
            } = convention.role
            else {
                continue;
            };
            attach_global_default(
                units,
                detector.id(),
                detector.name(),
                global_default,
                naming,
            );
        }
    }
}

fn attach_global_default(
    units: &mut [ExternalUnit],
    detector_id: &str,
    manager_name: &str,
    global_default: GlobalDefaultFile,
    naming: InstalledVersionNaming,
) {
    let Some(local_state_idx) = units
        .iter()
        .position(|u| u.detector_id == detector_id && u.category == StorageCategory::LocalState)
    else {
        return;
    };
    let Some(installs_idx) = units
        .iter()
        .position(|u| u.detector_id == detector_id && u.category == StorageCategory::Installation)
    else {
        return;
    };
    let settings_path = units[local_state_idx].path.join(global_default.file_name);
    let Some(text) = crate::fs_gate::read::bounded_string(
        &settings_path,
        crate::fs_gate::read::BoundedCap::LOCKFILE,
    )
    .ok() else {
        return;
    };
    let declared = match global_default.format {
        // `toolchain_declarations` already models this file shape, and
        // its typed reader is the one place the field name lives.
        GlobalDefaultFormat::TomlTopLevelString => {
            toolchain_declarations::parse_rustup_default_toolchain(&text)
        }
    };
    let Some(declared) = declared else {
        return;
    };
    let installed = readdir_names(&units[installs_idx].path);
    let matched = if installed.contains(&declared) {
        Some(declared.clone())
    } else if naming == InstalledVersionNaming::ChannelWithHostTriple {
        toolchain_declarations::resolve_rustup_channel_to_dir(&declared, &installed)
    } else {
        None
    };
    let field = global_default.field;
    let ev = match matched {
        Some(installed_name) => crate::evidence::Evidence::known(
            crate::evidence::FactKind::Consumer,
            crate::evidence::FactSubtype::DeclaredConsumer,
            crate::evidence::FactValue::Text(installed_name),
            crate::evidence::EvidenceSource::ConfigDeclaration {
                path: settings_path.display().to_string(),
            },
            now(),
        ),
        None => crate::evidence::Evidence::unknown(
            crate::evidence::FactKind::Consumer,
            crate::evidence::FactSubtype::DeclaredConsumer,
            crate::evidence::EvidenceSource::ConfigDeclaration {
                path: settings_path.display().to_string(),
            },
            now(),
            crate::reason!(
                "{manager_name} {field} '{declared}' matches no installed toolchain directory"
            ),
        ),
    }
    .with_note(format!(
        "global default ({manager_name} {} {field}), not a per-project declaration",
        global_default.file_name
    ));
    units[installs_idx].evidence.push(ev);
}

/// Every detector that declares a per-project build-output store gets
/// its subfolders joined back to the projects that produced them.
fn attach_build_output_associations(
    units: &[ExternalUnit],
    project_roots: &[PathBuf],
    unit_evidence: &mut HashMap<usize, Vec<crate::evidence::Evidence>>,
    cache: &mut CacheMap,
    registry: &locations::Registry,
) {
    for detector in registry.detectors() {
        for convention in detector.manager_conventions() {
            let ConventionRole::BuildOutputWorkspaceIndex { anchor } = convention.role else {
                continue;
            };
            for idx in anchored_units(units, detector.id(), anchor) {
                attach_workspace_index(units, idx, project_roots, unit_evidence, cache);
            }
        }
    }
}

/// The build-output-store -> workspace join, cached by each subfolder's
/// own `info.plist` `(size, mtime)`.
///
/// `read_workspace_path` spawns `plutil`. Uncached, that is one
/// subprocess per locally-built Xcode project on *every* `report --view
/// external`, every unified `propose` and every TUI refresh -- fifty
/// spawns for fifty projects that did not change, which is what the PR
/// #123 review measured (6 folders, 6 spawns, then 6 more on an
/// identical second pass). An unchanged folder now costs a table lookup.
fn attach_workspace_index(
    units: &[ExternalUnit],
    idx: usize,
    project_roots: &[PathBuf],
    unit_evidence: &mut HashMap<usize, Vec<crate::evidence::Evidence>>,
    cache: &mut CacheMap,
) {
    let derived_data_path = units[idx].path.clone();
    for subfolder in external_associations::list_derived_data_subfolders(&derived_data_path) {
        let info_plist = subfolder.join("info.plist");
        let Ok(meta) = crate::fs_gate::symlink_metadata(&info_plist) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let name = subfolder
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let key = subfolder.display().to_string();
        let fp = crate::assoc_store::fingerprint_string(&[(
            format!("info.plist:{}", meta.len()),
            mtime_secs(&meta),
        )]);
        let cached: Option<(String, String)> = cache
            .get(&key)
            .filter(|e| e.fingerprint == fp)
            .and_then(|e| e.rows.first())
            .map(|row| {
                (
                    row.first().cloned().unwrap_or_default(),
                    row.get(1).cloned().unwrap_or_default(),
                )
            });
        let (outcome, detail) = match cached {
            Some(hit) => {
                crate::work_counters::record_cache_hit();
                hit
            }
            None => {
                crate::work_counters::record_cache_miss();
                let fresh = match external_associations::read_workspace_path(&info_plist) {
                    Ok(Some(path)) => ("workspace".to_string(), path),
                    Ok(None) => ("no-workspace".to_string(), String::new()),
                    Err(e) => ("error".to_string(), e),
                };
                cache.insert(
                    key,
                    crate::assoc_store::CachedRows {
                        fingerprint: fp,
                        observed_at: 0,
                        rows: vec![vec![fresh.0.clone(), fresh.1.clone()]],
                    },
                );
                fresh
            }
        };
        let ev = match outcome.as_str() {
            "error" => crate::evidence::Evidence::unavailable(
                crate::evidence::FactKind::Consumer,
                crate::evidence::FactSubtype::DeclaredConsumer,
                crate::evidence::EvidenceSource::BuildMetadata {
                    path: info_plist.display().to_string(),
                },
                now(),
                crate::evidence::Reason::carried(detail),
            ),
            "workspace" => external_associations::xcode_derived_data_association(
                Some(detail.as_str()),
                project_roots,
            ),
            _ => external_associations::xcode_derived_data_association(None, project_roots),
        };
        unit_evidence
            .entry(idx)
            .or_default()
            .push(ev.with_note(format!("DerivedData/{name}")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locations::Provenance;
    use crate::report::{ArtifactRow, ProjectRow, Source, WorktreeKind, WorktreeRow};

    fn empty_artifact_row(kind: ArtifactKind, path: &Path) -> ArtifactRow {
        ArtifactRow {
            kind,
            path: path.to_path_buf(),
            bytes: 0,
            mtime_max: 0,
            ecosystem: None,
            hardlinked: false,
            dedup_stale: false,
            allocated_bytes: None,
            allocated_growth_bytes: None,
            local_bytes: 0,
            track: None,
            growth_bytes: None,
            regrowth_count: 0,
            observed_at: 0,
            confidence: crate::entities::Confidence::High,
            source: Source::new("test"),
            note: None,
            created_at: None,
            containers: Vec::new(),
            shared_with: Vec::new(),
            dangling: false,
            evidence: Vec::new(),
        }
    }

    fn external_unit(detector_id: &str, category: StorageCategory, path: &Path) -> ExternalUnit {
        ExternalUnit {
            detector_id: detector_id.to_string(),
            detector_name: detector_id.to_string(),
            category,
            provenance: Provenance::BuiltinConvention,
            path: path.to_path_buf(),
            bytes: 0,
            mtime_max: 0,
            hardlinked: false,
            growth_bytes: None,
            regrowth_count: 0,
            observed_at: 0,
            consumers: Vec::new(),
            note: None,
            evidence: Vec::new(),
        }
    }

    fn empty_report(root: &Path, projects: Vec<ProjectRow>) -> Report {
        Report {
            store_dir: None,
            observed_at: now(),
            root: root.to_path_buf(),
            projects,
            unowned: Vec::new(),
            reconciliation: crate::report::Reconciliation {
                unique_estimate: None,
                attributed: 0,
                unowned: 0,
                walked_total: 0,
                du_total: None,
                docker_attributed: 0,
                docker_unowned: 0,
            },
            series_by_key: Default::default(),
            total_series: Vec::new(),
            series_window_secs: 0,
            notes: Vec::new(),
            dirs_by_worktree: None,
            files_by_worktree: None,
            schedule_line: None,
            summary: Default::default(),
            github_enrichment: None,
            nested_artifacts: Vec::new(),
        }
    }

    fn one_worktree_project(name: &str, root: &Path, artifacts: Vec<ArtifactRow>) -> ProjectRow {
        ProjectRow {
            project_id: name.into(),
            name: name.into(),
            worktrees: vec![WorktreeRow {
                worktree_id: name.into(),
                path: root.to_path_buf(),
                kind: WorktreeKind::Main,
                artifacts,
                signals: Vec::new(),
                branch: None,
                github: None,
                merge_complete: None,
                idle_secs: None,
            }],
            ecosystems: Vec::new(),
            remote: None,
        }
    }

    #[test]
    fn pinned_rust_toolchain_declaration_attaches_evidence_both_ways() {
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path().join("proj");
        std::fs::create_dir_all(&project_root).unwrap();
        std::fs::write(project_root.join("rust-toolchain"), "1.82.0\n").unwrap();

        let rustup_home = tmp.path().join("rustup");
        std::fs::create_dir_all(rustup_home.join("toolchains/1.82.0-x86_64-apple-darwin")).unwrap();

        let mut report = empty_report(
            tmp.path(),
            vec![one_worktree_project(
                "proj",
                &project_root,
                vec![empty_artifact_row(ArtifactKind::Source, &project_root)],
            )],
        );
        let mut units = vec![external_unit(
            crate::locations::rustup::RUSTUP_DETECTOR_ID,
            StorageCategory::Installation,
            &rustup_home.join("toolchains"),
        )];

        attach_associations(&mut report, &mut units, None);

        let source_row = &report.projects[0].worktrees[0].artifacts[0];
        assert!(
            source_row
                .evidence
                .iter()
                .any(|e| e.is_known() && e.kind == crate::evidence::FactKind::Consumer),
            "project's Source row must carry a resolved installation fact: {:?}",
            source_row.evidence
        );
        assert!(
            units[0].evidence.iter().any(|e| e.is_known()),
            "installation unit must carry a resolved consuming-project fact: {:?}",
            units[0].evidence
        );
    }

    #[test]
    fn cached_declarations_reuses_result_when_files_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".ruby-version"), "3.2.0\n").unwrap();
        let mut cache = CacheMap::new();
        let first = cached_declarations(tmp.path(), &mut cache);
        assert_eq!(first.len(), 1);
        // Mutate the cache entry's stored declarations directly to a
        // sentinel the real file would never produce, proving the
        // second call reused the cache instead of re-reading the file.
        cache
            .get_mut(&tmp.path().display().to_string())
            .unwrap()
            .rows[0][1] = "sentinel".into();
        let second = cached_declarations(tmp.path(), &mut cache);
        assert_eq!(second[0].version_spec, "sentinel");
    }

    #[test]
    fn cached_declarations_reparses_after_mtime_change() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".ruby-version"), "3.2.0\n").unwrap();
        let mut cache = CacheMap::new();
        let _ = cached_declarations(tmp.path(), &mut cache);
        // Simulate a real edit: change content and force a different
        // mtime (some filesystems have 1s mtime resolution).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(tmp.path().join(".ruby-version"), "3.3.0\n").unwrap();
        let second = cached_declarations(tmp.path(), &mut cache);
        assert_eq!(second[0].version_spec, "3.3.0");
    }

    #[test]
    fn cargo_dependency_shared_by_two_projects_is_a_known_list_on_the_unit() {
        let tmp = tempfile::tempdir().unwrap();
        let registry_src = tmp.path().join("cargo/registry/src");
        std::fs::create_dir_all(registry_src.join("index.crates.io-abc/serde-1.0.203")).unwrap();

        let mk_project = |name: &str| -> ProjectRow {
            let root = tmp.path().join(name);
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(
                root.join("Cargo.lock"),
                "[[package]]\nname = \"serde\"\nversion = \"1.0.203\"\n",
            )
            .unwrap();
            one_worktree_project(
                name,
                &root,
                vec![empty_artifact_row(ArtifactKind::Source, &root)],
            )
        };
        let p1 = mk_project("project-a");
        let p2 = mk_project("project-b");

        let mut report = empty_report(tmp.path(), vec![p1, p2]);
        let mut units = vec![external_unit(
            crate::locations::cargo_home::CARGO_HOME_DETECTOR_ID,
            StorageCategory::Cache,
            &registry_src,
        )];

        attach_associations(&mut report, &mut units, None);

        let consumer_fact = units[0]
            .evidence
            .iter()
            .find(|e| e.is_known() && e.kind == crate::evidence::FactKind::Consumer)
            .expect("expected a known consumer fact on the registry/src unit");
        match &consumer_fact.status {
            crate::evidence::FactStatus::Known(crate::evidence::FactValue::List(labels)) => {
                assert_eq!(labels.len(), 2, "both projects must be named: {labels:?}");
            }
            other => panic!("expected a known list of two consumers, got {other:?}"),
        }
    }

    #[test]
    fn npm_cache_is_always_unknown_never_guessed_from_package_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("package-lock.json"),
            r#"{"packages":{"node_modules/lodash":{"version":"4.17.21"}}}"#,
        )
        .unwrap();
        let mut report = empty_report(
            tmp.path(),
            vec![one_worktree_project(
                "proj",
                &root,
                vec![empty_artifact_row(ArtifactKind::Source, &root)],
            )],
        );
        let npm_cache = tmp.path().join("npm");
        std::fs::create_dir_all(&npm_cache).unwrap();
        let mut units = vec![external_unit(
            crate::locations::npm::NPM_DETECTOR_ID,
            StorageCategory::Cache,
            &npm_cache,
        )];
        attach_associations(&mut report, &mut units, None);
        // The tempting shortcut this rejects: matching package-lock.json's
        // `lodash@4.17.21` entry back to a specific npm cacache file.
        assert!(units[0].evidence.iter().all(|e| !e.is_known()));
        assert!(units[0].evidence.iter().any(|e| matches!(
            &e.status,
            crate::evidence::FactStatus::Unknown { reason }
                if reason.contains("content-addressed")
        )));
    }
}
