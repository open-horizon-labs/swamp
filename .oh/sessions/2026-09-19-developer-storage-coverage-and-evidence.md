# Developer-storage coverage and decision evidence

## Aim

Developers understand what grew and make justified keep / remove / investigate decisions without opening every project or reconstructing context by hand. Canonical outcome: [disk-growth-by-project](../outcomes/disk-growth-by-project.md), W0.

Mechanism hypothesis: understandable coverage, trustworthy history and relevant project/activity/recovery evidence reduce outside checks without misleading assurances. Success is fewer unnecessary external checks on real decisions, not more indexed paths. Scanning does not authorize removal. Human authorization and existing protections remain intact.

## Problem Space

The full requested scope is developer storage across configured roots and a built-in ecosystem catalog, with current decision evidence and trustworthy historical byte measurements. The user explicitly rejected an assistant-proposed bounded first increment: decomposition must not reduce scope to an MVP.

Base defaults: ~/src, ~/Library/Developer, ~/Library/Caches. Detect documented actual locations for mise, asdf, pyenv, uv, Conda, rbenv, RVM, ruby-install/chruby, nvm, rustup, Cargo, npm, pnpm, Gradle, Maven, Go, Python package caches, Xcode/Android SDK and simulator storage, Homebrew, Hugging Face, Ollama and Docker Desktop host backing storage. This is the discussed catalog minimum, not a claim to support every tool ever published. Arbitrary includes and an extensible catalog cover additional locations.

Default candidates are built into swamp, not copied into config. User config can add/exclude locations, disable discovery adapters or turn defaults off. Missing binaries do not prove absent storage. Manager overrides, shared roots and aliases matter. All interfaces must use the same effective scope.

Existing report artifacts mostly require project/worktree context; external/shared units need independent measurement identity. Current scheduled roots are observed separately against volume-keyed topology/history. Correct multi-root comparisons need observation completeness and scope-change semantics. Existing action recovery labels are broad per-kind assumptions, not instance-level proof.

Hard constraints: no double counting; unobserved is not deleted; unknown is not unused/safe; no execution of scanned project configuration; no silently expanded authorization; distinguish logical, allocated, estimated reclaimable and observed freed bytes. Retain incremental observation, current+reverse-delta storage, bus/consumer boundaries and facts-not-verdicts. No migration support is requested.

Evidence: prior source/official-document review plus current code inspection. No full-catalog integration, performance budget or decision-benefit benchmark has yet been demonstrated.

## Problem Weave

Source: 2026-09-19 conversation, three independent framing passes (user/workflow, system/correctness, evidence/trust) on a shared evidence contract. Coordinator supplied and verified relevant source context. The passes are independent framing, not independent factual verification.

### Hierarchy and provenance

- **W0** — parent none, depth 0: Make informed storage decisions with less investigation. Necessity: More coverage and signals must improve real keep/remove/investigate decisions. Parallel assumption: Understandable coverage, history and consequence evidence can reduce outside checks.
- **W1** — parent W0, depth 1: Know what was measured and what changed. Necessity: Ambiguous totals and coverage changes undermine storage decisions. Parallel assumption: Existing incremental observation and reverse-delta history provide a foundation.
- **W1a** — parent W1, depth 2: Understand and control coverage. Necessity: Users must distinguish unobserved storage from absent storage. Parallel assumption: Built-in detection plus explicit config differences can cover developer storage without a hand-maintained directory list.
- **W1b** — parent W1, depth 2: Preserve accounting and history across coverage changes. Necessity: Overlaps, missing access and scope changes must not fabricate storage changes. Parallel assumption: Explicit observation boundaries and adversarial fixtures can establish these semantics.
- **W2** — parent W0, depth 1: Understand the consequences of a decision. Necessity: Inventory alone does not explain the effects of removal. Parallel assumption: Existing project and action context can be extended with domain-specific evidence.
- **W2a** — parent W2, depth 2: Separate consumers, activity and current-use evidence. Necessity: Referenced, accessed, modified and running establish different facts. Parallel assumption: Domain sources can narrow uncertainty without pretending to provide a universal last-use ledger.
- **W2b** — parent W2, depth 2: Explain recovery conditions and consequential unknowns. Necessity: Rebuildable or unknown alone does not support a justified choice. Parallel assumption: Restoration prerequisites, reclaimability limits and focused checks can make incomplete evidence useful.
- **W3** — parent W0, depth 1: Keep the process worth using routinely. Necessity: Correct but expensive or unhelpful reports do not achieve W0. Parallel assumption: Representative decision episodes and repeatable observation workloads can expose regressions and missing context.

