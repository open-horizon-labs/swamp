---
id: one-byte-formatter
severity: hard
statement: "There is one byte formatter in the product, decimal, SI-labelled; the TUI re-exports core's."
outcome: disk-growth-by-project
audit: byte_units_only_in_the_formatter
runtime_tests:
  - crates/core/tests/render_snapshot.rs
---

## Rationale
The TUI once divided by 1024 under a GB label while core divided by 1000; rows visibly failed to sum. AST audit `one_byte_formatter`.

## Detection

Mechanism: gate audit, runtime test.

**Gate audit.** `byte_units_only_in_the_formatter`: outside `render.rs`, no string literal renders a placeholder followed by a byte unit (`"{v:.1} KiB"`, `"{} MB"`) -- whatever the divisor is called -- and no bare unit label (`"KiB"`, `"MB"`: a unit table) appears outside render.rs and the two reviewed unit *parsers* (`docker`, `filter`).

Retired 2026-09-22: the `one_byte_formatter` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.
