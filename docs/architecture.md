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

### Recently written files and entries that fail mid-walk

Allocated bytes are `st_blocks * 512`, always the figure the filesystem reports. On ext4, XFS and overlayfs over them a file whose data is still dirty reports a token block count until writeback (measured on a Docker overlayfs: 512 bytes for 5 KB, 20 KB and 1 MiB files alike), so a tree written a moment ago reads near-empty and then "grows" when flushed. A fresh sparse file reports the same token count as a fresh dirty file and no `stat` field tells them apart, so swamp does not substitute a length-based figure (that would invent bytes for sparse images). Instead a regular file modified in the last 120 seconds that reports some blocks but less than one 4 KiB unit is counted exactly as reported and recorded in the work counters as `pending_allocation_files`, with `pending_allocation_bytes` as the most those files can still add. A total with pending files is not final. Zero blocks, a full unit or more, and any file older than the window carry no such flag. APFS and tmpfs assign blocks at write time and never take this path.

Measurement degrades per entry. An entry that vanishes between the listing and its `lstat` (`ENOENT`, or `ESTALE` as overlayfs and network filesystems return it) is skipped. Any other per-entry error (`EIO`, `EACCES`, a listing error mid-stream) makes that entry "not measured": the rest of the directory is still counted, the affected rollup is incomplete, and an incremental refresh falls back to a full walk rather than storing a partial number as complete.

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

`swamp observe` performs this work without rendering a report. The TUI uses observation in the background. Before any derived-table reads, an absent or incompatible store-format marker causes the shared observer to reset recognized Swamp-owned derived generations under a serialized writer lock and perform a fresh scan. The marker is committed only after the observation pipeline succeeds; partial or missing roots are recorded normally and do not defer schema reset. Configuration, protection intent, notes, ledger state, user-declared consumer associations, unknown files, and active enrichment remain. Once the marker is current, compatible history survives narrow-root observations. `swamp report` is a pure read and skips incompatible cached generations rather than attempting cleanup. The marker stays generation 2 through v0.8.0, on purpose: an installed v0.7.x resets the store on any marker it does not know (newer too), and a machine with two installs and a scheduled observe would reset it on every alternation, wiping history each time. v0.8.0's additions are therefore new sibling tables that v0.7.x ignores (`unit_meta.parquet`: last-used and the structured overlap, keyed by unit id; `unit_children.parquet`: the depth-2 rows; `external/overlap_marks.parquet`: when a unit's worktree overlap last changed; `manager_facts.parquet`: what a package manager's own tooling said in the last scheduled `observe`, machine wide, replaced whole by each pass, verbatim in its `text` column, absent until the first pass and then read as "not observed yet"), and `external_units.parquet` keeps exactly the v0.7.5 columns (a test pins them). A sibling row applies only to the `external_units` row with the same `observed_at`, so rows a v0.7.x pass left behind are never shown as current. A missing sibling table means no last-used and no drilldown, never an error. The reset rule is one-directional: only an older (or absent, or unrecognized) marker resets; a newer marker is read where possible and never reset or written (`swamp observe` says "store written by a newer swamp; not modifying it"). Stored build-store units carry a fingerprint of the swamp version, the detector catalog version and a digest of every build adapter's source (`build_stores::adapter_revision`), so an adapter changed under the same catalog version invalidates those rows (and only those: the fingerprint is per row, and a mismatch is a re-identification, never a store reset). It lives in the existing per-row fingerprint rather than a table of its own because v0.7.x treats that field as opaque and a row written by either version is refused by the other, never mixed.

**Why key-file access time is probed on every observation.** `incremental-walk-only-changed-subtrees` and `no-second-traversal-on-report-path` say unchanged units are replayed without listing. Access time is the one fact that breaks that: reading a file raises no FSEvents event, so a replayed unit would keep an access time only as new as its last walk, always older than reality. So each observation lists the declared `bin` directories of a unit (bounded: 4,096 listings, then "probe limit reached") and takes one `lstat` per key file; it opens nothing, counts in the work counters, and `report` never does it. Cost measured on this machine: a cold `find` over the Cellar, mise, rustup, pyenv and espressif `bin` layouts together is under 0.6 s, and an incremental observe is unchanged within noise (12.1 s against 12.0 s).

