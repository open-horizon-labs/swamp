---
id: fsevents-before-full-walk
severity: hard
statement: "An observation asks its platform's change-observation source what changed before it walks anything; a full walk happens only when the replay refuses, the store has no anchor, --full is passed, or the classification rules version changed. Where the platform has no persisted change history, the refusal names that rather than reporting a missing backend."
outcome: disk-growth-by-project
audit: none
audit_none_reason: "2026-09-22: ordering inside `stage_tracked_with_source` is control flow no path-reference rule can check; the runtime tests assert it, and the canned replay source that could bypass it is out of production"
compile_fail:
  - fs_events_testing_is_not_in_production
  - bus_stage_is_minted_by_the_bus
runtime_tests:
  - crates/core/tests/fsevents_incremental.rs
  - crates/core/tests/unit_root_event_cursors.rs
---

## Rationale
The whole point of persisting an event id is that the next observation is a replay plus a re-walk of the implicated subtrees, not a fresh walk. On ~/src (41 GB, 107 worktrees) a full walk is ~7 s; the replay is ~0.1 s.

## Detection

Mechanism: type, runtime test.

**Runtime test.** `fsevents_incremental.rs` asserts that an unchanged tree is answered from the replay without a walk, that every refusal reason (and an older rules version, even with no stored event id) falls back to a full walk, and that `--full` forces one.

**Type.** The canned replay source exists only under the `testing` feature, so the TUI's live refresh replays a real plan (`fs_events::LivePlanSource`); walks take a `bus::Stage`. The ordering rule is the same on both platforms -- only which source answers differs. Which refusal a platform without replay gives is decided in one place, from `platform::ContinuitySource`, and audit `platform_capabilities_gate_their_backends` fails a second copy of that decision.

Retired 2026-09-22: the `fsevents_before_full_walk` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `fs_events_testing_is_not_in_production`, `bus_stage_is_minted_by_the_bus`.

## Limits
On Linux the replay always refuses (`no_persisted_change_history`), so the ordering is satisfied trivially there and this guardrail buys nothing until #81 (the live watch) narrows it. It is still the right shape: it is what makes the live watch a new source rather than a new code path.
