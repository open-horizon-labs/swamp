---
id: detector-ids-only-in-registry
severity: hard
statement: "Detector identity lives in locations/ (and the scope layer that consumes it). Consumers -- association wiring, recovery hints, reports, CLI, TUI -- match on capabilities the detector declares (Detector::manager_conventions(), Detector::recovery_hint()), never on detector id constants or id string literals."
outcome: coverage-aware-storage-history
audit: ids_only_in_their_module
runtime_tests:
  - crates/core/tests/external_units.rs
---

## Rationale

`consumer_wiring.rs` matched manager names onto detector ids by hand, so
adding a detector meant editing a table somewhere else or silently
getting no associations. The same pattern -- knowledge about a detector
held outside the detector -- is what let discovery read raw detector
output instead of the authorized scope, and what makes the "add a
detector, edit four files" failure mode of the adapter layer.

A detector that declares what it satisfies (`pyenv` ↔ `.python-version`)
can be added in one place.

## Detection

Mechanism: gate audit, runtime test.

**Gate audit.** `ids_only_in_their_module`: a detector id literal (the value of a `*_DETECTOR_ID`) appears only in its detector module and the registry -- in a `match`, a comparison or an item-level table alike -- and the same for a *path* to another module's `*_DETECTOR_ID` constant (reviewed exceptions: `scope`, which interprets detector output, and `locations::permitted`). The ids that are also an ecosystem's everyday name (`npm`, `go`, `maven`, ...) are a reviewed list in the rule.

Retired 2026-09-22: the `detector_ids_only_in_registry` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

## Runtime tests that complete it

- `crates/core/tests/external_units.rs` and the association tests, which
  assert a new detector's conventions produce associations without a
  wiring edit
