---
id: incremental-walk-only-changed-subtrees
severity: hard
statement: "The incremental path re-walks only the worktrees and folded artifacts FSEvents implicated and carries every other row forward from the store; it never calls the full parallel walk."
outcome: disk-growth-by-project
audit: none
audit_none_reason: "2026-09-22: which subtrees a replay re-walks is data flow no path-reference rule can check; the runtime tests assert it against a full walk"
compile_fail:
  - bus_stage_is_minted_by_the_bus
runtime_tests:
  - crates/core/tests/fsevents_incremental.rs::touching_one_artifact_resizes_only_that_row_and_matches_a_full_walk
  - crates/core/tests/fsevents_incremental.rs::deep_change_inside_a_folded_artifact_resizes_from_interior_rows_and_matches_a_full_walk
---

## Rationale
Incremental means incremental. If the incremental path can fall into a full walk for anything but a refusal, the store's anchor is decoration.

## Detection

Mechanism: runtime test.

**Runtime test.** The incremental tests change one artifact (and one file deep inside a folded artifact), replay, and assert that only that row was re-sized and that the result equals a full walk's, with the work counters bounding what was listed and statted.

Retired 2026-09-22: the `incremental_walk_only_changed_subtrees` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `bus_stage_is_minted_by_the_bus`.
