# Build-artifact understanding and selective action

## Aim

Explain why build/cache storage is large and what grew without manual filesystem investigation. Enable separately reviewed selective removal while retaining unrelated builds and dependencies. Contributes to W0 / outcome #40; not a replacement for full developer-storage coverage.

## Problem Space

The motivating Cargo build directory contained roughly 24 GiB across dependency/test executables, incremental caches, examples and release output. Multiple hashes and timestamps do not establish obsolescence. The missing capability is identification inside artifacts, not merely cleanup or additional scan roots. Identification units, historical measurement boundaries and atomic removal groups can differ.

Operate on existing/explicit roots and inventory. No dependency on completing global root discovery; maintain compatible shared identity/evidence seams with coverage work. Preserve incremental measurement, current+reverse-delta history, no duplicate accounting, source/freshness/unknowns, no project-code execution during scans, human authorization and precise action rechecks. No migration support or automatic cleanup.

Scope includes Rust/Cargo, Gradle/Maven, Node, Python/Go, Apple/Android build storage and Docker/BuildKit, with tested identification and action capability matrices. Generic does not mean every ecosystem exposes identical generation metadata. Named unavailable precision remains visible; omitted adapter implementation is not an unknown fact.

## Problem Weave

Selected local strategy BA0 has parent W0. All owners unassigned; implementation owners claim issues. Review trigger for every step: unsupported precision, false history, unsafe scope, unacceptable cost or no reduction in manual investigation.

| Step | Parent | Necessity | Parallel assumption | Outcome contribution |
|---|---|---|---|---|
| BA0 | W0 | Build-container totals alone do not explain accumulation | Identification/history and reviewed actions improve decisions | W1/W2/W3 |
| BA1 | BA0 | Users need to identify and inspect before choosing an action | Read-only domain evidence supplies useful role/variant attribution | W1/W2a |
| BA1b | BA0 | Nested views need truthful totals and change history | Existing incremental/reverse-delta machinery supports nested identities | W1b |
| BA2 | BA0 | Selective removal must preserve unrelated output | Identified groups and existing protections can enforce exact scope | W2b |
| BA3 | BA0 | Correct-looking rows alone do not establish utility or sustainable cost | Adversarial fixtures, measurements and human review expose failures | W3 |

**GBA:** BA1 + BA1b + BA2 + BA3, all required and selected. BA0 is one independently useful strategy contributing to W0; it does not claim sufficiency for the whole outcome. Original W1/W1b/W2a/W2b/W3 lineage remains on moved issues; it is not a dependency on all sibling strategies.

## Solution Space

**Selected:** shared nested identification/history/report contracts with ecosystem adapters, read-only inspection first and scoped selective action downstream. User authorized this as an independent epic under outcome #40. No schema/public API is mandated here; review one-way architecture decisions before execution.

**Rejected:** claiming generic age-based obsolescence (not age-based cleanup suggestions), whole-directory/native-clean-only as an identification substitute, Rust-only completion, and making all global-root discovery a delivery prerequisite.

**Deferred:** mandatory build wrapping and continuous whole-machine execution telemetry. Optional existing user-initiated build records can strengthen evidence without blocking baseline classification.

**Accepted:** ecosystem-specific maintenance, bounded metadata and explicit future-use/recovery uncertainty. Independent delivery does not waive protection, accounting, or full cross-ecosystem scope.

### Risk retirement

These are planned checks, not claims of passed validation.

- Retire by evidence: interleaved feature/toolchain/target fixtures must defeat newest-basename-wins; absent/stale build records must not become unused verdicts.
- Retire by evidence: nested/shared/hardlinked and metadata-only reclassification fixtures must defeat double counting and fake physical growth.
- Retire by evidence: active builders, stale proposals, symlink swaps, protected descendants and companion-file fixtures must defeat broad or racing deletion; validated rebuilds must preserve unrelated retained artifacts.
- Retire by evidence: no-change and one-artifact-change benchmarks must defeat full recursive rescanning and per-scan snapshot duplication.
- Accepted with rationale: future need and exact recovery/reclaimability may be unknowable; show limitations, require human decisions.
- Triggered: unsupported layout or native operation scope invalidates advertised precision; coarsen supported capability or refuse the action without hiding the identification.

### S&T Selection

BA0, BA1, BA1b, BA2 and BA3 are all selected. Parent W0 remains selected. GBA is complete; global G0/G1/G2 remain selected in the outcome session. Owners remain unassigned and review triggers above apply to every row.

## Plan

