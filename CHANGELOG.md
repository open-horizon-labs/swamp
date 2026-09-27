# Changelog

Release notes describe behavior at the named version. Older timings are individual
observations, not general performance guarantees. See the README for current use.

## v0.7.1

- Remove recognized retired store files automatically after a complete successful
  observation. The one-time housekeeping pass preserves configuration, protection,
  notes, current Parquet tables, compatible history, and active enrichment data.
- Serialize observation writers across CLI, scheduled, and TUI refresh paths.

## v0.7.0

Swamp 0.7.0 adds Linux support, toolchain and version-manager storage discovery,
and project-linked agent storage. It extends disk usage and history beyond
checkouts, with build details that help you choose what to keep or remove.

### See installed toolchains and which projects reference them

- Discover storage managed by mise, asdf, pyenv, uv, Conda, rbenv, RVM,
  ruby-install, nvm, and rustup, including supported location overrides.
- Distinguish installations, environments, downloads, caches, shims, and other
  manager state rather than treating each tool home as one unexplained total.
- Match project declarations such as `.tool-versions`, `mise.toml`,
  `.python-version`, `.ruby-version`, `.nvmrc`, and `rust-toolchain.toml` to
  measured mise/asdf/pyenv/rbenv/RVM/nvm/rustup installations. Show resolved
  references on both the project and the installation; keep rustup's global
  default separate from project declarations.
