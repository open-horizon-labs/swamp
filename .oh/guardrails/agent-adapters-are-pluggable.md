---
id: agent-adapters-are-pluggable
severity: hard
statement: "Agent-tool adapters are independent and statically registered, exactly as location detectors already are. No adapter names another adapter; no central match over tool-id constants decides behaviour; every adapter module is registered exactly once; the registry's tool ids and the support matrix's tool ids are the same set."
outcome: disk-growth-by-project
audit: ids_only_in_their_module, adapters_do_not_reach_gates
runtime_tests:
  - crates/core/tests/agent_matrix_matches_docs.rs
---

## Rationale

Found 2026-09-21: `agents/mod.rs::identify_for_tool` is a fourteen-arm
`match tool_id` over concrete adapter modules; `multi_location_tool`
hardcodes Cline and Roo Code; Aider has a bespoke path threaded through
`discover_and_measure`; and `actions.rs::execute_agent_session_removal`
duplicates the same fourteen arms for re-identification. Adding the
fifteenth tool means editing four unrelated places, and forgetting one
of them produces a tool that identifies but cannot be rechecked at
execution — a safety boundary that silently does not cover a tool.

`agents/pi.rs` additionally fell back to Oh My Pi's header shape. A
change to Oh My Pi's format would then change Pi's identification, for
no reason a reader of either file could see.

Location detectors already solved this with a `Detector` trait and a
static registry. Adapters follow.

## Detection

Mechanism: gate audit, runtime test.

**Gate audit.** `ids_only_in_their_module`: a tool id literal (the value of an adapter's `*_TOOL_ID`) appears only in that adapter's module and the registries, never in a central `match`, if-chain or table -- compared as the exact literal, wherever it is written (the CLI included), and no module names another adapter's `*_TOOL_ID` constant. `adapters_do_not_reach_gates`: no tool adapter names another tool adapter's module (shared mechanics live in a family module that declares no tool id).

Retired 2026-09-22: the `agent_adapters_are_pluggable` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

## What replaced the matches

- `agents::AgentAdapter` (`id`, `name`, `capabilities`, `identify`,
  `reidentify`, `project_local_units`) with static registration in
  `agents::registry::Registry::with_builtins`.
- `multi_location_tool`'s hardcoded Cline/Roo Code match became
  `AdapterCapabilities::decomposes_every_location`, declared by the
  adapter.
- Aider's bespoke per-repo path became
  `AdapterCapabilities::project_local_units` plus
  `AgentAdapter::project_local_units`.
- `actions.rs`'s duplicate fourteen-arm match became
  `agents::reidentify_for_tool`, one registry lookup, which runs with
  the identification cache **disabled** so an approval is never spent
  against a cached derivation.
- Pi's fallback to Oh My Pi's header shape is gone: the shared
  byte-offset mechanics live in the neutral `pi_family.rs`, each adapter
  passes only the layouts its own tool documents, and a header an
  adapter cannot parse is an explicit unknown-format outcome.

## Runtime tests that complete it

- `crates/core/tests/agent_matrix_matches_docs.rs` — the published
  support matrix, parsed back and compared with the matrix constant and
  the registry's ids.
- `agents::registry::tests` — exactly-once registration, registry ids ==
  matrix ids, every adapter id is also a detector id, and the two
  capabilities are declared by exactly the adapters that should have
  them (so replacing the hardcoded match did not quietly widen it).
- each adapter's own five required tests (see
  `agent-adapter-test-contract.md`)
