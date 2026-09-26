---
id: discovery-consumes-effective-scope
severity: hard
statement: "Only crate::scope interprets detector candidates. Discovery and measurement consume EffectiveScope::authorized_roots(), which has already applied exclusions, disabled detectors and explicit-root replacement. An excluded home yields zero units; a disabled detector yields zero units; a missing or unreadable root yields a coverage note and zero units, never a tombstone."
outcome: coverage-aware-storage-history
audit: none
audit_none_reason: "2026-09-22: the property is a type: raw detector candidates are private to `scope`, and resolving locations takes a `PermittedDetectors` built only from the config"
compile_fail:
  - permitted_detectors_come_from_config
  - permitted_detectors_are_not_a_literal
  - detector_candidates_are_private
  - detector_candidates_cannot_be_destructured
runtime_tests:
  - crates/core/tests/reviewer_counterexamples.rs::excluded_agent_home_must_not_be_scanned
  - crates/core/src/scope.rs::tests::authorized_roots_drops_excluded_and_explains_missing
  - crates/core/src/scope.rs::tests::explicit_roots_do_not_authorize_detector_paths_outside_them
  - crates/core/tests/explicit_root_scope_exclusions.rs
---

## Rationale

`excluded_agent_home_must_not_be_scanned` in the 2026-09-21 review:
excluding the entire Claude Code home still produced one agent unit and
one external unit. Both `external::discover_and_measure` and
`agents::discover_and_measure` iterated `scope.detectors`' raw
`Resolved` candidates, which is detector *output*, not authorized
scope — so no exclusion, no disabled detector and no explicit-root
replacement could reach them. The CLI and TUI compounded it by
re-resolving the scope with empty explicit roots, widening commands the
user had deliberately narrowed.

An exclusion the tool accepts and then ignores is worse than an
exclusion it refuses.

## Detection

Mechanism: type, runtime test.

**Type.** `DetectorSummary::locations` is private (reading or destructuring it outside `scope` does not compile); `Registry::resolve` takes a `locations::permitted::PermittedDetectors`, which only `from_config` builds.

Retired 2026-09-22: the `discovery_consumes_effective_scope` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `permitted_detectors_come_from_config`, `permitted_detectors_are_not_a_literal`, `detector_candidates_are_private`, `detector_candidates_cannot_be_destructured`.

## Runtime tests that complete it

- `crates/core/tests/reviewer_counterexamples.rs::excluded_agent_home_must_not_be_scanned`
- `crates/core/src/scope.rs::tests::authorized_roots_drops_excluded_and_explains_missing`
- `crates/core/src/scope.rs::tests::explicit_roots_do_not_authorize_detector_paths_outside_them`
  (2026-09-23: this section used to also cite
  `crates/core/tests/reviewer_counterexamples_stack2.rs::an_excluded_home_must_stay_excluded_under_an_explicit_root`,
  the CE1 case the prior test could not see because it passed `&[]` for
  explicit roots -- that file is gone from this branch with no deletion
  commit in its history; discovered as a stale reference during
  stack/27's verification, unrelated to this chunk's own changes.)
- `crates/core/tests/explicit_root_scope_exclusions.rs` — every
  exclusion case (an excluded home, an excluded nested location, a
  disabled detector, a protected path in either spelling) under an
  explicit root that *contains* it.
- `crates/cli/tests/` — `report <root> --view agents --json` on a
  fixture root that does not contain the agent home returns zero units
  with a coverage note saying why.