A scope with no observation returns an explicit error, not a fabricated empty report. Report timestamps refer to the observation, not the moment someone requested JSON.

Implementation entry point: [report/pass.rs](../crates/core/src/report/pass.rs).

## Incremental observation

### Fold the work, not just the display

A large build directory is an artifact in the project model. Inside it, folded directory measurements retain enough structure to invalidate changed containers and reuse unchanged aggregates. Selected detail is materialized where needed; ordinary refresh does not require serializing every build file into a detailed report cache.

Platform events say where measurements may have changed. They are not themselves byte totals. A changed container is remeasured; an unchanged one can reuse its aggregate when the event coverage supports that conclusion.

Deeper Cargo inspection is a separate, bounded read through `inspect-cargo`. It does not run Cargo or persist a second artifact database. This keeps diagnostic detail out of the normal refresh cost.

An external unit is reused at two grains. When the event window shows nothing under the unit, its stored total is replayed with no listing at all. When it shows changes, only the changed part is walked: a full measurement records each immediate subfolder's folded total (only when no file is hard-linked across two subfolders or from the root's own files, and root files plus subfolders add up to the total exactly; an unreadable subfolder gets no total and the measurement is stored marked as a lower bound, never replayed whole), and a later pass replays every subfolder the window vouches for, whose own directory stamp is unchanged (a folder renamed into place under the same name is walked) and which holds no hard-linked file, then walks the root's files and the remaining subfolders. A walked linked file whose other links are not all in the walked part makes the pass walk the whole unit. Known limit: a hard link made from outside the unit to a file in a replayed subfolder fires no event under the unit, so the unit's hard-linked label (not its bytes) lags until that subfolder changes. The result, including the depth-2 drilldown, equals a full walk; build stores whose adapter identifies from every directory row are still walked whole. A directory FSEvents cannot itemize (`MustScanSubDirs`) refuses only the unit roots that contain it or sit inside it; dropped events and wrapped ids refuse every root in the stream. Known limit: a stored cursor is checked against the device id, against FSEvents' current id (an id from the future is refused) and against the oldest id FSEvents can still vouch for on the device; it is not checked against the volume's FSEvents database UUID (`FSEventsCopyUUIDForDevice`, which the bundled `fsevent-sys` does not bind). An FSEvents database that was reset and has since passed the stored id again would not be detected; recording the UUID with each cursor is a store-format change left for a later release.

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

## Volume ledger

The report is about scope. The volume ledger (#169, #170) is about the disk:
it answers "where did the space go" for the whole APFS container, including
what no path reaches, and says what it did not measure.

- **A measurement, not a view.** `volume_ledger.parquet` (one row per location:
  path, category, allocated bytes or null, overlap bytes, entry count,
  unreadable count, `measured_at`, method, exactness, note) and
  `volume_ledger_meta.parquet` (one row: when the run finished, the cursor, the
  budget and what it used, the `statfs` container snapshot and `diskutil`'s Data
  volume figure) are new files. No existing table changed and the store-format
  marker did not move, so an older swamp ignores them and a format reset leaves
  them. Bytes are null exactly when a row is `not_measured`.
- **One pass, only in `observe`.** `volume_ledger::pass::run` follows a successful
  observation (`swamp observe`, scheduled or `--volume`), under the single-flight
  observe lock and without the writer lock, which it takes for the two writes
  only. It copies the observation's own unit and root totals (nothing measured
  twice; units are disjoint by construction, except an agent home's view of its
  own unit and mounted disk images, which are subtracted as overlap), plans a
  coarse walk of the data volume outside them, and measures at background priority
  with a bounded pool under a time budget. The cursor is
  `cycle_started_at`: a location is pending until its row is at least that new.
