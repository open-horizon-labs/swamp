//! target: docs/ADRs/001-event-bus-report-pipeline.md
//! mode: replace
//! by: audit:guardrail_metadata
//! why: an ADR claiming an audit validates it, where no such audit is registered
---
id: 001-event-bus-report-pipeline
status: implemented
validate:
  cargo_tests:
    - bus::tests::follow_on_events_are_routed_to_subscribers
  audits:
    - consumers_are_pluggable
---

# Event bus report pipeline

The `audits:` entry above resolves to nothing, so this ADR's claim that
it is validated is unbacked.
