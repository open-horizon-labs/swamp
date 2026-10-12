# Advanced analytics: retained growth and cleanup history

Use this reference when today's directories or the last report's growth window cannot answer the question: historical peaks, repeated growth, sizes before cleanup, and which Cargo variants occupy an existing profile. Start with the CLI. Query retained store facts only for questions its report cannot express. Do not run `observe` merely to read history: it writes state, can compact retained deltas, and does not recover observations that were never collected.

Contents: CLI triage; store discovery and schemas; historical peaks; timelines; cleanup ledger; Cargo variants; interpretation checklist.

## CLI triage

```sh
command -v swamp
swamp --version
swamp scope --json
swamp report --view builds --json --limit 20 --unit-limit 8
swamp report --view projects --json --limit 20
swamp report --project <project-name-or-id> --dirs --depth 3
```

Use the same explicit roots as the stored observation when needed. `report` has no `--since` or `history` subcommand: its `since`, `observed_at`, coverage, and growth come from the last stored pass. Check pagination before treating rows as exhaustive. Capture large JSON output to a temporary file and print selected fields rather than the full evidence arrays. See [commands-and-json.md](commands-and-json.md) and [coverage-and-history.md](coverage-and-history.md).

## Store discovery and schemas

The default store is `$HOME/.local/share/swamp`, overridden by `SWAMP_DIR`. Swamp stores its history in Parquet, not SQLite. DuckDB can query these files without importing Swamp or compiling Rust:

```sh
command -v duckdb
duckdb -json -c "DESCRIBE SELECT * FROM read_parquet('/absolute/store/runs.parquet')"
```

Use an in-memory DuckDB connection: omit a database filename and `-readonly` (DuckDB refuses read-only mode for an in-memory database). `SELECT`, `DESCRIBE`, and temporary views read the source Parquet files; they do not update the Swamp store. If DuckDB is unavailable, use an existing Python environment with `pyarrow` or `duckdb`, or install the reader in an isolated temporary environment within the task's authorization.

The recipes below were checked against Swamp 0.8.6, store format marker `housekeeping.version` = `2`, and the repository's Parquet definitions. These are internal tables, not a versioned SQL API. Inspect schemas before using them; a marker alone does not establish every table's presence. Replace `<STORE>`, `<HISTORY>`, `<SCOPE_KEY>`, and IDs with discovered values; escape a literal apostrophe in a SQL path as `''`.

| Files | Purpose and useful fields |
|---|---|
| `coverage.parquet` | Recorded root, `scope_key`, status, and `observed_at`; distinguishes scope from storage changes |
| `runs.parquet` | Last pass per `scope_key`, `observed_at`, `since_secs`, and `retention_days` |
| `projects.parquet` | Current display metadata keyed by `scope_key`, `project_id` |
| `worktrees.parquet` | Current path metadata keyed by `scope_key`, `worktree_id` |
| `<numeric-directory>/current.parquet` | Latest artifact states, including absent rows |
| `<numeric-directory>/deltas/*.parquet` | Previous artifact states retained as reverse deltas |
| `nested_artifacts.parquet` | Current adapter interiors and variant facts, not a historical inventory of individual compiler files |
| `ledger.parquet`, `ledger_facts.parquet` | Action records and the evidence displayed when the action happened |

Root-scoped history directories use an opaque numeric identity derived from canonical root and device. This is different from the fact tables' `scope_key`. Do not infer a root from directory ordering or equate those keys. Inventory candidate histories, then match exact artifact/project/worktree identities against the relevant CLI report and fact tables. If ownership remains ambiguous, report the history directory and known IDs instead of guessing a root.

```sql
SELECT scope_key, root_path, status, to_timestamp(observed_at) AS observed_at
FROM read_parquet('<STORE>/coverage.parquet')
ORDER BY observed_at DESC;

SELECT filename, count(*) AS artifact_rows,
       to_timestamp(min(observed_at)) AS earliest_row,
       to_timestamp(max(observed_at)) AS latest_changed_row
FROM read_parquet('<STORE>/[0-9]*/current.parquet', filename = true,
                  union_by_name = true)
GROUP BY filename
ORDER BY latest_changed_row DESC;
```

