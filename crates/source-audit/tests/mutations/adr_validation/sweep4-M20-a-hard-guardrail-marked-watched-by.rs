//! target: .oh/guardrails/sweep4-unwatched-hard-guardrail.md
//! mode: create
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M20
//! by: audit:guardrail_metadata
//! why: a hard guardrail marked `audit: none`, watched by a runtime test that asserts nothing and is never compiled
//! blind-spot (old model): a runtime test only has to *parse* as a `#[test]` whose body contains the substring `assert` (a local named `_unasserted` will do), in any .rs file under `crates/*/tests` -- including a subdirectory Cargo never builds
---
id: sweep4-unwatched-hard-guardrail
severity: hard
statement: "Execution never moves a path the user protected."
audit: none
audit_none_reason: "2026-09-22: watched at runtime instead"
runtime_tests:
  - sweep4_watches_protection
---

## Detection

Nothing, in fact.
//! file: crates/core/tests/sweep4_never_built/watch.rs
//! mode: create
#[test]
fn sweep4_watches_protection() {
    let _unasserted = ();
}
