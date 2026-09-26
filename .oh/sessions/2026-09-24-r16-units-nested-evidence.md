# 2026-09-24 -- R16: JSON-in-the-store decomposition, units/consumers, nested artifacts, evidence

Branch `stack/26-aim-review-repairs`, worktree
`/Users/muness1/src/open-horizon-labs/swamp-builds`. Source:
`CHUNK_R16.md` ("THIS SLICE ONLY -- units, consumers, nested artifacts,
evidence"), continuing `.oh/sessions/2026-09-24-r15-project-worktree-
artifact-tables.md`'s overlay seam. Before this slice: the three
pre-existing CI failures (see the separate R16 CI-red session note) and
a CI-tier change (full tier off the push path, then onto release tags)
that arrived mid-session as amendments; neither is this note's subject.

## What was done

Same policy as R14/R15: no migration; a store written before a table
existed is read as before. All Arrow lives in
`crates/core/src/growth/columns.rs`; `growth.rs` owns the domain
conversions; `report.rs` wires the write (inside `observe_scope`, same
gate as the snapshot) and the read (`report_scope_from_store`, one
`rebuild_*_from_tables` call per table, evidence last).

1. **`external_units.parquet` / `agent_units.parquet`** -- one shared
   `StoredUnitRow` shape for both files. `id`, `source_id`/`source_name`,
   `category`, `path`, `bytes`, `mtime_max`, `observed_at`,
   `growth_bytes`, `regrowth_count` are common. `complete`/
   `linkage_state`/`linkage_basis`/`project_id`/`protected`/
   `protect_reason` are agent-only (`None` on an external row): an
   external unit has no partial-fold cap and no direct project link --
   its associations are the child table below. `consequence` is each
   type's own `note` field (neither struct has a field literally called
   that).
2. **`unit_consumers.parquet`** -- the child table for
   `ExternalUnit::consumers` (an `AgentUnit` has no consumer list, only
   a singular `project_link`, which lives on the unit row).
3. **`nested_artifacts.parquet`** -- one row per `NestedArtifact`, from
   either producer that builds them (`Report.nested_artifacts` and
   `ReportSnapshot.store_interiors`, the same production type from two
   different call sites); an `origin` column says which list a row came
   from, so reading splits them back apart losslessly.
4. **`evidence.parquet`** -- one row per `crate::evidence::Evidence`,
   across every entity kind that carries the #53 decision-evidence
   contract: an `ArtifactRow`, an `ExternalUnit`/`AgentUnit`, a
   `NestedArtifact`'s `decision_evidence` (not its older, narrower
   `producer_evidence`/`consumer_evidence`). `row_key` is
   `"<entity_kind>:<id>"` so one table keys every entity kind.

`observe_scope` writes all four under the snapshot's gate, from the
already-measured `observation.external_units`/`.agent_units`/
`.merged.nested_artifacts`/`.store_interiors` -- no second discovery
pass. `report_scope_from_store` rebuilds `external_units`/`agent_units`/
`nested_artifacts`/`store_interiors` from the first three tables
(overlaying not-yet-migrated fields from the snapshot by id, same
pattern as R15's artifacts), then replaces every entity's
`evidence`/`decision_evidence` from `evidence.parquet` -- run last so it
sees the final lists, and never overlaid (an entity can legitimately
have zero evidence, which is not distinguishable from "table not
written" by row count alone; `growth::evidence_table_exists` checks the
file's existence instead).

## Deviations from CHUNK_R16's literal column list (each one because
byte-identity or reality required it, same discipline as R15's `remote`)

- Units: no `allocated` column -- neither `ExternalUnit` nor `AgentUnit`
  has a bytes concept distinct from `bytes` itself (that is
  `ArtifactRow`'s, already migrated in R15); there is nothing to store.
  `consequence` maps to `note`, the closest field either type actually
  carries.
- `unit_consumers.parquet`: `consumer_project_id` is always `None`
  (`ExternalConsumer` has no such field, nothing produces one yet);
  `basis` maps to `ExternalConsumer::note`.
- `nested_artifacts.parquet`: `container_id` is CHUNK_R16's "parent
  artifact row key" -- the closest thing that exists (the id of the
  *container* `NestedArtifact` whose `path` equals the owning
  `ArtifactRow`'s path; a `NestedArtifact` has no other pointer back to
  an artifact row). Added `origin` (not in the chunk's list) so the two
  producer lists split back apart without guessing from id/path
  overlap.
- `evidence.parquet`: the chunk's `value_num`/`value_ts`/`value_text`
  alone cannot reconstruct `FactValue`'s seven variants or
  `FactStatus::Conflicting` (a `Vec<FactValue>`, always length >= 2 in
  production -- `reclaimability.rs`/`toolchain_declarations.rs`/
  `external_associations.rs` all populate it). Added `value_kind` (the
  `FactValue` tag) and `conflicting_extra` (every candidate after the
  first, `|`-joined -- every production site uses one `FactValue`
  variant across all its candidates, so one `value_kind` describes
  them all). Added `reason` (`Unknown`/`Unavailable`/`Conflicting`'s
  message) and `freshness_expires_after_secs`/`freshness_coverage_note`:
  named on `Evidence`/`FactStatus` but not in the chunk's list, and
  dropping them would change `agent_json`'s serialized `"evidence"`
  array. Every label function (`FactKind`/`FactSubtype`/`EvidenceSource`)
  is an exhaustive `match`, not a serde round-trip, matching every other
  `label`/`from_label` pair in this codebase (`AgentCategory::from_label`
  is the stated precedent: a new variant is a compile error here, not
  a value that silently drops).

## Tests (adversarial; each names the shortcut it fails)

- `crates/core/src/growth.rs`'s own `mod tests` (internal, so arbitrary
  `ExternalUnit`/`AgentUnit`/`NestedArtifact`/`Evidence` values are cheap
  to construct without a detector-based fixture):
  - `unit_tables_round_trip_every_field_and_every_linkage_state` -- all
    nine `ProjectLinkState` variants (`Linked` x2 with both
    `LinkSource`s, `Unresolved`, `Missing`, `NotAProject`, `Moved`,
    `Remote`, `Shared`, `NotApplicable`), an `ExternalUnit` with two
    consumers (one with a note, one without); a second scope key's rows
    do not disturb the first's.
  - `nested_artifact_table_round_trips_every_field_and_splits_by_origin`
    -- a fully populated `report`-origin and `store-interior`-origin row
    round-trip and never mix.
  - `evidence_table_round_trips_every_status_and_source_variant` -- one
    `Evidence` per `FactValue` variant, `Unknown`, `Unavailable`, two
    `Conflicting` cases (`Text` and `Bytes` candidates, the two shapes
    production actually builds), every `EvidenceSource` variant used at
    least once, `event_at`/`freshness`/`note` all set on at least one
    entry, list order preserved; `evidence_table_exists` pinned `false`
    before any write and `true` after.
- `crates/core/tests/units_nested_evidence_tables.rs` -- a real fixture
  (a Cargo-home directory with actual cache bytes, `CARGO_HOME`; a
  repo-linked Claude Code session, `CLAUDE_CONFIG_DIR`; both detectors
  enabled, nothing else), through `observe_scope` then
  `report_scope_from_store`:
  - `observe_writes_the_four_tables_and_report_rebuilds_units_and_nested_artifacts_from_them`
    -- asserts the fixture produces at least one external unit, one
    linked agent unit, and prints the store listing; asserts all four
    files exist; asserts `external_units`/`agent_units`/
    `nested_artifacts`/`store_interiors` equal the observed ones as
    `serde_json::Value`. Fails a lossy rebuild.
  - `report_reads_units_and_nested_artifacts_from_the_tables_not_the_snapshot_json`
    -- tampers every migrated field in the snapshot's own JSON copy
    (bytes, names, protected, consumer labels, nested-artifact path);
    the rebuild must still equal the untampered observation. Fails
    "keep deserializing the snapshot and merely also write the tables".
- `crates/core/tests/store_contents_are_allowlisted.rs` needed no edit
  (already allows every `*.parquet` name).

## Store listing after one observe on the fixture (printed by the test)

```
agent_units.parquet
evidence.parquet
external_units.parquet
nested_artifacts.parquet
projects.parquet
report_rows.parquet
unit_consumers.parquet
worktree_facts.parquet
worktrees.parquet
<volume>/current.parquet, dirs.parquet, fsevents.json, observation.lock,
  topology.json, unowned.parquet
external/current.parquet, external/folded.parquet
associations/*.parquet
last_report-*.json.zst, last_run.json, scope.json, config.toml
```

Seven typed tables now exist at the top of the store (protect,
projects, worktrees, worktree_facts, external_units, agent_units,
unit_consumers, nested_artifacts, evidence -- nine, not counting the
per-volume/associations ones); `fsevents.json`/`topology.json`/
`last_report-*.json.zst`/`scope.json`/`last_run.json` remain the JSON
control/cache files R15's note already named as later slices.

## Verification

In order: `cargo fmt --all --check`, `cargo test --workspace --locked`
(with `GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=commit.gpgsign
GIT_CONFIG_VALUE_0=false`), `cargo clippy --workspace --all-targets
--locked -- -D warnings`, `cargo run -p swamp-source-audit` (11/11),
`scripts/check.sh`. All green. One cargo invocation at a time against
the shared target dir; no polling loops; `ps` confirmed clean before
the final report.

## What remains (item A, tables 5-~10; not attempted)

Coverage, series, summary, `unowned.parquet`'s JSON cells,
`topology.json`, `fsevents.json`, `docker_facts.json`, `scope.json`,
`last_run.json`, `ledger.jsonl`, then deleting `report_rows.parquet`'s
remaining JSON cells and `last_report-*.json.zst`, then the
serde_json-into-Parquet audit rule. The overlay in
`rebuild_projects_from_tables`/the new unit and nested-artifact rebuild
functions is still the seam the next slices shrink.