- These links identify declared consumers, not proven runtime use. An unmatched
  installation is not necessarily unused, and discovery does not imply that
  every installation supports cleanup. See the
  [tool-location catalog](https://github.com/open-horizon-labs/swamp/blob/v0.7.0/docs/locations.md).

### Run Swamp on Linux

- Linux x86_64 joins Apple silicon macOS as a supported release target, with
  a glibc-based archive built on Ubuntu 24.04 and tested on a newer Ubuntu runner.
- Linux uses inotify while the TUI or opt-in collector is running; uncovered
  intervals trigger a full walk. macOS continues to use persisted FSEvents.
- Linux scheduling uses systemd user timers; macOS uses LaunchAgent.
  Scheduled observation does not perform cleanup.

### Find growth across your development environment

- One effective scope combines defaults, enabled developer-tool detectors, and
  configured additions/exclusions. Inspect it with `swamp scope`, or pass explicit
  roots consistently to observation, reporting, and the TUI.
- External tool homes and agent storage have their own units and project links.
  Claude Code, Codex, Oh My Pi, OpenCode, and the wider agent catalog have
  adapter-specific coverage; missing or ambiguous ownership remains visible.
- Codex attribution reads its own thread-state database read-only rather than
  scanning conversation JSONL. Swamp's observation store remains Parquet.
- Multi-root reports preserve root coverage and observation-time history.
  Missing roots and newly added scope are not reported as ordinary storage changes.

### Understand the cleanup trade-off

- Building on 0.6's Cargo cleanup groups, shared role families extend build
  identification and consequences to other ecosystems. Cargo group/profile
  selection still expands to supported members, not the entire profile.
- Shared build-role families carry identification, age, accounting, and removal
  consequences into other ecosystem adapters. Project-local cleanup and
  shared-store inspection remain distinct capabilities.
- Age prioritizes review without claiming a build is obsolete. Shared hardlinks
  do not automatically prohibit removal or justify a promise about freed space.
- The TUI retains background observation, selection review, deletion progress,
  and cancellation between groups while extending the storage it can explain.
- On-demand Cargo inspection provides deeper bounded detail without making
  every ordinary refresh index individual dependency files.

### Keep repeated observation practical

- Folded directory measurements reuse unchanged containers. Current typed facts
  and reverse-delta history stay in compressed Parquet, without a separate JSON
  artifact cache or permanent inode inventory.
- Fast refresh retains explicitly stale unique-byte estimates instead of
  rewalking unchanged roots for hardlink accounting. `swamp observe --full`
  reconciles the observed scope and bounded container-sharing summaries.
- Report reads do not rescan directories or spawn enrichment processes.
  JSON build-unit pagination bounds detail while preserving full family summaries.
- Project/worktree deduplication, scope exclusions through path aliases, growth
  coverage, and text/JSON project filtering received regression fixes.

### Use the CLI and installable agent skill

- The agent interface is the CLI plus an installable skill. Install it through
  `npx skills add open-horizon-labs/swamp --skill swamp`;
  a bundled reference covers platform-appropriate binary installation.
  The MCP server and CLI deletion/approval commands are removed; removal is confirmed
  by a human in the TUI.

### Know the boundaries

History begins with observation. Ownership and cleanup coverage vary by adapter;
see the checked [build](https://github.com/open-horizon-labs/swamp/blob/v0.7.0/docs/build-artifacts.md) and
[agent-storage](https://github.com/open-horizon-labs/swamp/blob/v0.7.0/docs/agent-storage.md) matrices. Homebrew discovery is opt-in.

Filesystem cleanup moves paths to Trash, which must be emptied to reclaim space.
Docker image/volume removal has no Trash recovery. Allocated row sizes are not
guaranteed freed bytes, and cleanup does not re-check every fact after marking.

The [usage guide](https://github.com/open-horizon-labs/swamp/blob/v0.7.0/docs/usage.md) covers the workflow and controls. The rewritten
[architecture guide](https://github.com/open-horizon-labs/swamp/blob/v0.7.0/docs/architecture.md) explains folding, history, adapters,
enrichment, and the costs that remain.

## v0.6.3

- Show Cargo build details directly in project trees, grouped by cleanup consequence: compiler caches, compiled tests and examples, and build-script output. Keep the directory view available without counting it as additional storage.
- Show candidate counts, allocated sizes, modification ages and removal consequences. Use available terminal space for candidate previews; improve Unicode alignment, narrow layouts and scrolling.
- Allow selecting a cleanup category or profile to review its supported groups. Profile selection does not delete the entire profile directory or silently include unsupported outputs.
- Run cleanup review and execution in background workers. Show progress, completed/refused counts and the current path; Escape or Ctrl-C stops between groups without interrupting an in-flight move. Cancelled reviews preserve the prior selection, and failed or unattempted cleanup selections remain available for review.
- Add a repository-specific AST audit against known blocking cleanup/review calls on TUI event and rendering paths, with regression fixtures in workspace tests.
- Document build details with a real screenshot and explain age, shared-link accounting, selection scope and Trash behavior. Age suggests what to review; it does not prove disuse. Dependencies remain a folded aggregate, not a per-crate size map.

## v0.6.2

- Allow reviewed Cargo groups containing hardlinks to move to same-filesystem Trash. Unselected links remain intact; uncertain reclaimed space no longer blocks cleanup. Content, membership, identity, lock and authorization checks remain in place.
- Isolate observations, history and replay checkpoints by canonical scan root, fixing proposals after switching between a project and its parent in one store. Root aliases share a scope; each new scope starts its own baseline.
- Add `cleanup-check --offset` paging and `--within` discovery scope, candidate totals, unchecked counts and coverage limits. Show hardlink/accounting warnings with results so a small checked page cannot be mistaken for the total cleanup opportunity.

## v0.6.1

- Added `cleanup-check`: a bounded review of individual Cargo groups that produces unapproved plans or specific refusals, without widening selections or deleting anything.
- Distinguished category totals, unchecked groups and blocked outputs in Rust reports, JSON and the Builds TUI. Rust text reports show 30 rows by default (`--all` restores the complete list), with accounting guidance before the rows.
- Replaced confusing Cargo category-selection errors with instructions to choose individual groups. Added hardlink and lock reason codes, retry guidance, exact reviewed members and recovery details.
- Kept existing hardlink, freshness, lock and human-approval protections. This release improves cleanup discovery; it does not add support for removing hardlinked groups or prove that old builds are unused.

## v0.6.0

- Added Rust build drilldown for Cargo profiles, folded dependencies, test/example executables, incremental-cache and build-script groups, and final executables/libraries. Group history does not inflate project totals. Dependency sizes remain a directory aggregate, not a per-crate breakdown.
- Added reviewed selective cleanup for evidenced test/example executables and individual incremental/build-script groups. Cleanup checks contents, producer evidence, locks, hardlinks and occupancy before moving the selection to Trash with restore metadata. Shared dependencies and final outputs remain inspection-only; age is not proof that a build is unused.
- Kept ordinary compiler files folded in reports and history, with trusted unchanged-container reuse instead of a persisted per-file inventory.
- Made incremental hardlinked-artifact refreshes update directory allocations without a whole-artifact rewalk. Unique-byte totals are explicitly marked stale until a full scan reconciles them; stale unique measurements create history gaps and cannot spend standing-grant budgets.
- Compacted small reverse deltas and delayed replay-checkpoint publication until report persistence succeeds.
- Fixed Cargo cleanup lock lifetime under concurrent process creation. Added native Cargo build/cleanup/rebuild verification and parallel stress coverage.

## v0.5.2

- Narrowed Ruby dependency attribution to `vendor/bundle`, preserving unrelated vendored source.
- Added declarative project-name fallbacks for Gradle settings, Cabal, and Python `setup.cfg` manifests.
- Made strict JSON manifest names structural and top-level only.
- Added regressions for shared Rust workspace targets and independent nested project targets.

## v0.5.1

- Fixed project badges with linked worktree counts so the worktree glyph and multi-digit count remain visually separated in the terminal UI.

## v0.5.0

- Fixed persistence of existing artifact byte changes in `current.parquet`. Added regression coverage for successive updates and unchanged observations after an update.
- Renamed the repository, source packages, binaries, environment variables, and release artifacts to `swamp`. The v0.5.0 archive contains `swamp` and `swamp-mcp`.
- Reorganized the documentation into a product overview, usage reference, architecture guide, and contributor guide. Corrected outdated UI, filter, installation, and history claims.

## v0.4.0

### Docker removal

- Added image and volume removal to the CLI, MCP, and TUI through Docker. This includes objects without project attribution.
- Added object-specific recovery information to plans, confirmations, and ledger records. Filesystem paths go to Trash; Docker removals do not. Images may be pulled or rebuilt if their sources remain available; swamp makes no copy of volume contents.
- Refused individual build-cache removal because the action path does not support that unit.
- Added live object checks before removal and surfaced Docker's refusal text.
- Separated trashed bytes, permanent removals, and measured free-space change in results.
- Added a Docker fixture with attributed and unattributed images, a volume, dangling images, build cache, and a container that prevents image removal.

### Git tracking and actions

- Split the worktree remainder into tracked `source`, `ignored`, and `untracked` buckets using directory-level Git status with large-file corrections. Apportioned totals preserve the walk's measured bytes.
- Prevented deletion of the ignored/untracked aggregate buckets as single paths.
- Reused the Git exclude stack within a checkout.
- Allowed direct project actions to select its actionable artifacts. When none exist, a direct action can offer the checkout; bulk marking skips that fallback.

### Terminal UI

- Replaced row sparklines with logarithmic change bars: growth extends right in red, shrink left in green, and changes below 1 MB use a small tick.
- Sorted growth by signed value, so increases precede decreases.
- Made Right open/expand and Left collapse/return, matching the displayed navigation.
- Fixed negative growth formatting, duplicated carried-forward GitHub signals, and empty container names.

### Storage and checks

- Changed report-history Parquet writes to close a temporary file before renaming it over the destination. This protects readers from unfinished individual files; it is not a multi-file transaction.
- Corrected source checks that matched their own explanatory comments and removed their ripgrep dependency.
- Corrected the fixture's dangling-image case on Docker installations using the containerd image store.

## v0.3.0

### Live and incremental observation

- Added a live FSEvents watch while the TUI is open, with observation after 400 ms of quiet.
- Retained directory detail inside folded artifacts so updates can re-list changed interior directories.
- Added a whole-artifact fallback for hardlinked units and local byte measurements to support deduplicated incremental accounting.
- Re-listed known remainder directories without walking their entire worktree when possible.
- Built one history index per growth-annotation pass and skipped unchanged current-file writes.
- Reused and aged Git signals for untouched worktrees, cached Docker facts for five minutes, and limited discovery around changed directories.

The release recorded these observations on one `~/src` tree with 55 projects and about 44 GB. Hardware details, repeated samples, and a reproducible benchmark harness were not supplied with the table.

| Case | v0.2.0 | v0.3.0 |
|---|---|---|
| Source file touched | 7.5 s | 75 ms |
| Change inside a 16 GB `target/` | 7.5 s | 2.4 s |
| No change | 7.5 s | 86 ms |

### Artifact recognition

- Added recognition of regular `CACHEDIR.TAG` files with the required signature.
- Vendored ignore/language lists and added a harvest utility to report candidate artifact names without editing the classification table.
- Expanded marker-gated recognition across ecosystems, including ESP-IDF, Godot, Jekyll, Elm, Erlang, OCaml, Clojure, and Nim.
- Added ESP-IDF `managed_components/` and build variants, CMake build variants, and Python `requirements*` markers.

### UI and reporting

- Changed growth to red and shrink to green; used a dark selection background to preserve those colors.
- Added history charts based on observed changes and ecosystem badges after project names.
- Preserved selection during background refresh.
- Included directory rows in the startup observation.
- Corrected progress counters and limited percentage display to cases where the previous total is a usable estimate.
- Added `--version` and release-version smoke checks.

## v0.2.0

- Added project ecosystem detection, marker-gated artifact classification, type filters and sorting, a types view, and manifest-based names for repositories without remotes.
- Added size and age predicates, project globs, more sorts, and persisted UI choices.
- Added the keep-executables option for supported Rust and Python outputs.
- Added observation progress and UI refresh after removal.
- Forced a full walk after classification-rule changes.
- Added time-series display with unobserved buckets represented separately from zero changes.
- Reorganized report construction into consumers on an in-memory event bus. See [ADR 001](docs/ADRs/001-event-bus-report-pipeline.md).
- Added source audits for selected implementation constraints and a mutation script for walker checks.
- Added configuration commands and MCP report filtering.

## v0.1.0

- Introduced a terminal UI, CLI, and stdio MCP server for disk growth by project, checkout/worktree, and artifact.
- Added Parquet history with reverse deltas, FSEvents-based incremental observation, and optional scheduled observation.
- Grouped clones by remote and exposed worktree activity, Git tracking, and cached GitHub facts.
- Added Docker attribution by Compose/source evidence, with unmatched objects reported as unowned. Docker removal arrived in v0.4.0.
- Added filesystem reconciliation and an optional `du` comparison.
- Added action plans, CLI approval and standing grants, execution, and ledger records.

The initial release targeted Apple silicon macOS with unsigned binaries. History began with the first observation. Performance figures were observations from one machine.
