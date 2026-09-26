---
id: agent-adapter-test-contract
severity: hard
statement: "Every adapter proves the same five obligations: unknown format is explicit, content does not leak, reads are bounded, protected categories are protected by default, and project linkage is declared or explicitly unresolved. Codex proves the read obligation more strongly by asserting that no rollout-header bytes are read."
outcome: decision-relevant-storage-evidence
audit: guardrail_metadata
runtime_tests:
  - crates/core/tests/agent_storage_validation.rs
---

## Rationale

The 2026-09-21 review's P1 on completion claims: the support matrix
labelled every tool "Supported" while the session notes admitted an
assumed Windsurf layout and unconfirmed Cline/Roo project fields. Test
*coverage* varied per adapter, so "Supported" meant different things in
different rows and nothing checked the difference.

Naming the five obligations makes the label mean one thing. An adapter
that cannot honestly write one of these tests is not Supported, and the
matrix has to say `Unverified`.

## Detection

Mechanism: gate audit, runtime test.

**Gate audit.** `guardrail_metadata` requires every adapter module that declares a `*_TOOL_ID` to hold the five contract tests in its own file, each a running `#[test]` (not ignored in any spelling, `#[cfg_attr(.., ignore)]` included) that invokes an assertion macro. For the read obligation, adapters use `identification_reads_no_more_than_header_cap`; Codex instead uses `identification_reads_no_rollout_header_bytes`, asserting the stronger zero-rollout-read guarantee provided by its SQLite linkage source.

Retired 2026-09-22: the `agent_adapter_test_contract` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

## Runtime tests that complete it

- the five contract tests themselves, per adapter, with Codex's stronger read test in place of the generic header-cap test
- `crates/core/tests/agent_storage_validation.rs`
