---
id: extractors-are-pluggable
severity: hard
statement: "A new fact source (a Docker extractor, a GitHub enricher, an ecosystem detector) is a consumer registered in EventBus::with_builtins; it never requires a change to the walk or to report assembly."
outcome: disk-growth-by-project
audit: bus_static_registration, no_unreferenced_public_items
compile_fail:
  - bus_register_is_private
runtime_tests:
  - crates/core/src/bus/mod.rs::tests::builtins_cover_the_whole_pipeline_once_each
---

## Detection

Mechanism: type, gate audit, runtime test.

**Type.** Registration is private to the bus's registrar (`EventBus::with_builtins`).

**Gate audit.** `bus_static_registration`: outside the consumer modules and the registrar, nothing names a path into a consumer module, so the walk and report assembly cannot grow a fact source of their own; a consumer that is written but never registered is a public type nothing names (`no_unreferenced_public_items`).

Retired 2026-09-22: the `extractors_are_pluggable` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `bus_register_is_private`.
