---
id: activity-and-consumer-evidence-have-limits
severity: hard
statement: "Consumer, activity, and current-use claims must retain their source, freshness, and limits; missing evidence must not be presented as unused or safe to remove."
outcome: disk-growth-by-project
s_and_t_step: W2a
parent_step: W2
sufficiency_group: G2
owner: null
review_trigger: "A new detector, scope change, contradictory fact, or real user decision exposes a failure."
tactic_disposition: selected
audit: no_verdict_literals
compile_fail:
  - blank_reason_does_not_compile
  - reason_is_not_a_plain_string
  - fact_status_reason_is_not_a_plain_string
runtime_tests:
  - crates/core/tests/evidence_contract.rs
---

# Activity and consumer evidence have limits

## Rationale

Separate consumers, activity, and current-use evidence to support
[decision-relevant storage evidence](../outcomes/decision-relevant-storage-evidence.md).
Referenced, accessed, modified, and running establish different facts. A filesystem
timestamp alone does not identify a developer's last intentional use. No known
configuration reference does not prove that a tool version is unused. Shared
artifacts may have multiple consumers, including consumers outside scanned roots.

## Selected tactic and parallel assumption

Preserve source, freshness, coverage, and interpretive limits for each claim.
This discipline is necessary; a universal activity collector or confidence score
is not selected. Ecosystem-specific evidence may narrow uncertainty without
whole-machine execution monitoring. Its decision value still needs testing.

## Boundaries

- Distinguish direct observations, declared relationships, and inferred associations.
- Distinguish a fact's event time from the time swamp observed it.
- Unknown remains unknown; it does not become a safety verdict or authorize removal.
- A missing tool executable does not prove its stored installations are absent.
- Keep measurement useful even when attribution is incomplete.
- Explain consequential unknowns without requiring every unknown to block a decision.

This elaborates the existing
[facts-not-verdicts guardrail](agent-interface-facts-not-verdicts.md); its existing
vocabulary audit does not establish provenance, freshness, or semantic correctness.

## Detection

Mechanism: type, gate audit, runtime test.

**Type.** Every `FactStatus::Unknown`/`Unavailable`/`Conflicting` carries an `evidence::Reason`, and a `Reason` is made only by `reason!` (a literal template checked non-blank at compile time), `Reason::fixed` (a `&'static str` from a table) or `Reason::carried` (a reason another fact computed); the last two substitute an explicit "not recorded" text for a blank one. There is no `From<String>`, and `From<&str>` exists only under the `testing` feature. A blank literal, `String::from("")` or a `FactStatus` literal with a plain string does not compile.

**Gate audit.** `no_verdict_literals` rejects any production string literal or serialized field/variant name that asserts "unused", "safe to delete" and the like, so missing evidence cannot be rendered as a verdict.

Retired 2026-09-22: the `activity_and_consumer_evidence_have_limits` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `blank_reason_does_not_compile`, `reason_is_not_a_plain_string`, `fact_status_reason_is_not_a_plain_string`.

## Runtime tests that complete it

- `crates/core/tests/evidence_contract.rs` — the rendered form of an
  `Unknown`/`Unavailable` fact contains its reason, and the docs table
  lists every `ACTIVITY_EVIDENCE_INVENTORY` entry.

## Validation gap

What remains unvalidated is whether users read these limits correctly. Check stale
facts, unavailable activity records, configuration-only references, command-line
usage without persistent configuration, shared consumers, and out-of-scope projects.
Test whether users can distinguish what was observed from what was inferred.
Universal last-use detection remains deferred rather than promised.

## Lineage and invalidation

Source: 2026-09-19 problem weave, W2a, parent W2, depth 2, G2 (W2a + W2b, all
required); primarily evidence/trust framing. Proposed contributing role is
domain/evidence work; ownership is unassigned. The tactic is now selected in the
[full-scope solution-space session](../sessions/2026-09-19-developer-storage-coverage-and-evidence.md);
implementation and validation remain outstanding.
Reassess if a missing consumer reverses a supposedly supported decision, or users
interpret an old timestamp or missing relationship as permission to delete.
