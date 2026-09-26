# R18a-3b: `artifact_shape.parquet` typed; `report_json`/`ReportSnapshot`'s JSON persistence deleted (2026-09-24)

CHUNK_R18a-3b (the owner's "Progress log" entry after R18a-3) asked for
exactly one table plus a deletion: type `ArtifactRow`'s remaining shape
fields (`kind`/`path`/`track`/`confidence`/`source`/`note`/`created_at`/
`containers`/`shared_with`/`dangling`/`allocated_bytes`/
`allocated_growth_bytes`) into `artifact_shape.parquet` (+
`artifact_shape_lists.parquet` for the two list fields), keyed like
`artifact_row_key`; then DELETE `report_json`, `StoredReportSnapshotRow`,
`ReportSnapshot`, `write_report_snapshot`, `read_report_snapshot`,
`slim_report_for_snapshot_json`, `report_rows.parquet`, and every
overlay/fallback that read them. Both are done, in full.

## What shipped

**`artifact_shape.parquet`** (`crates/core/src/growth/columns.rs`,
`StoredArtifactShapeRow`): one row per artifact, keyed by
`(scope_key, worktree_id, seq)` where `seq` is the artifact's position
in `WorktreeRow.artifacts` at write time (Parquet row order is not
guaranteed on read, so order has to be a stored column, not an
assumption). Carries `project_id` for debugging, `rel_path` (the
artifact's path stripped of its worktree path -- reconstructing the full
path is the reader's job, see the empty-relative-path note below),
`kind` (`ArtifactKind`'s `Debug` tag, same convention every other typed
kind column already uses), `track` (`TrackState::label()`, nullable),
`confidence` (`Confidence::label()`), `source_tool` (`Source.tool`),
`note`, `created_at`, `dangling`, `allocated_bytes`,
`allocated_growth_bytes`, `growth_bytes` (an extra field beyond the
chunk's named list -- `ArtifactTableFacts`, R15 item 3, never typed it
either, so it was the one other field still only reachable through
`report_json`; found by diffing `ArtifactRow`'s full field list against
what `ArtifactTableFacts`/`rebuild_evidence_from_tables` already cover,
per the chunk's own instruction to enumerate). `evidence` reuses
`evidence.parquet` like every other entity kind (`"artifact"`) and is
not duplicated here; `bytes`/`local_bytes`/`mtime_max`/`hardlinked`/
`dedup_stale`/`regrowth_count`/`observed_at`/`ecosystem` stay in
`ArtifactTableFacts` (R15), unchanged.

**`artifact_shape_lists.parquet`** (`StoredArtifactShapeListRow`):
`containers`/`shared_with`, joined back to the parent row by
`(scope_key, worktree_id, seq, list_kind, item_seq)` -- the same
`unowned_summary.parquet`/`unowned_summary_lists.parquet` split.

**Write side**: `growth::write_artifact_shape_table` (public, same
per-scope-key wholesale-replace semantics as `write_project_worktree_tables`),
called from `report::observe_scope` right after
`write_project_worktree_tables`, over the same already-merged `projects`
tree -- no second pass.

**Read side**: `growth::artifact_shape_rows_by_worktree` reads both
tables for a scope key, groups by `worktree_id`, sorts by `seq`, and
reconstructs each row into a full `ArtifactRow` (facts fields at their
zero/default value, `evidence` empty -- both filled in by the existing
overlay steps that already ran on this list). `report::
rebuild_projects_from_tables` now sources its per-worktree artifact list
from this function instead of `old_artifacts_by_worktree` (which used to
come from the deserialized snapshot); everything downstream (the facts
overlay via `artifact_row_key`, evidence rebuild) is unchanged.

**A real bug caught mid-slice**: `artifact_shape_rows_by_worktree`
cannot join `worktree_path` itself (it only has a `worktree_id`), so it
returns `path` as the stored relative path and the caller joins it. The
worktree-root artifact row (a `Source`/`Ignored`/`Untracked` remainder)
stores an *empty* relative path, and `Path::join("")` appends a trailing
separator (`/w1/`, not `/w1`) -- a real round-trip regression the new
`artifact_shape_table_round_trips_every_field_and_keeps_order` unit test
caught on its first run. Fixed by special-casing the empty-relative-path
join in both `rebuild_projects_from_tables` and the test's own mirror of
that join.

**A second real bug caught by the full workspace suite, not by the
targeted slice tests**: gating "has this scope ever been observed" on
`read_project_worktree_tables` (`projects.parquet` having rows) breaks
any scope with genuinely zero discovered projects -- an external/agent-
units-only scope (the CLI's own Claude-Code-only fixture,
`crates/cli/tests/agent_storage_cli.rs`) never gets a `projects.parquet`
row at all, even after a real, successful `observe`, so the gate
reported `NoObservation` for a scope that plainly had one.
`growth::scope_observed_at` (a `summary.parquet` row for the scope key,
which `write_summary_table` always writes -- three `overview` rows,
unconditionally, even when every count is zero) replaces the gate and
also supplies `ReportSnapshot.observed_at`/`Report.observed_at` (which
used to come from `tables.projects.iter().map(|p| p.observed_at).max()`,
equally broken for a zero-project scope).

## Deleted

`StoredReportSnapshotRow`, `report_snapshot_schema`,
`write_report_snapshot_rows`, `read_report_snapshot_rows` (all in
`columns.rs`); `report_snapshot_path`, `slim_report_for_snapshot_json`,
`write_report_snapshot`, `read_report_snapshot` (all in `growth.rs`);
`report::snapshot_from_observation` (left with no callers once the three
test files that used it for JSON-snapshot tampering were rewritten to
tamper the real tables directly through their public writers -- flagged
and deleted by `swamp-source-audit`'s `no_unreferenced_public_items`
rule, not missed). `report_rows.parquet` is no longer written anywhere.
`ReportSnapshot` the *type* is not deleted: it is the one place
`report::report_scope_from_store`'s result and `report::observe_scope`'s
composed value share a name, and both the CLI and the TUI consume it by
that name. What is deleted is everything that ever serialized it to
JSON or read it back that way -- it is now built exclusively by
`report_scope_from_store` running every `rebuild_*_from_tables` function
over an empty starting value, never deserialized from a stored cell.

## Proof requested by the chunk

```
$ grep -nE 'pub \w+_json:' crates/core/src/growth/columns.rs
(no output)

$ grep -rn report_rows crates/
crates/core/tests/project_worktree_tables.rs:387:/// R18a-3b deleted `report_rows.parquet`/`report_json`/`ReportSnapshot`'s
crates/core/tests/project_worktree_tables.rs:402:        !fx.store.join("report_rows.parquet").exists(),
crates/core/tests/project_worktree_tables.rs:403:        "observe_scope must never write report_rows.parquet again"
crates/core/tests/units_nested_evidence_tables.rs:9://! (`report_json`/`StoredReportSnapshotRow`/`report_rows.parquet`)
crates/core/src/growth.rs:8696,8698,8705,8835,8837:  report_rows / report_stored / report_lists / report_evidence
crates/core/src/report.rs:3212,3217:  report_rows / interior_rows
```

Given honestly, not fabricated as empty: the first four hits are this
slice's own doc comments/assertions *proving* the deletion (they name
the deleted file only to say it is gone and to assert its absence on
disk). The remaining hits are a pre-existing, unrelated naming
collision: `report_rows`/`interior_rows`/`report_stored`/`report_lists`/
`report_evidence` are R16-era local variable names inside
`read_nested_artifact_table`'s callers (a tuple of *nested-artifact*
rows, nothing to do with the deleted render-cache row) that happen to
contain the substring "report_rows". Renaming them is out of this
slice's scope (a cosmetic, unrelated diff over working, tested code) and
was not attempted. Every doc comment that named `report_rows.parquet`
*as a currently-existing file* elsewhere in `crates/` (columns.rs,
growth.rs, report.rs, `crates/tui/src/lib.rs`, `crates/cli/src/schedule.rs`,
`crates/source-audit/src/rules/gate.rs`, and two test file headers) was
rewritten to describe it as deleted/historical, or to drop the literal
filename in favor of "the old JSON render-cache row" / "stored Parquet
tables". `docs/architecture.md`, `docs/usage.md` and `DESIGN.md` were
also updated (they had current-tense claims that the shape fields "still
come from `report_rows.parquet`" and that the TUI's paint cache "is
`report_rows.parquet`", both now false).

```
$ grep -n 'report_rows.parquet' crates/core/tests/store_contents_are_allowlisted.rs
(no output -- it was never explicitly named there; the store's wildcard
`.parquet` rule already covered it, and `a_full_cycle_leaves_only_
allowlisted_files_in_the_store` passes with the file genuinely absent
from a real observe/report/trash-move cycle)
```

## Exact list of non-Parquet files still written to the store

From `crates/core/tests/store_contents_are_allowlisted.rs`'s allow-list
(unchanged by this slice -- this is R18b's job, not this one's):

- `config.toml`
- `ledger.jsonl`
- `last_run.json`
- `fsevents.json`
- `docker_facts.json`
- `ui_state.json`
- `scope.json`
- `restore.json` (inside a Trash envelope)
- `last_report.json` / `last_report.json.zst` and the per-root/per-scope
  `last_report-<hash>.json.zst` pattern (`consumers/signals.rs`,
  `consumers/cargo.rs`'s incremental-walk previous-pass cache -- R18a-4)
- a Linux collector's `continuity/*.json` checkpoint and `*.sync`
  request files
- `*.lock` files and the store's own `VERSION` marker

Every other file under the store is `*.parquet`.

## Tests

- `growth::tests::artifact_shape_table_round_trips_every_field_and_keeps_order`
  -- every shape field, list order, artifact order within a worktree, a
  second scope's rows not disturbing the first's, and the empty-
  relative-path (worktree-root) join.
- `growth::tests::artifact_shape_tables_rebuild_reflects_a_direct_tamper_not_the_original_value`
  -- on-disk tamper of both tables, read back through
  `artifact_shape_rows_by_worktree`.
- `crates/core/tests/project_worktree_tables.rs`:
  - `report_reads_projects_and_worktrees_from_a_direct_tamper_of_the_tables`
    and `report_reads_artifact_shape_from_a_direct_tamper_of_the_table`
    replace the old JSON-snapshot-tamper test (there is no snapshot left
    to tamper): both tamper through the real public writers
    (`write_project_worktree_tables`/`write_artifact_shape_table`) and
    assert the next `report_scope_from_store` reflects the tamper.
  - `no_report_json_snapshot_remains_and_the_report_still_rebuilds_exactly`
    replaces `the_remaining_parts_still_come_from_the_snapshot` per the
    chunk's own instruction: asserts `report_rows.parquet` is never
    written and a plain read-back still equals the observed report
    field for field.
  - `observe_writes_the_three_tables_and_every_view_renders_identically_from_them`
    (pre-existing, unchanged in intent) still passes: every render/JSON
    view is byte-identical between the observed and rebuilt reports.
- `crates/core/tests/coverage_series_summary_notes_tables.rs`'s
  `report_reads_summary_and_notes_from_a_direct_tamper_of_the_tables`
  and `crates/core/tests/units_nested_evidence_tables.rs`'s
  `report_reads_units_from_a_direct_tamper_of_the_tables` replace their
  files' own now-impossible JSON-snapshot-tamper tests the same way, for
  the same reason (both used `write_report_snapshot`/
  `snapshot_from_observation`, both deleted).
- `crates/cli/tests/agent_storage_cli.rs` (pre-existing, unmodified)
  caught the `scope_observed_at` bug: both of its `report --view agents`
  tests failed on the first full-workspace run after this slice's
  initial (`read_project_worktree_tables`-gated) version, and pass after
  the fix.
- Full workspace suite (`cargo test --workspace --locked`): every test
  green, zero failures, run in full twice (once before the
  `scope_observed_at` fix surfaced the `agent_storage_cli.rs` regression,
  once after).

## Verification run this session (in order)

- `cargo build -p swamp-core --lib` -- clean.
- `cargo build --workspace` -- clean.
- `cargo test --workspace --no-run` -- every test target compiles.
- `cargo test -p swamp-core --lib growth::` -- all green, including the
  2 new `artifact_shape` tests.
- `cargo test -p swamp-core --test project_worktree_tables --test
  coverage_series_summary_notes_tables --test units_nested_evidence_tables
  --test unowned_worktree_entries_schedule_github_wiring --test
  store_contents_are_allowlisted --test report_is_a_pure_read --test
  report_golden` -- green (this is where the empty-relative-path bug and
  the tampered_fields threshold both surfaced and were fixed).
- `cargo test --workspace --locked` -- full run, first pass: 2 failures
  in `crates/cli/tests/agent_storage_cli.rs` (the `scope_observed_at`
  bug). Fixed; full run again: 0 failures.
- `cargo fmt --all --check` -- one diff (two long lines in the new
  `artifact_shape_rows_by_worktree`), fixed with `cargo fmt --all`,
  clean after.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` --
  clean.
- `cargo run --locked -p swamp-source-audit` -- one failure first run
  (`no_unreferenced_public_items`: `snapshot_from_observation` had no
  callers left after the test rewrites); deleted the function, clean
  after.
- `scripts/check.sh` (`SWAMP_TARGET_DIR` pointed at the shared warm
  target dir) -- full pass, `== done`, no `FAILED` anywhere in the log.

## Deviations from the chunk text, and why

None. Both items in R18a-3b's text (the table, and the deletion) are
done exactly as written; no requirement was narrowed or deferred. The
two bugs this slice found (`Path::join("")`'s trailing separator; the
zero-projects "no observation" gate) are new work this slice's own
changes required, not scope this slice chose to take on.

## Follow-ups for the next slice (R18a-4 / R18b)

- `last_report-*.json.zst` (`consumers/signals.rs`'s previous git
  signals, `consumers/cargo.rs`'s previous nested-artifacts cache) is
  untouched -- still the per-root incremental-walk cache, a different,
  lower-level need than the scope-wide snapshot this slice finished
  deleting. R18a-4's job, per the owner's progress log.
- `fsevents.json`/`docker_facts.json`/`scope.json`/`last_run.json`/
  `ledger.jsonl` are all still JSON; R18b's job.
