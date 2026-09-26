# Standalone CLI agent interface plan and clean-dev-dirs idea salvage

## Aim and authority

Two separate requests: plan removal of MCP in favor of skill + references + CLI, outside feature epics; extract useful ideas from clean-dev-dirs using salvage. The latter means learning extraction, not executable preservation or an instruction to restart/rewrite swamp. No application implementation, dependency installation, cleanup, upstream code copying or user-config modification is authorized here.

## Plan

- [#103](https://github.com/open-horizon-labs/swamp/issues/103): CLI-first agent skill, on-demand references and tested command contracts. Depends on none; no parent epic.
- [#104](https://github.com/open-horizon-labs/swamp/issues/104): remove MCP after replacement verification, including workspace, releases, current documentation, audits and contradictory open-plan requirements. Depends on #103; no parent epic.

Keep every current agent outcome available through CLI, no optional retained MCP. Skill rules are not isolation from a shell agent; source audit currently equates absence of grant-writing MCP functions with human-only authorization, a claim that must be narrowed when removing that transport. Do not pretend --actor establishes human identity. Historical releases/changelogs remain historical; no migration support or user installation cleanup is implied.

## Salvage

### Ecosystem comparison follow-up: concrete extraction

Compared upstream scanner detectors at the pinned revision below with swamp's `crates/core/src/ecosystem.rs`. This supersedes the broad reuse recommendation below for ecosystem detection: **do not import the upstream scanner or catalog wholesale.** All 16 upstream ecosystem families are already represented. No additional build-directory names were found to import; Ruby has a more precise nested boundary, described below. This is source inspection, not runtime equivalence testing. No application code was changed.

| Concrete difference | What to retain for swamp |
| --- | --- |
| Ruby upstream targets `vendor/bundle`; swamp labels the entire `vendor` directory as dependencies next to Gemfile. | Narrow identification to the actual bundle subtree; preserve unrelated vendored source as separately identified storage. Fixture: Gemfile plus vendor/bundle and vendor/handwritten. This is an attribution/selection boundary improvement, not permission to delete either. Do not inherit upstream's assumption that `.bundle` is disposable. |
| Upstream reads Gradle `rootProject.name` from settings.gradle or settings.gradle.kts; swamp's JVM name source is only pom.xml. | Add declarative Gradle display-name fallback when implemented. Fixture: Gradle-only project with a name different from its folder. Do not execute build scripts. |
| Upstream reads Cabal `name:`; swamp's Haskell name source is only package.yaml. | Add *.cabal display-name fallback. Fixture: Cabal-only project without package.yaml. |
| Upstream falls back to Python setup.cfg metadata and setup.py; swamp uses pyproject.toml only. | Retain static setup.cfg metadata-name fallback. Do not copy permissive setup.py line matching or execute setup.py; dynamic names may remain unknown. |
| Upstream parses Node/PHP JSON with serde_json; swamp's JsonName reader expects a line beginning with the name key. | Use structural JSON parsing for JSON manifests. Fixture: compact package.json with name on the opening-brace line, plus nested name fields that must not win. Keep JSONC a separate format rather than pretending strict JSON supports it. |
| Upstream skips Rust artifacts beneath any ancestor whose Cargo.toml contains an exact [workspace] line. | Retain the test question, not the heuristic: distinguish a shared target from an independent nested project's target, accounting each physical artifact once. This comparison does not establish a current swamp workspace-accounting defect. |

Lower-value name-reader differences: upstream handles uppercase CMake PROJECT and quoted names, reads Ruby gemspec assignments rather than filename stems, and reads deno.jsonc names. These are parser fixtures to evaluate, not new ecosystem features; do not copy permissive line parsing of executable manifests.

Catalog audit: Rust target; Node node_modules; Python caches/venvs/build/dist; Go vendor; Maven/Gradle target/build; C/C++ build; Swift .build; .NET bin/obj; Ruby .bundle and vendor/bundle; Elixir _build; Deno vendor/node_modules; PHP vendor; Haskell .stack-work/dist-newstyle; Dart .dart_tool/build; Zig zig-cache/zig-out; Scala target. Swamp already has those families and broader artifact patterns (Ruby currently via its overbroad vendor rule). Upstream's extra `pipenv.lock` marker is not adopted without authoritative evidence for that spelling.

Do not inherit first-match-wins ecosystem detection, requiring a build artifact before recognizing a project, or blanket hidden-directory pruning. These conflict with swamp's mixed-ecosystem identification and agent-storage coverage.

**Trash assessment:** a modest portability dependency decision, not an architectural discovery. Linux interoperability involves restore metadata, collision avoidance, per-volume placement and permission/symlink checks, not just rename into one directory. See https://specifications.freedesktop.org/trash/latest/ and https://docs.rs/trash/latest/trash/. Evaluate the crate against those requirements and platform constraints; no requirement to adopt clean-dev-dirs itself. Moving to Trash must not be presented as reclaimed space or silently fall back to permanent deletion.

**Salvaged:** 2026-09-19, clean-dev-dirs ideas at observed upstream main commit 2f9f9de1d99b48113a5bffb151c05b98d790314b.
**Reason:** Earlier framing mistakenly equated the user's salvage request with preserving executable files. Correct frame is extracting reusable lessons before deciding whether to borrow anything.
**Original aim:** Avoid rebuilding ordinary discovery/cleanup plumbing while retaining swamp's project model, trustworthy history and precise authorized actions.

### Learnings and reusable fragments

1. **A reusable library under the CLI is a real integration seam.** src/lib.rs exports Scanner, project types, filters and Cleaner; it is not only an executable. Salvage the API decomposition and evaluate individual modules versus a whole dependency. Cargo.toml includes CLI/UI dependencies and a build dependency, so whole-crate reuse has a footprint to assess. This informs existing #79/#84, not a new adoption mandate.
2. **Separate discovery from sizing/action.** src/scanner.rs explicitly has project discovery then parallel size calculation, with jwalk traversal and Rayon work. Keep the distinction: swamp discovery and attribution have different stopping rules. Borrow the phase separation and benchmark generic traversal, not the upstream pruning predicates blindly (#80).
3. **Single-worker behavior matters.** scanner.rs explicitly selects serial jwalk for threads=1 because its comment identifies a possible global-pool stall. Given swamp's prior worker-pool bug, test one-worker/CPU-contention cases when evaluating libraries. This is a local decision-changing edge case, not proof a dependency is generally broken.
4. **Machine output is a first-class mode.** Scanner.with_quiet hides progress output for JSON. Preserve stdout purity across the whole CLI, not merely final serialization; verify action warnings too. Useful to #103.
5. **Configuration needs absence-aware precedence.** src/config/file.rs uses optional fields for CLI > file > default layering. Preserve the distinction between missing values and explicit false/empty, particularly for includes/excludes and defaults=false. Already relevant to #41, not an excuse to copy another tool's semantics.
6. **Reuse OS primitives without inheriting policy.** Cleaner separates RemovalStrategy and calls the trash crate. Borrow the backend seam and evaluate trash (#85); keep swamp's proposal/grant/recheck/ledger policy outside it.
7. **Retention is a reviewable action phase, not a backup guarantee.** executables.rs copies selected Rust/Python outputs, but supports only particular layouts. Swamp already has keep_executables. This is one reusable pattern, not the meaning of this salvage request, and not a new feature commitment.

### Do not inherit / guardrails grounded in the inspected code

- utils/size.rs flattens traversal failures, ignores metadata failures and sums metadata.len(); a missing/unreadable root yields zero. Useful for a best-effort one-shot cleaner, incompatible with swamp's authoritative growth/allocated-byte interpretation. Adapter errors must preserve partial/unknown coverage. Existing coverage-changes-are-not-storage-changes guardrail owns this; do not create a duplicate rule.
- scanner.rs uses filter_map(Result::ok) and returns Vec<Project>; evaluate whether a library boundary exposes enough error/completeness evidence before using it as a measurement source. Generic traversal is reusable; a lossy high-level result may not be.
- cleaner.rs logs a preservation failure and continues toward removal. Swamp must refuse removal if preservation was a requested precondition and failed. Do not copy this execution flow wholesale.
- executables.rs copies to basename destinations with fs::copy. Before any reuse, test collisions, existing destination overwrite, symlinks, configuration variants and partial copy failures; copied bytes alone do not establish a runnable/restorable package.
- Measured logical bytes removed/moved are not observed freed space. Keep Trash, hardlinks, sparse files and shared storage accounting separate; do not inherit a generic cleaner's freed-space label as fact.

### Frame shift

“Borrow a cleanup tool” → “Reuse tested components and local patterns behind swamp's stronger accounting/action contracts.” README feature claims alone cannot establish API fit. No claim that upstream's trade-offs are bugs for its own use case.

### Missing context / coordination

The upstream public library and actual error semantics should have been checked before recommending bespoke shims. Prior assistant conflated a named salvage skill with a similarly named product feature. The user selects adoption; this extraction records evidence and destinations, not authorization to import code or expand epics.

### Fresh-start recommendation

Use this as input to the already-planned reuse assessment (#79/#80/#84/#85) and standalone CLI work, not as a new required dependency or rewrite mandate. Assess library licensing/version/footprint, then run isolated equivalence and failure fixtures if implementation is later requested. Preserve existing metis/guardrails; no generic best-practice entries added.

### Source pointers

Reviewed upstream main with observed commit 2f9f9de1d99b48113a5bffb151c05b98d790314b; implementation-time adoption should pin/recheck its chosen revision.
- https://github.com/clean-dev-dirs/clean-dev-dirs/blob/2f9f9de1d99b48113a5bffb151c05b98d790314b/src/lib.rs
- https://github.com/clean-dev-dirs/clean-dev-dirs/blob/2f9f9de1d99b48113a5bffb151c05b98d790314b/src/scanner.rs
- https://github.com/clean-dev-dirs/clean-dev-dirs/blob/2f9f9de1d99b48113a5bffb151c05b98d790314b/src/utils/size.rs
- https://github.com/clean-dev-dirs/clean-dev-dirs/blob/2f9f9de1d99b48113a5bffb151c05b98d790314b/src/config/file.rs
- https://github.com/clean-dev-dirs/clean-dev-dirs/blob/2f9f9de1d99b48113a5bffb151c05b98d790314b/src/cleaner.rs
- https://github.com/clean-dev-dirs/clean-dev-dirs/blob/2f9f9de1d99b48113a5bffb151c05b98d790314b/src/executables.rs

## Reconciliation, 2026-09-21

#103 and #104, planned above as standalone work, are implemented:
`crates/core/src/agent_json.rs` extracts the MCP tools' JSON-shaping
logic for CLI reuse; `report --json` now honors `--view`/`--project`/
`--filter` and gains `--limit`/`--offset` bounding; `skills/swamp/`
ships the skill plus five references (`commands-and-json.md`,
`filters.md`, `cleanup-and-recovery.md`, `coverage-and-history.md`,
`trust-model.md`); `crates/mcp` is deleted along with its workspace
membership, packaging and CI references. The `human_only_authorization`
source audit is rewritten exactly as anticipated here -- from "MCP has
no grant-writing functions" to a transport-independent statement that
authorization-minting functions are reachable only from named CLI/TUI
call sites, with `--actor` documented as attribution, never identity
proof. `<scratchpad>/issue-reconciliation-mcp.md` carries the
replacement text for other open issues that named MCP.
