---
id: agent-adapters-do-not-traverse
severity: hard
statement: "Adapters do not walk the filesystem. Directory structure reaches them through the folded walk rows in their identification context, or through the capped locations::shallow_list helper for the single-level listings a layout genuinely requires."
outcome: disk-growth-by-project
audit: adapters_do_not_reach_gates, gate_paths_only_inside_gates
runtime_tests:
  - crates/core/tests/incremental_external_and_agent_measurement.rs
---

## Rationale

The adapter-scoped half of `no-second-traversal-on-report-path.md`. Each
of the fourteen adapters open-coded its own recursive `read_dir`, so a
report over a large tool home paid for the walk once in the folded walk
and again, per adapter, per call. Keeping this rule adapter-scoped as
well as path-scoped means a new adapter fails the build immediately
rather than quietly re-adding the cost.

## Detection

Mechanism: gate audit, clippy, runtime test.

**Gate audit.** A directory listing is `fs_gate::read_dir`, which only the walker modules may name (`gate_paths_only_inside_gates`); `Path::read_dir` in any spelling -- method or UFCS -- is a gate violation, and `adapters_do_not_reach_gates` confines adapters to `IdentifyCtx`, whose listing is the capped shallow list.

**Clippy.** `disallowed_methods` rejects `Path::read_dir` and `std::fs::read_dir` type-resolved in every non-gate module.

Retired 2026-09-22: the `agent_adapters_do_not_traverse` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

## Runtime tests that complete it

- `crates/core/tests/incremental_external_and_agent_measurement.rs`