W1b is primarily technical framing; W2a primarily evidence/trust; W2b combines user and evidence passes; W3 combines all three. W1a carries the user's explicit preference. Necessity is for the enabling condition, not a claim that one implementation is uniquely indispensable.

### Sufficiency groups

| Group | Parent | Children | Mode | Collective claim | Known gap |
|---|---|---|---|---|---|
| G0 | W0 | W1, W2, W3 | all required | Trustworthy measurements, consequence evidence and routine usefulness support real decisions | Human decision benefit must be observed |
| G1 | W1 | W1a, W1b | all required | Inspectable/controllable scope plus sound accounting/history makes coverage useful | Concurrent mutation and exact physical sharing have platform limits |
| G2 | W2 | W2a, W2b | all required | Consumer/activity facts plus recovery and consequential unknowns explain choices | Future need is not universally observable |

### Relationships and opposition

- Dependency: broad defaults depend on correct multi-root observation boundaries; new visibility must not become fabricated byte growth.
- Complement: measurement remains useful before project attribution; shared/unattributed is a first-class state.
- Overlap: actionable unknowns combine workflow and evidence framing, but fact collection and its presentation remain separate changes.
- Trade-off: evolving default coverage versus predictable scope. Selected policy: defaults-following users get catalog additions, with explicit scope provenance/change notices and honest history baselines; defaults=false provides explicit scope.
- Trade-off: stronger activity/current-use evidence versus collection cost and privacy. Collect supported read-only domain evidence with limits; no fabricated universal execution history.
- Tension: concise review versus honest qualifications. Surface material unknowns and provenance on inspection; validate comprehension rather than hide facts.
- No universal completeness, recovery or exact reclamation claim. Unknown states must not conceal unimplemented agreed adapters.

## Solution Space

### Analysis

**Problem:** Explain developer-storage growth and decision consequences across projects and external/shared tooling without manual reconstruction.
**Key constraint:** Broader coverage and richer evidence must not corrupt history or imply unsupported safety.
**Success signal:** Full agreed catalog and interfaces work together; multi-root adversarial cases preserve truthful totals/history; real decisions require fewer outside checks without false reassurance.
**Decision criteria:** Full requested scope, historical correctness, evidence honesty, user control, incremental cost, coherent interfaces, extensible domain integrations.
**Critical assumptions:** Read-only metadata/tool sources can provide useful partial evidence; explicit unknowns can help decisions; normal no-change observations can remain incremental.

### Candidates considered

| Option | Level | Approach | Main trade-off | Disposition |
|---|---|---|---|---|
| A | Band-aid | Append a hardcoded path list to today's single-root workflow | More coverage without reliable history or evidence | rejected |
| B | Reframe | Full coverage-aware developer-storage model, built-in catalog and config differences, domain evidence across interfaces | More coordinated work and explicit limits | selected |
| C | Redesign | Whole-machine continuous execution/file-access telemetry as the primary model | Greater monitoring cost/privacy surface; cannot reconstruct unrecorded past use | deferred as a mechanism, not as permission to omit activity evidence |
| D | Local optimum | Inventory-only or bounded first increment | Easier delivery but leaves requested scope incomplete | rejected by user |

Interpretive variety: A preserves the current scan frame; B separates scope, measurement and decision evidence; C substitutes telemetry; D reduces the outcome. Failure of B to reduce investigation despite correct data triggers a return to the problem frame, not simply collecting more fields.

### Risk retirement plan

These are planned dispositions, not claims that evidence already exists.

