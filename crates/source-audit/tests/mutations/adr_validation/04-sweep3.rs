//! target: .oh/guardrails/sweep-unwatched-hard-guardrail.md
//! mode: create
//! by: audit:guardrail_metadata
//! why: re-review 3 sweep -- a new `severity: hard` guardrail with no `audit:` field at all -- unwatched, and the build says nothing (blind spot: the rule only checks that an `audit:` value *resolves*; a guardrail that omits the field is silently exempt, which is exactly how CE4's guardrail went unwatched)
---
severity: hard
---

# Sweep: a hard guardrail nothing watches

Statement: execution never moves a path the user protected.

This file carries no `audit:` field, so `adr_validation` never looks at it.
