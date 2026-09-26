---
id: store-data-is-parquet-not-json-sidecars
severity: hard
statement: "Per-unit, per-worktree and per-row data -- measurements, identities, associations, identification caches, evidence caches -- lives only in the existing Parquet current + reverse-delta store. JSON under the store directory is limited to a fixed list of small control and recovery files."
outcome: disk-growth-by-project
audit: json_writes_allowlisted
compile_fail:
  - json_writes_name_a_store_file
runtime_tests:
  - crates/core/tests/store_contents_are_allowlisted.rs
---

## Rationale

The handoff's hard constraint is explicit: "No new SQLite store, giant
JSON artifact cache, parallel database or per-file persistent
inventory." The stack nevertheless introduced `external_consumers.json`
(external.rs) and `toolchain_declarations_cache.json` /
`dependency_identities_cache.json` (consumer wiring and external
associations). Each is per-unit data, grows without bound, is rewritten
whole on every change, and cannot be compacted or retained by the
existing reverse-delta machinery — a parallel database with a JSON
syntax.

## Detection

Mechanism: type, gate audit, runtime test.

**Type.** Every `JsonFile` variant's file name is fixed (or a validated id under a fixed prefix); there is no per-unit or free-named JSON file to write.

**Gate audit.** `json_writes_allowlisted`: no serializer outside the gate's store module.

Retired 2026-09-22: the `store_data_is_parquet_not_json_sidecars` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `json_writes_name_a_store_file`.

## Runtime tests that complete it

- `crates/core/tests/store_contents_are_allowlisted.rs` — a full
  observe → report → Trash-move cycle on a multi-ecosystem fixture (the
  CLI action path -- propose/approve/execute/grants -- no longer exists;
  the TUI Trash move is the one supported action), then a recursive walk
  of `SWAMP_DIR` asserting every file matches exactly one allow-list
  pattern, that no file other than `*.parquet` exceeds 64 KiB, and that a
  Trash envelope holds exactly `restore.json` plus the moved members.

## 2026-09-24: `unowned.json` was the violation this rule was meant to catch

`<volume>/unowned.json` grew to 473 MB on a real default-scope store —
one JSON row per unowned *file*, wrongly allow-listed as a "control
file" because its name looked like one. Fixed: `walk.rs` now folds every
directory's direct unowned files into one row before it ever reaches
storage, and the rows land in `<volume>/unowned.parquet`
(`growth::columns::{StoredUnownedRow, read_unowned_rows,
write_unowned_rows}`), a measurement cache replaced wholesale each walk
like `external/folded.parquet`, not reverse-delta history. `JsonFile::Unowned`
is removed. The lesson generalizes: the allow-list test's per-file size
cap now applies to every non-Parquet file, not only `.json`/`.jsonl`
names, so a JSON *cache* wearing a different extension (the
`last_report*.json.zst` report cache, in particular) is still caught if
it starts scaling with observed data instead of staying a small
summary. `agent_protect.json` is done (R14, stack/26): the human keep
list is now `protect.parquet` (`path`, `added_at`), typed columns, no
JSON cell -- see `crates/core/src/protection.rs`. `projects.parquet`,
`worktrees.parquet`, `worktree_facts.parquet` and the current-artifact
table's `ecosystem` column are done (R15, stack/26): `swamp report`
builds `Report.projects` from them and reads only the remaining parts
from `report_rows.parquet`'s JSON cell (see
`crates/core/tests/project_worktree_tables.rs`). `external_units.parquet`,
`agent_units.parquet`, `unit_consumers.parquet`, `nested_artifacts.parquet`
and `evidence.parquet` are done (R16). `coverage.parquet`,
`notes.parquet` and `<volume>/topology.parquet` (replacing
`topology.json`) are done (R17, stack/26). The render-cache row itself
(`report_rows.parquet`) and `last_report*.json.zst` are gone (R18a-3b,
R18a-4). R20 then *deleted* the tables R17-R18a-3 had given to derived
report fields (`summary`, `series`, `unowned_summary`,
`worktree_entries`, `github_enrichment`) -- the store holds facts, a
read computes the views (`store-is-facts-report-is-views`); the exact
table list is `crates/core/tests/store_contents_are_allowlisted.rs`.
R18b (2026-09-24) converted the last control files: `cursors.parquet`
(per volume; was `fsevents.json`), the `docker_*` tables (was
`docker_facts.json`), the `scope*` tables (was `scope.json`),
`scheduled_runs.parquet` (was `last_run.json`), `ledger.parquet` +
`ledger_facts.parquet` (was `ledger.jsonl`, and its `evidence` JSON
blob is now typed key/value rows) and the Linux collector's
`continuity/<id>.parquet` + `<id>_entries.parquet` (was `<id>.json`).
`fs_gate::store::JsonFile` has exactly one variant left, `UiState`
(`ui_state.json`, the TUI's remembered filter/view), and the allow-list
is exactly `config.toml`, `ui_state.json`, lock files and the
scheduled-observation text log.
