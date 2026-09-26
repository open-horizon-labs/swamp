---
id: protection-fails-closed
severity: hard
statement: "Human keep/protect intent is loaded through one function that returns a Result; unreadable or malformed protection state is unknown, never an empty keep list, and every action refuses until it can be read. There is exactly ONE protection predicate (agents::protection_conflict), it tests both directions -- a unit beneath a protected path and a unit containing one -- and no other function answers the question except by delegating to it. The list is written atomically."
outcome: decision-relevant-storage-evidence
audit: sinks_have_no_path_predicates
compile_fail:
  - protect_list_has_only_conflict
  - protect_list_has_no_default
  - protect_listing_is_display_only
runtime_tests:
  - crates/tui/src/app.rs::tests::agents_view_mark_row_refuses_a_protected_unit_with_the_reason_not_a_generic_message
  - crates/tui/tests/scope_preserving_refresh.rs::marking_an_ordinary_row_that_contains_a_protected_file_is_refused_with_the_reason
---

## Rationale

Two findings from the 2026-09-21 review:

- `protected_descendant_must_prevent_parent_cache_proposal` —
  `is_human_protected` only asked whether the candidate lay beneath a
  protected path. Protecting `debug/log.txt` therefore did nothing to
  stop `debug/` being proposed and removed, destroying exactly what the
  human asked to keep.
- The review's closing note: a malformed `agent_protect.json` was parsed
  with `unwrap_or_default()` into an empty list, silently *lifting*
  every protection, and writes were non-atomic so a crash mid-write
  could produce that malformed file in the first place.

A protection mechanism that fails open is worse than none: it reports
that intent was recorded and then ignores it.

A third finding, from the integration owner's own mutation check on
2026-09-21, is why this guardrail now insists on *one* predicate.
Mutating protection to a single direction and running everything
**passed** -- this audit and every runtime test. The reason: the audit
inspected `protection_conflict`, while
`actions::propose_checking_protection` (the path for ordinary
filesystem rows) went through a second predicate,
`agents::is_human_protected`, a `bool` wrapper that *looked* like a
delegation. No test proposed an ordinary directory that contained a
protected descendant, so the surviving direction was never exercised
where it mattered.

Two spellings of one question means two things to inspect, and an audit
will always be reading the other one. `is_human_protected` is therefore
deleted rather than fixed, and the audit forbids reintroducing it under
any name.

## Detection

Mechanism: type, gate audit, runtime test.

**Type.** `protection::ProtectList` is opaque: its only query is `conflict(candidate)` (both directions), and a corrupt or unreadable protect file is an error, never an empty list -- the type has no `Default`, so `.unwrap_or_default()` on it does not compile.

**Type (re-review 5, finding 7; 2026-09-23 update).** A keep-list change is `swamp protect add|remove`'s own handler (`protection::protect_add`/`protect_remove`) -- human-only by convention (it is the CLI's job, and the TUI never calls it, only reads what it wrote), not by a spent token: there is no `HumanConfirmed`/`Authorized` machinery left to bind it to. The production listing, `ProtectListing`, is display-only (`Display`/`Serialize`, `len`, `is_empty`): no entries, no iterator, no containment query; the raw path list exists only under the `testing` feature. The TUI's mark step reloads the list fresh from the report's own store directory before every mark -- never a cached copy -- so a protection added mid-session is honoured immediately.

**Gate audit.** `sinks_have_no_path_predicates`: the execution sinks call no path containment method (`starts_with`, `strip_prefix`, `ancestors`) of their own, so a second, one-directional protection predicate cannot be written there. `HumanConfirmed::cli_protect` is pinned to `cmd_protect`.

Retired 2026-09-22: the `protection_fails_closed` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `protect_list_has_only_conflict`, `protect_list_has_no_default`, `protect_changes_are_human_only`, `protect_listing_is_display_only`.

## Runtime tests that complete it

- `crates/core/tests/reviewer_counterexamples.rs::protected_descendant_must_prevent_parent_cache_proposal`
- `crates/core/tests/execution_rechecks.rs` — corrupt protect file
  refuses every action and `swamp protect list` reports the corruption
  rather than an empty list; protection added on an ancestor and on a
  descendant after approval both refuse.
- `crates/core/tests/evidence_action_recheck.rs::ordinary_unit_containing_protected_descendant_is_refused_at_proposal`
  and its sibling `..._beneath_a_protected_ancestor_...` — the exact
  case the mutation escaped, on the **ordinary** filesystem row path, at
  proposal time. Both directions, in the same file, so a future
  one-directional mutation fails whichever half it drops.
- `crates/tui/tests/scope_preserving_refresh.rs::marking_an_ordinary_row_that_contains_a_protected_file_is_refused_with_the_reason`
  — the same case on the TUI's mark path, which now proposes through
  `propose_checking_protection` so the refusal arrives when the human
  marks the row rather than at execution.
