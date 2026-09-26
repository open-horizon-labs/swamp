//! Node.js build outputs, installed dependency trees and shared package
//! stores (#68).
//!
//! The adapter is per-tool, not per-ecosystem: a `.next/cache` is not a
//! `.next`, a `coverage/` is not a `dist/`, and `node_modules` is not
//! any of them. #68 rejects both failure modes explicitly -- "not one
//! generic node_modules/dist row and not an unimplemented unknown for
//! every tool" -- so each directory name this adapter knows carries its
//! own role, its own consequence and its own tested boundary, and a
//! directory it does not know is an explicit residual rather than a
//! silent omission.
//!
//! # What is never done
//!
//! No npm script is run and no JavaScript configuration is loaded. That
//! rules out the two things that would answer the most questions
//! (`npm ls --json`, and evaluating `next.config.js` to learn where the
//! output went), and it is not negotiable: both execute code from
//! whatever repository happened to be on disk. Where the answer is only
//! available that way, the unit says so.
//!
//! # Identity, and what is not inferred
//!
//! A package's identity is its own `package.json` `name`/`version`, read
//! bounded. Nothing is inferred from a directory's name: `node_modules/
//! lodash` holding a package that calls itself something else is
//! reported as what it calls itself, and a `package.json` that cannot be
//! read leaves the identity unknown rather than guessed from the
//! basename.
//!
//! No **build generation** is invented. npm, pnpm and yarn record none,
//! and neither a directory's modification time nor a hash in a filename
//! establishes one output supersedes another.
//!
//! # Shared stores
//!
//! pnpm's store is content-addressed and hardlinked into each project's
//! `node_modules/.pnpm`; npm's `_cacache` is shared across every
//! project. Their bytes are charged once -- the walk deduplicates by
//! inode -- and the entries here carry `SharedHardlink` membership so no
//! view adds a project's copy to the store's own total. Which project
//! links a given content object is not derivable from the object, and
//! the unit says that rather than picking one.

use super::{BuildAdapter, BuildCapabilities, BuildContainer, BuildCtx, NestedUnitBuilder};
use crate::artifact::{ArtifactRole, ArtifactVariant, Membership, NestedArtifact, relative_path};
use crate::entities::Confidence;
use std::path::{Path, PathBuf};

/// How many top-level installed packages get a bounded `package.json`
/// read, largest first.
///
/// A monorepo's `node_modules` holds tens of thousands of packages, and
/// one manifest read each would be tens of megabytes on every refresh
/// for identity nobody asked about. The largest entries are the ones a
/// size decision is about, so those are the ones that get an identity;
/// the rest keep their directory name as their path and an explicit
/// "identity not read" limit. The number of skipped packages is
/// reported, so the limit is a quantity rather than a hedge.
const PACKAGE_IDENTITY_BUDGET: usize = 200;

pub struct Adapter;

/// A directory name this adapter knows, and what it is.
///
/// Every entry is a directory a tool in the Node ecosystem creates, and
/// the `tool` column is what makes the capability matrix per-tool rather
/// than per-ecosystem: a user asking "what is `.turbo`?" gets Turborepo,
/// not "a Node cache".
struct Known {
    name: &'static str,
    role: ArtifactRole,
    tool: &'static str,
    consequence: &'static str,
    /// Whether this name is ambiguous enough that another ecosystem's
    /// marker at the project root should win it (`build`, `out`).
    ambiguous: bool,
}

