# R18a-3: unowned/worktree-entries/schedule/github typed; `report_json` NOT deleted (2026-09-24)

CHUNK_R18a-3 asked for four things: (1) `Report.unowned` ->
`unowned_summary.parquet`; (2) `dirs_by_worktree`/`files_by_worktree` ->
`worktree_entries.parquet`; (3) `schedule_line` -> a `summary.parquet`
row, `github_enrichment` -> `github_enrichment.parquet`; (4) enumerate
every remaining `Report` field still coming from `report_json` and type
it this slice, then DELETE `report_json`, `StoredReportSnapshotRow`,
`ReportSnapshot`, `write_report_snapshot`, `slim_report_for_snapshot_json`,
`report_rows.parquet` and every read path that touched them.

Items 1-3 are done. Item 4's enumeration surfaced a real, pre-existing
gap that makes the deletion in its final paragraph unsafe to do this
slice — implemented below, not narrowed silently, per the worker brief's
rule against relabeling a requirement.

## What shipped

**`unowned_summary.parquet`** (+ `unowned_summary_lists.parquet`):
`Report.unowned`, the scope-wide list `observe_scope` already merges
across every root in scope — distinct from the per-volume
`unowned.parquet` R18a's first session typed (that one is per-root;
`Report.unowned` is the already-merged view a scope-wide read needs).
Keyed by `(scope_key, seq)`, not `path_or_object` — two roots merged
into one scope can each independently produce an unowned row with the
same shared-cache basename, so `path_or_object` alone is not a safe key
here the way it is for the single-volume table. `unowned_summary_lists.
parquet` carries `containers`/`shared_with`, disambiguated by
`list_kind`, same shape as the per-volume table's own child list table.
Evidence reuses `evidence.parquet` directly (entity kind
`"unowned-summary"`, id = the row's `seq`) — folded into the *same*
`evidence_entities` list `observe_scope` already builds for artifacts/
external units/agent units/nested artifacts, not a second
`write_evidence_table` call (a second call would re-read the file,
filter out this scope's rows the first call just wrote, and push only
the unowned ones — silently deleting the other entities' evidence).

**`worktree_entries.parquet`**: `Report.dirs_by_worktree`/
`.files_by_worktree`, one table with a `kind` column (`"dir"` | `"file"`)
per the chunk's own suggestion, rather than two files. Columns: dirs-
only `parent_rel_path`/`track`/`allocated_total`/`own_allocated`/
`file_count`/`entry_count`/`symlink_count`/`complete`, files-only
`allocated`, and `mod_time_min`/`growth_bytes`/`observed_at` shared by
both. `dirs_by_worktree`/`files_by_worktree` are always populated
together by the same `include_dirs` flag `observe_scope` takes, so "any
row exists for this `scope_key`" is the one signal the rebuild needs:
both come back `None` together (an older store, or a pass that did not
ask for dirs), or `Some` together — proven by a dedicated unit test
(`worktree_entries_table_round_trips_every_field_and_is_none_together_
when_empty`).

**`schedule_line`**: a `summary.parquet` row (`metric = "schedule"`,
`key = "line"`), chosen over `runs.parquet` (R18b, not landed yet — the
chunk said "choose one and say why") because `summary.parquet` already
exists, is already written/read in the exact same `observe_scope`/
`report_scope_from_store` call this field needs, and needs only one new
nullable column (`text`) rather than a whole new table for one string.
`write_summary_table` gained a `schedule_line: Option<&str>` parameter
so the row lands in the same single wholesale write as every other
summary row, not a second write to the same file (same reasoning as the
evidence fold-in above).

**`github_enrichment.parquet`**: `Report.github_enrichment`, at most one
row per `scope_key`, wholesale-replaced (zero rows when a pass ran no
live enrichment).

## What did NOT ship, and why: `report_json` stays

Mid-slice, after items 1-3 were wired and `report_scope_from_store` was
switched to bootstrap from a from-scratch `Report` instead of
deserializing `report_json` (per the chunk's final paragraph), the full
workspace test suite caught a real regression:
`project_worktree_tables::observe_writes_the_three_tables_and_every_
view_renders_identically_from_them` failed — every rebuilt artifact
row's array came back empty.

The cause: `report::rebuild_projects_from_tables` seeds its rebuilt
artifact list's *shape* — `ArtifactRow::kind`/`path`/`track`/
`confidence`/`source`/`note`/`created_at`/`containers`/`shared_with`/
`dangling`/`allocated_bytes`/`allocated_growth_bytes` — from
`old_artifacts_by_worktree`, built from whatever
`snapshot.report.projects` already held *before* the rebuild runs (only
then does it overlay `bytes`/`local_bytes`/`mtime_max`/`hardlinked`/
`dedup_stale`/`regrowth_count`/`ecosystem`/`observed_at` from the
current-artifact-history table). R15 item 3's own doc comment already
named this precisely: it typed only those eight/nine facts into
`ArtifactTableFacts`, explicitly documenting the rest as "explicitly
later slices" — and no slice between R15 and R18a-2 picked it up.
`report_json`'s deserialized `Report.projects` was the *only* remaining
source for that shape; it was never migrated because R16-R18a-2 were
each scoped to their own named tables (units, nested artifacts, evidence,
coverage/series/summary/notes) and this artifact-shape debt was never
anyone's assigned item.

Proceeding with the deletion anyway would not fail loudly: a store
written and then read back through `report_scope_from_store` after
`report_json` was gone would silently render every artifact with a
blank kind/path/tracking/confidence/containers — exactly the kind of
fabricated-gap regression the worker brief's lessons section names
("Adapter fixture tests do NOT prove combined behavior... test both call
orders"; the equivalent failure mode here is "delete the fallback before
its replacement exists"). Given the choice between (a) shipping that
regression to hit the letter of the chunk's last paragraph, or (b)
keeping the cell one more slice and stating the gap precisely, I chose
(b) — implementing the requirement (typing 1-3) and flagging the
concern (4's deletion is unsafe), per the worker brief's explicit rule:
"If a requirement seems wrong, implement the requirement, state the
concern in your report, and let the user decide. Never relabel a
contract and call it approved."

`report_json` is further slimmed instead: `slim_report_for_snapshot_json`
now also clears `unowned`/`dirs_by_worktree`/`files_by_worktree`/
`schedule_line`/`github_enrichment` before serializing (same discipline
R17 already applied to `notes`/`summary`/`reconciliation`/series) — the
new tables are authoritative for those five fields even though the cell
still exists for the artifact-shape fields.

**Proof requested by the chunk, given honestly**: `grep -nE 'pub \w+_json:'
crates/core/src/growth/columns.rs` still prints one hit
(`StoredReportSnapshotRow.report_json`) — deliberately, for the reason
above. `report_rows.parquet` is still written. Both facts are stated
here rather than fabricated as done.

```
$ grep -nE 'pub \w+_json:' crates/core/src/growth/columns.rs
1330:    pub report_json: String,
```

## What R18a-4 (or a new R18a-3b) must do first

Type `ArtifactRow`'s remaining fields (`kind`, `path`, `track`,
`confidence`, `source`, `note`, `created_at`, `containers`,
`shared_with`, `dangling`, `allocated_bytes`, `allocated_growth_bytes`)
into a table — likely a new `artifact_shape.parquet` (or an extension of
`worktree_facts.parquet`), with child tables for `containers`/
`shared_with` — keyed the same way `artifact_row_key` already keys the
current-artifact-history facts, so `rebuild_projects_from_tables` can
build `old_artifacts_by_worktree`'s row *shape* from a table instead of
`snapshot.report.projects`. Only after that is `report_json` actually
empty and safe to delete. This is real, separate work — sized like its
own slice, not a five-minute follow-up.

## Tests

- `growth::tests::unowned_summary_table_round_trips_every_field_and_keyed_by_seq_not_path`
  — round trip, including two rows sharing one `path_or_object`.
- `growth::tests::unowned_summary_tables_rebuild_reflects_a_direct_tamper_not_the_original_value`
  — on-disk tamper of `unowned_summary.parquet`/`unowned_summary_lists.parquet`.
- `growth::tests::worktree_entries_table_round_trips_every_field_and_is_none_together_when_empty`
  — round trip across two worktrees, both kinds, plus the `None`/`None`
  symmetry proof.
- `growth::tests::worktree_entries_table_rebuild_reflects_a_direct_tamper_not_the_original_value`
- `growth::tests::github_enrichment_table_round_trips_and_is_none_when_absent`
  — `Some` round trip, `None` for an absent scope, and a later pass with
  no live enrichment clearing an earlier row.
- `growth::tests::github_enrichment_table_rebuild_reflects_a_direct_tamper_not_the_original_value`
- `growth::tests::schedule_line_round_trips_through_summary_table`
- `crates/core/tests/unowned_worktree_entries_schedule_github_wiring.rs`
  (new file): `observe_writes_the_four_tables_and_report_rebuilds_
  unowned_worktree_entries_and_github` (real `observe_scope` + a real
  `report_scope_from_store` read, proving the wiring, not just the
  table-level round trip) and `report_reads_unowned_from_the_table_
  reflecting_a_direct_tamper` (on-disk tamper reachable through the
  real entry point).
- `crates/core/tests/project_worktree_tables.rs`'s
  `the_remaining_parts_still_come_from_the_snapshot` flips direction:
  before this slice it proved `unowned` still came from `report_json`;
  now it proves the opposite — a JSON-only tamper of `.report.unowned`
  must *not* leak into the rebuild, since `unowned_summary.parquet` now
  wins. The old assertion direction would have failed after this slice
  landed, so it was corrected rather than left pinning a boundary that
  moved (this is the one existing test this slice's change required
  editing, not deleting).
- `store_contents_are_allowlisted.rs`: unchanged — its allow-list only
  ever named non-Parquet control files, none of which is
  `report_rows.parquet`, so no edit was needed there; every new
  `*.parquet` file is already covered by its wildcard rule.
- Full workspace suite (`cargo test --workspace --locked`): see the
  accompanying report for this run's exact pass/fail counts; all new and
  edited tests pass in isolation and in the full run performed while
  writing this note.

## Verification run this session (in order)

- `cargo build -p swamp-core --lib` / `cargo build --workspace` — clean.
- `cargo test -p swamp-core --lib growth::` — all growth-module tests
  green, including the 8 new ones.
- `cargo test -p swamp-core --test project_worktree_tables --test
  coverage_series_summary_notes_tables --test
  units_nested_evidence_tables --test
  unowned_worktree_entries_schedule_github_wiring --test
  store_contents_are_allowlisted --test report_is_a_pure_read --test
  report_golden` — green (this caught the artifact-shape regression
  described above on the first run, before it was reverted to keeping
  `report_json`).
- `cargo test --workspace --locked` — full run; see the report for the
  exact tail.
- `cargo fmt --all --check` — clean (after one `cargo fmt --all` round
  for the new code).
- `cargo clippy --workspace --all-targets --locked -- -D warnings` — see
  report.
- `cargo run --locked -p swamp-source-audit` — see report.
- `scripts/check.sh` — see report.

## Deviations from the chunk text, and why

Explicitly named above: item 4's final paragraph ("DELETE `report_json`,
`StoredReportSnapshotRow`, `ReportSnapshot`, ... `report_rows.parquet`")
is not done. Everything else in the chunk (items 1-3, the table designs,
the tamper-test requirement) is done as written. This is a real
mid-session discovery, not a pre-existing intent to narrow scope — the
first attempt at the deletion is what surfaced it, and it was reverted
rather than shipped once the workspace test suite caught it.
