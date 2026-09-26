# R18a: unit provenance/members, unowned lists/evidence, cargo-guidance clock fix -- partial (2026-09-24)

CHUNK_R18a asked for five numbered items, "nothing may be deferred."
This session completed items 1's unit-table sub-piece, item 3 in full,
and item 4 in full (root cause supplied by the integration owner after
this session's own trace of the report path came up empty -- see
below); it made real progress on item 1 but did not finish the rest of
it, and did not attempt item 2 at all. This note is the honest
accounting the brief requires; it is not a claim that the slice is
done.

## What was actually shipped

1. `external_units.parquet`/`agent_units.parquet` (`StoredUnitRow`) gain
   six new typed columns: `provenance_kind`/`provenance_value` (an
   `ExternalUnit`'s `locations::Provenance` variant tag + payload --
   `"builtin"`/`None` for `BuiltinConvention`, `"env-var"`/the var name
   for `EnvVar`, etc.), `hardlinked` (both families -- a real per-unit
   fold fact, previously only available from the snapshot's JSON copy),
   and, agent-only, `tool_home`, `relative_path`, `action`
   (`AgentActionCapability::label`). `external_unit_from_stored`/
   `agent_unit_from_stored` read the typed column first and fall back to
   `old` (the snapshot copy) only for a store written before these
   columns existed.
2. New child table `agent_unit_members.parquet`: `scope_key`, `unit_id`,
   `seq`, `path`, `bytes`, `kind` (`AgentMemberKind::label`) -- an
   `AgentUnit::members` list, written/read alongside the unit tables in
   `write_unit_tables`/`read_unit_tables`.
3. `unowned.parquet` (item 3) drops `containers_json`/`shared_with_json`/
   `evidence_json` entirely. Two new per-volume tables, co-located with
   `unowned.parquet` (no `scope_key` -- `path_or_object` is already that
   table's own unique key within one volume):
   - `unowned_lists.parquet`: `path_or_object`, `list_kind`
     (`"container"` | `"shared-with"`), `seq`, `value`.
   - `unowned_evidence.parquet`: reuses `evidence.parquet`'s own row
     shape and (de)serialization (`stored_evidence_rows`/
     `evidence_from_stored_rows`) rather than inventing a second
     encoding, with `scope_key` written as `""` (unused; the file itself
     is already volume-scoped, one file per volume like
     `unowned.parquet`) and `row_key = path_or_object`.
4. **Item 4, the flaky test -- fixed.** Root cause supplied by the
   integration owner (this session's own trace of
   `report::report_scope_from_store`'s call graph found nothing, because
   the bug was not on that path at all): `Report.nested_artifacts` had
   `#[serde(serialize_with = "crate::cargo_cleanup::serialize_units")]`
   (`report.rs`), and `serialize_units` called `guidance(unit)` --
   `guidance_at(unit, crate::entities::now())` -- **at serialization
   time**, not observation time. So the test's two `serde_json::to_value`
   calls (one on `observation.merged`, one on the rebuilt `snapshot.
   report`) each independently computed `modified_age_secs` from
   whatever the wall clock happened to read at that instant -- a real
   "`swamp report --json` is not a pure read" bug, not just a test
   artifact. Fix: `NestedArtifact` gained its own `guidance:
   cargo_cleanup::Guidance` field (`#[serde(rename = "cleanup")]`,
   reproducing the old JSON shape exactly), computed exactly once per
   observe pass by a new `report::attach_cargo_guidance` (called
   alongside `attach_nested_decision_evidence` inside
   `attach_decision_evidence`, which `bus::run_report` already calls once
   per root) from that pass's own fixed `observed_at`. The
   `serialize_with` hook and `serialize_units` are gone. `Guidance`'s six
   `&'static str` fields became owned `String` (needed so a value read
   back from `nested_artifacts.parquet`'s new typed `guidance_*` columns
   -- `guidance_recommendation`/`guidance_modified_age_secs`/
   `guidance_consequence`/`guidance_scope`/`guidance_check_status`/
   `guidance_reason_code`/`guidance_message`/`guidance_next_action` --
   can be reconstructed without leaking memory or guessing which static
   constant it came from); `CheckResult`'s matching fields followed for
   the same reason (`cargo_cleanup::check`, the human-initiated live
   review action, is otherwise unchanged and still calls `guidance(unit)`
   live on purpose -- a bounded, on-demand check is not the "report is a
   pure read" contract). `render.rs`/`tui/model.rs`'s few
   `match guidance(...).field { "literal" => ... }` sites needed
   `.as_str()`; their `== "literal"` comparisons needed no change
   (`PartialEq<str>` for `String`).

## Tests (adversarial; each names the shortcut it fails)

- `growth::tests::unit_tables_round_trip_every_field_and_every_linkage_state`
  (extended, not new): now constructs the external unit with
  `Provenance::EnvVar(...)` (was `BuiltinConvention`, which round-trips
  trivially even through the old JSON path) and gives one agent unit two
  real `AgentMember` rows; both `external_unit_from_stored`/
  `agent_unit_from_stored` calls now pass `old: None`, which fails the
  shortcut "still secretly reading the snapshot fallback" outright (the
  old code path could only produce a value when `old` was `Some`).
- `growth::tests::unowned_round_trips_lists_and_evidence_through_their_own_tables`
  (new): a Docker-object row with two containers, one shared-with entry,
  and one `Evidence` value, plus a plain filesystem row with none of
  those, round-tripped through `write_unowned`/`read_unowned`. Asserts
  the docker row's lists keep order and the evidence round-trips exactly
  (fails "swap the JSON cell for a table but still glue rows together
  wrong"), asserts the plain row picks up nothing from the docker row's
  child rows (fails "join without filtering by key"), and asserts a
  second `write_unowned` with fewer rows leaves no stale child rows
  behind (fails "append instead of replace wholesale," matching
  `unowned.parquet`'s own no-growth-semantics contract).
- `growth::tests::nested_artifact_table_round_trips_every_field_and_splits_by_origin`
  (extended): gives the fixture unit a real, non-default `Guidance`
  (`modified_age_secs: Some(7_200)`, a real recommendation/consequence/
  etc., not empty strings) and adds a second assertion that rebuilds
  with `old: None` and compares only `.guidance`, failing the shortcut
  "still secretly falling back to the snapshot's stale copy."
- `cargo_recommendations.rs::report_json_with_nested_artifacts_serializes_byte_identically_across_a_real_clock_gap`
  (new -- the direct regression test): builds one `Report` with a fixed
  `observed_at` and one nested unit, calls `attach_decision_evidence`
  once (the same finalizer `bus::run_report` calls), serializes it to
  JSON, sleeps a real 1.1 real-world seconds (crossing a whole-second
  boundary on purpose), serializes it again, and asserts the two JSON
  strings are byte-identical -- this is exactly the scenario that used
  to produce "39 vs 40." Also asserts `modified_age_secs` in the output
  equals the report's own fixed age, not a value derived from `now()` at
  test-run time.
- Existing tests re-run unchanged in substance and still pass:
  `project_worktree_tables.rs` (including the now-former-flake test,
  run standalone five times with no failure both before and after the
  fix -- the fix removes the *mechanism*, not just this one symptom),
  `units_nested_evidence_tables.rs`, `report_is_a_pure_read.rs`,
  `coverage_series_summary_notes_tables.rs`, `report_golden.rs`
  (byte-identical golden output unaffected), the TUI's
  `frames.rs::cargo_cleanup_guidance_frames` (rendering unaffected --
  `render.rs`/`model.rs` still call `guidance`/`guidance_at` live, which
  is unchanged), the full `swamp-core` lib suite (828 passed before this
  session's additions; three new/extended tests bring it to 831).

## What is NOT done (the honest remainder)

**Item 1, the big piece.** `report_rows.parquet` is not deleted.
`StoredReportSnapshotRow`'s `report_json`/`external_units_json`/
`agent_units_json`/`store_interiors_json` cells are all still present on
disk and still read by `growth::read_report_snapshot`. What changed:
`external_units_json`/`agent_units_json` are no longer *needed* as a
merge fallback for the fields this session migrated (provenance,
hardlinked, tool_home, relative_path, action, members) -- but the cells
themselves, and the code path that reads them, are untouched. Also
untouched: `Report.unowned` (no `unowned_summary.parquet` -- the R17
session note's blocker, "unowned rows have no natural single-root
partition key," still applies, and a scope-wide aggregation table was
not designed this session), `dirs_by_worktree`/`files_by_worktree` (no
`worktree_dirs.parquet`/`worktree_files.parquet`), `schedule_line` (no
column on `summary.parquet` or elsewhere), `github_enrichment` (no
`github_enrichment.parquet`), and `nested_artifacts.parquet`'s own long
not-yet-migrated field list (`parent_id`, `membership`, `is_dir`,
`device`, `inode`, `logical_bytes`, `physical_bytes`, `physical_total`,
`coverage`, `producer_evidence`/`consumer_evidence`, `action_group`,
`present`, `growth_bytes`, `regrowth_count`, `action`, `reported_by`,
`writer_lock`, and the variant's `package`/`version`/`toolchain`/
`features`/`generation`/`unknowns`) -- a migration comparable in size to
this session's unit-table work, not attempted.

**Item 2.** `last_report-*.json.zst` / `growth::write_last_report`/
`load_last_report` and their two per-root consumers
(`consumers/signals.rs`, `consumers/cargo.rs`) are entirely untouched.
Not attempted this session at all.

**Item 4, the flaky test -- now fixed.** This session's own investigation
traced every `crate::entities::now()`/`std::time::Instant::now()` call
reachable from `report::report_scope_from_store`'s call graph and found
none live on that path -- correctly, since the bug was not there. The
integration owner supplied the actual root cause (see "What was
actually shipped" item 4 above): `Report.nested_artifacts`'s
`serialize_with` hook computed cargo-cleanup guidance from a live clock
at *serialization* time, not observation time, so the test's two
`serde_json::to_value` calls could each see a different `now()`. Fixed
by computing `NestedArtifact::guidance` exactly once at observe time and
serializing it as a plain field. Verified with a new test that forces a
real wall-clock gap between two serializations of the same `Report` and
asserts byte-identical output (see above) -- not a rerun-to-green.

**Item 5, partially.** Round-trip and tamper-shaped tests exist for
everything this session actually shipped (see above), including the
item-4 fix. The `store_contents_are_allowlisted` test was not updated
to name exactly which non-Parquet files remain (its current allow-list
already accepts every new file here via the blanket `.parquet` rule, so
it did not need a change for *this* session's tables specifically, but
the broader "list exactly which non-Parquet files remain" ask from the
chunk was not done, since that requires the item-1/item-2 work above
too).

## Non-Parquet files remaining in the store (unchanged by this session)

`config.toml`, `ui_state.json`, `fsevents.json`, `docker_facts.json`,
`scope.json`, `last_run.json`, `ledger.jsonl`, `last_report-*.json.zst`,
plus lock files. None of these were touched here, per the hard
constraint not to touch `fsevents.json`/`docker_facts.json`/
`scope.json`/`last_run.json`/`ledger.jsonl` (R18b's job) and because
`last_report-*.json.zst` (item 2) was not attempted.

## Verification run this session

In order, all green: `cargo build -p swamp-core --lib` (clean); `cargo
build --workspace`; `cargo test --workspace --locked` (full run, exit
0, every test binary "ok", grepped for `FAILED`/`panicked` with no
hits); `cargo fmt --all --check` (clean after two `cargo fmt --all`
passes -- the second after the item-4 fix's edits); `cargo clippy
--workspace --all-targets --locked -- -D warnings` (one round of real
findings from the item-4 fix -- four `useless_conversion` lints in
`tui/src/model.rs` from `.consequence.into()` now being a `String ->
String` no-op -- fixed, not suppressed); `cargo run -p
swamp-source-audit` (all 11 audits `ok`); `scripts/check.sh` (fresh
run via `SWAMP_TARGET_DIR`, exit 0, every step including `named-targets`
and `greps` completed). Targeted re-runs of the specific tests this
session's changes touch (`project_worktree_tables`, `report_golden`,
`cargo_recommendations`, `nested_artifact_evidence_is_delivered`,
`review_cargo_regressions`, `units_nested_evidence_tables`,
`report_is_a_pure_read`) all green.

## Why this session stopped here

R18a's own five items are, combined, a multi-day migration. This
session shipped items 3 and 4 in full and a real slice of item 1 (the
unit-table typed-column migration), but the rest of item 1 --
`report_rows.parquet` itself, `Report.unowned`/`dirs_by_worktree`/
`files_by_worktree`/`schedule_line`/`github_enrichment`, and
`nested_artifacts.parquet`'s own long not-yet-migrated field list (a
second typed-column migration comparable in size to this session's
unit-table work) -- and all of item 2 (the per-root git-signals/cargo
replay cache) remain. Per the worker brief's own rule ("if genuinely
blocked, finish everything else, then state the blocker precisely; do
not narrow scope silently"), this note names exactly what shipped, what
didn't, and why, rather than presenting a partial slice as the whole
chunk. The next worker should start from "what is NOT done" above, in
the order the chunk lists them within item 1, then item 2.
