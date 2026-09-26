# R20: store = facts, report = view (2026-09-24)

Owner-run slice (no sub-agent), correcting the owner's own brief for
R17-R18a-3b: "type every field of `Report`" produced tables for values
that are computed from other tables. The user asked why the store held
them at all and whether Parquet made them worth keeping; measured, it
does not (store: 5 MB, 37 files; largest table 1.9 MB; a fold over the
fact rows costs microseconds, one more Parquet open costs 1-3 ms).

## Classification (evidence, per table)

| table | origin on observe | verdict |
| --- | --- | --- |
| `summary.parquet` | `report::summarize(projects)` | derived -> deleted |
| `summary.parquet` reconciliation rows | per-root walk totals (`consumers/projects.rs`, `gate.rs`) | fact -> five nullable columns on the root's `coverage.parquet` row; the scope sum is computed |
| `summary.parquet` schedule row | `schedule::header_line` (launchd/systemd state at observe time) | run fact -> `runs.parquet.schedule_line` |
| `series.parquet` | `growth::history_series` over the volume history | derived -> deleted; recomputed at `observed_at` |
| `notes.parquet` | consumer run diagnostics (fsevents mode, daemon reachability, GitHub notes) | run fact -> kept |
| `unowned_summary.parquet` (+lists) | duplicate of each volume's `unowned.parquet`; the Docker rows the gate joined after the checkpoint were the only part not on disk | deleted; Docker rows -> `<volume>/docker_unowned.parquet` (+lists, evidence; same three-file shape as `unowned`) |
| `worktree_entries.parquet` | `dirs.parquet`/`files.parquet` grouped, plus growth and tracking | deleted; derived from the volume tables and their history; tracking -> `<volume>/dir_tracks.parquet` (the walk reads `.gitignore`; a report never can) |
| `github_enrichment.parquet` | live-enrichment counters | run fact -> three nullable columns on `runs.parquet` (`enrich.parquet` already held the fetched data) |
| `artifact_shape.parquet` `allocated_bytes`/`allocated_growth_bytes`/`growth_bytes` | the artifact's measured directory row and its history | derived -> columns dropped |
| `runs.parquet` (new) | `observed_at`, `since_secs`, `retention_days`, `include_dirs`, GitHub counters, `schedule_line` | run facts; also the "has this scope been observed" marker (every full pass writes one row, even for a scope with zero projects) |

## Determinism fixes the derivation exposed

- The per-root bus runs used their own `entities::now()`
  (`bus::ctx_for_excluding`); the scope observation had another. Series
  and growth were bucketed at whichever second each root's bus started.
  `report_full_mode_scoped_tracked` now takes the scope's `observed_at`
  and every root's rows, growth window and history are computed at that
  one instant, which `runs.parquet` records; `consumers/history.rs`
  uses `ctx.observed_at` instead of a second clock read.
- `annotate_readonly`/`annotate_readonly_dirs`/`annotate_readonly_files`
  now measure growth against history points strictly before
  `observed_at`: a read after the pass that wrote those rows sees them
  in `current.parquet`, and "closest point to the window start" picked
  the row itself (growth 0 where the pass had said "unknown").
- The drill-down maps are sorted by relative path on both paths
  (`report::sort_drill_down`); the walk's order was traversal order,
  the table's is table order, and `HashMap<_, Vec<_>>` serializes the
  vector in order.
- `report::attach_allocated_from_dirs` and `merge_series_into` are the
  one implementation each of what the observe pass and the read do.

## Also removed

- `fs_gate::store::JsonFile::Plan`/`::Grants` (`plans/<id>.json`,
  `grants.json`): the CLI action path was cut in stack/27; nothing
  wrote them any more. (The user's real store still has a `plans/`
  directory with 3.3 MB of old proposals -- theirs to delete.)
- `growth::scope_observed_at` now reads `runs.parquet`.

## Guardrails

- New audit `parquet_writers_are_the_named_fact_tables`
  (`TABLE_WRITERS`, keyed by module and function) + 3 rejecting
  mutations (new writer for a view, aliased writer, generic helper in
  the columns module) + 1 accept.
- `store_contents_are_allowlisted.rs` lists every table by exact name.
- `derived_views_are_computed_not_stored.rs` (no derived table after
  observe; every derived field equal; three fact tampers move the view).
- `report_read_stays_fast.rs`: 3,000-artifact store, 1 s strict / 3 s
  CI; measured 20 ms warm, 25 ms cold.
- `.oh/guardrails/store-is-facts-report-is-views.md`.

## Leftovers seen, not touched

- `crates/core/src/store.rs` (`Store::write` -> `volume-<id>.parquet`)
  is the R1-era `swamp scan --store` writer, still wired in the CLI.
  Not written by any observe/report cycle, so not in the allow-list.
  Candidate for deletion with the `scan` command.
- R18b: `fsevents.json`, `docker_facts.json`, `scope.json`,
  `last_run.json`, `ledger.jsonl` -> Parquet; `last_run.json`'s fields
  belong on `runs.parquet`.