| Risk | Planned disposition | Tempting patch the check must fail | Required evidence/rationale | Stop/pivot trigger |
|---|---|---|---|---|
| Scope changes masquerade as growth/deletion | Retired by evidence, pending tests | Loop over roots against shared state; assume missing rows were deleted | Root reorder/overlap/add/remove/permission/disconnect/full-vs-incremental fixtures | False tombstone, duplicated bytes or order-dependent history |
| External identity depends on guessed ownership | Retired by evidence, pending tests | Put shared units under a synthetic project and duplicate references | Association-change and shared-consumer fixtures retain one measurement/history | Attribution changes byte identity or doubles totals |
| atime or missing config becomes unused verdict | Retired by evidence, pending tests | Rename mtime to last-used; treat empty references as no consumers | Read-only access, absent/stale source, ad-hoc usage and conflicting-evidence fixtures | Unsupported use/safety assertion |
| Detectors execute untrusted config | Retired by evidence, pending tests | Source shell/activate project environment to discover paths | Fixture config that would cause effects if executed; timeout/error checks | Project code execution or secret disclosure |
| Global catalog misses the requested families | Retired by evidence, pending catalog matrix | Implement one manager and declare extensibility complete | Every named family represented in detector and integration fixtures | Missing agreed family/interface passed off as unknown |
| Claimed recovery loses unique local data | Retired by evidence, pending tests | Blanket cache=local_rebuild | Local-only Maven, mutable device/volume, absent source/private dependency fixtures | Unsupported recovery guarantee |
| Size is advertised as exact freed space | Retired by evidence, pending tests | Sum directory or Docker apparent sizes | Shared/hardlink/sparse/Trash/selection-set fixtures with bounded estimates | Double count or unsupported exact reclaim promise |
| Defaults evolve unexpectedly | Accepted with rationale | N/A | User requested built-in defaults; visible scope changes, excludes, disabled detectors and defaults=false preserve control | Unexpected undisclosed scope expansion |
| Read-only sources cannot establish last intentional use or future need | Accepted with rationale | N/A | Preserve unknown and source semantics; no omniscience promise | UI or agents treat unknown as no use |
| Full implementation costs more than one short task | Accepted with rationale | N/A | Dependency-linked coherent issues; split oversized implementation tasks without dropping acceptance scope | Scope silently narrowed to MVP |
| Rich evidence does not improve decisions, or repeated cost is excessive | Triggered | More fields or full scans on every refresh | Real decision cases and repeatable full-catalog workloads, baseline/thresholds recorded honestly | Unchanged investigation, false confidence, or justified cost limits exceeded |

### Recommendation and accepted trade-offs

Select B for the **full scope**, not a first increment. Preserve current-only enrichment separately from historical size/presence; preserve domain-specific facts instead of a universal usage/safety score. Native location discovery can be useful without perfect associations. Every requested evidence category and interface must be implemented, even where a particular artifact legitimately yields a named unknown.

No automatic cleanup, new blanket authorization, migration machinery, privileged whole-disk crawler or speculative universal telemetry is selected. Existing action paths must carry the new evidence and rechecks; unsupported manager operations must be explicitly inspection-only, never generic recursive deletion.

Catalog scope is the complete named list above. Additional tool families are normal catalog growth, not a prerequisite to claim universal coverage. Evidence adapters verify current official formats during implementation; no guessed metadata schema is mandated.

### S&T Selection

| Step ID | Disposition | Parent | Sufficiency group | Owner | Responsible role | Review trigger |
|---|---|---|---|---|---|---|
| W0 | selected | none | G0 | unassigned | product/workflow | A new detector, scope change, contradictory fact, or real user decision exposes a failure. |
| W1 | selected | W0 | G0 | unassigned | core engineering | A new detector, scope change, contradictory fact, or real user decision exposes a failure. |
| W1a | selected | W1 | G1 | unassigned | product/workflow and core engineering | A new detector, scope change, contradictory fact, or real user decision exposes a failure. |
| W1b | selected | W1 | G1 | unassigned | core engineering | A new detector, scope change, contradictory fact, or real user decision exposes a failure. |
| W2 | selected | W0 | G0 | unassigned | domain/evidence engineering | A new detector, scope change, contradictory fact, or real user decision exposes a failure. |
| W2a | selected | W2 | G2 | unassigned | domain/evidence engineering | A new detector, scope change, contradictory fact, or real user decision exposes a failure. |
| W2b | selected | W2 | G2 | unassigned | domain/evidence and workflow | A new detector, scope change, contradictory fact, or real user decision exposes a failure. |
| W3 | selected | W0 | G0 | unassigned | product/workflow and performance | A new detector, scope change, contradictory fact, or real user decision exposes a failure. |

Owner remains unassigned, preserving recorded null ownership; responsible roles do not assign a person. All required parents and siblings are selected: G0, G1 and G2 are complete. Recording previously did not select tactics; this solution-space step now does. User authorized proceeding and explicitly required full scope. Implementation assignments occur on claim.

### Execution handoff

Preserve all accepted scope and guardrails. Use existing tests/audits and add the named adversarial checks; source audits alone cannot establish semantic evidence or multi-root behavior. No test is claimed passed by this planning record. Choose/review concrete data schema and public API details before implementation; if a one-way architecture commitment emerges, run dissent before execution.

