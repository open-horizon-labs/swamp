---
id: no-dead-public-evidence-api
severity: hard
statement: "Every pub fn in the evidence, activity, occupancy, recovery, reclaimability, consumer-wiring, association, toolchain-declaration and recheck modules has at least one non-test caller in the workspace. A capability the docs claim is either wired into the live pipeline or deleted along with the claim."
outcome: decision-relevant-storage-evidence
audit: no_unreferenced_public_items
runtime_tests:
  - crates/core/tests/evidence_api_is_wired.rs
  - crates/core/tests/agent_matrix_matches_docs.rs
---

## Rationale

The PR #123 review's central finding: three of the five #55 current-use
sources, the whole #54 access-time path, and four of seven #59
accounting functions had **zero production callers**, while the
CHANGELOG, `DESIGN.md` and `docs/architecture.md` stated those
capabilities as delivered. A module full of well-tested functions nobody
calls is not a delivered feature; it is a claim with unit tests attached.

Named dead entry points at the time of the review:
`activity::{access_time_evidence, tool_reported_use_evidence,
docker_last_used_evidence}`,
`occupancy::{docker_running_container_evidence, manager_lock_evidence,
simulator_booted_evidence}`,
`reclaimability::{apfs_clone_or_snapshot_bound, sparse_file_accounting,
estimate_selection, observed_free_space_change}`,
`recovery::{maven_artifact_recovery, mutable_environment_recovery,
toolchain_installation_recovery}`.

## Detection

Mechanism: gate audit, runtime test.

**Gate audit.** `no_unreferenced_public_items`: every public function and type in the workspace crates is named somewhere (a path, a method call, a serde attribute, an inline format capture). A trait impl does not make what it calls live, and `#[allow(dead_code)]` in production code is rejected outright (it hides the dead caller that would make a dead API look used).

Retired 2026-09-22: the `no_dead_public_evidence_api` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

## Runtime tests that complete it

- the executable matrix↔docs test where a table exists
  (`crates/core/tests/agent_matrix_matches_docs.rs`)
- for prose claims, the session note lists each claim with the test that
  backs it