const KNOWN: &[Known] = &[
    // ---- generated output -------------------------------------------
    Known {
        name: "dist",
        role: ArtifactRole::Output,
        tool: "bundler (vite, rollup, tsc, webpack)",
        consequence: "rebuild with this project's build script (commonly `npm run build`)",
        ambiguous: false,
    },
    Known {
        name: "build",
        role: ArtifactRole::Output,
        tool: "create-react-app, tsc",
        consequence: "rebuild with this project's build script (commonly `npm run build`)",
        ambiguous: true,
    },
    Known {
        name: "out",
        role: ArtifactRole::Output,
        tool: "next export, tsc",
        consequence: "rebuild with this project's build script",
        ambiguous: true,
    },
    Known {
        name: ".next",
        role: ArtifactRole::Output,
        tool: "Next.js",
        consequence: "rebuild with `next build`; the dev server also regenerates it",
        ambiguous: false,
    },
    Known {
        name: ".nuxt",
        role: ArtifactRole::Output,
        tool: "Nuxt",
        consequence: "rebuild with `nuxt build`",
        ambiguous: false,
    },
    Known {
        name: ".svelte-kit",
        role: ArtifactRole::Output,
        tool: "SvelteKit",
        consequence: "rebuild with `vite build`; `svelte-kit sync` regenerates the types",
        ambiguous: false,
    },
    Known {
        name: ".output",
        role: ArtifactRole::Output,
        tool: "Nitro (Nuxt 3)",
        consequence: "rebuild with `nuxt build`",
        ambiguous: false,
    },
    Known {
        name: "storybook-static",
        role: ArtifactRole::Output,
        tool: "Storybook",
        consequence: "rebuild with `storybook build`",
        ambiguous: false,
    },
    Known {
        name: "out-tsc",
        role: ArtifactRole::Output,
        tool: "Angular CLI",
        consequence: "rebuild with `ng build`",
        ambiguous: false,
    },
    // ---- test and coverage output ------------------------------------
    Known {
        name: "coverage",
        role: ArtifactRole::TestOutput,
        tool: "istanbul / nyc / vitest / jest",
        consequence: "a test rerun with coverage enabled regenerates it",
        ambiguous: false,
    },
    Known {
        name: ".nyc_output",
        role: ArtifactRole::TestOutput,
        tool: "nyc",
        consequence: "a test rerun with coverage enabled regenerates it",
        ambiguous: false,
    },
    Known {
        name: "playwright-report",
        role: ArtifactRole::TestOutput,
        tool: "Playwright",
        consequence: "a `playwright test` run regenerates it",
        ambiguous: false,
    },
    Known {
        name: "test-results",
        role: ArtifactRole::TestOutput,
        tool: "Playwright",
        consequence: "a `playwright test` run regenerates it",
        ambiguous: false,
    },
    // ---- caches and intermediates -------------------------------------
    Known {
        name: ".turbo",
        role: ArtifactRole::Intermediate,
        tool: "Turborepo",
        consequence: "the next task run is a cache miss and re-executes rather than replaying",
        ambiguous: false,
    },
    Known {
        name: ".parcel-cache",
        role: ArtifactRole::Intermediate,
        tool: "Parcel",
        consequence: "the next `parcel build` is a cold build",
        ambiguous: false,
    },
    Known {
        name: ".vite",
        role: ArtifactRole::Intermediate,
        tool: "Vite",
        consequence: "the next dev server start re-optimizes dependencies",
        ambiguous: false,
    },
    Known {
        name: ".cache",
        role: ArtifactRole::Intermediate,
        tool: "babel-loader / webpack / gatsby",
        consequence: "the next build recompiles what it would otherwise have replayed",
        ambiguous: true,
    },
    Known {
        name: ".angular",
        role: ArtifactRole::Intermediate,
        tool: "Angular CLI",
        consequence: "the next `ng build` is a cold build",
        ambiguous: false,
    },
    Known {
        name: ".expo",
        role: ArtifactRole::Intermediate,
        tool: "Expo",
        consequence: "the next Expo start rebuilds its cache",
        ambiguous: false,
    },
    Known {
        name: ".metro",
        role: ArtifactRole::Intermediate,
        tool: "Metro (React Native)",
        consequence: "the next Metro start rebuilds its cache",
        ambiguous: false,
    },
    // ---- installed dependencies ---------------------------------------
    Known {
        name: "node_modules",
        role: ArtifactRole::InstalledDependencies,
        tool: "npm / pnpm / yarn",
        consequence: "reinstall with `npm ci` (or `pnpm install`) -- needs registry access",
        ambiguous: false,
    },
    Known {
        name: "bower_components",
        role: ArtifactRole::InstalledDependencies,
        tool: "bower",
        consequence: "reinstall with `bower install` -- needs registry access",
        ambiguous: false,
    },
    Known {
        name: "jspm_packages",
        role: ArtifactRole::InstalledDependencies,
        tool: "jspm",
        consequence: "reinstall with `jspm install` -- needs registry access",
        ambiguous: false,
    },
    Known {
        name: "web_modules",
        role: ArtifactRole::InstalledDependencies,
        tool: "snowpack",
        consequence: "reinstall with `snowpack` -- needs registry access",
        ambiguous: false,
    },
];

/// Loose metadata files a Node toolchain leaves at a project root.
/// Files, not directories, so they never carry a folded measurement and
/// are statted individually.
const KNOWN_FILES: &[(&str, &str, &str)] = &[
    (
        "tsconfig.tsbuildinfo",
        "TypeScript",
        "the next `tsc --build` is a full type-check rather than an incremental one",
    ),
    (
        ".eslintcache",
        "ESLint",
        "the next `eslint` run re-lints every file rather than only changed ones",
    ),
];

