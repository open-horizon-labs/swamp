//! The build-adapter capability matrix: a checked code artifact, not a
//! prose claim (#64: "a checked coverage matrix"; #68: "a checked
//! per-tool capability matrix").
//!
//! Every ecosystem family swamp's own catalog can produce build
//! artifacts for gets a row here, including the ones no adapter
//! implements yet. That is deliberate. A matrix listing only what is
//! implemented answers "what does swamp support?" with a list that looks
//! complete; a matrix listing every family with a status answers it with
//! the truth, and the gap is visible to a user deciding whether swamp
//! can explain their disk.
//!
//! The registry, this table and `docs/build-artifacts.md` are checked
//! against each other in both directions
//! (`.oh/guardrails/build-adapter-matrix-matches-docs.md`).

use crate::artifact::RoleFamily;

/// Whether a family has an adapter behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// An adapter is registered and tested.
    Implemented,
    /// The family is recognised by the ecosystem catalog and its
    /// containers are measured as whole artifact rows, but nothing
    /// identifies their interior yet.
    Planned,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Implemented => "implemented",
            Self::Planned => "planned",
        }
    }
}

/// One ecosystem family's published capability claim.
#[derive(Debug, Clone)]
pub struct MatrixEntry {
    /// Equal to the adapter id when `status` is
    /// [`Status::Implemented`]; a family name otherwise.
    pub id: &'static str,
    pub name: &'static str,
    pub status: Status,
    /// Role families this adapter identifies. Empty for a planned row.
    pub families: &'static [RoleFamily],
    /// The layouts and versions the fixtures actually cover. Not "all
    /// versions": a claim here is a claim a test backs.
    pub known_layouts: &'static [&'static str],
    /// What this adapter cannot attribute, in its own words. #64 asks
    /// for attribution limits as a first-class column because "supported"
    /// with no limits is the claim nobody can keep.
    pub attribution_limits: &'static [&'static str],
    /// The granularity at which an operation could ever be offered --
    /// separate from whether one is offered, which is `actions` below.
    pub operation_granularity: &'static str,
    /// Supported operation and its limits; identification alone is not an action.
    pub actions: &'static str,
}

const INSPECTION_ONLY: &str = "inspection only";
const PROJECT_TRASH: &str = "TUI Trash for project-local outputs, test output and intermediates; shared stores and installations inspection only";

