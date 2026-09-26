---
id: reverse-delta-current-plus-deltas
severity: hard
statement: "The store holds one current-state file plus a reverse-delta log; an observation rewrites current and appends the previous values of changed rows as a delta, for artifacts, directories and files alike."
outcome: disk-growth-by-project
audit: none
audit_none_reason: "2026-09-22: the property is a type: history rows are written only through `ArtifactHistory`/`ExternalHistory` in a module private to `growth`"
compile_fail:
  - history_rows_are_private_to_the_store
runtime_tests:
  - crates/core/tests/report_growth.rs
---

## Rationale
Reverse deltas make the latest state a single read and history a replay backwards, which is what a sparkline or a growth window needs. The DuckDB→Go port once discarded this design; it does not get discarded again.

## Detection

Mechanism: type, runtime test.

**Type.** `growth::columns` is private to `growth`; `ArtifactHistory` and `ExternalHistory` load `current.parquet`, accumulate the reverse delta and `commit` both -- there is no other writer of history rows.

Retired 2026-09-22: the `reverse_delta_current_plus_deltas` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `history_rows_are_private_to_the_store`.