Human verification: Muness reviews usefulness, recovery tolerance and whether unresolved facts still require opening projects. This is not substituted by agent judgment. If an implementation task exceeds a reasonable focused unit, split it with the same S&T lineage and full acceptance criteria rather than cutting its scope.

### Selected extension: identification inside artifacts, before cleanup

User-approved extension (September 19): explain what is inside a large build/cache artifact before discussing deletion. The motivating observation was roughly 24 GiB of Cargo output, including test/example executables and incremental caches across multiple variants and old/new crate names. Multiple hashed files and older timestamps establish neither supersession nor disuse.

Select a shared nested-artifact identification/report/history contract with ecosystem-specific adapters. Preserve the full catalog and all selected W0/W1/W1a/W1b/W2/W2a/W2b/W3 steps and their parents, groups, owners and review triggers; this refines their tactics, not a new strategy branch or Rust-only increment. W1 owns identification and read-only drill-down; W1b owns nested accounting/history; W2a owns build identity and sourced relationships; W2b owns selective-removal consequences; W3 validates explanation usefulness and cost. G0/G1/G2 remain complete.

Identification must expose role (tests, examples, dependencies, intermediates, outputs, caches), measured membership, producer/consumer relationships, build variants where knowable, source/freshness, size/growth and unknown residuals. Classification and evidence refresh must not fabricate historical byte changes or rewrite old observations. A cleanup boundary can differ from an identification unit and must be modeled separately.

Use Rust/Cargo, Gradle/Maven, Node/Python/Go, Apple/Android build output and Docker/BuildKit fixtures to establish cross-ecosystem behavior. The generic contract must also account for existing catalog families without pretending every store has build generations: preserve category/entry-level identification and named format/version limits. Fine-grained unsupported cases remain inspectable, not automatically deletable. No agreed ecosystem may be silently replaced with a generic placeholder.

Candidates: blanket age deletion rejected as an obsolete/safety inference (age remains an explicit user filter); native-clean-only rejected as insufficient identification/history; shared model plus domain adapters selected; optional ingestion of existing user-initiated build records selected as evidence, mandatory build wrapping or whole-machine telemetry deferred. Never execute project builds/configuration to discover artifacts. No schema/public API is fixed by this plan; review one-way design choices before implementation.

Risk retirement (planned, not verified): adversarial checks must reject keeping only the newest filename across feature/architecture/toolchain variants; deleting isolated files without associated metadata; treating cached/unmodified as unused; parent/child or hardlink double counts; reclassification appearing as growth; and preview/execution races. Use alternate build variants, renamed crates, missing/partial metadata, shared caches, failed/in-progress builds, protected descendants, symlink swaps and repeat-build fixtures. Defer/refuse removal when coordination or exact manager scope cannot be established; never broaden a reviewed action. Future need and universal recovery remain accepted unknowns, with explicit consequences. Trigger reconsideration if supported layouts cannot justify the advertised granularity or fine-grained scanning defeats incremental cost.

Success: answer “why is this build directory 24 GiB, and what grew?” across CLI/TUI/MCP with cleanup disabled; separately preview and authorize removal of identified older build groups while retaining unrelated variants/dependencies, with accurate limitations. Human verification remains Muness's review of explanation and decision usefulness. No cleanup is performed by planning.

### Outcome and independently deliverable strategy hierarchy

User correction: #40 is W0, a higher-level outcome, not one release-sized mega epic. Selected: extract build-artifact understanding/history and its inspection/cleanup UI into an independently deliverable epic contributing to W0. Preserve all original coverage/evidence commitments and existing W1/W2/W3 lineage; do not make completing the full scan-root catalog a prerequisite for understanding already-scanned build artifacts.

Local selected S&T under W0: BA0 = understand accumulated build storage and selectively act on identified units; BA1 = identify and explain roles/variants through read-only inspection; BA1b = preserve nested measurement/history; BA2 = review and execute exact selective build-artifact removal; BA3 = verify independent usefulness, safety and incremental cost. BA0 parent is W0; BA1/BA1b/BA2/BA3 parent is BA0. Sufficiency group GBA requires BA1 + BA1b + BA2 + BA3; all are selected. Owners remain unassigned; review trigger is unsupported identification/action precision, false historical changes, unacceptable cost or failure to reduce outside investigation. Existing W-step contributions remain on each issue as lineage, not as a requirement to finish every sibling implementation before delivering BA0.

