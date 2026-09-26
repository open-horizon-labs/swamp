---
id: coverage-changes-are-not-storage-changes
severity: hard
statement: "Changed scan coverage, incomplete observation, or lost access must not be reported as observed filesystem growth or deletion; overlapping coverage must not double-count measured bytes."
outcome: disk-growth-by-project
s_and_t_step: W1b
parent_step: W1
sufficiency_group: G1
owner: null
review_trigger: "A new detector, scope change, contradictory fact, or real user decision exposes a failure."
tactic_disposition: selected
audit: none
audit_none_reason: "2026-09-22: tombstones and regrowth bumps are writes to private rows that only an `Owned` claim unlocks, which is a type, not a source pattern"
compile_fail:
  - history_rows_are_private_to_the_store
runtime_tests:
  - crates/core/tests/coverage_changes_are_not_storage_changes.rs
  - crates/core/tests/scope_unique_accounting.rs
---

# Coverage changes are not storage changes

## Rationale

Preserve accounting and history across coverage changes so users can trust what
actually changed. Unobserved is not deleted. Newly discovered is not proof of
newly created. Removing an include, adding an exclusion, losing permissions, or
disconnecting a volume changes what is known, not necessarily what exists.

This constrains [coverage-aware storage history](../outcomes/coverage-aware-storage-history.md)
and the parent outcome. Shared/project relationship views must not inflate aggregate
totals, and measured bytes must not silently become an exact reclamation promise.

## Selected tactic and parallel assumption

Establish invariants for overlaps, partial scans, identity, and changing roots.
These distinctions are necessary; a particular persistence schema is not selected.
Deterministic fixtures can exercise the observation sequences before broad defaults
are enabled. That is a validation hypothesis, not a claim of completed checks.

## Detection

Mechanism: type, runtime test.

**Type.** Stored rows (`growth::columns::StoredRow` and friends) have private fields; a row is tombstoned only by `ArtifactHistory::tombstone`/`ExternalHistory::tombstone`, which take an `Owned<'_>` minted only by an ownership's `claim(key)` (this pass covered and observed the row's region), and regrowth is counted only inside `observe`. `row.present = false` or `row.regrowth_count.saturating_add(1)` outside that module does not compile.

Retired 2026-09-22: the `coverage_changes_are_not_storage_changes` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `history_rows_are_private_to_the_store`.

## Runtime tests that complete it

`crates/core/tests/coverage_changes_are_not_storage_changes.rs` — three
passes over an unchanged tree, with the coverage changing between them
(add an exclusion, disable a detector, make a location inaccessible,
switch to an explicit root) produce zero growth, zero regrowth and zero
tombstones. Run by name from `scripts/check.sh`.

(2026-09-23: this section used to also cite
`crates/core/tests/reviewer_counterexamples_stack2.rs::a_config_only_exclusion_must_not_invent_growth_or_regrowth`,
the 2026-09-22 re-review's CE4 -- excluding a nested Cargo location
reported the parent as having grown 64 KB and un-excluding it scored a
regrowth, with zero bytes changed on disk. That file is gone from this
branch with no deletion commit in its history; discovered as a stale
reference during stack/27's verification, unrelated to this chunk's own
changes. A follow-up should either restore that counterexample as a
test or find where its coverage moved to.)

## Validation gap

2026-09-26: the owner selected fast refresh with explicitly unreconciled
unique-byte estimates between explicit full reconciliations. The scope overlay
does not reassign history charges. `scope_unique_accounting.rs` covers scope
round trips, root order, cross-root links, changed-container/unchanged traversal
cost, and invalidation before a partial-family observation. Walker tests cover
device/inode keys, excluded paths, symlinks and a 20,000-entry sharing fixture.
Known missing roots remain coverage facts and do not erase historical rows;
the reconciled overlay describes the observed set, never a growth delta.

Concurrent mutation during a pass and shared filesystem extents (clones or
snapshots, not inode hardlinks) remain limits. Actual cross-device mounted
fixtures and per-row shared-with attribution remain unverified here. Check nested and
aliased roots, root-order changes, scope additions/removals, permission failures,
disconnected volumes, partial runs, and identity changes against actual filesystem
changes. Preserve the distinction between incomplete observations and tombstones.
Do not describe the overlay as exact reclaimable storage or atomic auditing.

The existing separate-root loop and volume-keyed state are evidence of a boundary
that needs checking, not a passed multi-root correctness test. Relevant source:
[observation loop](../../crates/cli/src/schedule.rs) and
[growth/history](../../crates/core/src/growth.rs).

## Lineage and invalidation

Source: 2026-09-19 problem weave, W1b, parent W1, depth 2, G1 (W1a + W1b, all
required). Technical framing; proposed contributing role is core engineering,
with no owner assigned. Recording the hard constraint did not select its tactic;
the subsequent [solution-space session](../sessions/2026-09-19-developer-storage-coverage-and-evidence.md)
does so without asserting implementation. False disappearance, inflated totals, or root-order-
dependent history invalidates the implementation against this constraint.
