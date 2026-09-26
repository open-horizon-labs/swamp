---
id: event-bus-pluggable-consumers
severity: hard
statement: "The report pipeline is consumers on an in-memory event bus (tokio runtime, static registration, dynamic routing): no consumer imports another, no consumer registers consumers at runtime, and every report is assembled by EventBus::run."
outcome: disk-growth-by-project
audit: bus_static_registration
compile_fail:
  - bus_register_is_private
  - bus_has_no_empty_constructor
  - bus_stage_is_minted_by_the_bus
runtime_tests:
  - crates/core/src/bus/mod.rs::tests::builtins_cover_the_whole_pipeline_once_each
---

## Rationale
This is the repo-native-alignment architecture the tool was specified to follow, and it was not built the first time: `report_full_mode_with_source` grew into a thousand-line hardwired stage chain and each new enrichment (git signals, GitHub, Docker, ecosystems, tracking) was bolted into it. The bus is the only coupling between stages, so a new fact source is one file that registers itself.

## Detection

Mechanism: type, gate audit, runtime test.

**Type.** `EventBus::new` and `register` are private to `bus::registry`; `with_builtins` is the one registrar. Pipeline stages take a `bus::Stage` that only `EventBus::run` mints.

**Gate audit.** `bus_static_registration`: no consumer module names another consumer module or the `EventBus`, and nothing outside the consumers and the registrar names a path into one.

Retired 2026-09-22: the `event_bus_pluggable_consumers` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `bus_register_is_private`, `bus_has_no_empty_constructor`, `bus_stage_is_minted_by_the_bus`.
