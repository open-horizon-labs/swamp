---
id: agent-adapters-do-not-emit-content
severity: hard
statement: "Adapters return data; they never print, log or trace. Rendering happens in the shared layer, which is the only place that knows what may be shown."
outcome: decision-relevant-storage-evidence
audit: adapters_do_not_reach_gates
runtime_tests:
  - canary_content_never_appears_in_output
---

## Rationale

The privacy contract is about what leaves the process, not only about
what enters a struct field. An adapter debugging a header parse with
`eprintln!("{line}")` would print a conversation's first line to the
terminal and into any CI log — past every redaction the rendering layer
applies, and past the canary assertions, which check returned values
rather than stdout.

## Detection

Mechanism: gate audit, runtime test.

**Gate audit.** `adapters_do_not_reach_gates`: adapter production code invokes no printing or panicking macro (`println!`, `eprintln!`, `dbg!`, `panic!`, ...) and no `unwrap`/`expect` family method (whose panic message would carry a path or content to stderr).

Retired 2026-09-22: the `agent_adapters_do_not_emit_content` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

## Runtime tests that complete it

- every adapter's `canary_content_never_appears_in_output`
