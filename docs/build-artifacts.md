# Build artifacts

A `target/`, a `node_modules/`, a Gradle `build/` or a Maven `target/`
is one row in a swamp report and one number. That number answers "how
big", and nothing else. This document is the reference for the layer
that answers **what is inside it, how old each part is, and what it
would cost to get that part back** (#64, #65, #66, #67, #68, #69, #70,
#71) -- for project containers and, since 2026-09-22, for the
machine-wide stores beside them: a Maven repository, a Gradle home, npm
and pnpm stores, Go's module and build caches, pip and uv caches,
DerivedData, CoreSimulator, the Android SDK, and BuildKit's cache.

**Identification is not cleanup.** Adapters describe what they find;
the TUI can move supported project-local outputs, test output and
intermediates to Trash. Shared stores, installations and unknown layouts
remain inspection-only. Cargo uses its existing fingerprint-aware cleanup
groups. The checked matrix below states the boundaries per adapter.

## What an adapter may claim

- **What a directory is**: a generated output, a test or coverage
  report, an incremental cache, an installed dependency tree, an entry
  in a store shared across projects, tool metadata, or an unidentified
  residual.
- **When it was last modified**, with the source of that timestamp
  stated (one file's `mtime`, a folded directory's rolled-up newest
  entry, or a time the tool itself recorded).
- **How many bytes**, on one stated accounting basis -- allocated,
  logical, or unique-allocated. Bases are never mixed in one total.
- **What removing it would cost**, in the ecosystem's own words:
  "rebuild with `next build`", "reinstall with `npm ci` -- needs
  registry access", "a test rerun with coverage enabled regenerates it".

## What an adapter never claims

- That anything is unused, obsolete, superseded, stale or safe to
  remove. **Modification age is not use**, and a newer similarly-named
  file does not supersede an older one.
- A **build generation**. Neither npm, nor pnpm, nor Gradle, nor Maven
  records one, so swamp does not invent one from a timestamp or a hash.
- An identity it did not read. A package is named by its own manifest,
  never by its directory's basename; a Maven artifact's coordinates come
  from the repository layout, never from a filename.
- That an artifact can be downloaded again when no evidence says so.
  Maven's `unknown-origin` is a first-class answer.

## How identification is bounded

| Bound | Rule | Guardrail |
|---|---|---|
| No second traversal | Structure comes from the folded walk's own directory rows, or a capped `locations::shallow_list` | `build-adapters-do-not-traverse` |
| No unbounded reads | Content only through `bounded_io::read_manifest`, capped at 256 KiB, counted | `build-adapters-read-bounded-manifests-only` |
| No project code | No `npm`, `gradle`, `mvn` or `cargo` is ever run, and no JavaScript config is loaded | `build-adapters-are-inspection-only` |
| No stale replay | An unchanged container is replayed only under trusted FSEvents coverage, never a directory stamp | `build-adapters-reuse-under-event-coverage` |
| No overstated unit | Units are built through `NestedUnitBuilder`, whose defaults are unsupported coverage, unknown basis, unknown time source, inspection only | `build-units-built-through-builder` |
| No silent adapter | One line in `build_adapters::Registry::with_builtins()`, one matrix row, one row here | `build-adapters-are-pluggable` |
| Same five proofs each | `unknown_layout_is_explicit_not_empty`, `identification_reads_no_more_than_manifest_cap`, `no_project_or_build_code_is_executed`, `variants_never_collapse_by_basename`, `age_is_not_obsolescence` | `build-adapter-test-contract` |

## Role families

Every adapter's roles collapse into the same families, so a view can
group them without knowing which ecosystem produced a row.

| Family | What it holds | Typical consequence of removal |
|---|---|---|
| `outputs` | What a build produced | a rebuild |
| `tests` | Test and coverage output | a test rerun |
| `intermediates` | Caches kept to make the next build faster | a slower build, same result |
| `dependencies` | Installed dependency trees inside a project | a reinstall, which needs registry access |
| `shared-store` | Entries in a store shared across projects | a re-download, and other projects may link to the same bytes |
| `installations` | Installed SDKs, runtimes, interpreters and toolchains (an Android platform, a simulator runtime, a uv-managed Python, a Go toolchain download) | a reinstall, usually a download -- never a rebuild |
| `state` | Retained archives and mutable device state (an `.xcarchive`, a simulator device, an AVD) | nothing regenerates it: an archive's symbols or a device's data may be the only copy |
| `metadata` | What a tool recorded about a build | the tool rewrites it |
| `residual` | Present, measured, not identified | unknown |
| `unknown` | Not measured | unknown |

## Support matrix

| Adapter | Status | Families | Known layouts | Attribution limits | Operation granularity | Actions |
|---|---|---|---|---|---|---|
| `cargo` (Rust) | implemented | container, outputs, tests, intermediates, dependencies, metadata, residual | `target/<profile>/`, `target/<triple>/<profile>/`, `deps/`, `examples/`, `incremental/`, `build/`, `.fingerprint/`, custom `target-dir`/`build-dir` from `.cargo/config[.toml]` or `CARGO_TARGET_DIR` | Cargo's intermediate layout is version-dependent; ordinary scans fold dependencies; explicit inspect-cargo reports evidenced targets/variants, not inferred package identities; a command-line `--target-dir` is invisible to an observer | profile directory, or one target's executable plus its fingerprint | TUI Cargo cleanup groups with fingerprint-aware membership |
| `node` (Node.js) | implemented | outputs, tests, intermediates, dependencies, shared-store, metadata, residual | `node_modules/` top-level and `@scope/` packages, `node_modules/.pnpm`, `node_modules/.cache`, `dist`, `build`, `out`, `.next`, `.nuxt`, `.svelte-kit`, `.output`, `.vercel/output`, `storybook-static`, `out-tsc`, `coverage`, `.nyc_output`, `playwright-report`, `test-results`, `.next/cache`, `.turbo`, `.parcel-cache`, `.cache`, `.vite`, `.angular`, `.expo`, `.metro`, `tsconfig.tsbuildinfo`, `.eslintcache`, npm `_cacache`, pnpm store | a package's identity is its own `package.json` (the 200 largest top-level packages get a bounded read; the rest say so), and an unreadable one leaves it unknown; workspace hoisting means a top-level package's dependent member is not recorded on disk; pnpm store objects are content-addressed, so which project links one is not derivable; no build generation is inferred | one output directory, one cache directory, or one installed tree | TUI Trash for project-local outputs, test output and intermediates; shared stores and installations inspection only |
| `gradle` | implemented | outputs, tests, intermediates, shared-store, metadata, residual | `build/{classes,libs,distributions,resources,generated,intermediates,tmp,kotlin,reports,test-results}`, `.gradle/<version>/`, `<gradle-user-home>/caches/{modules-N,transforms-N,jars-N,build-cache-N}`, `modules-2/files-2.1/<group>/<artifact>`, `modules-2/metadata-*`, `<gradle-user-home>/wrapper/dists/`, `daemon/<version>/`, `native/`, `jdks/` | build scripts and plugins are never evaluated, so a reassigned `buildDir` or a plugin's own output directory is an unidentified residual; a `transforms-*`/`build-cache-*` entry is keyed by a hash whose inputs Gradle does not record; which project last wrote a shared cache entry is not recorded | one project build directory, or one cache category directory | TUI Trash for project-local outputs, test output and intermediates; shared stores and installations inspection only |
| `maven` | implemented | outputs, tests, shared-store, metadata, residual | `target/{classes,test-classes,generated-sources,generated-test-sources,surefire-reports,failsafe-reports,site,maven-status,maven-archiver,test-run-info}`, packaged `*.jar`/`*.war`/`*.ear`/`*.zip`/`*.aar`, `<local-repository>/<group>/<artifact>/<version>/` with `_remote.repositories`, `maven-metadata-local.xml` and `*.lastUpdated` origin evidence | origin is read from `_remote.repositories` entries and `maven-metadata-local.xml`; with neither (or only a `*.lastUpdated` attempt record) it is **unknown** and swamp never promises a re-download; POM properties and parent-inherited versions are not resolved, and an unresolved `${property}` version directory is an explicit residual; plugins are never evaluated, so a plugin's output under `target/` is an unidentified residual | one project target directory, or one repository artifact version | TUI Trash for project-local outputs, test output and intermediates; shared stores and installations inspection only |
| `android` (Android) | implemented | outputs, tests, intermediates, installations, state, metadata, residual | <module>/build/ of a module with src/main/AndroidManifest.xml: the Gradle conventional entries plus outputs/{apk/<flavor>/<buildType>,bundle/<variant>,mapping/<variant>,aar,logs}, intermediates/<task>/<variant>, test-results/<task>, reports; .cxx/<Variant>/<hash>/<abi>, .cxx/cmake/<variant>/<abi>, .externalNativeBuild/cmake/<variant>/<abi>; <sdk>/{platforms,build-tools,system-images/<api>/<tag>/<abi>,emulator} with source.properties; ~/.android/avd/<name>.avd with config.ini and emulator lock files | variants come from the plugin's directory names (build type, flavor, test set); build scripts are never evaluated, so a custom buildDir or a plugin's own output is an unidentified residual; a .cxx configuration hash is the plugin's key; its inputs are not recorded; an R8 mapping matches only the build that wrote it; which release shipped is not recorded; SDK packages and AVDs are installations and device state, never build output | one module build directory, one native variant/ABI, one SDK package, or one AVD | TUI Trash for project-local outputs, test output and intermediates; shared stores and installations inspection only |
| `python` (Python) | implemented | outputs, tests, intermediates, dependencies, shared-store, installations, metadata, residual | dist/*.whl, dist/*.tar.gz named by the wheel and sdist filename conventions; build/{lib,lib.<plat>-<impl>-<ver>,temp.<plat>-<impl>-<ver>,bdist.<plat>,scripts}; *.egg-info with PKG-INFO, .eggs/<name>-<ver>-py<X.Y>.egg; __pycache__, .pytest_cache, .mypy_cache/<python>, .ruff_cache/<ruff>, .pytype, .pyre, .hypothesis, htmlcov, .coverage; .venv/venv/env and .tox/.nox envs: pyvenv.cfg, lib/pythonX.Y/site-packages with *.dist-info (top_level.txt, direct_url.json); pip cache {http,http-v2,wheels,selfcheck}; uv cache {archive,wheels,built-wheels,sdists,simple,git,interpreter,environments,builds}-vN with pypi/<name> and path/<hash>; uv python/<impl>-<ver>-<os>-<arch>-<libc>, uv tools/<tool> | a wheel's or sdist's identity is its filename; the archive's METADATA inside it is not opened; a site-packages directory is named only through a distribution's top_level.txt (the first 400 dist-info directories are read); an unclaimed one stays unknown; no interpreter is started and no package imported; a missing interpreter is reported from pyvenv.cfg's home; uv links packages from its cache into environments; shared bytes are charged once, where the walk first met them; cache entries are keyed by hashes; which project needed one is not recorded | one output or cache directory, one distribution file, one environment, or one cache bucket | TUI Trash for project-local outputs, test output and intermediates; shared stores and installations inspection only |
| `go` (Go) | implemented | outputs, intermediates, dependencies, shared-store, installations, metadata, residual | vendor/ with vendor/modules.txt; bin/ beside go.mod; dist/<build>_<os>_<arch> beside a goreleaser config; GOMODCACHE/<escaped module>@<version>/, golang.org/toolchain@<version>; GOMODCACHE/cache/download/<escaped module>/@v/<version>.{zip,mod,info}, sumdb/; GOCACHE/00..ff buckets, trim.txt, testexpire.txt | a build-cache entry's name is a hash of an action's inputs: it identifies no package, no test binary and no build, and nothing on disk says a build will read it again; module identity comes from the escaped path; the go command is never run; download versions are listed for the 2,000 largest modules; the rest are sized; a binary in bin/ does not record which build or commit produced it | one output directory or binary, one module version, one build-cache bucket | TUI Trash for project-local outputs, test output and intermediates; shared stores and installations inspection only |
| `xcode-swift` (Xcode / Swift) | implemented | outputs, tests, intermediates, dependencies, installations, state, metadata, residual | DerivedData/<Project>-<hash>/ with info.plist WorkspacePath (XML or binary plist); Build/Products/<Configuration>-<platform>/{*.app,*.xctest,*.dSYM,*.framework,*.swiftmodule}; Build/Intermediates.noindex, Index.noindex, ModuleCache.noindex, Logs/Test/*.xcresult, SourcePackages/{checkouts,repositories,artifacts} with workspace-state.json; a project-local DerivedData or -derivedDataPath folder; SwiftPM .build/{<triple>/<debug or release>,checkouts,repositories,artifacts,.lock}; Archives/<date>/*.xcarchive with Info.plist; iOS/watchOS DeviceSupport/<version (build)>; CoreSimulator Devices/<UDID>/device.plist, Profiles/Runtimes/*.simruntime, Caches | the source project is read from info.plist's WorkspacePath; a folder without one keeps an unknown project, and the folder name's hash is not reversed; a missing workspace is reported as a fact, never as a verdict on the build; archives, device support, runtimes and simulator devices are never build output; an active build is evidence only: a `.lock` file or a modification in the last ten minutes; build phases and package plugins are never run | one DerivedData project folder, one products configuration, one archive, one runtime, or one simulator device | TUI Trash for project-local outputs, test output and intermediates; shared stores and installations inspection only |
| `docker-buildkit` (Docker / BuildKit) | implemented | intermediates, metadata, residual | daemon build-cache records from `docker system df -v --format json` (API 1.39+); per-builder records from `docker buildx du --verbose` (text or JSON); builders from `docker buildx ls --format json`; API version from `docker version`; record types regular, source.local, source.git.checkout, exec.cachemount, frontend, internal | every field is the daemon's: sizes are logical and per record (a parent's size is never in a child's), times are the daemon's records, in-use/shared/reclaimable are what it reported; the host disk image holding the records is measured separately and never added; which project's build produced a record is not recorded by the daemon; an unavailable daemon, a missing buildx or an old API is stated, not treated as an empty cache | daemon-side only: `docker buildx prune --filter id=<id>` removes a record with its dependents; `docker builder prune` removes every record not in use that matches its filters | inspection only |

Every family the catalog can produce build artifacts for now has an
implemented row. A **planned** row, if one is added, means the family's
containers are measured as whole artifact rows and nothing identifies
their interior -- a real gap, printed rather than omitted.

## Origin evidence: Maven's three answers

This is the one claim that decides whether removing something is
recoverable at all, so it is not guessed. The evidence is what Maven's
Resolver (the enhanced local repository manager) writes beside an
artifact:

| Evidence beside the version directory | Origin | What swamp says |
|---|---|---|
| `_remote.repositories` with a repository id (`lib-2.0.jar>central=`) | downloaded | "the next build that needs this version downloads it again from central -- needs access to that repository" |
| `_remote.repositories` with an **empty** id (`app-1.0.jar>=`), or `maven-metadata-local.xml` beside the version, or the artifact's `maven-metadata-local.xml` listing this version | locally installed | "this version was installed from a local build; recreating it needs that project's source and an `mvn install`" |
| only a `*.lastUpdated` file | unknown origin | the same as below, plus "a remote resolution attempt was recorded; that is not evidence the bytes here were downloaded" |
| nothing usable (absent, unparseable, or larger than the 256 KiB manifest cap) | unknown origin | "swamp found no origin evidence beside this artifact, so it cannot say whether it can be downloaded again" |

The presence of `_remote.repositories` alone proves nothing: the
Resolver writes it for `mvn install` too, with an empty repository id
(its `LOCAL_REPO_ID`). Where local and remote evidence are both present
for a version, **local** wins: reporting "downloaded" for an artifact
that only exists because somebody ran `mvn install` is the mistake that
costs an artifact. Cost: one capped listing per version directory, one
bounded read of `_remote.repositories` when present, and only when it is
absent one `stat` and at most one bounded read of the artifact-level
`maven-metadata-local.xml`.

## Shared stores and double counting

pnpm hardlinks its content-addressed objects into each project's
`node_modules/.pnpm`; npm's `_cacache`, Gradle's `modules-N` and Maven's
local repository are each shared by every project on the machine. The
walk charges each inode once, wherever it first met it, so these bytes
are already counted exactly once at the report level. Units inside a
shared store therefore:

- carry `shared-hardlink` (pnpm) or `unknown` membership and **no**
  physical charge, so a view can never add a project's copy to the
  store's own total;
- state that the linking projects are not derivable from the entry;
- declare no action, because the same bytes may be in use by a project
  swamp is not looking at.

## Machine-wide stores in the live report

The stores above are not beneath any checkout: they are detector-resolved
external locations (`swamp report --view external`), measured and
history-tracked by the external observation. Their interiors are joined
there, in the same pass, never from the per-root build consumer -- which
would observe the same bytes twice:

1. **Two declarations, nothing else.** A detector says which of its
   locations is which kind of store (`Detector::build_stores`, e.g. Go:
   the module cache is the ancestor of the `cache/download` location it
   derived; the build cache is its build-output location). An adapter
   says which kinds it identifies (`BuildAdapter::store_kinds`). The join
   compares the two and nothing else -- no detector id, no adapter id, no
   path suffix, so a custom `GOMODCACHE`, `GRADLE_USER_HOME`,
   `maven.repo.local` or DerivedData location is handled exactly like the
   default (`.oh/guardrails/build-stores-join-by-capability.md`).
2. **The walk the store needed anyway.** A store is measured through the
   one folded walk; when its units cannot be replayed, that walk keeps the
   per-directory rows it produces and hands them to the adapter. No
   second traversal.
3. **Replay under the same window.** An unchanged store's folded total
   is replayed under the pass's event window, and so are its identified
   units, from `associations/build_stores.parquet` (one row per unit and
   field, fingerprinted with the swamp version). Zero listings, zero
   stats, zero manifest bytes (measured below).
4. **Their own history, their own sweep.** Units inside a store keep
   size/presence/growth/regrowth on a separate key family
   (`build-store:` rows in the external current table), swept only
   inside stores this observation identified, never inside a unit it
   could not read completely, never inside an excluded region. A
   disabled, excluded or unreadable store, or the agent observation
   running in either order, changes nothing about them
   (`crates/core/tests/build_store_join.rs`). The store's own row is the
   external unit's, with its history.

**BuildKit** is the one store the daemon answers for. The Docker
consumer's already-fetched facts (bounded, allow-listed, cached for five
minutes) are handed to the BuildKit adapter per builder, and the records
appear under `--view docker` (text, `--json` as `buildkit`, and the TUI's
Docker view).

**Where it shows.** `swamp report --view external` prints each store's
family rows under its unit ("inside (identification only ...)"); `--json`
carries them as `interiors`, keyed by the unit's path, in the same shape
`--view builds --json` uses. In the TUI's External view a store's row
opens (`Enter`/`→`) onto the same family groups a project container
shows, every row inspection only.

## Collapsed family rows

A container's interior is summarized one row per family, and the rules
are the same in `--view builds`, its `--json` and the TUI:

- **Only nonempty supported candidates are counted.** A unit whose
  layout the adapter does not understand is reported once, as "Not
  identified"; an empty unit is counted as empty. Neither inflates a
  family.
- **Descendants are not counted twice.** The summary stops at the
  outermost unit: a `dist/` and the `dist/assets/` inside it are one
  candidate.
- **Bases are never mixed.** A family holding allocated and logical
  numbers reports no total rather than a wrong one.
- **Leaves reconcile to the container.** Whatever the outermost units do
  not account for -- the container's own loose files, entries no unit
  claims -- is reported as unaccounted bytes on the "Not identified" row.
  Members adding up to more than their container is reported as "not
  reconciled", never as a residual of zero.
- **The oldest known modification is not a last use**, and unknown ages
  are counted separately and sorted last, never ranked as ancient.
- Each row leads with review guidance derived from the family alone
  ("Start here: slower next build", "Review: reinstall from registry",
  "Shared: other projects may link", ...), then the largest member's
  consequence in the adapter's own words, and says how many other
  consequences the family holds.

## Incremental refresh

An unchanged container's units are replayed only when **both** hold:
the pass's trusted event window covers the container and reports
nothing under it, and the stored units were written by the adapter that
claims the container *this* pass. The second condition exists because
the claim is decided by marker files beside the container
(`settings.gradle` appearing next to a Node project's `build/`), outside
the event window: without it a Gradle build directory would replay the
Node adapter's rows until something inside it changed.

A directory the walk could not list inside a container is recorded as
an incomplete, zero-byte row, and completeness rolls up to every
ancestor: a `node_modules` with one unreadable package reports
"the walk could not read all of this directory", never a complete,
smaller tree.

## Where this appears

- `swamp report <root> --view builds` (and `--view deps` for installed
  dependency trees) -- the artifact rows, plus the collapsed per-family
  summary for every identified container.
- `swamp report <root> --view builds --json` / `--view deps --json` --
  each identified row carries an `interior` object: the same family
  rows (`recommendation`, `consequence`, `count`, `bytes`, `basis`,
  `oldest_modified`, `unknown_age`, `action`), the unsupported and
  unaccounted residual, and every unit with its `role`, `adapter`,
  `basis`, `time_source`, `action`, `consequence` and `coverage`.
- `swamp report <root> --view rust` -- the Cargo drill-down, unchanged.
- The TUI project tree, where a Node/Gradle/Maven container expands into
  its family groups (closed until opened), each leading with guidance,
  then count and oldest modification; an opened group lists its members
  oldest first, each leading with its consequence. Cargo containers keep
  their purpose groups. Which presentation a container gets follows the
  roles its units carry, never an adapter id. Space on a supported
  project-local output/cache group marks its exact supported members;
  Space on a member marks that path alone. The confirmation explains
  consequences before Enter moves the selection to Trash. Shared stores,
  installations, archives and unknown layouts remain inspection-only here.

## Ecosystem notes

**Python (#69).** Distributions are named by the wheel and sdist
filename conventions (the archive's own METADATA is inside it and is not
opened); `*.egg-info` by its `PKG-INFO`; an environment by its
`pyvenv.cfg` (interpreter, and whether uv created it -- uv links
packages from its cache, so shared bytes are charged once), and each
site-packages entry through a distribution's `top_level.txt`, never by
matching a directory name to one. An editable install's cost is its local
source (`direct_url.json`). A missing interpreter (`pyvenv.cfg`'s `home`
is gone) is stated. No interpreter is ever started and no package
imported.

**Go (#69).** Module identity is the escaped path (`!x` is `X`) and the
`@version` component; a download version is sized from its own `.zip`
(or `.mod` when only the module graph was fetched). A `golang.org/toolchain`
module is an installation. GOCACHE's `00`..`ff` buckets are opaque: an
entry's key identifies no package, no test binary and no build, and
nothing on disk says a build will read it again; Go's own `trim.txt` is
reported as the tool fact it is. No `go` command runs, least of all
`go generate`.

**Xcode / Swift (#70).** A DerivedData folder's project is its
`info.plist` `WorkspacePath`, read from the XML or binary plist without
`plutil`; a missing workspace is a fact, not a verdict. Products are
split by configuration and platform (`Debug-iphonesimulator` and
`Release-iphoneos` never collapse), test bundles and `.xcresult`s are
test output, `.dSYM`s are metadata that match only their own build.
Archives, device support, simulator runtimes and devices are never build
output. An active build is evidence only: SwiftPM's `.lock`, or a
modification in the ten minutes before the observation.

**Android (#70).** A module's `build/` is Android's only when the module
has `src/main/AndroidManifest.xml`; it reuses the Gradle adapter's
conventional table through the neutral `jvm_common` helper and adds the
plugin's own layout, with the variant (build type, flavor, test set)
from its directory names. SDK packages are installations; AVDs are
device state, with the emulator's lock files as active-writer evidence.

**Docker / BuildKit (#71).** Every field is the daemon's: sizes are
logical and per record (a parent's size is never inside a child's, so a
parent/child view never sums the same storage twice), times are the
daemon's records (`created`, and `last used` verbatim), and in-use,
shared and reclaimable are what it reported. An unavailable daemon, a
missing buildx, a builder that did not answer, an API older than 1.39 and
facts older than the five-minute cache are each stated. The host disk
image holding the records is measured separately in `--view external`
and never added. **Native action granularity:** `docker buildx prune
--filter id=<id>` removes a record together with the records that depend
on it; `docker builder prune` removes every record not in use that matches
its filters. Neither is offered, and no Docker file is ever touched
(#73 is where actions land).

## Cost

An unchanged container costs nothing: its units are replayed under the
pass's trusted event window, with zero directory listings and zero
manifest reads. A changed container pays one capped listing per
directory the adapter needs to look inside and one bounded manifest read
per named metadata file. Both are counted (`crate::work_counters`).
Through the real pipeline, on a 301-package Node checkout
(`build_adapter_history::cost_report_real_pipeline_unchanged_and_one_group_change`):
an unchanged refresh re-identifies no container and reads zero manifest
bytes; a change inside `dist/` re-identifies `dist/` alone and reads no
`package.json`. The measured numbers are in
`.oh/sessions/2026-09-21-build-adapters-node-jvm.md`.

The store join, through `external::observe_external` over a Maven
repository and the three Go stores
(`build_store_join::an_unchanged_store_replays_its_units_with_zero_listings_and_zero_reads`):
an unchanged pass lists, stats and reads nothing and replays all four
stores; a change inside the Maven repository re-walks and re-identifies
that store alone while the Go stores replay. Numbers in
`.oh/sessions/2026-09-21-build-adapters-python-go-apple-android-docker.md`.
