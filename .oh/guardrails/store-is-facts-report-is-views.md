---
id: store-is-facts-report-is-views
severity: hard
statement: "The store holds observation facts and their history. Everything a report shows that is a function of those facts -- a total, a series, a drill-down grouping, an artifact's growth -- is computed when the report is read, never written to a table of its own. A new Parquet table is a new fact, named in one place, with the observation it comes from."
outcome: disk-growth-by-project
audit: parquet_writers_are_the_named_fact_tables
runtime_tests:
  - crates/core/tests/derived_views_are_computed_not_stored.rs
  - crates/core/tests/report_read_stays_fast.rs
  - crates/core/tests/store_contents_are_allowlisted.rs
---

## Rationale

R17 through R18a-3b (2026-09-24) typed the JSON render cache out of the
store one `Report` field at a time -- and, because the brief said "type
every field", gave tables to fields that are not facts at all:
`summary.parquet` (a fold over the project rows), `series.parquet` (the
history bucketed), `unowned_summary.parquet` (a copy of each volume's
`unowned.parquet`), `worktree_entries.parquet` (a copy of `dirs.parquet`
/`files.parquet` plus their growth), `github_enrichment.parquet` (a
second copy of `enrich.parquet`'s counters), and growth columns on
`artifact_shape.parquet`. Each was a second source of truth that had to
agree with the facts it was computed from, cost a write on every
observe, and bought nothing on read: the whole store is a few megabytes,
the fact tables are hundreds to thousands of rows, and a fold over them
costs microseconds -- less than opening one more Parquet file. R20
deleted them.

The two things that are legitimately stored although they look derived
are stored because a *read cannot recompute them*: cleanup guidance on
a nested artifact (computed from `observed_at`, so a stored observation
renders identically every time -- a clock read at render time is what
made `report --json` non-reproducible), and the tracking state of a
walk's top-level directories (`dir_tracks.parquet`: `.gitignore` is read
by the walk, and a report never opens files under a worktree).

## Detection

Mechanism: gate audit, runtime test.

**Gate audit.** `parquet_writers_are_the_named_fact_tables`: the one
Parquet writer, `fs_gate::columns::write_parquet_atomic`, is referenced
only from the functions named in `TABLE_WRITERS`
(`crates/source-audit/src/rules/gate.rs`), one per fact table, keyed by
module and function. A new writer -- a new table, a generic
"write any batch" helper, an aliased import -- is a failing audit until
it is named there, in the runtime allow-list, and in
`docs/architecture.md`'s table with the observation it records.
Mutation corpus: `crates/source-audit/tests/mutations/
parquet_writers_are_the_named_fact_tables/`.

**Runtime tests.** `derived_views_are_computed_not_stored.rs`: after a
real `observe_scope`, no table exists for any derived field, every
derived field reads back equal to what the pass produced, and tampering
a fact (a coverage row's walk totals, a volume's Docker unowned rows, a
directory's tracking state) moves the view. `store_contents_are_allowlisted.rs`:
the store holds exactly the named fact tables and nothing else.
`report_read_stays_fast.rs`: a 3,000-artifact store derives every view
within 1 s (strict) / 3 s (CI); measured at 20 ms.

## What is a fact

Rows that come from an observation -- a walk, a detector, a daemon
answer, a `git` or `gh` query -- or the reverse-delta history of those
rows, plus the parameters of the run that produced them
(`runs.parquet`: `observed_at`, the growth window, whether the
drill-down was requested, the live-enrichment counters, the scheduler
status the pass saw). `Report.reconciliation`'s per-root totals are
facts of each root's walk and live as columns on its `coverage.parquet`
row; their sum is not stored.
