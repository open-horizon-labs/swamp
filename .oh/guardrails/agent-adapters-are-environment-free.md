---
id: agent-adapters-are-environment-free
severity: hard
statement: "An adapter knows only the home it is handed. It never reads the process environment, never resolves a home directory, and hardcodes no user path."
outcome: coverage-aware-storage-history
audit: adapters_do_not_reach_gates, gate_paths_only_inside_gates
runtime_tests:
  - crates/core/tests/agent_storage_validation.rs
---

## Rationale

Home resolution -- including every documented override variable -- is a
detector's job, and detectors are already fixture-injectable through
`locations::Environment`. An adapter that reads `HOME` itself is
untestable without touching the real machine, which is exactly what the
privacy rule forbids: fixtures must be synthetic, and no test may read a
real tool home.

## Detection

Mechanism: gate audit, runtime test.

**Gate audit.** `adapters_do_not_reach_gates`: an adapter module (`agents::*` except the registry, matrix, unit and bounded-I/O plumbing) may not name `std::env`, `dirs`, `home`, the detectors, the actions or the gate; its only I/O is the `IdentifyCtx` it is handed. `gate_paths_only_inside_gates` rejects `libc` (so `getpwuid`) anywhere outside the gate, and an adapter building a path from an absolute or `~/` literal (`PathBuf::from("/Users/..")`) is rejected too.

Retired 2026-09-22: the `agent_adapters_are_environment_free` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

## Runtime tests that complete it

- every adapter's own tests, all of which inject a `tempfile` home