/// Names inside `node_modules` that are not packages.
const NODE_MODULES_SPECIALS: &[(&str, ArtifactRole, &str, &str)] = &[
    (
        ".pnpm",
        ArtifactRole::SharedStoreEntry,
        "pnpm",
        "reinstall with `pnpm install`; the entries are hardlinks into pnpm's content-addressed \
         store, so removing them frees space only where the store copy is also gone",
    ),
    (
        ".cache",
        ArtifactRole::Intermediate,
        "per-tool cache under node_modules",
        "the owning tool rebuilds its cache on the next run",
    ),
    (
        ".bin",
        ArtifactRole::Metadata,
        "npm / pnpm / yarn",
        "reinstall recreates the executable shims",
    ),
    (
        ".vite",
        ArtifactRole::Intermediate,
        "Vite",
        "the next dev server start re-optimizes dependencies",
    ),
];

/// Shared-store layouts this adapter identifies, by the directory name
/// found immediately inside the store root.
const STORE_ENTRIES: &[(&str, &str)] = &[
    (
        "content-v2",
        "npm cache content, addressed by integrity hash",
    ),
    ("index-v5", "npm cache index entries"),
    ("tmp", "npm cache scratch space"),
    ("files", "pnpm content-addressed objects"),
    ("v3", "pnpm store, layout version 3"),
    ("v10", "pnpm store, layout version 10"),
];

impl BuildAdapter for Adapter {
    fn trash_roles(&self) -> &'static [ArtifactRole] {
        &[
            ArtifactRole::Output,
            ArtifactRole::TestOutput,
            ArtifactRole::Intermediate,
        ]
    }
    fn id(&self) -> &'static str {
        "node"
    }

    fn name(&self) -> &'static str {
        "Node.js"
    }

    fn capabilities(&self) -> BuildCapabilities {
        BuildCapabilities {
            identifies_shared_stores: true,
            attributes_package_identity: true,
            actions_available: true,
        }
    }

    fn store_kinds(&self) -> &'static [crate::locations::BuildStoreKind] {
        use crate::locations::BuildStoreKind;
        &[BuildStoreKind::NpmCache, BuildStoreKind::PnpmStore]
    }

    fn containers(&self, project_root: &Path, candidates: &[PathBuf]) -> Vec<BuildContainer> {
        if !crate::fs_gate::is_file(project_root.join("package.json")) {
            return Vec::new();
        }
        // An ambiguous name (`build`, `out`, `.cache`) belongs to
        // whichever ecosystem's marker is actually at this root. A
        // Gradle project that also has a package.json keeps its
        // `build/` -- the ecosystem catalog's own rule, applied here so
        // two adapters never both claim one directory.
        let jvm = [
            "pom.xml",
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
        ]
        .iter()
        .any(|m| crate::fs_gate::is_file(project_root.join(m)));
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut claim = |path: PathBuf| {
            if seen.insert(path.clone()) && crate::fs_gate::is_dir(&path) {
                out.push(BuildContainer::project(
                    "node",
                    path,
                    project_root.to_path_buf(),
                ));
            }
        };
        for name in KNOWN.iter().filter(|k| !(k.ambiguous && jvm)) {
            claim(project_root.join(name.name));
        }
        // `.vercel/output` is the one two-segment layout; claiming
        // `.vercel` itself would pull in the project link metadata
        // beside it, which is not a build artifact.
        claim(project_root.join(".vercel/output"));
        for c in candidates {
            let Some(name) = c.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if KNOWN
                .iter()
                .any(|k| k.name == name && !(k.ambiguous && jvm))
            {
                claim(c.clone());
            }
        }
        // The project root carries the loose metadata files. Claimed
        // last and only when one of them exists, so an ordinary Node
        // project does not get an empty extra container.
        if KNOWN_FILES
            .iter()
            .any(|(n, _, _)| crate::fs_gate::is_file(project_root.join(n)))
        {
            out.push(BuildContainer::project(
                "node",
                project_root.to_path_buf(),
                project_root.to_path_buf(),
            ));
        }
        out
    }

    fn identify(&self, container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
        if container.shared {
            return identify_shared_store(container, ctx);
        }
        let name = container
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if container.project_root.as_deref() == Some(container.path.as_path()) {
            return identify_loose_metadata(container, ctx);
        }
        match name {
            "node_modules" => identify_node_modules(container, ctx),
            _ => identify_output_tree(container, ctx),
        }
    }
}

fn known_for(name: &str) -> Option<&'static Known> {
    KNOWN.iter().find(|k| k.name == name)
}