pub const MATRIX: &[MatrixEntry] = &[
    MatrixEntry {
        id: "cargo",
        name: "Rust / Cargo",
        status: Status::Implemented,
        families: &[
            RoleFamily::Container,
            RoleFamily::Outputs,
            RoleFamily::Tests,
            RoleFamily::Intermediates,
            RoleFamily::Dependencies,
            RoleFamily::Metadata,
            RoleFamily::Residual,
        ],
        known_layouts: &[
            "target/<profile>/",
            "target/<target-triple>/<profile>/",
            "deps/, examples/, incremental/, build/, .fingerprint/",
            "custom target-dir/build-dir from .cargo/config[.toml] or CARGO_TARGET_DIR",
        ],
        attribution_limits: &[
            "Cargo's intermediate layout is an implementation detail and version-dependent",
            "ordinary scans fold dependencies; explicit inspect-cargo reports evidenced targets/variants, not inferred package identities",
            "a command-line --target-dir override is invisible to an observer",
        ],
        operation_granularity: "profile directory, or one target's executable plus its fingerprint",
        actions: "TUI Cargo cleanup groups with fingerprint-aware membership",
    },
    MatrixEntry {
        id: "node",
        name: "Node.js",
        status: Status::Implemented,
        families: &[
            RoleFamily::Outputs,
            RoleFamily::Tests,
            RoleFamily::Intermediates,
            RoleFamily::Dependencies,
            RoleFamily::SharedStore,
            RoleFamily::Metadata,
            RoleFamily::Residual,
        ],
        known_layouts: &[
            "node_modules/ top-level packages, scoped @scope/name",
            "node_modules/.pnpm/ virtual store",
            "dist, build, out, .next, .nuxt, .svelte-kit, .output, .vercel/output, storybook-static",
            "coverage, .nyc_output, playwright-report, test-results",
            ".next/cache, .turbo, .parcel-cache, .cache, .vite, node_modules/.cache/<tool>",
            ".eslintcache, tsconfig.tsbuildinfo",
            "npm _cacache, pnpm content-addressable store",
        ],
        attribution_limits: &[
            "a package's identity comes from its own package.json name/version; a package.json \
             that cannot be read leaves the identity unknown",
            "workspace hoisting means a top-level package may be a dependency of any workspace \
             member, and which one is not recorded on disk",
            "pnpm store entries are content-addressed: which project links a given object is not \
             derivable from the object",
            "no build generation is inferred -- npm records none",
        ],
        operation_granularity: "one output directory, one cache directory, or one installed tree",
        actions: PROJECT_TRASH,
    },
    MatrixEntry {
        id: "gradle",
        name: "Gradle",
        status: Status::Implemented,
        families: &[
            RoleFamily::Outputs,
            RoleFamily::Tests,
            RoleFamily::Intermediates,
            RoleFamily::SharedStore,
            RoleFamily::Metadata,
            RoleFamily::Residual,
        ],
        known_layouts: &[
            "build/classes, build/libs, build/tmp, build/reports, build/test-results, \
             build/generated, build/intermediates",
            ".gradle/ project cache",
            "<gradle-user-home>/caches/{modules-2,jars-*,transforms-*,build-cache-*}",
            "<gradle-user-home>/wrapper/dists/<dist>-<hash>",
            "<gradle-user-home>/daemon/<version>",
            "<gradle-user-home>/caches/modules-2/metadata-*",
            "each of caches/, wrapper/dists/, daemon/, native/ as its own machine-wide store",
        ],
        attribution_limits: &[
            "build scripts and plugins are never evaluated, so a custom buildDir or a plugin's \
             own output directory is an unidentified residual",
            "a transforms-* or build-cache-* entry is keyed by a hash with no recorded inputs",
            "which project last wrote a shared cache entry is not recorded on disk",
        ],
        operation_granularity: "one project build directory, or one cache category directory",
        actions: PROJECT_TRASH,
    },
    MatrixEntry {
        id: "maven",
        name: "Maven",
        status: Status::Implemented,
        families: &[
            RoleFamily::Outputs,
            RoleFamily::Tests,
            RoleFamily::SharedStore,
            RoleFamily::Metadata,
            RoleFamily::Residual,
        ],
        known_layouts: &[
            "target/{classes,test-classes,generated-sources,surefire-reports,failsafe-reports,\
             site}",
            "target/*.jar, *.war",
            "<local-repository>/<group>/<artifact>/<version>/",
            "_remote.repositories, *.lastUpdated, maven-metadata-local.xml origin evidence",
        ],
        attribution_limits: &[
            "origin comes from _remote.repositories entries (a repository id means downloaded, an \
             empty id means installed by `mvn install`) or maven-metadata-local.xml; with neither, \
             or with only a *.lastUpdated attempt record, the origin is unknown and swamp never \
             promises a re-download",
            "POM property and parent-inherited versions are not resolved; an unresolved \
             ${property} is an explicit identity gap",
            "plugins are never evaluated, so a plugin's own output directory under target/ is an \
             unidentified residual",
        ],
        operation_granularity: "one project target directory, or one repository artifact version",
        actions: PROJECT_TRASH,
    },
    MatrixEntry {
        id: "android",
        name: "Android",
        status: Status::Implemented,
        families: &[
            RoleFamily::Outputs,
            RoleFamily::Tests,
            RoleFamily::Intermediates,
            RoleFamily::Installations,
            RoleFamily::State,
            RoleFamily::Metadata,
            RoleFamily::Residual,
        ],
        known_layouts: &[
            "<module>/build/ of a module with src/main/AndroidManifest.xml: the Gradle conventional \
             entries plus outputs/{apk/<flavor>/<buildType>,bundle/<variant>,mapping/<variant>,aar,logs}, \
             intermediates/<task>/<variant>, test-results/<task>, reports",
            ".cxx/<Variant>/<hash>/<abi>, .cxx/cmake/<variant>/<abi>, .externalNativeBuild/cmake/<variant>/<abi>",
            "<sdk>/{platforms,build-tools,system-images/<api>/<tag>/<abi>,emulator} with source.properties",
            "~/.android/avd/<name>.avd with config.ini and emulator lock files",
        ],
        attribution_limits: &[
            "variants come from the plugin's directory names (build type, flavor, test set); \
             build scripts are never evaluated, so a custom buildDir or a plugin's own output is \
             an unidentified residual",
            "a .cxx configuration hash is the plugin's key; its inputs are not recorded",
            "an R8 mapping matches only the build that wrote it; which release shipped is not \
             recorded",
            "SDK packages and AVDs are installations and device state, never build output",
        ],
        operation_granularity: "one module build directory, one native variant/ABI, one SDK \
                                package, or one AVD",
        actions: PROJECT_TRASH,
    },
    MatrixEntry {
        id: "python",
        name: "Python",
        status: Status::Implemented,
        families: &[
            RoleFamily::Outputs,
            RoleFamily::Tests,
            RoleFamily::Intermediates,
            RoleFamily::Dependencies,
            RoleFamily::SharedStore,
            RoleFamily::Installations,
            RoleFamily::Metadata,
            RoleFamily::Residual,
        ],
        known_layouts: &[
            "dist/*.whl, dist/*.tar.gz named by the wheel and sdist filename conventions",
            "build/{lib,lib.<plat>-<impl>-<ver>,temp.<plat>-<impl>-<ver>,bdist.<plat>,scripts}",
            "*.egg-info with PKG-INFO, .eggs/<name>-<ver>-py<X.Y>.egg",
            "__pycache__, .pytest_cache, .mypy_cache/<python>, .ruff_cache/<ruff>, .pytype, .pyre, \
             .hypothesis, htmlcov, .coverage",
            ".venv/venv/env and .tox/.nox envs: pyvenv.cfg, lib/pythonX.Y/site-packages with \
             *.dist-info (top_level.txt, direct_url.json)",
            "pip cache {http,http-v2,wheels,selfcheck}; uv cache {archive,wheels,built-wheels,\
             sdists,simple,git,interpreter,environments,builds}-vN with pypi/<name> and path/<hash>",
            "uv python/<impl>-<ver>-<os>-<arch>-<libc>, uv tools/<tool>",
        ],
        attribution_limits: &[
            "a wheel's or sdist's identity is its filename; the archive's METADATA inside it is not \
             opened",
            "a site-packages directory is named only through a distribution's top_level.txt \
             (the first 400 dist-info directories are read); an unclaimed one stays unknown",
            "no interpreter is started and no package imported; a missing interpreter is reported \
             from pyvenv.cfg's home",
            "uv links packages from its cache into environments; shared bytes are charged once, \
             where the walk first met them",
            "cache entries are keyed by hashes; which project needed one is not recorded",
        ],
        operation_granularity: "one output or cache directory, one distribution file, one \
                                environment, or one cache bucket",
        actions: PROJECT_TRASH,
    },
    MatrixEntry {
        id: "go",
        name: "Go",
        status: Status::Implemented,
        families: &[
            RoleFamily::Outputs,
            RoleFamily::Intermediates,
            RoleFamily::Dependencies,
            RoleFamily::SharedStore,
            RoleFamily::Installations,
            RoleFamily::Metadata,
            RoleFamily::Residual,
        ],
        known_layouts: &[
            "vendor/ with vendor/modules.txt",
            "bin/ beside go.mod; dist/<build>_<os>_<arch> beside a goreleaser config",
            "GOMODCACHE/<escaped module>@<version>/, golang.org/toolchain@<version>",
            "GOMODCACHE/cache/download/<escaped module>/@v/<version>.{zip,mod,info}, sumdb/",
            "GOCACHE/00..ff buckets, trim.txt, testexpire.txt",
        ],
        attribution_limits: &[
            "a build-cache entry's name is a hash of an action's inputs: it identifies no \
             package, no test binary and no build, and nothing on disk says a build will read \
             it again",
            "module identity comes from the escaped path; the go command is never run",
            "download versions are listed for the 2,000 largest modules; the rest are sized",
            "a binary in bin/ does not record which build or commit produced it",
        ],
        operation_granularity: "one output directory or binary, one module version, one \
                                build-cache bucket",
        actions: PROJECT_TRASH,
    },
    MatrixEntry {
        id: "xcode-swift",
        name: "Xcode / Swift",
        status: Status::Implemented,
        families: &[
            RoleFamily::Outputs,
            RoleFamily::Tests,
            RoleFamily::Intermediates,
            RoleFamily::Dependencies,
            RoleFamily::Installations,
            RoleFamily::State,
            RoleFamily::Metadata,
            RoleFamily::Residual,
        ],
        known_layouts: &[
            "DerivedData/<Project>-<hash>/ with info.plist WorkspacePath (XML or binary plist)",
            "Build/Products/<Configuration>-<platform>/{*.app,*.xctest,*.dSYM,*.framework,\
             *.swiftmodule}",
            "Build/Intermediates.noindex, Index.noindex, ModuleCache.noindex, Logs/Test/*.xcresult, \
             SourcePackages/{checkouts,repositories,artifacts} with workspace-state.json",
            "a project-local DerivedData or -derivedDataPath folder",
            "SwiftPM .build/{<triple>/<debug or release>,checkouts,repositories,artifacts,.lock}",
            "Archives/<date>/*.xcarchive with Info.plist; iOS/watchOS DeviceSupport/<version (build)>",
            "CoreSimulator Devices/<UDID>/device.plist, Profiles/Runtimes/*.simruntime, Caches",
        ],
        attribution_limits: &[
            "the source project is read from info.plist's WorkspacePath; a folder without one \
             keeps an unknown project, and the folder name's hash is not reversed",
            "a missing workspace is reported as a fact, never as a verdict on the build",
            "archives, device support, runtimes and simulator devices are never build output",
            "an active build is evidence only: a `.lock` file or a modification in the last ten \
             minutes",
            "build phases and package plugins are never run",
        ],
        operation_granularity: "one DerivedData project folder, one products configuration, \
                                one archive, one runtime, or one simulator device",
        actions: PROJECT_TRASH,
    },
    MatrixEntry {
        id: "docker-buildkit",
        name: "Docker / BuildKit",
        status: Status::Implemented,
        families: &[
            RoleFamily::Intermediates,
            RoleFamily::Metadata,
            RoleFamily::Residual,
        ],
        known_layouts: &[
            "daemon build-cache records from `docker system df -v --format json` (API 1.39+)",
            "per-builder records from `docker buildx du --verbose` (text or JSON)",
            "builders from `docker buildx ls --format json`; API version from `docker version`",
            "record types regular, source.local, source.git.checkout, exec.cachemount, frontend, \
             internal",
        ],
        attribution_limits: &[
            "every field is the daemon's: sizes are logical and per record (a parent's size is \
             never in a child's), times are the daemon's records, in-use/shared/reclaimable are \
             what it reported",
            "the host disk image holding the records is measured separately and never added",
            "which project's build produced a record is not recorded by the daemon",
            "an unavailable daemon, a missing buildx or an old API is stated, not treated as an \
             empty cache",
        ],
        operation_granularity: "daemon-side only: `docker buildx prune --filter id=<id>` \
                                removes a record with its dependents; `docker builder prune` \
                                removes every record not in use that matches its filters",
        actions: INSPECTION_ONLY,
    },
];