- **Walk the data volume from its own mount point.** On macOS `/System/Volumes/Data`
  holds exactly the data-side of `/System` (`/System/Library/AssetsV2`) and `/usr`
  (`/usr/local`); the sealed system volume is never listed and is the System line
  from `diskutil`. `st_dev` cannot tell them apart (every path under `/` reports one
  device), so a device test would either walk the sealed volume or skip
  `AssetsV2`. Rows keep logical paths (`/System/Library/AssetsV2`); `Mapped` is the
  only place the mount point exists.
- **System volumes and snapshots** come from three read-only spawns
  (`diskutil apfs list -plist`, `diskutil info -plist /System/Volumes/Data`,
  `tmutil listlocalsnapshots /`), allow-listed in `fs_gate::spawn` (fixed argv,
  kill on timeout, counted). A missing program, a timeout, a permission error or an
  unreadable answer is a note on a not-measured row, never a zero.
- **The bookkeeping is computed at read time** (`volume_ledger::account`), not
  stored: accounted + everything else + system volumes + the protected-folder
  estimate (the Data volume's consumed bytes minus what was measured, only while
  something is unreadable or pending) + a signed, named residual equals the
  container's used bytes. That is bookkeeping (`bookkeeping_balanced`), not
  evidence about the walk: the estimate is a leftover, so it balances anything.
  The check that can fail is the **spot audit**: up to five folders measured this
  run are re-measured by an independent naive sum and compared; a difference
  beyond `max(1%, 4 MiB)` sets `audit_flag`. Audit results are ordinary
  `audit`-category ledger rows. Purgeable space, mounted images, another
  volume of the container, locations on other volumes and network shares are
  listed and never added.
- **Decoupled and bounded.** The pass runs after the observation and its lock,
  under `volume-pass.lock`. The mount table is read before any path is touched
  (network, FUSE and automounter mounts are never statted or entered); workers are
  detached and the pass stops waiting at twice its budget, records the stuck path
  as not measured and lets the next run resume from the cursor. A local heartbeat
  (the last path each worker touched) names the stuck path; unifying it with the
  shared stall beacon in PR #191 is a follow-up.
- **Reading never walks.** `report --view disk`, the `disk` object of
  `report --json` and any other reader call `volume_ledger::read_account`: two
  Parquet reads, no listing, no stat, no spawn (the work counters assert it).

## Enrichment and freshness

Git facts add dirty state, unpushed commits, branch information, and activity context. GitHub enrichment adds cached PR and merge information when available. Docker supplies its own object identities and accounting. These facts have different sources and refresh costs from filesystem measurements.

### Enrichment caches

Enrichment is pass-through: `report` and the TUI read stored rows and start no process; `observe` asks a producer only when its stored answer cannot be shown to still hold. Warm costs below are one unchanged scheduled pass on the maintainer's store copy (#181).