/// The container row itself: role, tool, consequence, folded bytes.
fn container_unit(container: &BuildContainer, ctx: &BuildCtx) -> NestedArtifact {
    let name = container
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let known = known_for(name);
    let role = known
        .map(|k| k.role.clone())
        .unwrap_or(ArtifactRole::Output);
    let mut b = NestedUnitBuilder::new(container, role, container.path.clone()).is_dir(true);
    if let Some(d) = ctx.folded().get(&container.path) {
        b = b.folded(d);
    } else {
        b = b.limit("this directory was not measured by the walk this pass");
    }
    match known {
        Some(k) => {
            b = b
                .supported_with_reason(format!("`{}` is {}'s documented directory", k.name, k.tool))
                .evidence(
                    "node-layout",
                    format!("produced by {}", k.tool),
                    Confidence::Medium,
                )
                .consequence(k.consequence);
        }
        None => {
            b = b.unsupported_layout(format!(
                "`{name}` is not a directory name this adapter identifies"
            ));
        }
    }
    b.build()
}

/// A build-output directory: the container plus the sub-directories that
/// are a *different* thing from their parent.
///
/// `.next/cache` is the case that matters. It sits inside a build output
/// but is an incremental cache, with a different consequence -- removing
/// the output means a rebuild, removing the cache means a *slower*
/// rebuild -- and rolling it into `.next` would hide the fact that most
/// of `.next` is often the cache.
fn identify_output_tree(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![container_unit(container, ctx)];
    let name = container
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    for child in ctx.folded().children(&container.path) {
        let child_name = child
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let is_framework_cache =
            (name == ".next" || name == ".nuxt" || name == ".angular") && child_name == "cache";
        if !is_framework_cache {
            continue;
        }
        units.push(
            NestedUnitBuilder::new(container, ArtifactRole::Intermediate, child.path.clone())
                .folded(child)
                .supported_with_reason(format!(
                    "`{name}/cache` is the framework's incremental cache, not part of the served \
                     output"
                ))
                .evidence(
                    "node-layout",
                    format!("incremental cache inside {name}"),
                    Confidence::Medium,
                )
                .consequence(
                    "the next build recompiles what it would otherwise have replayed; the served \
                     output is unaffected",
                )
                .build(),
        );
    }
    units
}

/// The loose per-tool metadata files at a project root.
fn identify_loose_metadata(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = Vec::new();
    for (name, tool, consequence) in KNOWN_FILES {
        let path = container.path.join(name);
        let Some(meta) = ctx.stat(&path) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        units.push(
            NestedUnitBuilder::new(container, ArtifactRole::Metadata, path)
                .from_file_metadata(&meta)
                .supported_with_reason(format!("`{name}` is {tool}'s own incremental state file"))
                .evidence(
                    "node-layout",
                    format!("written by {tool}"),
                    Confidence::High,
                )
                .consequence(*consequence)
                .build(),
        );
    }
    units
}

