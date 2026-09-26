---
id: dir-mtime-int32-minutes
severity: hard
statement: "Per-directory and per-file store rows carry the newest mtime as int32 minutes (mod_time_min), the store-depth design from the Go plumbing."
outcome: disk-growth-by-project
audit: gate_paths_only_inside_gates
compile_fail:
  - history_rows_are_private_to_the_store
runtime_tests:
  - crates/core/tests/dirs_and_files.rs::mod_time_min_round_trips_as_i32_minutes
---

## Detection

Mechanism: type, gate audit, runtime test.

**Type.** The directory and file row schemas and their writers are private to `growth::columns`.

**Gate audit.** Arrow schema types may be named only in the column-store modules, so no other module can declare a `mod_time*` column; the round-trip test below pins the Int32 minutes encoding.

Retired 2026-09-22: the `dir_mtime_int32_minutes` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `history_rows_are_private_to_the_store`.
