---
id: tui-actions-off-event-thread
severity: hard
statement: "Cleanup review and execution must not run synchronously on the TUI event/render path; workers report progress and stop between groups."
outcome: disk-growth-by-project
audit: tui_event_thread_has_no_gate_calls
runtime_tests:
  - crates/tui/src/app.rs::tests::background_delete_finishes_and_worker_failure_is_visible
---

## Rationale

A 617-group cleanup froze the UI for 82 seconds. Correct filesystem outcomes
did not make a frozen confirmation screen acceptable. Group marking can perform
expensive checks too; both review and execution belong on background workers.

## Detection

Mechanism: type, gate audit, runtime test.

**Gate audit.** `tui_event_thread_has_no_gate_calls`: from the TUI's `event_loop` (and every workspace impl of a trait the compiler calls implicitly -- `Drop`, `Display`, `Deref`, operators), over every path, UFCS and method-name edge except those inside `worker::spawn` closures, nothing reaches a blocking gate capability (`destroy`, `spawn`, bounded reads, `read_dir`); a call whose callee is not a path is rejected.

Retired 2026-09-22: the `tui_actions_off_event_thread` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

(2026-09-23: this section's `Type.` line and its `human_confirmation_is_not_a_struct_literal` compile-fail case described `HumanConfirmed`, which is deleted along with the rest of the CLI action path -- see `.oh/guardrails/human-only-authorization.md`. What this guardrail is actually about, the TUI's worker/event-thread boundary, is unaffected: `execute_one`/`execute_plan_progress` still run only inside `worker::spawn`, never on the render/event thread.)

## Limits and runtime checks

This is a conservative name-based graph, not compiler type resolution. It can
conflate same-named methods; macros, dynamic dispatch, new external blocking APIs,
local import aliases and indirect worker handles need review. Join checks recognize
receivers named with `handle` or `worker`, not all possible JoinHandle values;
string joining remains allowed. The explicit sink list and worker-boundary syntax
must evolve with the architecture. This audit does not claim all UI I/O is absent.

Keep runtime tests for cancellation, partial outcomes, ledger preservation and
responsive progress rendering. Static structure alone cannot prove responsiveness
or correct cancellation semantics.