| Enrichment | Key | Expiry | Negative answers | Warm pass, before | Warm pass, after |
|---|---|---|---|---|---|
| GitHub PR / merge facts | worktree, branch, tip commit | merged: never; otherwise 24 h; `--enrich` forces | no PR, `gh` unavailable and failed lookups are stored, 24 h | 1 `gh auth status` spawn; tip commits read one worktree at a time | no spawn; tip commits read on a pool of 4 |
| Homebrew reports (`brew autoremove --dry-run`, installed on request) | bytes and newest mtime of `Cellar` and `Caskroom` plus every brew unit (no key, so brew is asked, when either has no recorded total) | 24 h | a `not observed` answer is never reused | 2 brew spawns, about 1.3 s | none |
| mise reports (prune dry run, global list) | none: they read configuration outside mise's units | asked every pass | recomputed | 2 spawns, about 0.1 s | unchanged |
| rustup default toolchain | none (one settings-file read) | every pass | recomputed | one file read | unchanged |
| Agent identification | per file: adapter version, length, mtime, ctime, inode; per container: shape key and the event window | none | stored as a fingerprint-only row | replayed | unchanged |
| Git facts per worktree | worktree id under the event window | a full walk recomputes them | carried forward | replayed | unchanged |
| Docker objects | daemon answer, `cached_at` | 300 s (60 s unavailable); a default `observe` asks live | stored, 60 s | 7 spawns, 1.3 s (3 s at background priority) | unchanged: daemon state has no content key swamp can read |
| Last used | none (key-file access times change on read) | every pass | recomputed | bounded scan per unit (4096 listings) | unchanged |
| Volume ledger system facts | the ledger's own cycle (24 h, 120 s budget) | not run until due | not stored | none when not due; 3 spawns per resume while a cycle is open | unchanged |
| External unit sizes | event window, per unit and per immediate subfolder | none | an incomplete fold is stored marked as a lower bound: never replayed whole, its readable subfolders replayed | busy units walked whole (`~/Library/Caches` 163k files) | only changed subfolders walked |

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

Protection is explicit user intent, not inferred from age. Recognized data with unsupported lifecycle semantics is named as such on the confirm (no rule, no record of what getting it back costs), and a person may still move it to Trash. Conversation history and checkpoints are not described as rebuildable merely because an agent created them.

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
- **Removal is human-confirmed.** What you see and own you may move to Trash: the TUI confirms every path, states what swamp does not know, and refuses only a path that is not a real deletable folder or file, an OS refusal, a plan that changed since you marked it (Reclaim/External/Disk moves recheck the entry and its place at Enter), an unwritable ledger, an overlap, or your own `swamp protect` mark. There is no CLI deletion command.
- **Trash retains storage.** Moving a path to Trash is recoverable but does not free its blocks. Docker removals do not use Trash.
- **Every program from fixed locations, never `PATH` (#199).** `fs_gate::program_paths::candidates` lists each allow-listed program's absolute locations per platform; `plan` takes the first that exists, checks file and directory (owner you or root; no world-write; group-write only for the macOS `admin` group), and refuses rather than skipping it; none present is "not available". `brew`/`mise` get a scrubbed environment, everything else the inherited one with `arg0` set to the program's name. Test builds (`testing` feature) take fakes only from `SWAMP_TEST_PROGRAM_DIR`.
- **Tool-managed removal has no undo, so it keeps its review-to-`Y` recheck; its advisory facts are warnings, its correctness refusals stay.** `crate::tool_removal` removes one mise version or one simulator runtime through the manager's own command, after the manager's own dry run, from the TUI only (`tool_removal::execute` may be named only by `crates/tui` actions). The manager binary is a `fs_gate::spawn::ToolBin`, resolved by `fs_gate::program_paths` (the one resolver and child-environment builder, shared with the manager probe): a fixed list of directories, never `PATH`; the first candidate that exists must be owned by the user or root and not writable by others (its directory may be group-writable only for the macOS `admin` group), or it refuses; an environment built from nothing; read-only tool invocations and removals are two separate shape allow-lists, and a dry-run shape without its flag is a removal shape. `Y` (never Enter) re-runs the whole review and refuses on any change. `.oh/guardrails/tool-removal-refuses-on-manager-facts.md` lists what refuses and what only warns; a test build panics on any tool spawn outside its sandbox of fake managers.
- **Identity can change.** Moving paths or changing remotes can interrupt attribution and history continuity.
- **No atomic filesystem snapshot.** Files may change while observed; multi-table storage is not a transactional snapshot of a live filesystem.
- **Linux needs continuous event coverage for incremental reuse.** Without it, Swamp walks fully rather than guessing.
- **No fixed performance guarantee.** Changed-container count, table size, enrichment, filesystem behavior, and reconciliation affect cost.

For the developer workflow rather than implementation detail, start with [usage](usage.md).
