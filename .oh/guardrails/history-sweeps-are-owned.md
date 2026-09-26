---
id: history-sweeps-are-owned
severity: hard
statement: "A sweep of the shared current table may mark a row absent only when the row belongs to the observation's own key family and lies inside a region that observation covered completely this pass. Exclusion, a disabled detector, a missing or unreadable root, or simply not being part of this pass all mean the row is left alone. Coverage changes are not storage changes."
outcome: coverage-aware-storage-history
audit: gate_paths_only_inside_gates
compile_fail:
  - history_rows_are_private_to_the_store
runtime_tests:
  - crates/core/tests/reviewer_counterexamples.rs::unchanged_combined_observation_must_not_invent_regrowth
  - crates/core/tests/shared_history_ownership.rs
---

## Rationale

`unchanged_combined_observation_must_not_invent_regrowth`: external and
agent discovery share one current table (deliberately — one store, one
key family, two granularities), and each swept it for keys it had not
seen. Running them in sequence on a completely unchanged filesystem
therefore tombstoned the other family's rows, and the next pass recorded
the resurrection as regrowth. The TUI's startup calls them in exactly
that order.

Invented growth is the failure mode that destroys trust fastest: the
user sees a number move on a disk where nothing moved.

## Detection

Mechanism: type, gate audit, runtime test.

**Type.** `ArtifactHistory::tombstone(Owned<'_>, ..)` and `ExternalHistory::tombstone(Owned<'_>, ..)` are the only ways to mark a stored row absent; `Owned` has a private field and is minted only by `ObservationOwnership::claim`/`ArtifactOwnership::claim`, which answer from the key family and the regions this pass covered -- never from "this pass did not see it" alone.

**Gate audit.** `gate_paths_only_inside_gates`: `ObservationOwnership::new` is called only by the discovery passes (`external`, `agents`), from the roots they covered completely; an ownership window covering "everything" cannot be built elsewhere.

Retired 2026-09-22: the `history_sweeps_are_owned` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `history_rows_are_private_to_the_store`.

## Runtime tests that complete it

- `crates/core/tests/reviewer_counterexamples.rs::unchanged_combined_observation_must_not_invent_regrowth`
- `crates/core/tests/shared_history_ownership.rs` — external-then-agent
  and agent-then-external both yield zero regrowth and byte-identical
  stored rows; disabling a detector between observations yields coverage
  notes and no tombstones; re-enabling yields no regrowth; a real delete
  and recreate yields exactly one regrowth.