/// `node_modules`: one unit per top-level entry, with package identity
/// read from the package's own manifest for the largest entries.
fn identify_node_modules(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut units = vec![container_unit(container, ctx)];
    let mut packages: Vec<(PathBuf, super::FoldedDir)> = Vec::new();

    for child in ctx.folded().children(&container.path) {
        let name = child
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if let Some((_, role, tool, consequence)) =
            NODE_MODULES_SPECIALS.iter().find(|(n, _, _, _)| *n == name)
        {
            let shared = *role == ArtifactRole::SharedStoreEntry;
            let mut b = NestedUnitBuilder::new(container, role.clone(), child.path.clone())
                .folded(child)
                .supported_with_reason(format!("`node_modules/{name}` is {tool}'s own directory"))
                .evidence(
                    "node-layout",
                    format!("maintained by {tool}"),
                    Confidence::Medium,
                )
                .consequence(*consequence);
            if shared {
                // pnpm hardlinks every file here from its global store.
                // The walk charges each inode once, wherever it first
                // met it, so this subtree's bytes may already be charged
                // to the store -- and claiming an exclusive total here
                // is exactly the double count #65 forbids.
                b = b
                    .membership(Membership::SharedHardlink)
                    .limit(
                        "entries are hardlinks into pnpm's content-addressed store; the bytes are \
                         charged once and which copy holds the charge is not recorded",
                    )
                    .no_action_because(
                        "these files are hardlinks shared with pnpm's store and possibly other \
                         projects",
                    );
            }
            units.push(b.build());
            continue;
        }
        if name.starts_with('@') {
            // A scope directory is not a package; its children are.
            for pkg in ctx.folded().children(&child.path) {
                packages.push((pkg.path.clone(), (*pkg).clone()));
            }
            continue;
        }
        if name.starts_with('.') {
            units.push(
                NestedUnitBuilder::new(container, ArtifactRole::Residual, child.path.clone())
                    .folded(child)
                    .unsupported_layout(format!(
                        "`node_modules/{name}` is not an entry this adapter identifies"
                    ))
                    .build(),
            );
            continue;
        }
        packages.push((child.path.clone(), (*child).clone()));
    }

    // Identity for the largest entries first: those are the ones a size
    // decision is about, and the budget is what keeps a monorepo's
    // forty thousand packages from costing forty thousand reads.
    packages.sort_by_key(|a| std::cmp::Reverse(a.1.allocated_total));
    let budget = PACKAGE_IDENTITY_BUDGET.min(packages.len());
    let skipped = packages.len() - budget;
    for (i, (path, dir)) in packages.into_iter().enumerate() {
        let read_identity = i < budget;
        let identity = read_identity
            .then(|| package_identity(ctx, &path))
            .flatten();
        let mut variant = ArtifactVariant::default();
        let mut b =
            NestedUnitBuilder::new(container, ArtifactRole::InstalledDependencies, path.clone())
                .folded(&dir)
                .supported_with_reason("a top-level entry of an npm-layout node_modules tree")
                .consequence(
                    "reinstall with `npm ci` (or `pnpm install`) -- needs registry access",
                );
        match identity {
            Some((name, version)) => {
                variant.package = Some(name.clone());
                variant.version = version.clone();
                b = b.evidence(
                    "node-package-json",
                    match &version {
                        Some(v) => format!("{name}@{v}"),
                        None => format!("{name}, no version declared"),
                    },
                    Confidence::High,
                );
                if version.is_none() {
                    variant.unknowns.push("version".into());
                }
            }
            None => {
                variant.unknowns.push("package".into());
                variant.unknowns.push("version".into());
                b = b.limit(if read_identity {
                    "this package's package.json could not be read or parsed; its identity is \
                     unknown and is not guessed from the directory name"
                        .to_string()
                } else {
                    format!(
                        "identity not read: only the {PACKAGE_IDENTITY_BUDGET} largest entries \
                         get a manifest read"
                    )
                });
            }
        }
        // Never a build generation: npm records none, and neither a
        // timestamp nor a hash establishes one install supersedes
        // another.
        variant.unknowns.push("build-generation".into());
        units.push(b.variant(variant).build());
    }
    if skipped > 0
        && let Some(root) = units.first_mut()
    {
        *root = NestedUnitBuilder::amend(root.clone())
            .limit(format!(
                "{skipped} of this tree's top-level entries were sized but not identified: only \
                 the {PACKAGE_IDENTITY_BUDGET} largest get a manifest read"
            ))
            .build();
    }
    units
}

/// A package's declared identity, from its own manifest.
///
/// Never the directory's basename: a directory called `lodash` holding a
/// package that calls itself something else is reported as what it calls
/// itself, and an unreadable manifest is an unknown identity.
fn package_identity(ctx: &BuildCtx, dir: &Path) -> Option<(String, Option<String>)> {
    let manifest = ctx.manifest(&dir.join("package.json"))?;
    if manifest.truncated {
        return None;
    }
    let json: serde_json::Value = serde_json::from_str(&manifest.text).ok()?;
    let name = json.get("name")?.as_str()?.to_string();
    let version = json
        .get("version")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Some((name, version))
}