Necessity/assumptions: BA1 makes opaque large directories explainable, using read-only domain evidence; BA1b makes changes trustworthy, using the existing incremental/reverse-delta pipeline; BA2 permits precise authorized action, using identified groups and existing action protections; BA3 establishes that this independent capability is useful and sustainable through fixtures, measurements and human review. Shared interfaces with coverage/evidence work require coordination, not duplicate stores or mutually blocking epics. Existing scanned/explicit roots and Docker inventory suffice as inputs; expanded automatic root discovery remains separately selected in W1a. Independent delivery does not drop any cross-ecosystem adapter or protection from BA0.

## Plan

**Updated:** 2026-09-19
**Outcome:** [#40: understand developer-storage growth and make informed storage decisions](https://github.com/open-horizon-labs/swamp/issues/40)
**Status:** #40 is the outcome tracker with 24 direct children: #41–#63 plus independent epic #74. Build-artifact issues #64–#73 and validation #75/#76 are 12 native children of #74. No implementation starts through this planning update.

| S&T Step | Disposition | Issue/Epic | Parent Step | Depends On |
|---|---|---|---|---|
| W0 | selected | [#40](https://github.com/open-horizon-labs/swamp/issues/40) | none | children below |
| W1a | selected | [#41: Resolve built-in scan defaults, includes, exclusions and detector overrides](https://github.com/open-horizon-labs/swamp/issues/41) | W1 | none |
| W1b | selected | [#42: Make multi-root observations and history coverage-aware](https://github.com/open-horizon-labs/swamp/issues/42) | W1 | #41 |
| W1 | selected | [#43: Model and persist shared and external storage units without fabricated project owners](https://github.com/open-horizon-labs/swamp/issues/43) | W0 | #42 |
| W1a | selected | [#44: Add a source-aware registry for developer-storage location detection](https://github.com/open-horizon-labs/swamp/issues/44) | W1 | #41 |
| W1a | selected | [#45: Detect mise, asdf, pyenv, uv and Conda installations and environments](https://github.com/open-horizon-labs/swamp/issues/45) | W1 | #44, #43 |
| W1a | selected | [#46: Detect Ruby, Node and Rust version-manager storage](https://github.com/open-horizon-labs/swamp/issues/46) | W1 | #44, #43 |
| W1a | selected | [#47: Detect shared dependency and build-cache stores across developer ecosystems](https://github.com/open-horizon-labs/swamp/issues/47) | W1 | #44, #43 |
| W1a | selected | [#48: Discover Xcode and Android build, SDK and simulator storage](https://github.com/open-horizon-labs/swamp/issues/48) | W1 | #44, #43 |
| W1a | selected | [#49: Discover Homebrew, local model stores and Docker host backing storage](https://github.com/open-horizon-labs/swamp/issues/49) | W1 | #44, #43 |
| W1a | selected | [#50: Use resolved multi-root scope in CLI reports and scheduled observation](https://github.com/open-horizon-labs/swamp/issues/50) | W1 | #42, #43, #45, #46, #47, #48, #49 |
| W1 | selected | [#51: Support multi-root reports, coverage inspection and live refresh in the TUI](https://github.com/open-horizon-labs/swamp/issues/51) | W0 | #50 |
| W1 | selected | [#52: Expose configured multi-root coverage and shared storage consistently over MCP](https://github.com/open-horizon-labs/swamp/issues/52) | W0 | #50 |
| W2a | selected | [#53: Add a current-state evidence contract with source, freshness and coverage](https://github.com/open-horizon-labs/swamp/issues/53) | W2 | #43 |
| W2a | selected | [#54: Collect and refresh last-activity evidence without conflating it with last use](https://github.com/open-horizon-labs/swamp/issues/54) | W2 | #53 |
| W2a | selected | [#55: Attach live-use and occupancy evidence to external and shared storage units](https://github.com/open-horizon-labs/swamp/issues/55) | W2 | #53, #48, #49 |
| W2a | selected | [#56: Associate installed tool versions with project declarations and configured defaults](https://github.com/open-horizon-labs/swamp/issues/56) | W2 | #53, #45, #46 |
| W2a | selected | [#57: Associate external build output and shared dependencies with known projects](https://github.com/open-horizon-labs/swamp/issues/57) | W2 | #53, #47, #48, #49 |
| W2b | selected | [#58: Replace blanket recovery labels with artifact-specific recovery evidence](https://github.com/open-horizon-labs/swamp/issues/58) | W2 | #53, #56, #57 |
| W2b | selected | [#59: Expose reclaimable-space estimates with shared-storage and filesystem limits](https://github.com/open-horizon-labs/swamp/issues/59) | W2 | #43, #53, #49 |
| W2 | selected | [#60: Present activity, consumers, recovery and evidence gaps in CLI, TUI and MCP](https://github.com/open-horizon-labs/swamp/issues/60) | W0 | #51, #52, #54, #55, #56, #57, #58, #59 |
| W2 | selected | [#61: Carry coverage and decision evidence through existing action review and execution](https://github.com/open-horizon-labs/swamp/issues/61) | W0 | #60 |
| W3 | selected | [#62: Verify incremental cost and history integrity across the full detector catalog](https://github.com/open-horizon-labs/swamp/issues/62) | W0 | #61 |
| W3 | selected | [#63: Validate real keep/remove/investigate decisions and document the complete behavior](https://github.com/open-horizon-labs/swamp/issues/63) | W0 | #62, #76 |


| BA0 | selected | [#74: Understand build artifacts, inspect their history, and selectively clean them up](https://github.com/open-horizon-labs/swamp/issues/74) | W0 | independent epic; start #64 |

W0/W1/W2 are shared parent strategies tracked by the epic and relevant child subsets. W1b/W2a remain hard guardrails; selected tactical work does not imply completed enforcement. W1a, W2b and W3 are fully selected and planned in this session despite not having separate outcome-family records.

### Handoff

Start coverage/evidence work with #41, or independent build-artifact work with #64; claim subsequent work only when its actual dependencies are satisfied. Every issue contains self-contained context, acceptance criteria, S&T lineage, accepted trade-offs and adversarial-check requirements. No claimed human decision benefit or full-catalog performance result exists yet. Do not close the epic on a reduced MVP.

### Wiring follow-up

Canonical W0, W1/W2 capabilities and W1b/W2a guardrails were updated from candidate to selected, preserving parent/group/owner/review-trigger fields. Separate durable records for W1a, W2b and W3 are not required to preserve planning lineage and were not created implicitly. The existing record set plus this session is the canonical handoff.

### Identification-first extension wiring (historical; superseded by independent epic)

Added native sub-issues #64–#73 and updated #40, #43, #53, #57–#63 without replacing existing scope or acceptance criteria. #64 establishes nested identification; #65 handles accounting/history; #66–#71 implement domain identification; #72 exposes read-only drill-down; #73 adds reviewed selective removal. #58/#59 now depend on #64, #60 on #72, and #62 on #73. Pure identification has no dependency on cleanup. Every new issue preserves the selected step's parent, sufficiency group, owner, necessity, parallel assumption and review trigger. #62/#63 retain full-catalog validation and add identification-only usefulness, non-Rust fixtures, variant retention and incremental cost. No checks are claimed passed by planning.

### Current independent-epic handoff

#40 now tracks the higher-level W0 outcome, not one indivisible delivery. #74 owns the complete cross-ecosystem build-artifact understanding, history, inspection and selective-cleanup capability. Its canonical selected S&T, dependency table and acceptance signals are in [build-artifact-understanding](2026-09-19-build-artifact-understanding.md). Existing W-step lineage is preserved as contribution to W0; local BA steps describe the independently sufficient epic. Native hierarchy is #40 → #74 → #64–#73/#75/#76. No original coverage/evidence commitments were dropped.

Removed artificial cross-epic blocking links: #64 no longer waits on #43/#53; build adapters use existing/explicit roots without waiting for global discovery; #72 does not wait for multi-root rollout; #73 reuses/implements the needed existing action protections without waiting for catalog-wide #58–#61. The sister issues now describe explicit shared-contract coordination. #62 validates coverage/evidence; #75/#76 validate the independent build epic; #63 later reviews combined outcome usefulness and depends on #62/#76. Missing safety behavior remains implementation work, never a permission to omit safeguards.

## Reconciliation, 2026-09-21

#103/#104 landed: `crates/mcp` is removed. #52 ("Expose configured
multi-root coverage and shared storage consistently over MCP") and #60
("Present activity, consumers, recovery and evidence gaps in CLI, TUI
and MCP") name a transport that no longer exists; see
`<scratchpad>/issue-reconciliation-mcp.md` for the proposed replacement
text (CLI JSON + `skills/swamp/` in place of an MCP surface, same
domain requirements otherwise). W1/W2's coverage/evidence requirements
above (multi-root scope, shared storage, activity/consumer/recovery
gaps) are unchanged.
