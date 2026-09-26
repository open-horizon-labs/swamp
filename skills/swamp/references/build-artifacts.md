# Build artifacts (reference)

What is *inside* a `target/`, a `node_modules/`, a Gradle or Android
`build/`, a Maven `target/`, a `.venv`, a `dist/`, a Go `vendor/`, a
SwiftPM `.build` -- and inside the machine-wide stores beside them (a
Maven repository, a Gradle home, npm/pnpm stores, Go's module and build
caches, pip/uv caches, DerivedData, CoreSimulator, the Android SDK,
BuildKit's cache) -- and what removing a piece of it would cost.

**The CLI is read-only.** The TUI supports exact-path Trash for identified
project-local outputs, test output and intermediates. Shared stores,
installations and unknown layouts remain inspection-only. Cargo's
fingerprint-aware purpose groups use their separate membership plan
(`references/cleanup-and-recovery.md`). Describe the row's actual capability;
do not imply a CLI deletion command exists.

## Where it appears

- `swamp report <root> --view builds` -- artifact rows, then one
  collapsed row per role family for each identified container.
- `swamp report <root> --view builds --json` -- each unit's `adapter`,
  `role`, `basis`, `time_source`, `action`, `consequence`,
  `coverage.limits` and `variant`.
- `swamp report <root> --view deps` -- the same breakdown under an
  installed dependency tree.
- `swamp report <root> --view rust` -- the Cargo drill-down.
- `swamp inspect-cargo <profile-path> --json` -- explicit, bounded dependency
  inspection. Target/variant groups use existing fingerprint evidence;
  package identity stays unknown unless directly evidenced. Read coverage
  limits and residuals before comparing totals. No source/build execution,
  persistent inventory or per-crate deletion. TUI equivalent: `i` on a profile.
- TUI: opening a project expands an identified container into its
  families; a `node_modules` that is mostly pnpm store shows that.
- `swamp report --view external` -- each machine-wide store's family rows
  under its unit; `--json` carries them as `interiors`, keyed by the
  unit's path, in the same shape as `interior` above. TUI External view:
  a store row opens onto the same family groups.
- `swamp report <root> --view docker` -- BuildKit records per builder,
  in the daemon's terms; `--json` adds a `buildkit` array.

## Role families

`outputs`, `tests`, `intermediates`, `dependencies`, `shared-store`,
`installations` (SDKs, runtimes, interpreters, toolchain downloads --
never build output), `state` (archives, simulator devices, AVDs --
nothing regenerates them), `metadata`, `residual`, `unknown`. Cargo's older roles (`profile`,
`dependency`, `test-executable`, `example`, `build-script-output`,
`incremental`, `final-output`, `companion-metadata`) map into the same
families, so a summary never has a Rust column and an everything-else
column.

## Reading a unit honestly

| Field | What it establishes | What it does not |
|---|---|---|
| `mtime_max` + `time_source` | when something under this unit was last **written** | not when it was last used, read or executed |
| `bytes` + `basis` | a size on one stated basis | not reclaimable space, and never addable across bases |
| `role` / family | what kind of thing this is | not that it is obsolete or removable |
| `consequence` | what happens if the bytes go | not a recommendation to remove them |
| `action` | exact-path TUI Trash, inspection only, or unavailable-with-a-reason | not permission to delete or evidence of disuse |
| `coverage.limits` | exactly what swamp could not establish | |
| `variant.unknowns` | fields with no evidence behind them | |

Summaries carry `unknown_age` separately from `oldest_modified`: a unit
with no known time is counted, not folded in as epoch zero.

## What is never inferred

- **A build generation.** npm, pnpm, Gradle and Maven record none. A
  newer similarly-named output does not supersede an older one.
- **A package's identity from its directory name.** It comes from the
  package's own `package.json`; unreadable means unknown.
- **That a Maven artifact can be downloaded again.** Origin is
  `downloaded` (a repository id in `_remote.repositories`),
  `locally-installed` (an empty id there, or `maven-metadata-local.xml`)
  or `unknown-origin` (no usable evidence; a `*.lastUpdated` file alone is
  an attempt record, not a download). `unknown-origin` is a real answer;
  never upgrade it to "downloaded".
- **That a family's oldest member is its last use**, or that the family
  total is the whole container: summaries count nonempty supported units
  only, and `unsupported_*`/`unaccounted_bytes` in the `--json`
  `interior` are the rest.
- **Anything from running a build tool.** No `npm`/`gradle`/`mvn`/
  `cargo`, no JS config, no Gradle script, no Maven plugin.

## Shared stores

A pnpm object store, an npm `_cacache`, a Gradle user home and a Maven
local repository are shared by every project on the machine. Their
entries carry `shared-hardlink` membership and no physical charge, so a
project's copy is never added to the store's own total. Which projects
link a given entry is not derivable from the entry, and no action is
offered on one.

These stores are joined into the external observation's pass: the
detector declares which location is which store kind, the adapter
declares which kinds it identifies, and the interior's history lives on
its own key family. Only stores in the scope (enabled, not excluded,
readable) appear; say which ones a report actually covered.

## Coverage

Implemented interiors: Cargo, Node, Gradle, Maven, Python, Go,
Xcode/Swift, Android, Docker/BuildKit.

- **Go build cache (GOCACHE)**: bucket names are opaque hashes. Never say
  an entry is a test binary, belongs to a package, or will not be read
  again.
- **Docker/BuildKit**: sizes, times and in-use/shared/reclaimable are the
  daemon's facts; a record's size never includes its parents'. The host
  disk image is a separate number -- never add them. Native prune removes
  a record with its dependents; swamp offers no action.
- **Archives, simulator devices, AVDs** (`state`) and **SDKs/runtimes**
  (`installations`) are never "build output"; do not describe them as
  rebuildable. `docs/build-artifacts.md` in the repository is the
checked matrix, including per-ecosystem layouts and attribution limits.
