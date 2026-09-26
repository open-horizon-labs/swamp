---
id: agent-interface-facts-not-verdicts
severity: hard
statement: "Every agent-facing and human-facing surface states facts with their terms; no verdict vocabulary (safe, stale, unused, can be deleted) appears in rendered text or tool output."
outcome: disk-growth-by-project
audit: no_verdict_literals
runtime_tests:
  - crates/cli/tests/agent_json_contract.rs
---

## Rationale
Worktree staleness is not decidable. A verdict word is a claim the tool cannot back; a fact with terms is one the agent can reason from.

## Detection

Mechanism: gate audit, runtime test.

**Gate audit.** `no_verdict_literals` scans every production string literal (constants, `format!`/`concat!` arguments and `#[serde(rename)]` strings included, placeholders blanked) and every serialized field and variant name (snake/camel case read as words) for verdict phrases -- "safe to delete", "unused", "stale", "obsolete", ... -- unless the clause negates them. `dedup_stale` is the one reviewed serialized exception (a fact about a measurement).

Retired 2026-09-22: the `agent_interface_facts_not_verdicts` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.
