//! target: docs/ADRs/001-event-bus-report-pipeline.md
//! mode: replace
//! by: audit:guardrail_metadata
//! why: an ADR naming a cargo test that no source file defines -- a validation reference that executes nothing
---
id: 001-event-bus-report-pipeline
status: implemented
validate:
  cargo_tests:
    - bus::tests::the_bus_is_definitely_correct
  audits:
    - static_registration_only
---

# Event bus report pipeline

The named test does not exist anywhere in the workspace.
