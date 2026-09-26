---
id: 001-event-bus-report-pipeline
status: implemented
validate:
  cargo_tests:
    - bus::tests::follow_on_events_are_routed_to_subscribers
    - bus::tests::follow_on_events_dispatch_depth_first
    - bus::tests::consumer_receives_events_regardless_of_registration_order
    - bus::tests::registration_is_closed_once_run_starts
  audits:
    - bus_static_registration
---

# ADR 001: Report construction through an in-memory event bus

Decision date: 2026-09-18. Status: implemented.

## Context

Report construction originally placed discovery, attribution, project grouping, Git signals, GitHub, Docker, ecosystems, tracking, history, and assembly in one orchestration function. Each new source of facts added another dependency to that function.

The pipeline needed explicit stage inputs and a common entry point for the CLI, TUI, and MCP server.

## Decision

Each stage implements `Consumer`, declares the `EventKind` values it subscribes to, and emits follow-on events. The event bus owns registration and dispatch. Consumers exchange typed payloads rather than calling one another.

`EventBus::with_builtins()` registers the consumers before dispatch. The registry is sealed once a run begins. The assembly gate waits for the project, signal, GitHub, ecosystem, and Docker inputs; the final report assembler waits for both tracking annotations and history.

The runtime is Tokio on one thread. Subscribers to an event are polled with `join_all`, and follow-on events are dispatched depth-first in registration order. This permits cooperative concurrency, but synchronous filesystem and subprocess calls inside a consumer still block that runtime thread. Traversal and selected enrichment functions provide their own concurrency.

See the [architecture guide](../architecture.md#observation-pipeline) for the current event diagram and source links.

## Extension contract

A new stage needs a consumer implementation and registration. It may also need new event payloads, assembly inputs, report fields, serialization, and interface changes. “One file plus one registration line” applies only when the existing event and report contracts already cover the new fact.

Consumers must not import other consumers or register stages during a run. Report entry points call the bus; they do not invoke pipeline stages directly.

## Consequences

- Stage dependencies are expressed in event types and subscriptions.
- The CLI, TUI, and MCP server share report construction.
- Missing or unavailable enrichment must produce an explicit result so assembly can finish.
- Intermediate payloads and drafts add allocation and copying; this design does not make report construction constant-cost.
- The event bus is local to a report run. It is not a durable queue or a daemon.
- Source audits check selected structural constraints; tests check routing and report behavior. Run them through [the contributor checks](../../CONTRIBUTING.md#checks).

The implemented types and dispatch loop are in [bus/mod.rs](../../crates/core/src/bus/mod.rs), with stages under [consumers](../../crates/core/src/consumers/).

## Reconciliation, 2026-09-21

`crates/mcp` (the "MCP server" named above) was removed (#104). The
decision this ADR records -- one event-bus pipeline shared by every
interface -- is unchanged and still implemented; only the set of
interfaces sharing it changed, from CLI/TUI/MCP to CLI/TUI, where the
CLI's `--json` output is now the agent-facing surface. The original
Context/Decision/Consequences text above is left as written to preserve
the record of what was decided and why on 2026-09-18.

## Validation, 2026-09-22

The four semantic source audits this ADR listed
(`no_consumer_knows_other_consumers`, `static_registration_only`,
`all_report_paths_through_bus`, `extractors_are_pluggable`) are retired
with the rest of the `syn` call-graph model
([architecture: capability gates](../architecture.md#capability-gates)).
What they asserted is now held by:

- **types** -- `EventBus::new` and `register` are private to
  `bus::registry`, so `with_builtins` is the only registrar; every
  pipeline stage (the walk included) takes a `bus::Stage` that only
  `EventBus::run` mints, so no report path can bypass the bus. The
  `bus_register_is_private`, `bus_has_no_empty_constructor` and
  `bus_stage_is_minted_by_the_bus` compile-fail cases pin both;
- **one exact audit** -- `bus_static_registration`: no consumer module
  names another, and nothing outside the consumers and the registrar
  names a path into one.
