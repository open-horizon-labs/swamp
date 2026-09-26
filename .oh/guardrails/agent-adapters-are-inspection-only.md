---
id: agent-adapters-are-inspection-only
severity: hard
statement: "Identification never acts. An adapter declares an action capability on the unit it returns; only the shared sink (`fs_gate::destroy`, driven by the TUI) moves or removes anything. Adapters reference no plan, ledger or filesystem-mutating API."
outcome: decision-relevant-storage-evidence
audit: adapters_do_not_reach_gates, gate_paths_only_inside_gates
---

## Rationale

"Inspection is not authorization" is a handoff constraint. Fourteen
adapters, each free to call `fs::rename`, would be fourteen places a
human's Trash-move decision could be bypassed. The review's
counterexamples were all failures *at* the sink; they would have been
unrecoverable if the sink were not the only way through.

## Detection

Mechanism: gate audit.

**Gate audit.** `adapters_do_not_reach_gates` limits adapters to `IdentifyCtx`; `gate_paths_only_inside_gates` rejects `std::fs`, `libc` (`truncate`, `unlink`), `trash` and `std::process` outside the gate, and `fs_gate::destroy` outside the execution sinks (`crates/core/src/actions.rs`, `crates/core/src/cargo_cleanup.rs`, `crates/tui/src/actions.rs`).

Retired 2026-09-22: the `agent_adapters_are_inspection_only` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

2026-09-23: this guardrail's `RecheckProof`/`Authorized` compile-fail cases (`trash_move_needs_a_recheck_proof`, `recheck_proof_is_minted_only_by_run_all`) and their runtime test (`execution_rechecks.rs`) are removed along with those types -- see `.oh/guardrails/execution-sinks-recheck-live-state.md`. What remains true and enforced: an adapter still cannot reach `fs_gate::destroy` (or any other filesystem-mutating gate capability) directly.
