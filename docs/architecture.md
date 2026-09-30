# Architecture

Swamp turns filesystem measurements into a developer's decision: what grew, which project it belongs to, and what removing it would cost. It needs enough detail to explain a build or agent-storage unit, but not a permanent inventory of every file.

The main separation is **observation versus reporting**. Observation resolves scope, measures storage, enriches it, and writes typed facts. Reporting reads those facts. Opening another JSON view must not repeat the filesystem walk or query GitHub.

## The data model

A project groups checkouts and linked worktrees, normally through a normalized Git remote. A worktree contains artifacts such as source, repository data, dependencies, and build output. An artifact can contain adapter-identified units: compiler caches, compiled tests, generated output, or other ecosystem-specific groups.

Storage outside a checkout has its own identity. External units represent tool homes, shared caches, and installations. Agent units represent such things as sessions, attachments, and checkpoints. Consumer links connect these units to projects when there is evidence; they are not copies of the unit's bytes.

| Layer | Question it answers |
| --- | --- |
| Scope and coverage | Which locations were requested, discovered, observed, missing, or excluded? |
| Project and worktree | Whose development context is this? |
| Artifact or external/agent unit | What kind of storage is it? |
| Nested build unit | What can be inspected or selected independently? |
| Decision evidence | What is known about age, ownership, sharing, and removal consequences? |
| Observation and history | What changed, over what observed interval? |

Classification, ownership, accounting, and action support are separate facts. An adapter recognizing a path does not make it removable. An old modification timestamp is useful review evidence, not proof of disuse. An ignored directory is not necessarily disposable.

### Accounting is not a cleanup promise

Rows can contain shared hardlinks. Local allocated sizes therefore need not add up to scope-wide unique bytes. Purpose groups and the physical-directory view can also describe the same storage twice; the latter is a navigation alternative, not extra usage.

Ordinary refreshes update changed containers without walking unchanged roots to resolve all sharing. They keep the last unique-byte reconciliation with an explicit freshness state. An explicit full observation measures the observed scope and counts each device/inode once. It stores the result and bounded container-sharing summaries, not an inode inventory.

Even a fresh hardlink-deduplicated total is not guaranteed reclaimable space. Other links, Trash, APFS clone/snapshot extents, and Docker's accounting make that a different question. See [unique bytes and fast refresh](usage.md#unique-bytes-and-fast-refresh).

## Effective scope and location detectors

One scope resolver supplies observation, reporting, scheduling, and the TUI. It combines built-in roots, enabled detector locations, and configured additions/exclusions. Explicit roots replace the default root selection; exclusions still apply.

Detectors find locations and explain their provenance. They do not independently authorize traversal or removal. Homebrew is split: developer tooling (an allowlist in `docs/locations.md`) is on by default as units of its own, everything else under the prefix is one `Homebrew (other)` unit, and the full detector can be enabled explicitly. Source roots are declared by the user (`swamp config add-root`), never inferred from history, editor recents, git configuration or Spotlight. `swamp scope` exposes the catalog and each resolved location's status.

Roots are normalized and overlapping traversal is avoided. Canonical paths and symlink aliases are considered when applying exclusions. A multi-root report describes the scope it actually covers, rather than borrowing the name of one constituent root.

Missing or unreadable roots are coverage facts. They must not look like deleted storage or justify tombstoning a root that was not observed. History comparisons distinguish changes in scope from changes in measured storage.

