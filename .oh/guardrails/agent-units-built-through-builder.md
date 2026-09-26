---
id: agent-units-built-through-builder
severity: hard
statement: "Adapters build units through AgentUnitBuilder::new(tool, category, path), whose constructor applies the protected-by-default categories (credentials, configuration, skills, automation definitions, databases). Lifting a default protection requires an explicit reason."
outcome: decision-relevant-storage-evidence
audit: none
audit_none_reason: "2026-09-22: the property is a type: every field that decides protection or action capability is private to `agents::unit`, so no source audit is needed"
compile_fail:
  - agent_units_are_built_by_the_builder
  - agent_units_protection_not_via_a_binding
  - unprotect_needs_a_reason
runtime_tests:
  - crates/core/tests/agent_refusal_matrix.rs
---

## Rationale

"Protect credentials/config/skills/automation definitions by default" is
a guardrail from the handoff, currently implemented as a `match` on
category inside `discover_and_measure` plus each adapter remembering to
set `protected: true` on the paths its own tool treats specially. A
struct literal with `protected: false` is one keystroke, reviews as
noise, and silently makes a credentials file actionable.

A constructor that starts protected and requires an argued exception
inverts the default: forgetting to think about it yields the safe answer.

## Detection

Mechanism: type, runtime test.

**Type.** `CandidateAgentUnit`'s fields are private to `agents::unit` (only `path` and `note`, which decide nothing, are public); `AgentUnitBuilder` sets protected-by-default and is the only constructor. Assigning `protected`, directly or through a `&mut` binding, does not compile anywhere else, and `unprotect_with_reason` takes an `evidence::Reason`, so the lift always says why.

Retired 2026-09-22: the `agent_units_built_through_builder` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `agent_units_are_built_by_the_builder`, `agent_units_protection_not_via_a_binding`, `unprotect_needs_a_reason`.

## Runtime tests that complete it

- every adapter's `protected_categories_default_protected`
- `crates/core/tests/agent_refusal_matrix.rs`
