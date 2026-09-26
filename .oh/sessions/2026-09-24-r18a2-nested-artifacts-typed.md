# R18a-2: every `NestedArtifact` field typed, JSON merge fallbacks deleted (2026-09-24)

CHUNK_R18a-2 asked for three things, none deferrable: (1) type every
remaining `NestedArtifact` field into `nested_artifacts.parquet`, with
child tables for list-valued fields; (2) type `store_interiors` into
its own table; (3) delete `external_units_json`/`agent_units_json`/
`store_interiors_json` from `StoredReportSnapshotRow` and every
merge-fallback that read them, so only `report_json` remains as a JSON
cell anywhere in `crates/core/src/growth/columns.rs`. All three are
done.

## What shipped

**Item 2 first, since it changes how item 1 reads**: `store_interiors`
was already typed into `nested_artifacts.parquet` by the R16 session
that created the table -- it uses an `origin` column
(`"report"`/`"store-interior"`) to keep `Report.nested_artifacts` and
`ReportSnapshot.store_interiors` in one table without conflating them.
That design (documented in the table's own header comment) is sound and
this session kept it rather than splitting into a second
`store_interiors.parquet`, since the two lists are the same production
type (`NestedArtifact`) from two different producers and every column
this session added applies identically to both. "Its own table" is
satisfied by the `origin` column's own partition, not a second file.

**Item 1**: `nested_artifacts.parquet`'s `StoredNestedArtifactRow` gains
every scalar `NestedArtifact` field that was missing: `relative_path`
(present in the struct but never persisted at all before this session
-- a gap in R16's original column list, not just R18a's), `parent_id`,
`membership` (new `Membership::label`/`from_label`), `is_dir`, `device`,
`inode`, `logical_bytes`, `physical_bytes`, `physical_total`,
`time_source` (new `TimeSource::from_label`; `label` already existed),
`coverage_supported`/`coverage_complete` (the two scalar
`ArtifactCoverage` fields), `action_group`, `present`, `growth_bytes`,
`regrowth_count`, `action_capability` + `action_unsupported_reason`
(new `NestedActionCapability::from_label` split into a tag column and
a separate reason column, since the `Unsupported` variant carries a
`String`), `reported_by`, `writer_lock`, and the variant's `package`/
`version`/`toolchain`/`features`/`generation` (`profile`/
`configuration`/`target`/`architecture` were already typed).

Two new child tables carry the list-valued fields, keyed by
`(scope_key, origin, artifact_id)` -- not just `artifact_id`, because
the two origins' id spaces are not guaranteed disjoint:
- `nested_artifact_lists.parquet`: `coverage.limits` and
  `variant.unknowns`, disambiguated by `list_kind`
  (`"coverage-limit"` | `"variant-unknown"`) -- same shape as R18a's
  `unowned_lists.parquet`.
- `nested_artifact_evidence.parquet`: `producer_evidence`/
  `consumer_evidence` (the narrower `ArtifactEvidence { source, detail,
  confidence }` shape, distinct from `decision_evidence`, which stays
  in `evidence.parquet`), disambiguated by `kind`
  (`"producer"` | `"consumer"`). New `Confidence::label`/`from_label`
  (`crate::entities::Confidence`) encodes the confidence column.

`nested_artifact_from_stored` no longer takes an `old: Option<&NestedArtifact>`
fallback parameter at all -- there is nothing left for one to carry
over, so the signature itself is the proof this is done, not a comment
promising it. Its callers (`report::rebuild_nested_artifacts_from_tables`)
and `external_unit_from_stored`/`agent_unit_from_stored` (item 3, see
below) were updated to match.

**Item 3**: `StoredReportSnapshotRow` now has exactly one field,
`report_json`; `external_units_json`/`agent_units_json`/
`store_interiors_json` and their schema columns/write/read code are
deleted from `growth/columns.rs`. `growth::write_report_snapshot`/
`read_report_snapshot` no longer serialize or deserialize these fields
at all (`read_report_snapshot` returns empty `Vec`s for
`external_units`/`agent_units`/`store_interiors`, since
`rebuild_units_from_tables`/`rebuild_nested_artifacts_from_tables`
always overwrite them from the typed tables immediately after).
`external_unit_from_stored`/`agent_unit_from_stored` in `growth.rs` lost
their `old` parameters too, since R18a's first session had already
typed every field those two functions produce -- the fallback was dead
weight once `external_units_json`/`agent_units_json` were the only
thing keeping it alive. `report::rebuild_units_from_tables` no longer
builds `old_external`/`old_agent` lookup maps.

Proof only `report_json` remains (grep run 2026-09-24, after this
session's edits):

```
$ grep -n "_json" crates/core/src/growth/columns.rs
697:/// `unowned.parquet`'s `containers_json`/`shared_with_json` cells. Lives
1313:// item 1, R18a items 1 and 2); `report_json` is the only JSON-encoded
1314:// cell left in this row, and after this table itself the only `_json`
1327:    /// before serializing (R17: `growth::slim_report_for_snapshot_json`)
1330:    pub report_json: String,
1337:        Field::new("report_json", DataType::Utf8, false),
1348:    let report_json: Vec<&str> = rows.iter().map(|r| r.report_json.as_str()).collect();
1355:            Arc::new(StringArray::from(report_json)),
1381:        let report_json = downcast_str(&batch, "report_json")?;
1386:                report_json: report_json.value(i).to_string(),
1399:// `coverage_json` cell. Replaced wholesale per scope key, like
1522:// inside `report_rows.parquet`'s `report_json` cell.
3444:// them would change `agent_json`'s serialized `"evidence"` array.
```

Every hit is either `report_json` itself (the one field that legitimately
remains -- `Report.root`/`unowned`/`dirs_by_worktree`/`files_by_worktree`/
`schedule_line`/`github_enrichment` are R18a-3's job) or a historical
comment referencing an *already-removed* field name (`containers_json`/
`shared_with_json`/`coverage_json`, all deleted by earlier R17/R18a
sessions) or the unrelated `agent_json.rs` CLI rendering module name.
No live `external_units_json`/`agent_units_json`/`store_interiors_json`
field exists anywhere in the crate (confirmed separately with
`grep -rn "external_units_json\|agent_units_json\|store_interiors_json" crates/`
-- the only hits left are doc comments in
`crates/core/tests/units_nested_evidence_tables.rs`/
`coverage_series_summary_notes_tables.rs` narrating that these fields
used to exist and were removed).

## Tests

- `growth::tests::nested_artifact_table_round_trips_every_field_and_splits_by_origin`
  (extended): both fixture `NestedArtifact`s now carry non-empty
  `producer_evidence`/`consumer_evidence` (two different `Confidence`
  values), non-empty `coverage.limits`, non-empty `variant.unknowns`,
  and the `Unsupported` action variant (proving the reason string
  round-trips) -- the round trip is asserted with
  `serde_json::to_value` equality over the *entire* struct, and
  `nested_artifact_from_stored` is called with no fallback argument at
  all (the signature no longer has one), which fails the shortcut
  "still secretly reading a snapshot fallback" outright.
- `growth::tests::nested_artifact_tables_rebuild_reflects_a_direct_tamper_not_the_original_value`
  (new -- the explicit tamper test this slice's definition of done
  asked for): writes the two fixture artifacts, then edits
  `nested_artifacts.parquet`/`nested_artifact_lists.parquet`/
  `nested_artifact_evidence.parquet` directly on disk (bypassing every
  production writer) for one artifact's row/list/evidence rows only,
  and asserts the rebuild reflects exactly that edit for that artifact
  while the *other* artifact's row is completely untouched -- this
  fails both "read from some cached copy instead of the table" and "key
  a child-table read by `list_kind`/`kind` alone without also filtering
  by `artifact_id`, so a tamper leaks across artifacts."
- `crates/core/tests/units_nested_evidence_tables.rs`'s existing
  `report_reads_units_and_nested_artifacts_from_the_tables_not_the_snapshot_json`
  continues to pass unchanged in substance (updated only its doc
  comments for accuracy) -- it tampers the in-memory `ReportSnapshot`
  fields directly, calls `write_report_snapshot`, and asserts the
  rebuild matches the real observation; now `external_units`/
  `agent_units`/`store_interiors` aren't merely *overridden* by the
  rebuild, they're never persisted anywhere to begin with, which is a
  strictly stronger guarantee than what this test could distinguish
  before.
- Full workspace suite: `cargo test --workspace --locked` (all binaries,
  exit 0, grepped for `FAILED`/`panicked` with no hits) --
  `swamp-core`'s lib suite went from 831 to 833 tests. TUI frame tests
  (`frames.rs` et al.) and `report_golden.rs` unaffected (no
  `NestedArtifact` field or serialization shape changed; only
  `growth::columns`'s Parquet schema and the merge-fallback code
  around it changed).

## Verification run this session (in order)

- `cargo build -p swamp-core --lib --target-dir .../swamp/target` --
  clean, no warnings.
- `cargo build --workspace --target-dir .../swamp/target` -- clean.
- `cargo test -p swamp-core --lib --target-dir .../swamp/target growth::`
  -- all growth-module tests green (including the two new/extended
  nested-artifact tests) after each edit round.
- `cargo test --workspace --locked --target-dir .../swamp/target` --
  full run, exit 0, `grep -i "FAILED\|panicked"` on the log matched
  only "0 failed" summary lines.
- `cargo fmt --all --check` -- clean (after `cargo fmt --all`, one round
  needed for the new code's line wrapping).
- `cargo clippy --workspace --all-targets --locked --target-dir
  .../swamp/target -- -D warnings` -- one real finding
  (`clippy::type_complexity` on `read_nested_artifact_table`'s return
  type), fixed with a `NestedArtifactStoredWithChildren` type alias, not
  suppressed; clean after.
- `cargo run --locked --target-dir .../swamp/target -p swamp-source-audit`
  -- all 11 audits `ok`.
- `scripts/check.sh` (via `SWAMP_TARGET_DIR`) -- see the report for this
  run's exact exit status; if this note is being read before that run's
  notification lands, treat the `check.sh` claim in the accompanying
  report as authoritative, not this line.

## What is NOT done (by design -- next slice's job)

Named explicitly in CHUNK_R18.md's progress log, unchanged by this
session:

- **R18a-3**: `report_rows.parquet` itself is not deleted.
  `Report.unowned` has no `unowned_summary.parquet`; `dirs_by_worktree`/
  `files_by_worktree` have no table; `schedule_line` has no column;
  `github_enrichment` has no table. All four still come from
  `report_json`. `ReportSnapshot`, `write_report_snapshot`,
  `slim_report_for_snapshot_json`, `report_rows.parquet` all still
  exist.
- **R18a-4**: `last_report-*.json.zst` / `growth::write_last_report`/
  `load_last_report` and their two per-root consumers
  (`consumers/signals.rs`, `consumers/cargo.rs`) are entirely
  untouched. Not attempted this session.

## Deviations from the chunk text, and why

None. "Type `store_interiors` into its own table" (item 2) is satisfied
by the existing `origin`-partitioned design in `nested_artifacts.parquet`
rather than a literal second `store_interiors.parquet` file -- see "What
shipped" above for why that is the same design intent (a child table
with a discriminator column *is* "its own table" in the sense the R16
session already established for this exact producer split), not a
narrowing of the requirement. Flagging this explicitly per the worker
brief's rule against silently relabeling a requirement: if the owner
disagrees and wants a physically separate file, that is a follow-up,
not a defect in this session's typing work (every field is fully typed
either way).