Implementation: [scope.rs](../crates/core/src/scope.rs). User contract: [scope and coverage](usage.md#scope-and-coverage).

## Observation pipeline

The observation path has five responsibilities:

1. **Resolve scope.** Establish roots, exclusions, ownership boundaries, and coverage.
2. **Choose measurement work.** Use valid event coverage and folded summaries, or explain why a full walk is required.
3. **Identify and enrich.** Attribute projects and artifacts; run supported build, external-storage, and agent adapters; collect bounded evidence.
4. **Persist facts and history.** Store current typed rows and prior values needed for growth comparisons.
5. **Assemble reports.** Apply the requested view, project filter, and pagination to the stored observation.

`swamp observe` performs this work without rendering a report. The TUI uses observation in the background. Before any derived-table reads, an absent or incompatible store-format marker causes the shared observer to reset recognized Swamp-owned derived generations under a serialized writer lock and perform a fresh scan. The marker is committed only after the observation pipeline succeeds; partial or missing roots are recorded normally and do not defer schema reset. Configuration, protection intent, notes, ledger state, user-declared consumer associations, unknown files, and active enrichment remain. Once the marker is current, compatible history survives narrow-root observations. `swamp report` is a pure read and skips incompatible cached generations rather than attempting cleanup.

A scope with no observation returns an explicit error, not a fabricated empty report. Report timestamps refer to the observation, not the moment someone requested JSON.

Implementation entry point: [report/pass.rs](../crates/core/src/report/pass.rs).

## Incremental observation

### Fold the work, not just the display

A large build directory is an artifact in the project model. Inside it, folded directory measurements retain enough structure to invalidate changed containers and reuse unchanged aggregates. Selected detail is materialized where needed; ordinary refresh does not require serializing every build file into a detailed report cache.

Platform events say where measurements may have changed. They are not themselves byte totals. A changed container is remeasured; an unchanged one can reuse its aggregate when the event coverage supports that conclusion.

Deeper Cargo inspection is a separate, bounded read through `inspect-cargo`. It does not run Cargo or persist a second artifact database. This keeps diagnostic detail out of the normal refresh cost.

Implementation: [folded_measurement.rs](../crates/core/src/folded_measurement.rs).

### Platform contracts

On macOS, FSEvents provides a persisted change stream, including periods when Swamp was not running. A cursor is useful only when it covers the measurement interval. The observation commits its event anchor after successful measurement; missing or invalid continuity requires a broader walk.

On Linux, inotify covers only a live watch. Only the opt-in collector maintains that coverage (the TUI no longer opens a watch). A first observation, an unwatched interval, queue overflow, or lost coverage requires a full walk with a reason. Linux does not pretend that time without a watcher was unchanged.

Native implementations are selected per platform. The [platform guide](platform.md) describes watcher continuity, scheduling, filesystem measurement, and Trash behavior.

### Keep the interface responsive

Walking, reviewing a large selection, and deleting groups run off the UI thread. Progress distinguishes observation from deletion; cleanup reports completed and total groups. Cancellation stops further groups rather than pretending to undo completed moves.

This separation is checked by source audits and runtime tests. It matters to the product: a cleanup tool that stops responding during cleanup leaves the user unable to tell whether it is making progress.

### Remaining costs

Incremental traversal does not make every stage proportional to the number of changed files. Reading or rewriting current Parquet tables, assembling reports, resolving scope, and refreshing expired enrichment still have costs. A full unique-byte reconciliation deliberately walks more broadly. Measure these separately before attributing a slow refresh to the filesystem walker.

## History storage

### Current values and reverse deltas

The store uses compressed Parquet tables for current measurements and typed report facts. When a measured value changes, reverse deltas retain its prior value for historical comparisons. This avoids writing an entire historical snapshot of an unchanged tree on every observation.

The report facts include project/worktree identity, artifact shape, nested units, external and agent units, consumer links, evidence, coverage, notes, and run metadata. There is no separate JSON report cache or Swamp-owned SQLite database.

Directory summaries and selected large-file detail keep the index smaller than a per-file inventory. Folded storage is also the measurement reuse mechanism; it is not merely a renderer hiding an exhaustive index.

### What a comparison means

History starts at the first observation and is limited by retained measurements. Coverage is calculated against the stored observation time, so rereading an unchanged report does not create more history.

Multi-root comparisons retain each root's coverage and use the common available interval where required. A short or missing baseline is disclosed rather than described as a full requested window. Growth windows are observation data: request the intended window when observing.

History records sizes and metadata. It cannot restore files, prove which process wrote them, or establish that a file has not been used.

## Enrichment and freshness

Git facts add dirty state, unpushed commits, branch information, and activity context. GitHub enrichment adds cached PR and merge information when available. Docker supplies its own object identities and accounting. These facts have different sources and refresh costs from filesystem measurements.

Project linkage uses evidence such as recorded working directories, known checkout/worktree paths, and supported tool metadata. A path that moved, collides with another project name, or lacks reliable context must not silently become a confident assignment.

For Codex, the adapter reads Codex's own SQLite thread index **read-only**, selecting working-directory and rollout-path metadata. It resolves the configured database location and versioned state database rather than hardcoding `state_5.sqlite`. Missing, ambiguous, or incompatible state stays unavailable; there is no fallback scan of Codex conversation JSONL for attribution.

That external metadata reader is not Swamp's storage layer. Swamp continues to persist its observations in Parquet.

Adapters differ in metadata availability and supported formats. The checked [agent-storage matrix](agent-storage.md) records those boundaries. A recognized tool name is not a promise of complete attribution or cleanup support.

## Extension points

### Build adapters

Adapters identify units within an artifact and return stable identity, role, bytes/accounting basis, age evidence, and removal consequences. Shared role families drive aggregation and presentation across ecosystems; adapter-specific meaning stays attached to the unit.

The reusable decision model is **age + size + removal consequences**, not “Cargo directories are disposable everywhere.” A compiled test, an installed dependency, a shared package store, and a session checkpoint have different recovery costs.

Cargo profiles act as containers. Their purpose groups select supported descendant units, not whole profile directories. Other adapters use shared families such as build outputs, test output, caches, and dependencies. Project-local action support does not authorize deletion of a shared store.

The [build-artifact matrix](build-artifacts.md) is tested against the adapter registry. Extend identification, aggregation, stored facts, views, and action support together; do not add a label in one interface and imply end-to-end support.

### Agent and external storage

Agent adapters identify storage units and their relationships, not just large directories. Consumer links can connect one unit to several projects without charging a new copy to each. Shared attachments and references need different treatment from an isolated cache.

Protection is explicit user intent, not inferred from age. Recognized data with unsupported lifecycle semantics remains inspection-only. Conversation history and checkpoints are not described as rebuildable merely because an agent created them.

### Interfaces

The CLI, TUI, and skill share the stored decision evidence. JSON views use explicit pagination, including build-unit pages; summaries still describe the full filtered population. Text and JSON must apply the same scope and project selection.

The skill teaches the CLI contract. It is not a separate execution server and does not introduce a deletion API.

## Capability gates

Rust types, visibility boundaries, source audits, compile-fail tests, mutation tests, and runtime fixtures enforce selected architectural constraints. Examples include separating reporting from observation, keeping blocking work off UI event paths, requiring explicit protection handling, and preventing retired command paths from returning.

These checks are executable constraints, not an OS security boundary or proof that every filesystem race is impossible. The shipped binary excludes test-only fixture APIs.

`scripts/check.sh` runs the routine checks. `scripts/check-full.sh` adds compile-fail cases, mutation sweeps, and the isolated cost test. Release CI runs the full tier on macOS and Linux and smoke-tests packaged binaries.

## Stall watchdog and test hooks

`crate::beacon` is the shared progress beacon (#190): walk directories, git signals, the ignore lens, external units, the Codex database read and every bounded content read enter it with a phase and a path; `swamp observe` stops a pass with no progress for `observe_stall_secs`, logs the phase and path, and quarantines the path for 24 hours (`stalled-paths.tsv`, stored coverage `not measured (stalled on <date>)`).

One test hook exists, and only in test builds: with swamp-core's `testing` feature (enabled only by `[dev-dependencies]`, so `cargo test` builds the `swamp` binary with it and `cargo build --release` does not), `SWAMP_TEST_MODE=1` plus `SWAMP_TEST_PARK_DIR=<dir>` makes a walk entering exactly `<dir>` park without progressing, for the end-to-end watchdog test. `scripts/check.sh` fails if the shipped build graph enables `testing`, and `scripts/release-smoke.sh` fails if the packaged binary contains the variable name or parks on it.

## Limits of the current implementation

- **Not a forensic inventory.** Folded aggregates trade exhaustive detail for practical observation. Use on-demand inspection when you need more.
- **Freshness is explicit, not universal.** Stored Git/GitHub facts and unique-byte estimates can lag the filesystem. Reporting does not refresh them.
- **No guaranteed obsolete-build detection.** Age and rebuilding consequences help choose; neither proves a build will never be needed again.
- **No guaranteed freed-byte estimate.** Allocated, unique, shared, and reclaimable bytes are different quantities.
- **Coverage varies by adapter.** Checked matrices distinguish supported, partial, and unverified cases. Recognition alone does not enable cleanup.
- **Removal is human-confirmed.** The TUI confirms supported actions, but does not re-check every fact between marking and execution. There is no CLI deletion command.
- **Trash retains storage.** Moving a path to Trash is recoverable but does not free its blocks. Docker removals do not use Trash.
- **Identity can change.** Moving paths or changing remotes can interrupt attribution and history continuity.
- **No atomic filesystem snapshot.** Files may change while observed; multi-table storage is not a transactional snapshot of a live filesystem.
- **Linux needs continuous event coverage for incremental reuse.** Without it, Swamp walks fully rather than guessing.
- **No fixed performance guarantee.** Changed-container count, table size, enrichment, filesystem behavior, and reconciliation affect cost.

For the developer workflow rather than implementation detail, start with [usage](usage.md).