The artifact row timestamp changes when its tracked state changes; `max(observed_at)` is not necessarily the last complete scan. Read `runs.parquet` for that pass's time. An unchanged artifact can have an old row timestamp inside a fresh report.

For a coherent multi-table investigation, copy the selected history's `current.parquet`, delta files, and needed fact tables into a temporary snapshot outside `SWAMP_DIR`, preserving their relative layout. Compare the source file list, sizes, and modification times before and after copying. If they changed during a scheduled observation or compaction, retry once; if they keep changing, label the analysis non-atomic and stop claiming a precise sequence. Do not stop the scheduler or acquire Swamp's writer lock to perform analysis.

## Historical peaks

Run the following statements in one DuckDB session against the selected snapshot. `<HISTORY>` is one root-scoped numeric directory, not the entire store. If it has no deltas, remove the second path from `read_parquet`; an unmatched glob is an error, not an empty history.

```sql
CREATE TEMP VIEW artifact_history AS
SELECT DISTINCT project_id, worktree_id, kind, rel_path,
       bytes, local_bytes, present, observed_at, dedup_stale, regrowth_count
FROM read_parquet(['<HISTORY>/current.parquet',
                   '<HISTORY>/deltas/*.parquet'], union_by_name = true);

SELECT project_id, worktree_id, rel_path,
       max(bytes) FILTER (WHERE present AND NOT dedup_stale) AS peak_bytes,
       round(max(bytes) FILTER (WHERE present AND NOT dedup_stale)
             / 1073741824.0, 2) AS peak_gib,
       to_timestamp(min(observed_at)) AS first_retained_state,
       to_timestamp(max(observed_at)) AS last_changed_state,
       count(*) FILTER (WHERE present AND dedup_stale) AS stale_accounting_states
FROM artifact_history
WHERE kind = 'BuildOutput'
GROUP BY project_id, worktree_id, rel_path
HAVING max(bytes) FILTER (WHERE present AND NOT dedup_stale) > 5368709120
ORDER BY peak_bytes DESC
LIMIT 20;
```

The stored kind is **`BuildOutput`**, although CLI JSON describes it as `"build"`. SQL uses bytes; the decimal `.0` denominator and literal `5368709120` avoid integer division and INT32 overflow from `5 * 1073741824`. GiB means 1,073,741,824 bytes.

Use exact IDs to recover current display names and paths:

```sql
SELECT project_id, name, remote
FROM read_parquet('<STORE>/projects.parquet')
WHERE scope_key = '<SCOPE_KEY>' AND project_id = '<PROJECT_ID>';

SELECT worktree_id, path, branch
FROM read_parquet('<STORE>/worktrees.parquet')
WHERE scope_key = '<SCOPE_KEY>' AND worktree_id = '<WORKTREE_ID>';
```

A removed worktree can disappear from current metadata while its history remains. Keep its identity and relative artifact path; the ledger may supply its old label, or `restore.json` may supply an original path. A Trash basename is not proof of the original path. Across old or overlapping history directories, rank each scope separately; never sum peaks taken at different times or add multiple views of the same root.

## Growth timelines and regrowth

Reverse deltas retain a row's previous state at the timestamp when that old value was confirmed, not at the moment the delta file was written. A current row plus its retained deltas is a sequence of known states, not periodic full snapshots. File sequence numbers and modification times do not provide observation timestamps.

```sql
SELECT to_timestamp(observed_at) AS state_at,
       bytes, round(bytes / 1073741824.0, 2) AS gib,
       present, dedup_stale, regrowth_count
FROM artifact_history
WHERE project_id = '<PROJECT_ID>'
  AND worktree_id = '<WORKTREE_ID>'
  AND kind = 'BuildOutput' AND rel_path = 'target'
ORDER BY observed_at, present DESC, dedup_stale, bytes;
```

Preserve conflicting states at the same timestamp rather than selecting an arbitrary winner. `DISTINCT` removes identical records, not conflicts. Legacy or partially migrated histories can contain ties; inspect them and qualify the affected interval. For a two-point comparison, select two unambiguous, present, non-stale states and report their timestamps and byte difference. Do not interpolate growth through gaps or describe the first recorded size as growth from zero.

`present = false` is a tombstone, not a zero-byte size measurement of a directory that still exists. Its observation can lag a ledger action. `local_bytes` can retain the previous size on a tombstone; do not treat it as current occupancy. Missing or inaccessible coverage does not establish deletion. `regrowth_count` records an observed reappearance after a tombstone; ordinary increases while present do not count as regrowth.

