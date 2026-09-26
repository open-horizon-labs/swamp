---
id: computed-but-not-delivered
severity: soft
statement: "A fact is done only when it is wired from extraction through the schema to rendering and seen in real output; a populated struct field nobody renders is a defect."
outcome: disk-growth-by-project
audit: no_unreferenced_public_items
runtime_tests:
  - crates/core/tests/nested_artifact_evidence_is_delivered.rs
---

## Rationale

Carried from repo-native-alignment. Mole's `RepoRootID` was declared,
computed and never rendered for months.

The 2026-09-22 independent re-review found the same shape here, and found
it *because* this was the one guardrail in the directory whose frontmatter
said `audit: none`: `CHANGELOG.md` claimed decision evidence was
"attached to ... nested build-artifact units", while
`NestedArtifact::decision_evidence` was written only as `Vec::new()`
(`cargo_artifacts.rs`, `cli/src/main.rs`),
`report::attach_decision_evidence` never touched `report.nested_artifacts`,
`skip_serializing_if` hid the empty vector from `--view rust` JSON, and no
test existed. An unwatched guardrail is the one the next overclaim
violates.

## Detection

Mechanism: gate audit, runtime test.

**Gate audit.** `no_unreferenced_public_items`: a public function or type nothing in the workspace names (production or test) fails. What is computed but only ever empty is caught by the delivery tests below, not by a source rule.

Retired 2026-09-22: the `computed_but_not_delivered` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

## Runtime tests that complete it

- `crates/core/tests/nested_artifact_evidence_is_delivered.rs` — a real
  nested build artifact carries decision evidence, and `--view rust` JSON
  shows it.
- The frame goldens (`crates/tui/tests/frames`) and render snapshots,
  which render real fixture reports.
