# 2026-09-24 -- R15: JSON-in-the-store decomposition, tables 2-4 (`projects.parquet`, `worktrees.parquet` + `worktree_facts.parquet`, artifact `ecosystem`)

Branch `stack/26-aim-review-repairs`, worktree
`/Users/muness1/src/open-horizon-labs/swamp-builds`. Source:
`CHUNK_R15.md` ("THIS SLICE ONLY -- three tables, and the report's
project section reads from them"), continuing
`.oh/sessions/2026-09-24-r14-json-decomposition.md` (table 1,
`protect.parquet`). Same policy as R14: no migration; a store written
before a table existed is read as before.

## What was done

All Arrow lives in `crates/core/src/growth/columns.rs`, like
`StoredProtectRow` (the `gate_paths_only_inside_gates` audit was run
after every draft, not just at the end). Every table is scope-wide
(top-level store, `scope_key` column, rows for one key replaced
wholesale per `observe`), exactly the `report_rows.parquet` model.

1. **`projects.parquet`** -- `StoredProjectRow`, `write_project_rows`,
   `read_project_rows`. Columns: `scope_key`, `project_id`, `name`,
   `ecosystems` (`|`-joined, order kept -- `render.rs` `ecosystem::tags`,
   `summarize`, `agent_json` `list_projects_payload` read it), `remote`
   (nullable; `render::project_display_name` and `list_projects_payload`
   read it -- not in the chunk's column list, but read by both
   consumers, so byte-identity needs it), `bytes`, `local_bytes`,
   `allocated_bytes`, `growth_bytes` (nullable), `regrowth_count`,
   `worktree_count`, `observed_at`. The rollup columns are exactly what
   the chunk named; no render path reads them today (`ProjectRow` has no
   byte fields; `render.rs` sums artifacts on the fly), so they are
   written, not read back.
2. **`worktrees.parquet`** -- `StoredWorktreeRow`, `write_worktree_rows`,
   `read_worktree_rows`. Every scalar `render.rs`/`agent_json.rs` read
   from a `WorktreeRow` or its nested structs: `worktree_id`, `project_id`,
   `path`, `kind` (Debug label), `branch`, `idle_secs`, and `GithubFacts`
   flattened with a `github_` prefix (`default_branch`,
   `branch_exists_on_remote`, `unavailable_reason`, `merged_state` +
   `merged_at` + `merged_pr_number` for `MergedStatus::Yes`, `pr_state`
   + the seven `PullRequestInfo` scalars), `merge_complete_verdict`,
   `observed_at`. The chunk's "dirty / unpushed / tip_sha / upstream /
   lock facts" are not typed `WorktreeRow` fields in this codebase --
   they are entries of `signals: Vec<Signal>` -- so they live in the
   child table, not as columns.
   **`worktree_facts.parquet`** -- `StoredWorktreeFactRow`: the two
   list-valued facts (`signals` and `merge_complete.terms`), one row per
   entry with `fact_kind`, `name` (null for a term), `value`, `seq`.
3. **Artifacts** -- the existing per-volume current-artifact table
   (`StoredRow`, `current.parquet`) gained `ecosystem` (nullable Utf8).
   The chunk's other named columns (kind label, worktree-relative path,
   dedup_stale, newest mtime, hardlinked) were already there as `kind`,
   `rel_path`, `dedup_stale`, `mtime_max`, `hardlinked`. The ecosystem
   was previously computed one stage *after* the history is written
   (tracking), so `report::annotate_artifact_ecosystems` was split out
   of `annotate_tracking` (idempotent) and the growth consumer calls it
   first; the second call in tracking is now a no-op for those rows.
   `growth::artifact_table_facts_for_roots` reads the table across every
   root's volume directory, keyed by `growth::artifact_row_key` (the
   history's own key, including `observed_kind`'s nested-id case).
4. **`observe_scope`** writes the tables under the same gate as the
   snapshot (`growth::write_project_worktree_tables`, same merged tree,
   no second pass). **`report_scope_from_store`** rebuilds
   `Report.projects` from them (`rebuild_projects_from_tables`): project
   and worktree scalars, signals and GitHub/merge-complete facts from the
   three tables; artifact bytes/local_bytes/mtime_max/hardlinked/
   dedup_stale/regrowth_count/observed_at/ecosystem from the artifact
   table; the fields the chunk left to later slices (per-artifact
   evidence, confidence, source, track, containers, shared_with,
   dangling, note, created_at, allocated bytes/growth, growth_bytes) are
   overlaid from the snapshot's own row by key. Artifact list order is
   the snapshot's; project/worktree order is the tables' row order,
   which is the observed order. No table rows for the scope -> the
   snapshot's tree is returned unchanged.

## Tests (adversarial; each names the shortcut it fails)

