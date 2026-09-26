---
id: column-store-parquet-zstd
severity: hard
statement: "Every persisted observation is Parquet written with zstd compression, keyed per volume; no JSON, SQLite or bespoke binary store for rows."
outcome: disk-growth-by-project
audit: gate_paths_only_inside_gates
compile_fail:
  - parquet_codec_is_not_selectable
  - history_rows_are_private_to_the_store
runtime_tests:
  - crates/core/src/fs_gate/columns.rs::tests::every_column_chunk_written_is_zstd
---

## Rationale
Column store + reverse delta was the design carried from the Go plumbing. It is what makes growth over any window a lookup and keeps the store small enough to keep for months.

## Detection

Mechanism: type, gate audit, runtime test.

**Type.** `fs_gate::columns::write_parquet_atomic` takes a zstd *level*; the writer properties are built privately and always zstd.

**Gate audit.** `parquet` (and `zstd`) may be named only inside the gate, so no module can build an `ArrowWriter` with other options; Arrow schemas only in the column-store modules.

Retired 2026-09-22: the `column_store_parquet_zstd` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `parquet_codec_is_not_selectable`, `history_rows_are_private_to_the_store`.
