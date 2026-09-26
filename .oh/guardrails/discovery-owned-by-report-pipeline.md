---
id: discovery-owned-by-report-pipeline
severity: hard
statement: "One observation owns discovery. Outside tests, only report.rs::observe_scope may run external::discover_and_measure or agents::discover_and_measure, and it must run both; CLI and TUI take their units from that observation."
outcome: coverage-aware-storage-history
audit: gate_paths_only_inside_gates
compile_fail:
  - discovery_pass_is_minted_by_observe_scope
  - discovery_pass_has_no_public_constructor
runtime_tests:
  - crates/core/tests/shared_history_ownership.rs
  - crates/tui/tests/scope_preserving_refresh.rs
  - crates/core/tests/store_contents_are_allowlisted.rs
---

## Rationale

Ordering only started to matter because there were several independent
passes over one shared history table: the CLI ran its own, the TUI ran
its own at startup, and `report` ran a third. That is how
`unchanged_combined_observation_must_not_invent_regrowth` became
possible at all, and why the TUI's agent/external views could be stale
while its header said "live". `ObservationOwnership` makes each sweep
safe; a single pass makes the question not arise.

## Detection

Mechanism: type, gate audit, runtime test.

**Type.** `external::discover_and_measure_in` and `agents::discover_and_measure_in` take a `&report::DiscoveryPass`; its field and `begin` are private to `report`, which mints one in `observe_scope`. The token lives in its own module (`report::pass`), so even the rest of `report` cannot build one as a literal; the test constructor exists only under the `testing` feature.

**Gate audit.** `gate_paths_only_inside_gates` pins `DiscoveryPass::begin` to one call site, `report::observe_scope`.

Retired 2026-09-22: the `discovery_owned_by_report_pipeline` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `discovery_pass_is_minted_by_observe_scope`, `discovery_pass_has_no_public_constructor`.

## Why this checks call sites and not visibility

As first written, this guardrail required both functions to be
`pub(crate)`. That is directly incompatible with the reviewers' own
mandatory `crates/core/tests/reviewer_counterexamples.rs`, which calls
both from an integration test -- from outside the crate. That file is
copied in byte-for-byte and may never be edited, so `pub(crate)` would
stop the required evidence compiling. The rule and the evidence could
not both be satisfied, and the evidence wins.

Visibility was never the property the review falsified. What it found
was two *passes* over one table in an order nobody declared. So the rule
is now that, precisely. **Inside the crate this is strictly stronger
than the visibility check it replaces**: `pub(crate)` permitted any
number of core-internal passes, and the call-site rule permits one.
Integration tests may still call either function directly, which is how
the counterexamples exercise them in isolation.

Decision taken 2026-09-21 by the integration owner, recorded here rather
than left as a failing audit with a footnote.

## Limits

- The audit reads call *syntax*, so a call reached through a function
  pointer or a trait object would not be seen. Nothing in this workspace
  does that, and the two functions are concrete free functions.
- It says nothing about how often `observe_scope` itself is called. A
  caller that calls it twice for the same scope in one command would
  pass this audit; `crates/core/tests/shared_history_ownership.rs` is
  what makes that harmless, because each pass carries its own
  `ObservationOwnership`.
- `ObservationParts` lets a caller ask for fewer parts. A part not
  observed is a part not swept, so that is a cost decision rather than a
  correctness one -- but it does mean "this row is absent" is only ever
  concluded from a pass that actually covered it.

## Runtime tests that complete it

- `crates/core/tests/shared_history_ownership.rs` -- both orders, zero
  fabricated regrowth; a disabled detector between observations produces
  no tombstones.
- `crates/tui/tests/scope_preserving_refresh.rs` -- the TUI's refreshes
  carry external and agent units from the same observation.
- `crates/core/tests/store_contents_are_allowlisted.rs` -- drives
  `observe_scope` end to end.
