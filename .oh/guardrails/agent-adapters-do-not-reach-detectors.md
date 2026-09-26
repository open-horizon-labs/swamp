---
id: agent-adapters-do-not-reach-detectors
severity: hard
statement: "Adapters reference nothing under crate::locations except neutral vocabulary types (StorageCategory, Platform, Provenance). Detector ids and home resolution live in locations/."
outcome: coverage-aware-storage-history
audit: adapters_do_not_reach_gates
runtime_tests:
  - crates/core/tests/agent_matrix_matches_docs.rs
---

## Rationale

Every adapter currently defines its tool id as an alias of its
detector's id constant. That reads harmlessly and creates a cycle:
identification depends on detection's naming, so the registry cannot be
the single place that decides which tool an authorized home belongs to,
and the scope layer's authority over "what is in scope" leaks into the
adapter layer. The review's scope findings all had this shape -- a
decision about scope being re-made somewhere that should only have been
told the answer.

## Detection

Mechanism: gate audit, runtime test.

**Gate audit.** `adapters_do_not_reach_gates`: no adapter names `locations` (resolved through `use`, renames and glob imports), so neither a detector nor a detector id constant is reachable from one.

Retired 2026-09-22: the `agent_adapters_do_not_reach_detectors` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

## Runtime tests that complete it

- `crates/core/tests/agent_matrix_matches_docs.rs`