**Outcome:** [#40](https://github.com/open-horizon-labs/swamp/issues/40)
**Epic:** [#74](https://github.com/open-horizon-labs/swamp/issues/74)
**Updated:** 2026-09-19

| S&T Step | Disposition | Issue/Epic | Parent Step | Depends On |
|---|---|---|---|---|
| BA0 | selected | [#74](https://github.com/open-horizon-labs/swamp/issues/74) | W0 | its own children; not full #40 completion |
| BA1 | selected | [#64: Model nested artifact identities and roles independently of cleanup actions](https://github.com/open-horizon-labs/swamp/issues/64) | BA0 | none |
| BA1b | selected | [#65: Preserve nested artifact size and growth history without double counting](https://github.com/open-horizon-labs/swamp/issues/65) | BA0 | #64 |
| BA1 | selected | [#66: Identify Cargo test builds, examples, dependencies and incremental-cache variants](https://github.com/open-horizon-labs/swamp/issues/66) | BA0 | #64 |
| BA1 | selected | [#67: Identify Gradle and Maven build outputs and cache entries beneath their containers](https://github.com/open-horizon-labs/swamp/issues/67) | BA0 | #64 |
| BA1 | selected | [#68: Identify Node build outputs and shared cache entries with tool-specific roles](https://github.com/open-horizon-labs/swamp/issues/68) | BA0 | #64 |
| BA1 | selected | [#69: Identify Python and Go generated artifacts separately from installed and shared dependencies](https://github.com/open-horizon-labs/swamp/issues/69) | BA0 | #64 |
| BA1 | selected | [#70: Identify Apple and Android build products, tests and intermediates inside build storage](https://github.com/open-horizon-labs/swamp/issues/70) | BA0 | #64, #67 |
| BA1 | selected | [#71: Identify Docker and BuildKit cache records with shared-storage and producer evidence](https://github.com/open-horizon-labs/swamp/issues/71) | BA0 | #64 |
| BA1 | selected | [#72: Expose nested artifact identification and growth drill-down in CLI, TUI and MCP](https://github.com/open-horizon-labs/swamp/issues/72) | BA0 | #65, #66, #67, #68, #69, #70, #71 |
| BA2 | selected | [#73: Support reviewed selective removal of identified build artifact groups across ecosystems](https://github.com/open-horizon-labs/swamp/issues/73) | BA0 | #72 |
| BA3 | selected | [#75: Verify build-artifact accounting, incremental cost and selective-action safety independently](https://github.com/open-horizon-labs/swamp/issues/75) | BA0 | #72, #73 |
| BA3 | selected | [#76: Validate and document build-artifact explanation and cleanup as an independent workflow](https://github.com/open-horizon-labs/swamp/issues/76) | BA0 | #72, #73 |

## Execution Handoff

### Decision-contract clarification — 2026-09-20

User confirmed the reusable approach: **age + size + removal consequences**. Modification age is sufficient to recommend reviewing supported generated/cache units; neither last-access evidence nor proven supersession is required. Recent units remain selectable when supported, unknown/future ages do not rank as ancient, and advice does not authorize removal. Existing unique-data, occupancy and exact-scope protections remain.

Updated existing issue bodies, preserving selected lineage and dependencies: outcome #40; evidence #53/#54/#58; model #64; aggregation/history/storage #65; remaining ecosystem adapters #67–#71; presentation/action #72/#73; epic #74; validation #75/#76. No new epic or implementation was introduced. Cargo's current work is precedent, not cross-ecosystem completion.

Adapters identify units and provide timestamp semantics, size/accounting basis, consequences and action capability. Aggregation summarizes nonempty supported groups without parent/child duplication, retaining oldest-known candidate age and explicit unknown coverage. Storage reuses folded observations and current + reverse-delta Parquet; derive ages from timestamps, do not persist per-file inventories or generate byte deltas for evidence refresh. Deep inspection stays bounded/on demand. Architecture documentation records these as planned cross-ecosystem contracts, distinct from implemented Cargo guidance.

Acceptance includes collapsed-category usefulness, narrow/wide readability, mixed ages, overlapping groups, allocated versus reclaimable accounting, unchanged/changed-group costs and storage size. This clarification requires no new last-use oracle, parallel database or whole-tree refresh pass.

Start #64 independently. #65 and domain adapters follow; #70 reuses JVM/Gradle support from #67. #72 implements read-only views without cleanup; #73 supplies complete supported cleanup behavior and necessary shared protections. #75/#76 validate this epic independently. Outcome-wide #62/#63 remain separate, with #63 consuming #76's evidence later.

Coordinate #43/#53 shared contracts, #45–#49 root/manager facts, #57 relationships and #58–#61 recovery/presentation/actions. Reuse existing implementation or supply the minimum shared seam this epic needs; do not introduce duplicate stores or wait for unrelated catalog completion. If a missing shared prerequisite is truly substantial, split it explicitly under this epic and retain acceptance scope instead of silently removing it.

No action grants, cleanup or implementation are performed by planning. Human verification: Muness reviews explanation and decision usefulness; fixtures cannot substitute for that judgment. Full epic completion requires all adapters and its own validation; independently useful delivery is not permission to mark a Rust-only MVP complete.

## Reconciliation, 2026-09-21

#103/#104 landed: `crates/mcp` is removed. Where #72's linked table entry
and BA1 say "CLI, TUI and MCP", read that as "CLI (interactive and
`--json`), TUI, and the `skills/swamp/` agent skill" -- there is no MCP
server for #72's read-only views or #73's cleanup actions to expose
through. The domain requirement (collapsed-category usefulness,
allocated-vs-reclaimable accounting, unchanged/changed-group cost) is
unchanged.