/// The ids of families with a registered adapter.
pub fn implemented_ids() -> Vec<&'static str> {
    MATRIX
        .iter()
        .filter(|e| e.status == Status::Implemented)
        .map(|e| e.id)
        .collect()
}

pub fn get(id: &str) -> Option<&'static MatrixEntry> {
    MATRIX.iter().find(|e| e.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_matrix_matches_adapter_contracts() {
        let registry = crate::build_adapters::registry::Registry::with_builtins();
        for e in MATRIX {
            assert_eq!(
                e.actions == PROJECT_TRASH,
                registry
                    .get(e.id)
                    .is_some_and(|a| !a.trash_roles().is_empty()),
                "{} action documentation differs from its contract",
                e.id
            );
        }
    }

    #[test]
    fn every_implemented_row_states_its_attribution_limits() {
        for e in MATRIX.iter().filter(|e| e.status == Status::Implemented) {
            assert!(
                !e.attribution_limits.is_empty(),
                "{} claims support with no stated limits; \"supported\" with no limits is the \
                 claim nobody can keep",
                e.id
            );
            assert!(!e.families.is_empty(), "{} identifies no families", e.id);
            assert!(!e.known_layouts.is_empty(), "{} names no layouts", e.id);
        }
    }

    #[test]
    fn ids_are_unique_and_kebab_case() {
        let mut ids: Vec<&str> = MATRIX.iter().map(|e| e.id).collect();
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate matrix id");
        for id in ids {
            assert!(
                id.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "matrix id {id} is not kebab-case; ids must match module names and docs rows"
            );
        }
    }
}
