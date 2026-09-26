//! target: .oh/guardrails/no-second-traversal-on-report-path.md
//! mode: replace
//! by: audit:guardrail_metadata
//! why: a guardrail whose `audit:` names nothing that exists -- the guardrail reads as watched and is not
---
id: no-second-traversal-on-report-path
severity: hard
statement: "The ordinary report path traverses directories only in the folded walk."
outcome: disk-growth-by-project
audit: no_second_traversal_on_the_report_path
---

## Rationale

The audit name above is one word away from a real one, which is exactly
how a watched guardrail becomes an unwatched one.
