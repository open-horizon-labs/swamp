---
id: walk-optimized-parallel-pool
severity: soft
statement: "The full walk runs on the work-stealing pool with folded units sized as parallel Size jobs; no serial walk on the report path."
outcome: disk-growth-by-project
audit: none
audit_none_reason: "2026-09-22: the serial `attribution::attribute` is test-only code (`#[cfg(test)]`), so production has no serial walk to call; the pool tests measure the parallel one"
runtime_tests:
  - crates/core/src/walk.rs::tests::pool_drains_under_cpu_contention
---

## Rationale
15 s → 7.5 s on ~/src came from the pool and from sizing folded units in parallel. The serial `attribution::attribute` walk exists for tests only.

## Detection

Mechanism: runtime test.

**Runtime test.** The pool test drains the work-stealing pool under CPU contention; the serial `attribute` exists only under `#[cfg(test)]`, so no production code can reach it.

Retired 2026-09-22: the `walk_optimized_parallel_pool` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.