## Cleanup ledger: what was removed and how large it was

Current releases use two Parquet tables. The older `ledger.jsonl` layout is not the current ledger. Evidence facts join **`ledger_facts.record_id = ledger.id`**; `entity_id` is the affected unit and is not this join key.

```sql
WITH facts AS (
  SELECT record_id,
         max(value) FILTER (WHERE key = 'label') AS row_label,
         max(try_cast(value AS UBIGINT)) FILTER (WHERE key = 'bytes') AS shown_bytes,
         max(try_cast(value AS UBIGINT)) FILTER (WHERE key = 'observed_at') AS evidence_at
  FROM read_parquet('<STORE>/ledger_facts.parquet')
  GROUP BY record_id
)
SELECT l.id, to_timestamp(l.recorded_at) AS action_at,
       l.entity_id, f.row_label,
       round(f.shown_bytes / 1073741824.0, 2) AS shown_gib,
       to_timestamp(f.evidence_at) AS evidence_at,
       l.outcome, l.observed_path_state, l.recovery_location,
       l.measured_free_space_delta
FROM read_parquet('<STORE>/ledger.parquet') AS l
LEFT JOIN facts AS f ON f.record_id = l.id
WHERE f.shown_bytes > 5368709120
ORDER BY l.recorded_at DESC
LIMIT 25;
```

The query pivots scalar evidence keys; check for conflicting repeated values before relying on a scalar:

```sql
SELECT record_id, key, count(DISTINCT value) AS distinct_values
FROM read_parquet('<STORE>/ledger_facts.parquet')
WHERE key IN ('label', 'bytes', 'observed_at')
GROUP BY record_id, key
HAVING count(DISTINCT value) > 1;
```

If conflicts appear, read the matching fact rows in `seq` order instead of presenting the `max` value as definitive. `shown_bytes` is what the UI displayed from its observation, not a fresh action-time measurement or measured freed space. A whole-worktree ledger size also includes source and other artifacts; do not label it all Cargo output. Check `outcome` and `observed_path_state` before calling a move completed. A move to Trash does not free disk space until Trash is emptied; absent `measured_free_space_delta` means savings were not measured. See [cleanup-and-recovery.md](cleanup-and-recovery.md) for restore records.

## What occupies an existing Cargo profile

```sh
swamp inspect-cargo /absolute/project/target/debug --json --max-ms 5000
```

This bounded filesystem read does not run Cargo or write an observation. Use its `groups`, `target`, `target_kind`, `variant`, `fingerprint_paths`, and accounting fields to identify large libraries, compiled tests, and multiple fingerprints. Inspect `coverage.complete`, `coverage.limits`, residual groups, and `elapsed_ms` before calling the inspection exhaustive.

`allocated_bytes` counts directory entries; `unique_allocated_bytes` deduplicates inode identities within this inspection. Neither predicts freed space or APFS extent sharing. Multiple fingerprints are evidence of compiled variants, not proof of stale copies, different toolchains, different features, or complete build generations. Preserve unknown variant fields; an unmatched residual is not a guessed package. Current profile inspection explains current files and cannot reconstruct the contents of a target already removed.

## Interpretation checklist

- State the retained observation interval, selected root/history scope, accounting basis, and whether the analysis used a stable snapshot.
- Call a maximum a **recorded peak within retained observations**, not the largest size the directory ever reached. Retention and sampling can miss earlier or between-scan peaks.
- `bytes` is artifact attribution; `dedup_stale` means its unique charge needs reconciliation. Exclude those states from precise charge comparisons and disclose the exclusion. `local_bytes` counts the artifact's allocated entries and may overlap hardlinks elsewhere; do not substitute it silently.
- Do not add `Nested:` interior rows to their containing `BuildOutput` total. They describe contents already included in the container. Keep different accounting bases separate.
- Preserve missing identity, coverage gaps, conflicting timestamps, and unknown reclaimability. Do not manufacture deleted paths or assert that an old variant is unused.
- Storage records establish what accumulated and when. They do not establish which compiler flag caused growth or quantify the speed effect of a proposed flag; those require build evidence or measurement.