- `crates/core/tests/project_worktree_tables.rs`
  - `observe_writes_the_three_tables_and_every_view_renders_identically_from_them`
    -- Rust checkout + linked worktree + Node checkout; asserts the three
    files exist, prints the store listing, asserts the rebuilt `Report`
    equals the observed one as `serde_json::Value`, and that every text
    view (`render_text`, overview, types, kinds, worktrees, builds, deps,
    docker, reconciliation, unowned, per-project, per-project tree,
    per-worktree signals) and every JSON payload (`to_json`, six
    `view_payload`s, projects, worktrees, grown, docker) is
    byte-identical from both. Fails a lossy rebuild.
  - `report_reads_projects_worktrees_and_artifact_facts_from_the_tables_not_the_snapshot_json`
    -- rewrites `report_rows.parquet` with a snapshot whose names,
    ecosystems, remotes, branches, kinds, idle time, signals and artifact
    bytes/local_bytes/mtime/ecosystem/hardlinked/dedup/regrowth are all
    tampered; the report must still equal the untampered observation.
    Fails "keep deserializing `report_json` and merely also write the
    tables" (it did fail before the read side was wired, and again when
    the ecosystem was found to be written one stage too late).
  - `the_remaining_parts_still_come_from_the_snapshot` -- a tampered note
    and `walked_total` do show, and projects still equal the observation:
    the boundary is pinned from both sides, so the test above cannot pass
    by ignoring the snapshot.
- `growth::tests::project_and_worktree_tables_round_trip_every_field_and_keep_list_order`
  -- fully populated `GithubFacts` (merged Yes with `merged_at`/
  `pr_number`, PR with all seven scalars), sparse `GithubFacts::unknown`,
  `merge_complete` with three ordered terms, three signals with a
  repeated name in non-sorted order, two ecosystem tags, a remote, and a
  worktree with everything `None`; round-trips as `serde_json::Value`;
  a second scope key's rows do not disturb the first's. Fails a lossy
  flattening or an unordered child table.
- `growth::tests::artifact_history_round_trips_the_ecosystem_column`
  -- ecosystem is stored, a change with unchanged bytes still rewrites
  the row, a null cell reads `None`.
- `crates/core/tests/store_contents_are_allowlisted.rs` needed no edit:
  it already allows every `*.parquet` name; it runs and still passes.
- `report_is_a_pure_read` (pre-existing, byte-for-byte round trip and
  zero I/O in `report_scope_from_store`) still passes with the rebuild in
  the read path -- the three extra table reads are Parquet reads through
  the gate, which that test's counters do not count as walk work.

## Store listing after one observe on the test fixture (from the test's own print)

```
       0  <volume>/observation.lock
      88  <volume>/fsevents.json
     564  associations/agent_identifications.parquet
     648  associations/xcode_derived_data.parquet
     923  <volume>/topology.json
    1265  <volume>/unowned.parquet
    1625  associations/agent_containers.parquet
    2313  associations/declarations.parquet
    2325  associations/dependency_identities.parquet
    2439  worktree_facts.parquet
    2883  last_report-<hash>.json.zst
    3942  projects.parquet
    4211  <volume>/dirs.parquet
    4776  <volume>/current.parquet
    6757  report_rows.parquet
    7706  worktrees.parquet
```

The three new tables appear; there is no JSON equivalent of any of
them to linger (they never had one -- their data was cells inside
`report_rows.parquet`, which still holds the not-yet-migrated parts).
`fsevents.json`/`topology.json`/`last_report-*.json.zst` are the
remaining JSON control/cache files, all explicitly later tables.

## Verification

See the final report for the command tails; in order: `cargo fmt --all
--check`, `cargo test --workspace --locked`, `cargo clippy --workspace
--all-targets --locked -- -D warnings`, `cargo run -p
swamp-source-audit` (11/11), `scripts/check.sh`. One cargo invocation
at a time against the shared target dir; no polling loops.

## Decisions the chunk left open

- `remote` added to `projects.parquet` although the chunk's list omits
  it: both consumers read it, byte-identity requires it.
- `GithubFacts` has more than six scalars once its nested enums are
  counted; flattened anyway (all nullable, `github_` prefix) rather than
  a second child table, because every one is a scalar and the child
  table is for lists.
- Per-artifact `evidence`/`confidence`/`source`/`track`/Docker detail/
  allocated bytes stay in the snapshot and are overlaid by key: the
  chunk names evidence as a later slice and lists exactly which artifact
  columns move now. `growth_bytes` is not stored either -- it is fixed at
  the observation that computed it (R12) and is one of the overlaid
  fields.
- Commits: per table where the files allow it (artifact column;
  `projects.parquet`; `worktrees.parquet` + child), then one for the
  shared wiring/tests/docs. Intermediate commits carry dead-code
  warnings for not-yet-wired writers; the tip is warning-free.

## What remains (item A, tables 5-~10; not attempted)

Unchanged from the R14 note's list minus these three: nested artifacts,
external/agent units + consumers, evidence, coverage, series, summary,
`unowned.parquet`'s JSON cells, `topology.json`, `fsevents.json`,
`docker_facts.json`, `scope.json`, `last_run.json`, `ledger.jsonl`, then
deleting `report_rows.parquet`'s JSON cells and
`last_report-*.json.zst`, then the serde_json-into-Parquet audit rule.
The overlay in `rebuild_projects_from_tables` is the seam the next
slices shrink: each field that gets a table moves from "overlaid from
the snapshot" to "read from its table" there.
