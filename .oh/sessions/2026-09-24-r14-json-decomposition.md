# 2026-09-24 -- R14 item A: JSON-in-the-store decomposition, table 1 of ~10 (`protect.parquet`)

Branch `stack/26-aim-review-repairs`, worktree
`/Users/muness1/src/open-horizon-labs/swamp-builds`. Source:
`CHUNK_R14.md` item A -- the user's hard rule, no JSON data anywhere in
the store, which the two previous sessions on this stack (see
`.oh/sessions/2026-09-24-project-roots-vs-detector-locations.md`)
looked at and explicitly declined to start, for a real reason: `Report`
is a deeply nested tree and the full decomposition (`projects.parquet`
through `summary.parquet`, replacing `report_rows.parquet`'s JSON cells
and `last_report-*.json.zst`) is its own multi-session project.

This session did not accept "still not attempted" as the answer for a
third time. It converted the **first** table completely -- writer,
reader, tests, docs, audits, the runtime allow-list test -- and is
committing that coherent unit rather than leaving another zero.

## `agent_protect.json` -> `protect.parquet` (done, committed)

- `crates/core/src/protection.rs`: the human keep list's file went from
  a JSON control file (`serde_json` `ProtectFile { paths: Vec<String> }`)
  to a two-column Parquet table (`path` Utf8, `added_at` UInt64 epoch
  seconds -- a real column the old file never had). `load_protect`,
  `protect_add`, `protect_remove`, `protect_listing` all still work
  through the same public API; `ProtectList` is still opaque (its one
  query, `conflict`, is unchanged -- the `protection-fails-closed`
  compile-fail tests didn't need to move).
- The Arrow schema/writer/reader do **not** live in `protection.rs`:
  `gate_paths_only_inside_gates` confines Arrow (`arrow_array`/
  `arrow_schema`) to `growth::columns`/`assoc_store`/`store`/`github`/
  `fs_gate`, so `StoredProtectRow`/`write_protect_rows`/
  `read_protect_rows` live in `crates/core/src/growth/columns.rs`
  (`growth.rs`'s `mod columns;` widened to `pub(crate) mod columns;` so
  a sibling module can call it) -- caught by actually running
  `cargo run -p swamp-source-audit` after the first draft (which put
  the Arrow code directly in `protection.rs`) and reading the failure,
  not by intuition.
- `crates/core/src/fs_gate/store.rs`: `JsonFile::ProtectList` variant
  removed (its match arms too) -- the file it named no longer exists.
- Migration: none, per the brief. An old `agent_protect.json` from
  before this change is simply never read again; the human re-adds
  their paths. No compatibility shim.
- Docs: `docs/architecture.md` (protect intent description),
  `.oh/guardrails/store-data-is-parquet-not-json-sidecars.md` (moved
  `agent_protect.json` from the "not yet done" list to "done", updated
  the target allow-list description to match `CHUNK_R14.md`'s actual
  target -- `config.toml`, `ui_state.json`, lock files, everything else
  Parquet -- rather than the stale `ledger.jsonl`-stays-JSON note an
  earlier session left there), `CHANGELOG.md` (`## Unreleased`).
- `crates/core/tests/store_contents_are_allowlisted.rs`: `agent_protect.json`
  removed from `ALLOWED_NAMES` -- this is the runtime store test the
  chunk asks for, narrowed by exactly one entry, not rewritten wholesale
  (rewriting it to a target end-state before every table is converted
  would make the test lie about tables that don't exist yet).

### Tests (new, in `crates/core/src/protection.rs`)

- `protect_round_trips_through_parquet_with_added_at` -- add two paths,
  asserts `protect.parquet` exists, asserts `agent_protect.json` does
  **not** exist, asserts both rows carry a nonzero `added_at`, asserts
  `conflict` still matches both, then removes one and asserts exactly
  one row remains.
- `protect_add_is_idempotent_and_keeps_the_original_added_at` -- adds
  the same path twice across a real 1.1s sleep and asserts the row
  count stays 1 and `added_at` does **not** move to the second add's
  time. This is the adversarial case: the tempting shortcut is
  "overwrite the whole table on every add", which would silently reset
  `added_at` and is exactly what this test would catch.
- `a_corrupt_protect_table_fails_closed` -- writes garbage bytes where
  `protect.parquet` should be and asserts `load_protect` returns an
  `Err` containing "protection state unknown", not an empty list. This
  is the direct successor to the old JSON-parse-failure fail-closed
  behavior; same contract, new file format.
- `a_missing_protect_table_is_an_empty_list_not_an_error` -- the
  ordinary case is still not an error.

### Verification (this session, in order)

- `cargo build --locked -p swamp-core` -- clean, then
  `cargo build --workspace --locked` -- clean.
- `cargo run -p swamp-source-audit` -- failed once (Arrow code in the
  wrong module, see above), fixed, then 11/11 `ok`.
- `cargo fmt --all` -- clean.
- `cargo test --workspace --locked --no-fail-fast` -- full green (every
  `test result: ok`, 0 failed, checked the whole log for
  `test result: FAILED`/`error[` -- zero matches), run once.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` --
  clean.
- `cargo test -p swamp-core --lib` -- 823 passed, 0 failed, 2 ignored
  (unrelated `#[ignore]`d slow tests).
- `cargo test -p swamp-core --test store_contents_are_allowlisted` --
  the runtime store test, still green with `agent_protect.json` gone
  from the allow-list.
- `scripts/check.sh` (`SWAMP_TARGET_DIR` pointed at the shared warm
  target dir) -- full pass, `== done`, exit 0.
- `ps aux` after every step shows no cargo/swamp process left running
  from this session; no background polling loop was used (long-running
  commands were single `run_in_background` invocations, waited for via
  the completion notification, not a sleep loop).

Commit: `crates/core: agent_protect.json -> protect.parquet, no JSON
cell (R14 item A, table 1/~10)`.

## What remains (item A; not narrowed, not attempted further this session)

Per `CHUNK_R14.md`'s own decomposition, in the order it lists them,
none of the following were touched this session:

1. `projects.parquet`, `worktrees.parquet` -- the two hardest, since
   `Report`'s `ProjectRow`/`WorktreeRow`/`ArtifactRow` tree and its
   growth-series/reconciliation/summary fields are exactly the "deeply
   nested tree" the previous session described precisely (see that
   session's note for the row-key scheme this needs).
2. `artifacts.parquet` -- extend the existing table with render-only
   columns.
3. `nested_artifacts.parquet`.
4. `external_units.parquet`, `agent_units.parquet`,
   `unit_consumers.parquet` (child table).
5. `evidence.parquet`, `coverage.parquet`, `series.parquet`,
   `summary.parquet`.
6. `unowned.parquet`'s JSON cells (`containers_json`/`shared_with_json`/
   `evidence_json`) -- explicitly named in the brief, explicitly noted
   by the previous session as contested (the existing design doc calls
   cell-encoded JSON acceptable; the brief's own new audit rule, read
   literally, disagrees). Not resolved this session either way; still a
   real decision for whoever does it, not mine to make unilaterally by
   picking a side and moving on.
7. `topology.json` -> `topology.parquet`, `fsevents.json` ->
   `cursors.parquet`, `docker_facts.json` -> `docker_facts.parquet`
   (+TTL column -- `DockerFacts` has nested `Vec`s of images/build-cache/
   volumes/builders, so this is not a two-column table either),
   `scope.json` -> `scope.parquet`, `last_run.json` -> `runs.parquet`,
   `agent_protect.json` -> `protect.parquet` (**done this session**),
   `ledger.jsonl` -> `ledger.parquet`. `restore.json` inside Trash
   envelopes stays (explicitly out of scope per the brief -- it lives in
   the Trash, not the store).
8. Deleting `ReportSnapshot`'s JSON cells / `last_report-*.json.zst`
   entirely and rewriting `report_scope_from_store`/`observe_scope`
   to assemble from tables -- the big one, needs items 1-5 first.
9. The `serde_json::*`/`json!` sink audit rule itself (rejecting JSON
   flowing into Parquet writers or `fs_gate` store writes) -- not
   written this session; writing it before more tables convert would
   fail on tables not yet converted, which is not useful signal yet.

## Item C follow-up (sealed read-only volume reuse) -- not started

`CHUNK_R14.md`'s item C follow-up (statfs-based reuse for read-only
mounted volumes like CoreSimulator runtimes, `RegionStatus::
ReusedReadOnlyVolume`, the ≤ 2s re-measure target) was not attempted
this session. All the time went to doing item A's first table
completely and honestly rather than starting both and finishing
neither. The previous session's profiling (same file, "Item C" section)
is still the accurate diagnosis and starting point.

## Not pushed

Per the worker brief, push only when the whole chunk (A, B, C) is
green. Item A has 1 of ~10 tables done; item C's follow-up is
untouched. This branch is **not pushed**. The one commit from this
session is on `stack/26-aim-review-repairs`, on top of the three
commits already there from the previous session.
