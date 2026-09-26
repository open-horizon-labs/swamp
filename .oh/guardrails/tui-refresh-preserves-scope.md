---
id: tui-refresh-preserves-scope
severity: hard
statement: "Every TUI observation -- startup, background refresh, live watch, post-action re-observe -- goes through the scope-aware report path. Excluded subtrees and pruned external locations stay absent on refresh. External/agent unit vectors are refreshed only from an observation that covered the whole scope; a refresh narrowed to one root asks for no unit parts and leaves those vectors alone."
outcome: coverage-aware-storage-history
audit: gate_paths_only_inside_gates
compile_fail:
  - fs_events_testing_is_not_in_production
runtime_tests:
  - crates/tui/tests/scope_preserving_refresh.rs
  - crates/tui/tests/reviewer_counterexamples_stack2_tui.rs
  - crates/tui/tests/reviewer_counterexamples_stack2_tui.rs::a_live_refresh_of_one_root_must_not_empty_the_agent_view
---

## Rationale

Two P1 findings in the 2026-09-21 review:

- Startup used `report_scope_with_parts` with exclusions and external
  pruning, but `app.rs`'s background and live refreshes called
  single-root report functions that default `pruned_subtrees` to empty.
  A refresh could therefore reintroduce excluded data and double-count
  external bytes that startup had pruned.
- `finish_startup` was the only production caller updating the
  external/agent unit vectors, and `prune_removed` did not remove their
  successful action rows, so agent storage could stay stale while the
  header claimed the report was live.

The scope contract has to survive every update, not just the first
render.

## Detection

Mechanism: type, gate audit, runtime test.

**Gate audit.** `gate_paths_only_inside_gates`: the TUI may name only `report::observe_scope`, `load_last_report` and `merge_reports` among the report module's functions; a scopeless entry point (`report_full_mode`, `report_scope_with_parts`, a new `report_quick(root)`) named from the TUI is rejected.

**Runtime test.** The refresh tests start each TUI observation (startup, background, live) against a scope with an excluded subtree and a pruned external location and assert they stay absent, and that a one-root live refresh leaves the unit vectors alone.

**Type.** The live refresh replays a real `LivePlanSource`; the canned source is `testing`-only.

Retired 2026-09-22: the `tui_refresh_preserves_scope` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `fs_events_testing_is_not_in_production`.

## Runtime tests that complete it

- `crates/tui/tests/scope_preserving_refresh.rs` — an excluded subtree
  and an external location pruned from its parent root stay absent after
  a background refresh and after a successful action; `prune_removed`
  removes exactly the successful agent/external rows.
- `crates/tui/tests/reviewer_counterexamples_stack2_tui.rs::a_live_refresh_of_one_root_must_not_empty_the_agent_view`
  — the 2026-09-22 CE3, driving the real `observe_live` and applying its
  result exactly as `tui::lib`'s event loop does.