/// npm's `_cacache` or pnpm's content-addressed store.
fn identify_shared_store(container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
    let mut b = NestedUnitBuilder::new(
        container,
        ArtifactRole::SharedStoreEntry,
        container.path.clone(),
    )
    .is_dir(true)
    .supported_with_reason("a package-manager store shared by every project on this machine")
    .evidence("node-store", "shared package store", Confidence::Medium)
    .consequence(
        "packages are downloaded again on the next install -- needs registry access; projects \
         whose node_modules hardlink into this store keep working until they are reinstalled",
    )
    .no_action_because(
        "this store is shared across projects and swamp cannot tell which projects link into it",
    );
    if let Some(d) = ctx.folded().get(&container.path) {
        b = b.folded(d);
    } else {
        b = b.limit("this store was not measured by the walk this pass");
    }
    let mut units = vec![b.build()];
    for child in ctx.folded().children(&container.path) {
        let name = child
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let known = STORE_ENTRIES.iter().find(|(n, _)| *n == name);
        let rel = relative_path(&container.path, &child.path);
        let mut cb = NestedUnitBuilder::new(
            container,
            ArtifactRole::SharedStoreEntry,
            child.path.clone(),
        )
        .folded(child)
        .membership(Membership::SharedHardlink)
        .no_action_because(
            "a store entry may be hardlinked into any number of projects' node_modules",
        );
        cb = match known {
            Some((_, what)) => cb
                .supported_with_reason(format!("`{rel}` is {what}"))
                .consequence(
                    "the next install downloads this content again -- needs registry access",
                ),
            None => cb.unsupported_layout(format!(
                "`{rel}` is not a store layout this adapter identifies"
            )),
        };
        units.push(cb.build());
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{RoleFamily, TimeSource};
    use crate::build_adapters::{ContainerCache, FoldedDir, FoldedIndex};
    use crate::fs_events::EventCoverage;
    use std::fs;

    fn index(entries: &[(&PathBuf, u64, u64)]) -> FoldedIndex {
        FoldedIndex::from_dirs(entries.iter().map(|(p, b, m)| FoldedDir {
            path: (*p).clone(),
            allocated_total: *b,
            mtime_max: *m,
            complete: true,
        }))
    }

    fn run(c: &BuildContainer, idx: &FoldedIndex) -> Vec<NestedArtifact> {
        let none = EventCoverage::untrusted();
        let cache = ContainerCache::disabled();
        let ctx = BuildCtx::new(1_000, idx, &none, &cache);
        Adapter.identify(c, &ctx)
    }

    fn project(name: &str, root: &Path) -> BuildContainer {
        BuildContainer::project("node", root.join(name), root.to_path_buf())
    }

    #[test]
    fn unknown_layout_is_explicit_not_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let nm = tmp.path().join("node_modules");
        let weird = nm.join(".rollup.cache");
        fs::create_dir_all(&weird).unwrap();
        let c = project("node_modules", tmp.path());
        let idx = index(&[(&nm, 100, 500), (&weird, 40, 500)]);
        let units = run(&c, &idx);
        let residual = units
            .iter()
            .find(|u| u.path == weird)
            .expect("an unrecognised entry is still a row");
        assert!(!residual.coverage.supported);
        assert_eq!(residual.role, ArtifactRole::Residual);
        assert!(
            residual
                .coverage
                .limits
                .iter()
                .any(|l| l.contains(".rollup.cache")),
            "the limit names the entry: {:?}",
            residual.coverage.limits
        );
    }

    #[test]
    fn identification_reads_no_more_than_manifest_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let nm = tmp.path().join("node_modules");
        let pkg = nm.join("huge");
        fs::create_dir_all(&pkg).unwrap();
        // A package.json far larger than the cap. Identification stops
        // at the cap and reports an unknown identity rather than
        // parsing a prefix.
        fs::write(
            pkg.join("package.json"),
            format!(
                "{{\"name\":\"huge\",\"pad\":\"{}\"}}",
                "x".repeat(super::super::bounded_io::MAX_MANIFEST_BYTES * 2)
            ),
        )
        .unwrap();
        let c = project("node_modules", tmp.path());
        let idx = index(&[(&nm, 100, 500), (&pkg, 90, 500)]);
        let (units, counted) = crate::work_counters::measured(|| run(&c, &idx));
        assert!(
            counted.header_bytes_read <= super::super::bounded_io::MAX_MANIFEST_BYTES as u64,
            "identification read {} bytes; the cap is {}",
            counted.header_bytes_read,
            super::super::bounded_io::MAX_MANIFEST_BYTES
        );
        let u = units.iter().find(|u| u.path == pkg).unwrap();
        assert_eq!(
            u.variant.package, None,
            "a manifest that hit the cap yields an unknown identity, never a prefix parse"
        );
        assert!(u.variant.unknowns.iter().any(|x| x == "package"));
    }

    #[test]
    fn no_project_or_build_code_is_executed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(
            root.join("package.json"),
            b"{\"scripts\":{\"build\":\"exit 7\"}}",
        )
        .unwrap();
        // The two things an adapter would be tempted to run.
        fs::write(root.join("next.config.js"), b"module.exports = {}\n").unwrap();
        let nm = root.join("node_modules");
        fs::create_dir_all(nm.join(".bin")).unwrap();
        fs::write(nm.join(".bin/next"), b"#!/bin/sh\nexit 7\n").unwrap();
        let c = project("node_modules", root);
        let idx = index(&[(&nm, 100, 500), (&nm.join(".bin"), 10, 500)]);
        let (_units, counted) = crate::work_counters::measured(|| run(&c, &idx));
        assert_eq!(
            counted.subprocess_spawns, 0,
            "identification spawned a process; npm is never run and no JS config is loaded"
        );
    }

    #[test]
    fn variants_never_collapse_by_basename() {
        // Two workspace packages each with a `dist`. A name-only
        // grouper reports one `dist`; the identity is the path inside
        // its own container, so these are two units with two ids.
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("packages/a");
        let b = tmp.path().join("packages/b");
        fs::create_dir_all(a.join("dist")).unwrap();
        fs::create_dir_all(b.join("dist")).unwrap();
        let ca = project("dist", &a);
        let cb = project("dist", &b);
        let ua = run(&ca, &index(&[(&a.join("dist"), 10, 500)]));
        let ub = run(&cb, &index(&[(&b.join("dist"), 20, 500)]));
        assert_ne!(
            ua[0].id, ub[0].id,
            "two workspaces' dist directories are two identities"
        );
        assert_eq!(ua[0].role, ArtifactRole::Output);
        assert_ne!(ua[0].bytes, ub[0].bytes);
    }

    #[test]
    fn age_is_not_obsolescence() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let dist = root.join("dist");
        fs::create_dir_all(&dist).unwrap();
        let c = project("dist", root);
        let units = run(&c, &index(&[(&dist, 1_000, 1)]));
        let u = &units[0];
        assert_eq!(u.time_source, TimeSource::FoldedDirectoryModification);
        assert_eq!(
            u.action,
            crate::artifact::NestedActionCapability::InspectionOnly
        );
        assert!(u.consequence.as_deref().unwrap().contains("rebuild"));
        // The verdict-vocabulary scan over every string this unit can
        // put in front of a person lives in
        // `crates/core/tests/build_adapter_contract.rs::no_unit_renders_a_verdict`.
        // It is not repeated here, because spelling the banned words in
        // `crates/core/src` is itself what `scripts/check.sh`'s grep
        // audit rejects -- and a weaker copy of a stronger check is not
        // worth the exemption.
    }

    #[test]
    fn a_package_is_named_by_its_manifest_not_its_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let nm = tmp.path().join("node_modules");
        let dir = nm.join("lodash");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("package.json"),
            br#"{"name":"lodash-es","version":"4.17.21"}"#,
        )
        .unwrap();
        let c = project("node_modules", tmp.path());
        let units = run(&c, &index(&[(&nm, 100, 500), (&dir, 90, 500)]));
        let u = units.iter().find(|u| u.path == dir).unwrap();
        assert_eq!(u.variant.package.as_deref(), Some("lodash-es"));
        assert_eq!(u.variant.version.as_deref(), Some("4.17.21"));
    }

    #[test]
    fn a_malformed_manifest_leaves_the_identity_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let nm = tmp.path().join("node_modules");
        let dir = nm.join("broken");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("package.json"), b"{ this is not json").unwrap();
        let c = project("node_modules", tmp.path());
        let units = run(&c, &index(&[(&nm, 100, 500), (&dir, 90, 500)]));
        let u = units.iter().find(|u| u.path == dir).unwrap();
        assert_eq!(u.variant.package, None);
        assert!(
            u.coverage
                .limits
                .iter()
                .any(|l| l.contains("is not guessed from the directory name")),
            "{:?}",
            u.coverage.limits
        );
    }

    #[test]
    fn a_pnpm_virtual_store_is_not_charged_twice() {
        let tmp = tempfile::tempdir().unwrap();
        let nm = tmp.path().join("node_modules");
        let pnpm = nm.join(".pnpm");
        fs::create_dir_all(&pnpm).unwrap();
        let c = project("node_modules", tmp.path());
        let units = run(&c, &index(&[(&nm, 500, 500), (&pnpm, 480, 500)]));
        let u = units.iter().find(|u| u.path == pnpm).unwrap();
        assert_eq!(u.membership, Membership::SharedHardlink);
        assert_eq!(u.physical_total, 0, "a shared subtree carries no charge");
        assert!(matches!(
            u.action,
            crate::artifact::NestedActionCapability::Unsupported { .. }
        ));
    }

    #[test]
    fn a_framework_cache_is_separated_from_the_output_it_sits_in() {
        let tmp = tempfile::tempdir().unwrap();
        let next = tmp.path().join(".next");
        let cache = next.join("cache");
        fs::create_dir_all(&cache).unwrap();
        let c = project(".next", tmp.path());
        let units = run(&c, &index(&[(&next, 1_000, 500), (&cache, 900, 500)]));
        let out = units.iter().find(|u| u.path == next).unwrap();
        let cu = units.iter().find(|u| u.path == cache).unwrap();
        assert_eq!(out.role, ArtifactRole::Output);
        assert_eq!(cu.role, ArtifactRole::Intermediate);
        assert_eq!(cu.role.family(), RoleFamily::Intermediates);
        assert_ne!(
            out.consequence, cu.consequence,
            "removing the output means a rebuild; removing the cache means a slower one"
        );
    }

    #[test]
    fn an_ambiguous_directory_goes_to_the_ecosystem_whose_marker_is_present() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("package.json"), b"{}").unwrap();
        fs::write(root.join("build.gradle"), b"").unwrap();
        fs::create_dir_all(root.join("build")).unwrap();
        fs::create_dir_all(root.join("dist")).unwrap();
        let claimed: Vec<String> = Adapter
            .containers(root, &[])
            .into_iter()
            .map(|c| c.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(claimed.contains(&"dist".to_string()));
        assert!(
            !claimed.contains(&"build".to_string()),
            "a Gradle marker at the root means `build/` is Gradle's: {claimed:?}"
        );
    }

    #[test]
    fn a_workspace_packages_own_output_root_is_claimed_from_the_walks_candidates() {
        // A monorepo's outputs are not at the project root. The walk
        // classified `packages/ui/dist` as an artifact; the adapter
        // claims it from that candidate list rather than by guessing
        // where a bundler was configured to write -- which it could only
        // learn by loading a JavaScript config.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("package.json"), b"{}").unwrap();
        let nested = root.join("packages/ui/dist");
        fs::create_dir_all(&nested).unwrap();
        let claimed: Vec<PathBuf> = Adapter
            .containers(root, &[nested.clone()])
            .into_iter()
            .map(|c| c.path)
            .collect();
        assert!(
            claimed.contains(&nested),
            "a custom output root the walk found is claimed: {claimed:?}"
        );
    }

    #[test]
    fn old_output_beside_current_dependencies_is_reported_as_two_separate_ages() {
        // The tempting inference: `dist` is months older than
        // `node_modules`, so `dist` must be superseded. It is not --
        // nobody may have rebuilt because nobody changed the source.
        // Both ages are reported, and neither unit's role, coverage or
        // action changes because of the other's timestamp.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let dist = root.join("dist");
        let nm = root.join("node_modules");
        fs::create_dir_all(&dist).unwrap();
        fs::create_dir_all(&nm).unwrap();
        let old = 1_000u64;
        let recent = 900_000u64;
        let dist_units = run(&project("dist", root), &index(&[(&dist, 5_000, old)]));
        let nm_units = run(
            &project("node_modules", root),
            &index(&[(&nm, 90_000, recent)]),
        );
        let d = &dist_units[0];
        let n = &nm_units[0];
        assert_eq!(d.mtime_max, old);
        assert_eq!(n.mtime_max, recent);
        assert_eq!(
            d.action,
            crate::artifact::NestedActionCapability::InspectionOnly,
            "being older than the dependency tree changes nothing about what swamp can do"
        );
        assert!(d.coverage.supported && n.coverage.supported);
        assert_eq!(
            d.consequence.as_deref(),
            Some("rebuild with this project's build script (commonly `npm run build`)"),
            "the consequence is the role's, not a comparison with a sibling"
        );
    }

    #[test]
    fn an_incompletely_measured_tree_says_so_and_stays_identified() {
        let tmp = tempfile::tempdir().unwrap();
        let nm = tmp.path().join("node_modules");
        fs::create_dir_all(&nm).unwrap();
        let partial = FoldedIndex::from_dirs([FoldedDir {
            path: nm.clone(),
            allocated_total: 1_000,
            mtime_max: 500,
            complete: false,
        }]);
        let units = run(&project("node_modules", tmp.path()), &partial);
        let u = &units[0];
        assert!(!u.coverage.complete);
        assert!(
            u.coverage
                .limits
                .iter()
                .any(|l| l.contains("could not read all of this directory")),
            "an incomplete measurement is named, not silently reported as a total: {:?}",
            u.coverage.limits
        );
        assert!(
            u.coverage.supported,
            "an incomplete measurement is not an unsupported layout; they are different facts"
        );
    }

    #[test]
    fn no_build_generation_is_invented() {
        let tmp = tempfile::tempdir().unwrap();
        let nm = tmp.path().join("node_modules");
        let dir = nm.join("pkg");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("package.json"),
            br#"{"name":"pkg","version":"1.0.0"}"#,
        )
        .unwrap();
        let c = project("node_modules", tmp.path());
        let units = run(&c, &index(&[(&nm, 100, 500), (&dir, 90, 500)]));
        let u = units.iter().find(|u| u.path == dir).unwrap();
        assert!(
            u.variant.unknowns.iter().any(|x| x == "build-generation"),
            "npm records no build generation, and the unit says so: {:?}",
            u.variant.unknowns
        );
    }
}
