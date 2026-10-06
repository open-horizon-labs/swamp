---
id: tui-observation-is-explicit
severity: hard
statement: "The TUI starts scope observation only through explicit refresh or first startup without an index. Deletion and rendering apply known paths or read stored facts; worker wrappers must not bypass this boundary."
outcome: disk-growth-by-project
audit: tui_observation_is_explicit
runtime_tests:
  - crates/source-audit/tests/tui_observation.rs
  - crates/tui/src/app.rs::tests::deletion_updates_parent_and_cached_root_without_starting_an_observation
---

## Rationale

A successful deletion started the whole session's observation, including root history replay and tool and agent discovery. The deleted path was already known, but the user waited over sixty seconds for unrelated work. Moving that scan to a worker does not make it appropriate.

## Detection

Mechanism: gate audit, runtime test.

The registered AST audit fences each observation entry point by its enclosing function: `observe_scope` belongs only to `App::observe_in_background`; that method belongs only to `refresh_now` and `scan_if_no_index`; those entry points belong only to key dispatch and startup respectively. Live report and walker APIs are rejected throughout production TUI code. Stored report readers remain allowed. The existing CI audit command runs this rule automatically.

The rule examines resolved path references and method references, including aliases, function values, UFCS, macro tokens, and worker closures. Tests insert deletion calls, helper calls, worker wrappers and alternate live-report APIs and require rejection. Test-only code is exempt. Changes to approved entry points require updating this explicit boundary and its mutation checks.

## Limits

This is a reference boundary for the named observation and walker APIs, not proof of responsiveness or compiler type resolution. New scanning APIs must extend the boundary. Runtime checks still verify immediate pruning, correct size updates, terminal input, and cancellation.
